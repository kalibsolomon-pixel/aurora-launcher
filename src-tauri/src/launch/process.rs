//! Exact-child process spawning and process-local supervision.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncRead, AsyncReadExt};

use super::resolve::LaunchSpec;
use super::state::LaunchProcessStatus;

const MAX_CAPTURE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub instance_id: String,
    pub status: LaunchProcessStatus,
    pub process_id: Option<u32>,
    pub started_at_unix_seconds: Option<u64>,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
}

impl ProcessSnapshot {
    pub fn stopped(instance_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            status: LaunchProcessStatus::Stopped,
            process_id: None,
            started_at_unix_seconds: None,
            exit_code: None,
            message: None,
        }
    }
}

fn process_states() -> &'static Mutex<HashMap<String, ProcessSnapshot>> {
    static STATES: OnceLock<Mutex<HashMap<String, ProcessSnapshot>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn snapshot(instance_id: &str) -> ProcessSnapshot {
    process_states()
        .lock()
        .expect("launch process state is not poisoned")
        .get(instance_id)
        .cloned()
        .unwrap_or_else(|| ProcessSnapshot::stopped(instance_id))
}

pub type StateListener = Arc<dyn Fn(ProcessSnapshot) + Send + Sync + 'static>;

/// Starts the exact process described by `LaunchSpec`, then supervises the
/// exact returned child handle in the background. No process-name lookup,
/// shell, or reconstructed command line exists anywhere in this boundary.
pub async fn spawn_supervised(
    spec: LaunchSpec,
    logs_directory: &Path,
    listener: StateListener,
) -> Result<ProcessSnapshot, LaunchProcessError> {
    let instance_id = spec.instance_id().to_owned();
    let started_at = unix_seconds();
    let starting = ProcessSnapshot {
        instance_id: instance_id.clone(),
        status: LaunchProcessStatus::Starting,
        process_id: None,
        started_at_unix_seconds: Some(started_at),
        exit_code: None,
        message: None,
    };
    {
        let mut states = process_states()
            .lock()
            .expect("launch process state is not poisoned");
        if states
            .get(&instance_id)
            .is_some_and(|state| state.status.blocks_launch())
        {
            return Err(LaunchProcessError::AlreadyRunning { instance_id });
        }
        states.insert(instance_id.clone(), starting.clone());
    }
    listener(starting);

    let expected_logs = spec.working_directory().join("logs");
    if logs_directory != expected_logs || !logs_directory.starts_with(spec.working_directory()) {
        return fail_before_spawn(
            &instance_id,
            started_at,
            &listener,
            LaunchProcessError::Log(
                "the launch log path is not the exact derived instance logs directory".to_owned(),
            ),
        );
    }
    std::fs::create_dir_all(logs_directory).map_err(|error| {
        let failure = LaunchProcessError::Log(error.to_string());
        record_failure(
            &instance_id,
            started_at,
            None,
            failure.to_string(),
            &listener,
        );
        failure
    })?;
    let log_path = reserve_log_path(logs_directory).map_err(|error| {
        let failure = LaunchProcessError::Log(error.to_string());
        record_failure(
            &instance_id,
            started_at,
            None,
            failure.to_string(),
            &listener,
        );
        failure
    })?;

    let arguments = spec.command_arguments();
    let redactions = spec.sensitive_values();
    let mut command = tokio::process::Command::new(spec.java_executable());
    command
        .args(arguments.iter().map(|argument| argument.expose()))
        .current_dir(spec.working_directory())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Prevent ambient Java injection while retaining the ordinary host
        // environment graphics/audio/native libraries rely on.
        .env_remove("CLASSPATH")
        .env_remove("JAVA_TOOL_OPTIONS")
        .env_remove("_JAVA_OPTIONS")
        .env_remove("JDK_JAVA_OPTIONS");

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return fail_before_spawn(
                &instance_id,
                started_at,
                &listener,
                LaunchProcessError::Spawn(error.to_string()),
            );
        }
    };
    let process_id = child.id();
    let running = ProcessSnapshot {
        instance_id: instance_id.clone(),
        status: LaunchProcessStatus::Running,
        process_id,
        started_at_unix_seconds: Some(started_at),
        exit_code: None,
        message: None,
    };
    process_states()
        .lock()
        .expect("launch process state is not poisoned")
        .insert(instance_id.clone(), running.clone());
    listener(running.clone());

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    tokio::spawn(async move {
        let stdout_task = tokio::spawn(async move {
            match stdout {
                Some(stream) => read_bounded(stream).await,
                None => Vec::new(),
            }
        });
        let stderr_task = tokio::spawn(async move {
            match stderr {
                Some(stream) => read_bounded(stream).await,
                None => Vec::new(),
            }
        });
        let exit = child.wait().await;
        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();

        let (status, exit_code, message) = match exit {
            Ok(status) if status.success() => (LaunchProcessStatus::Exited, status.code(), None),
            Ok(status) => (
                LaunchProcessStatus::Failed,
                status.code(),
                Some("Minecraft exited unsuccessfully; review the instance launch log.".to_owned()),
            ),
            Err(_) => (
                LaunchProcessStatus::Failed,
                None,
                Some("Minecraft process supervision failed after start.".to_owned()),
            ),
        };
        if let Err(error) = write_redacted_log(&log_path, exit_code, &stdout, &stderr, &redactions)
        {
            eprintln!("[aurora-launcher] could not write the supervised launch log: {error}");
        }
        let snapshot = ProcessSnapshot {
            instance_id: instance_id.clone(),
            status,
            process_id: None,
            started_at_unix_seconds: Some(started_at),
            exit_code,
            message,
        };
        process_states()
            .lock()
            .expect("launch process state is not poisoned")
            .insert(instance_id, snapshot.clone());
        listener(snapshot);
    });

    Ok(running)
}

fn fail_before_spawn<T>(
    instance_id: &str,
    started_at: u64,
    listener: &StateListener,
    error: LaunchProcessError,
) -> Result<T, LaunchProcessError> {
    record_failure(instance_id, started_at, None, error.to_string(), listener);
    Err(error)
}

fn record_failure(
    instance_id: &str,
    started_at: u64,
    exit_code: Option<i32>,
    message: String,
    listener: &StateListener,
) {
    let snapshot = ProcessSnapshot {
        instance_id: instance_id.to_owned(),
        status: LaunchProcessStatus::Failed,
        process_id: None,
        started_at_unix_seconds: Some(started_at),
        exit_code,
        message: Some(message),
    };
    process_states()
        .lock()
        .expect("launch process state is not poisoned")
        .insert(instance_id.to_owned(), snapshot.clone());
    listener(snapshot);
}

async fn read_bounded(mut reader: impl AsyncRead + Unpin) -> Vec<u8> {
    let mut captured = Vec::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                let remaining = MAX_CAPTURE_BYTES.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..count.min(remaining)]);
            }
        }
    }
    captured
}

fn write_redacted_log(
    path: &Path,
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    redactions: &[String],
) -> std::io::Result<()> {
    let redact = |bytes: &[u8]| {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        for secret in redactions.iter().filter(|secret| !secret.is_empty()) {
            text = text.replace(secret, "[redacted]");
        }
        text
    };
    let content = format!(
        "Aurora supervised Minecraft process\nExit code: {}\n\n[stdout]\n{}\n[stderr]\n{}",
        exit_code.map_or_else(|| "unavailable".to_owned(), |code| code.to_string()),
        redact(stdout),
        redact(stderr),
    );
    std::fs::write(path, content)
}

fn reserve_log_path(logs: &Path) -> std::io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    for _ in 0..32 {
        let path = logs.join(format!(
            "aurora-launch-{}-{}.log",
            unix_seconds(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique launch log",
    ))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchProcessError {
    AlreadyRunning { instance_id: String },
    Spawn(String),
    Log(String),
}

impl LaunchProcessError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::AlreadyRunning { .. } => "launch_already_running",
            Self::Spawn(_) => "launch_spawn_failure",
            Self::Log(_) => "launch_process_failure",
        }
    }
}

impl fmt::Display for LaunchProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning { instance_id } => write!(
                formatter,
                "instance '{instance_id}' is already starting or running"
            ),
            Self::Spawn(_) => write!(formatter, "the managed Java process could not be started"),
            Self::Log(_) => write!(formatter, "the supervised launch log could not be prepared"),
        }
    }
}

impl std::error::Error for LaunchProcessError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_root(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "aurora-launch-process-{}-{name}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    async fn wait_terminal(instance_id: &str) -> ProcessSnapshot {
        for _ in 0..100 {
            let state = snapshot(instance_id);
            if !state.status.blocks_launch() {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("fake child did not reach a terminal state");
    }

    #[test]
    #[ignore]
    fn fake_child_success() {
        println!("stdout says FIXTURE-PROCESS-TOKEN");
        eprintln!("stderr says FIXTURE-PROCESS-TOKEN");
    }

    #[test]
    #[ignore]
    fn fake_child_failure() {
        eprintln!("expected fake failure");
        std::process::exit(17);
    }

    #[test]
    #[ignore]
    fn fake_child_slow() {
        std::thread::sleep(Duration::from_millis(500));
    }

    #[tokio::test]
    async fn supervised_process_transitions_logs_and_redacts() {
        let root = test_root("success");
        let spec = LaunchSpec::fake_process(
            "process-success",
            std::env::current_exe().unwrap(),
            "launch::process::tests::fake_child_success",
            root.clone(),
        );
        let running = spawn_supervised(spec, &root.join("logs"), Arc::new(|_| {}))
            .await
            .unwrap();
        assert_eq!(running.status, LaunchProcessStatus::Running);
        assert!(running.process_id.is_some());
        let exited = wait_terminal("process-success").await;
        assert_eq!(exited.status, LaunchProcessStatus::Exited);
        assert_eq!(exited.exit_code, Some(0));
        let log = std::fs::read_to_string(
            std::fs::read_dir(root.join("logs"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        assert!(log.contains("stdout says [redacted]"));
        assert!(log.contains("stderr says [redacted]"));
        assert!(!log.contains("FIXTURE-PROCESS-TOKEN"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn nonzero_spawn_failure_and_duplicate_instance_are_structured() {
        let root = test_root("states");
        let failing = LaunchSpec::fake_process(
            "process-failure",
            std::env::current_exe().unwrap(),
            "launch::process::tests::fake_child_failure",
            root.clone(),
        );
        spawn_supervised(failing, &root.join("logs"), Arc::new(|_| {}))
            .await
            .unwrap();
        let failed = wait_terminal("process-failure").await;
        assert_eq!(failed.status, LaunchProcessStatus::Failed);
        assert_eq!(failed.exit_code, Some(17));

        let missing = LaunchSpec::fake_process(
            "process-missing",
            root.join("does-not-exist"),
            "unused",
            root.clone(),
        );
        assert!(matches!(
            spawn_supervised(missing, &root.join("logs"), Arc::new(|_| {})).await,
            Err(LaunchProcessError::Spawn(_))
        ));

        let slow = LaunchSpec::fake_process(
            "process-duplicate",
            std::env::current_exe().unwrap(),
            "launch::process::tests::fake_child_slow",
            root.clone(),
        );
        spawn_supervised(slow.clone(), &root.join("logs"), Arc::new(|_| {}))
            .await
            .unwrap();
        assert!(matches!(
            spawn_supervised(slow, &root.join("logs"), Arc::new(|_| {})).await,
            Err(LaunchProcessError::AlreadyRunning { .. })
        ));

        let other = LaunchSpec::fake_process(
            "process-other",
            std::env::current_exe().unwrap(),
            "launch::process::tests::fake_child_slow",
            root.clone(),
        );
        spawn_supervised(other, &root.join("logs"), Arc::new(|_| {}))
            .await
            .unwrap();
        wait_terminal("process-duplicate").await;
        wait_terminal("process-other").await;
        std::fs::remove_dir_all(root).unwrap();
    }
}
