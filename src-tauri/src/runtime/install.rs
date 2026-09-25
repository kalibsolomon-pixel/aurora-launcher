//! Transactional installation and read-only validation of shared runtimes.

use futures_util::future::join_all;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use std::time::Instant;

use crate::cache::{AcquisitionError, ArtifactCache};
use crate::downloads::{DownloadOptions, Sha1ArtifactSource};
use crate::integrity::{Sha1Digest, verify_file_sha1};
use crate::paths::ManagedPaths;
use crate::runtime::plan::{JavaRuntimePlan, RuntimeEntry, join_relative};
use crate::runtime::state::{
    RUNTIME_STATE_FILE_NAME, RuntimeInstalledState, RuntimeStateEntryKind, RuntimeStateError,
    load_runtime_state,
};

const STAGING_PREFIX: &str = ".installing-";
const STAGED_RUNTIME_DIR: &str = "runtime";
const REPLACED_RUNTIME_DIR: &str = "replaced-runtime";
const RUNTIME_DOWNLOAD_CONCURRENCY: usize = 16;
pub const DEFAULT_JAVA_DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeInstallPhase {
    Acquiring,
    Materializing,
    Validating,
    Executing,
    Committing,
}

impl RuntimeInstallPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acquiring => "acquiring",
            Self::Materializing => "materializing",
            Self::Validating => "validating",
            Self::Executing => "executing",
            Self::Committing => "committing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRuntimeProgress {
    pub phase: RuntimeInstallPhase,
    pub completed_items: u32,
    pub total_items: u32,
    pub current_item: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RuntimeInstallFaults {
    fail_during_materialization: bool,
    fail_before_commit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRuntime {
    root: PathBuf,
    launch_executable: PathBuf,
    diagnostic_executable: PathBuf,
    state: RuntimeInstalledState,
    reused: bool,
}

impl InstalledRuntime {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn launch_executable(&self) -> &Path {
        &self.launch_executable
    }
    pub fn diagnostic_executable(&self) -> &Path {
        &self.diagnostic_executable
    }
    pub fn state(&self) -> &RuntimeInstalledState {
        &self.state
    }
    pub fn reused(&self) -> bool {
        self.reused
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeValidationStatus {
    Missing,
    Ready,
    Damaged,
}

impl RuntimeValidationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Ready => "ready",
            Self::Damaged => "damaged",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaDiagnostic {
    pub reported_major_version: u32,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeValidation {
    pub status: RuntimeValidationStatus,
    pub component: String,
    pub required_major_version: u32,
    pub runtime_version: Option<String>,
    pub root: PathBuf,
    pub launch_executable: Option<PathBuf>,
    pub checked_files: u32,
    pub verified_bytes: u64,
    pub diagnostic: Option<JavaDiagnostic>,
    pub problems: Vec<String>,
}

/// Ensures the exact planned runtime exists. A valid exact installation is
/// reused without network access; a manifest-proven damaged installation is
/// replaced transactionally; foreign or structurally invalid trees are never
/// overwritten.
pub async fn ensure_runtime(
    managed: &ManagedPaths,
    plan: &JavaRuntimePlan,
    options: &DownloadOptions,
    execute_diagnostic: bool,
    progress: &mut (dyn FnMut(InstallRuntimeProgress) + Send),
) -> Result<InstalledRuntime, RuntimeInstallError> {
    ensure_runtime_with_faults(
        managed,
        plan,
        options,
        execute_diagnostic,
        progress,
        RuntimeInstallFaults::default(),
    )
    .await
}

async fn ensure_runtime_with_faults(
    managed: &ManagedPaths,
    plan: &JavaRuntimePlan,
    options: &DownloadOptions,
    execute_diagnostic: bool,
    progress: &mut (dyn FnMut(InstallRuntimeProgress) + Send),
    faults: RuntimeInstallFaults,
) -> Result<InstalledRuntime, RuntimeInstallError> {
    let started = Instant::now();
    let lock = runtime_installation_lock(&plan.identity());
    let _guard = lock
        .try_lock()
        .map_err(|_| RuntimeInstallError::AlreadyInProgress {
            identity: plan.identity(),
        })?;
    let root = plan.installation_dir(managed);

    let initial = validate_runtime(
        managed,
        plan,
        execute_diagnostic,
        DEFAULT_JAVA_DIAGNOSTIC_TIMEOUT,
    )
    .await?;
    if initial.status == RuntimeValidationStatus::Ready {
        if std::env::var_os("AURORA_INSTALL_DIAGNOSTICS").is_some() {
            eprintln!(
                "[aurora-runtime] reused=true total_ms={} validated_files={} verified_bytes={}",
                started.elapsed().as_millis(),
                initial.checked_files,
                initial.verified_bytes
            );
        }
        let state = load_runtime_state(&root)?.expect("ready validation loaded state");
        return Ok(installed_result(plan, managed, state, true));
    }

    let replacing = decide_replacement(&root, plan)?;
    let files: Vec<_> = plan
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            RuntimeEntry::File { path, artifact, .. } => Some((path.clone(), artifact.clone())),
            _ => None,
        })
        .collect();
    let total = files.len() as u32;
    let cache = ArtifactCache::new(managed.clone());
    let mut acquired = HashMap::new();
    let mut downloads = 0usize;
    let mut downloaded_bytes = 0u64;
    for batch in files.chunks(RUNTIME_DOWNLOAD_CONCURRENCY) {
        let cache = &cache;
        let results = join_all(batch.iter().map(|(relative, artifact)| async move {
            let source = Sha1ArtifactSource::https_or_loopback(
                artifact.url(),
                &artifact.sha1().as_hex(),
                Some(artifact.size_bytes()),
            )
            .map_err(|error| {
                RuntimeInstallError::Plan(format!(
                    "runtime artifact '{relative}' is invalid: {error}"
                ))
            })?;
            let verified = cache.acquire_sha1(&source, options).await?;
            Ok::<_, RuntimeInstallError>((
                relative.clone(),
                verified.path,
                verified.origin,
                verified.bytes,
            ))
        }))
        .await;
        // Settle each bounded batch before returning an error, so no
        // in-flight transfer is dropped with an open staging file.
        for result in results {
            let (relative, path, origin, bytes) = result?;
            if origin == crate::cache::ArtifactOrigin::Downloaded {
                downloads += 1;
                downloaded_bytes += bytes;
            }
            acquired.insert(relative.clone(), path);
            progress(report(
                RuntimeInstallPhase::Acquiring,
                acquired.len() as u32,
                total,
                Some(relative),
            ));
        }
    }
    progress(report(RuntimeInstallPhase::Acquiring, total, total, None));
    let acquisition_finished = Instant::now();

    let component_root = managed.runtimes_dir().join(plan.component());
    let staging = component_root.join(format!("{STAGING_PREFIX}{}", plan.identity()));
    let staged_root = staging.join(STAGED_RUNTIME_DIR);
    prepare_staging(managed, plan, &staging, &staged_root)?;

    for (index, entry) in plan.entries().iter().enumerate() {
        progress(report(
            RuntimeInstallPhase::Materializing,
            index as u32,
            plan.entries().len() as u32,
            Some(entry.path().to_owned()),
        ));
        let target = join_relative(&staged_root, entry.path());
        if !target.starts_with(&staged_root) {
            return Err(RuntimeInstallError::Materialization {
                path: entry.path().to_owned(),
                reason: "derived path escaped staging".to_owned(),
            });
        }
        match entry {
            RuntimeEntry::Directory { .. } => std::fs::create_dir_all(&target)
                .map_err(|error| materialization(entry.path(), error))?,
            RuntimeEntry::File {
                path,
                executable,
                artifact,
            } => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| materialization(path, error))?;
                }
                let source = acquired.get(path).expect("every planned file was acquired");
                let copied =
                    std::fs::copy(source, &target).map_err(|error| materialization(path, error))?;
                if copied != artifact.size_bytes() {
                    return Err(RuntimeInstallError::Materialization {
                        path: path.clone(),
                        reason: format!(
                            "copied {copied} bytes, expected {}",
                            artifact.size_bytes()
                        ),
                    });
                }
                set_executable_if_needed(&target, *executable)
                    .map_err(|error| materialization(path, error))?;
                if faults.fail_during_materialization && index == 1 {
                    return Err(RuntimeInstallError::Materialization {
                        path: path.clone(),
                        reason: "deterministic fault injected during materialization".to_owned(),
                    });
                }
            }
            RuntimeEntry::Link {
                path,
                target: link_target,
            } => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| materialization(path, error))?;
                }
                create_runtime_link(link_target, &target)
                    .map_err(|error| materialization(path, error))?;
            }
        }
    }
    let materialization_finished = Instant::now();

    let state = RuntimeInstalledState::from_plan(plan, now_unix_seconds());
    progress(report(RuntimeInstallPhase::Validating, 0, total, None));
    let staged_validation = validate_root_against_state(&staged_root, plan, &state);
    if !staged_validation.problems.is_empty() {
        return Err(RuntimeInstallError::Validation(
            staged_validation.problems.join("; "),
        ));
    }
    let staged_validation_finished = Instant::now();
    // Completion marker written last. It is still invisible at the final path.
    std::fs::write(staged_root.join(RUNTIME_STATE_FILE_NAME), state.to_json())
        .map_err(|error| RuntimeInstallError::State(RuntimeStateError::Write(error)))?;

    if faults.fail_before_commit {
        return Err(RuntimeInstallError::Commit(
            "deterministic fault injected before runtime promotion".to_owned(),
        ));
    }

    progress(report(RuntimeInstallPhase::Committing, total, total, None));
    promote_staged_runtime(managed, plan, &root, &staging, &staged_root, replacing)?;
    let commit_finished = Instant::now();

    if execute_diagnostic {
        progress(report(
            RuntimeInstallPhase::Executing,
            total,
            total,
            Some(plan.diagnostic_executable().to_owned()),
        ));
    }
    let final_validation = validate_runtime(
        managed,
        plan,
        execute_diagnostic,
        DEFAULT_JAVA_DIAGNOSTIC_TIMEOUT,
    )
    .await?;
    if final_validation.status != RuntimeValidationStatus::Ready {
        return Err(RuntimeInstallError::Validation(
            final_validation.problems.join("; "),
        ));
    }
    if std::env::var_os("AURORA_INSTALL_DIAGNOSTICS").is_some() {
        eprintln!(
            "[aurora-runtime] reused=false total_ms={} acquisition_ms={} materialization_ms={} staged_validation_ms={} commit_ms={} final_validation_ms={} files={} cache_hits={} downloads={} downloaded_bytes={} limit={}",
            started.elapsed().as_millis(),
            acquisition_finished.duration_since(started).as_millis(),
            materialization_finished
                .duration_since(acquisition_finished)
                .as_millis(),
            staged_validation_finished
                .duration_since(materialization_finished)
                .as_millis(),
            commit_finished
                .duration_since(staged_validation_finished)
                .as_millis(),
            started
                .elapsed()
                .saturating_sub(commit_finished.duration_since(started))
                .as_millis(),
            total,
            total as usize - downloads,
            downloads,
            downloaded_bytes,
            RUNTIME_DOWNLOAD_CONCURRENCY,
        );
    }
    Ok(installed_result(plan, managed, state, false))
}

pub async fn validate_runtime(
    managed: &ManagedPaths,
    plan: &JavaRuntimePlan,
    execute_diagnostic: bool,
    timeout: Duration,
) -> Result<RuntimeValidation, RuntimeInstallError> {
    let root = plan.installation_dir(managed);
    let root_metadata = match std::fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RuntimeValidation {
                status: RuntimeValidationStatus::Missing,
                component: plan.component().to_owned(),
                required_major_version: plan.required_major_version(),
                runtime_version: None,
                root,
                launch_executable: None,
                checked_files: 0,
                verified_bytes: 0,
                diagnostic: None,
                problems: Vec::new(),
            });
        }
        Err(error) => {
            return Ok(damaged(
                plan,
                root,
                vec![format!("the runtime location is unreadable: {error}")],
            ));
        }
    };
    if !root_metadata.file_type().is_dir() {
        return Ok(damaged(
            plan,
            root,
            vec!["the runtime location exists but is not a directory".to_owned()],
        ));
    }
    let state = match load_runtime_state(&root) {
        Ok(Some(state)) => state,
        Ok(None) => {
            return Ok(damaged(
                plan,
                root,
                vec!["the runtime completion state is missing".to_owned()],
            ));
        }
        Err(error) => return Ok(damaged(plan, root, vec![error.to_string()])),
    };
    let mut validation = validate_root_against_state(&root, plan, &state);
    if validation.problems.is_empty() && execute_diagnostic {
        match run_java_diagnostic(
            &join_relative(&root, state.diagnostic_executable()),
            plan.required_major_version(),
            timeout,
        )
        .await
        {
            Ok(diagnostic) => validation.diagnostic = Some(diagnostic),
            Err(reason) => validation.problems.push(reason),
        }
    }
    validation.status = if validation.problems.is_empty() {
        RuntimeValidationStatus::Ready
    } else {
        RuntimeValidationStatus::Damaged
    };
    Ok(validation)
}

fn validate_root_against_state(
    root: &Path,
    plan: &JavaRuntimePlan,
    state: &RuntimeInstalledState,
) -> RuntimeValidation {
    let mut problems = Vec::new();
    if !state.matches_plan(plan) {
        problems.push(
            "the installed runtime identity does not match the exact resolved plan".to_owned(),
        );
    }
    let mut checked_files = 0u32;
    let mut verified_bytes = 0u64;
    for entry in state.entries() {
        let path = join_relative(root, entry.path());
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                problems.push(format!(
                    "{} is missing or unreadable: {error}",
                    entry.path()
                ));
                continue;
            }
        };
        match entry.kind() {
            RuntimeStateEntryKind::Directory => {
                if !metadata.file_type().is_dir() {
                    problems.push(format!("{} is not a directory", entry.path()));
                }
            }
            RuntimeStateEntryKind::File => {
                checked_files += 1;
                if !metadata.file_type().is_file() {
                    problems.push(format!("{} is not a regular file", entry.path()));
                    continue;
                }
                let expected = match Sha1Digest::parse(entry.sha1().unwrap_or_default()) {
                    Ok(value) => value,
                    Err(error) => {
                        problems.push(format!("{} records invalid SHA-1: {error}", entry.path()));
                        continue;
                    }
                };
                match verify_file_sha1(&path, &expected, entry.size_bytes()) {
                    Ok(bytes) => verified_bytes += bytes,
                    Err(error) => problems.push(format!(
                        "{} failed integrity validation: {error}",
                        entry.path()
                    )),
                }
                if entry.executable() && !has_executable_permission(&path) {
                    problems.push(format!("{} is not executable", entry.path()));
                }
            }
            RuntimeStateEntryKind::Link => {
                if !metadata.file_type().is_symlink() {
                    problems.push(format!("{} is not a symbolic link", entry.path()));
                } else if std::fs::read_link(&path).ok().as_deref()
                    != Some(Path::new(entry.target().unwrap_or_default()))
                {
                    problems.push(format!(
                        "{} does not point to its recorded target",
                        entry.path()
                    ));
                }
            }
        }
    }
    RuntimeValidation {
        status: RuntimeValidationStatus::Damaged,
        component: plan.component().to_owned(),
        required_major_version: plan.required_major_version(),
        runtime_version: Some(state.runtime_version().to_owned()),
        root: root.to_path_buf(),
        launch_executable: Some(join_relative(root, state.launch_executable())),
        checked_files,
        verified_bytes,
        diagnostic: None,
        problems,
    }
}

fn damaged(plan: &JavaRuntimePlan, root: PathBuf, problems: Vec<String>) -> RuntimeValidation {
    RuntimeValidation {
        status: RuntimeValidationStatus::Damaged,
        component: plan.component().to_owned(),
        required_major_version: plan.required_major_version(),
        runtime_version: None,
        root,
        launch_executable: None,
        checked_files: 0,
        verified_bytes: 0,
        diagnostic: None,
        problems,
    }
}

fn decide_replacement(root: &Path, plan: &JavaRuntimePlan) -> Result<bool, RuntimeInstallError> {
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(RuntimeInstallError::Storage(error)),
    };
    if !metadata.file_type().is_dir() {
        return Err(RuntimeInstallError::TargetConflict {
            path: root.display().to_string(),
            reason: "the runtime location is not a directory".to_owned(),
        });
    }
    match load_runtime_state(root) {
        Ok(Some(state)) if state.component() == plan.component() && state.identity() == plan.identity() => Ok(true),
        Ok(Some(_)) => Err(RuntimeInstallError::TargetConflict { path: root.display().to_string(), reason: "the installed-state identity does not prove ownership of this exact runtime path".to_owned() }),
        Ok(None) => Err(RuntimeInstallError::TargetConflict { path: root.display().to_string(), reason: "the directory has no runtime completion state; Aurora will not overwrite a partial or foreign tree".to_owned() }),
        Err(error) => Err(RuntimeInstallError::State(error)),
    }
}

fn prepare_staging(
    managed: &ManagedPaths,
    plan: &JavaRuntimePlan,
    staging: &Path,
    staged_root: &Path,
) -> Result<(), RuntimeInstallError> {
    prove_staging(managed, plan, staging)?;
    ensure_real_directory_or_missing(&managed.runtimes_dir())?;
    ensure_real_directory_or_missing(&managed.runtimes_dir().join(plan.component()))?;
    match std::fs::symlink_metadata(staging) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            std::fs::remove_dir_all(staging).map_err(RuntimeInstallError::Storage)?;
        }
        Ok(_) => {
            return Err(RuntimeInstallError::TargetConflict {
                path: staging.display().to_string(),
                reason: "the exact staging path exists but is not a real directory".to_owned(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(RuntimeInstallError::Storage(error)),
    }
    std::fs::create_dir_all(staged_root).map_err(RuntimeInstallError::Storage)
}

fn ensure_real_directory_or_missing(path: &Path) -> Result<(), RuntimeInstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(RuntimeInstallError::TargetConflict {
            path: path.display().to_string(),
            reason: "a managed runtime ancestor exists but is not a real directory".to_owned(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeInstallError::Storage(error)),
    }
}

fn prove_staging(
    managed: &ManagedPaths,
    plan: &JavaRuntimePlan,
    staging: &Path,
) -> Result<(), RuntimeInstallError> {
    let component_root = managed.runtimes_dir().join(plan.component());
    let expected = component_root.join(format!("{STAGING_PREFIX}{}", plan.identity()));
    if staging != expected || !staging.starts_with(managed.runtimes_dir()) {
        return Err(RuntimeInstallError::Storage(std::io::Error::other(
            "refusing to modify a path that is not the exact derived runtime staging directory",
        )));
    }
    Ok(())
}

fn promote_staged_runtime(
    managed: &ManagedPaths,
    plan: &JavaRuntimePlan,
    root: &Path,
    staging: &Path,
    staged_root: &Path,
    replacing: bool,
) -> Result<(), RuntimeInstallError> {
    prove_staging(managed, plan, staging)?;
    if replacing {
        let retired = staging.join(REPLACED_RUNTIME_DIR);
        std::fs::rename(root, &retired)
            .map_err(|error| RuntimeInstallError::Commit(error.to_string()))?;
        if let Err(error) = std::fs::rename(staged_root, root) {
            let _ = std::fs::rename(&retired, root);
            return Err(RuntimeInstallError::Commit(format!(
                "promotion failed and the previous runtime was restored: {error}"
            )));
        }
        if let Err(error) = std::fs::remove_dir_all(staging) {
            eprintln!(
                "[aurora-launcher] runtime committed but retired-tree cleanup failed: {error}"
            );
        }
    } else {
        std::fs::rename(staged_root, root)
            .map_err(|error| RuntimeInstallError::Commit(error.to_string()))?;
        let _ = std::fs::remove_dir_all(staging);
    }
    Ok(())
}

fn installed_result(
    plan: &JavaRuntimePlan,
    managed: &ManagedPaths,
    state: RuntimeInstalledState,
    reused: bool,
) -> InstalledRuntime {
    InstalledRuntime {
        root: plan.installation_dir(managed),
        launch_executable: plan.launch_executable_path(managed),
        diagnostic_executable: plan.diagnostic_executable_path(managed),
        state,
        reused,
    }
}

async fn run_java_diagnostic(
    executable: &Path,
    expected_major: u32,
    timeout: Duration,
) -> Result<JavaDiagnostic, String> {
    let output = run_process(executable, &["-version"], timeout).await?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(format!(
            "managed Java diagnostic exited with status {}: {}",
            output.status,
            sanitize_diagnostic(&combined)
        ));
    }
    let reported = parse_reported_java_major(&combined).ok_or_else(|| {
        format!(
            "managed Java diagnostic did not report a recognizable version: {}",
            sanitize_diagnostic(&combined)
        )
    })?;
    validate_reported_major(reported, expected_major)?;
    Ok(JavaDiagnostic {
        reported_major_version: reported,
        summary: sanitize_diagnostic(&combined),
    })
}

fn validate_reported_major(reported: u32, expected: u32) -> Result<(), String> {
    if reported == expected {
        Ok(())
    } else {
        Err(format!(
            "managed Java reported major version {reported}, expected {expected}"
        ))
    }
}

async fn run_process(
    executable: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    let mut command = tokio::process::Command::new(executable);
    command.args(arguments).kill_on_drop(true).env_clear();
    #[cfg(windows)]
    for key in ["SYSTEMROOT", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(unix)]
    command.env("LANG", "C").env("LC_ALL", "C");
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| {
            format!(
                "managed Java diagnostic exceeded the {} second timeout",
                timeout.as_secs_f64()
            )
        })?
        .map_err(|error| format!("managed Java diagnostic could not start: {error}"))
}

fn parse_reported_java_major(output: &str) -> Option<u32> {
    crate::runtime::plan::parse_java_major(output.split('"').nth(1)?)
}

fn sanitize_diagnostic(value: &str) -> String {
    let flattened = value.replace(['\r', '\n'], " ");
    flattened
        .chars()
        .take(4096)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(unix)]
fn set_executable_if_needed(path: &Path, executable: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if executable {
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
#[cfg(not(unix))]
fn set_executable_if_needed(_path: &Path, _executable: bool) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn has_executable_permission(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|value| value.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
#[cfg(not(unix))]
fn has_executable_permission(_path: &Path) -> bool {
    true
}

#[cfg(unix)]
fn create_runtime_link(target: &str, path: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}
#[cfg(windows)]
fn create_runtime_link(_target: &str, _path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "official Windows runtime manifests do not define links; refusing ambiguous Windows link creation",
    ))
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}
fn report(
    phase: RuntimeInstallPhase,
    completed_items: u32,
    total_items: u32,
    current_item: Option<String>,
) -> InstallRuntimeProgress {
    InstallRuntimeProgress {
        phase,
        completed_items,
        total_items,
        current_item,
    }
}
fn materialization(path: &str, error: std::io::Error) -> RuntimeInstallError {
    RuntimeInstallError::Materialization {
        path: path.to_owned(),
        reason: error.to_string(),
    }
}

fn runtime_installation_lock(identity: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    let locks = LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(identity.to_owned())
        .or_default()
        .clone()
}

#[derive(Debug)]
pub enum RuntimeInstallError {
    AlreadyInProgress { identity: String },
    Plan(String),
    Acquisition(AcquisitionError),
    State(RuntimeStateError),
    TargetConflict { path: String, reason: String },
    Materialization { path: String, reason: String },
    Validation(String),
    Commit(String),
    Storage(std::io::Error),
}

impl fmt::Display for RuntimeInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInProgress { identity } => write!(
                f,
                "managed runtime '{identity}' is already being installed in this process"
            ),
            Self::Plan(reason) => write!(f, "the managed runtime plan is invalid: {reason}"),
            Self::Acquisition(error) => write!(
                f,
                "a managed runtime artifact could not be acquired: {error}"
            ),
            Self::State(error) => write!(f, "{error}"),
            Self::TargetConflict { path, reason } => write!(
                f,
                "managed runtime target '{path}' is unsafe to replace: {reason}"
            ),
            Self::Materialization { path, reason } => write!(
                f,
                "managed runtime entry '{path}' could not be materialized: {reason}"
            ),
            Self::Validation(reason) => write!(f, "managed runtime validation failed: {reason}"),
            Self::Commit(reason) => write!(f, "managed runtime activation failed: {reason}"),
            Self::Storage(error) => write!(f, "managed runtime storage failed: {error}"),
        }
    }
}

impl std::error::Error for RuntimeInstallError {}
impl From<AcquisitionError> for RuntimeInstallError {
    fn from(value: AcquisitionError) -> Self {
        Self::Acquisition(value)
    }
}
impl From<RuntimeStateError> for RuntimeInstallError {
    fn from(value: RuntimeStateError) -> Self {
        Self::State(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::metadata::{
        RuntimeDownload, RuntimeDownloads, RuntimeFileDocument, RuntimeFileKind, RuntimeSelection,
    };
    use crate::runtime::plan::{RuntimeArchitecture, RuntimeOperatingSystem, RuntimePlatform};
    use crate::test_support::{TestResponse, TestServer};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore = "controlled managed Java performance diagnostic"]
    async fn benchmark_controlled_runtime_installation() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Instant;

        let mut bodies = BTreeMap::new();
        bodies.insert("/bin-java.exe".to_owned(), b"java-runtime".to_vec());
        bodies.insert("/bin-javaw.exe".to_owned(), b"javaw-runtime".to_vec());
        for number in 0..128u32 {
            bodies.insert(
                format!("/file-{number:04}"),
                format!("runtime file {number:04}").into_bytes(),
            );
        }
        let bodies = Arc::new(bodies);
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let server = TestServer::spawn(Arc::new({
            let bodies = Arc::clone(&bodies);
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            move |request| {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(count, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(18));
                let response = bodies
                    .get(&request.path)
                    .map(|body| TestResponse::ok(body))
                    .unwrap_or_else(|| TestResponse::status(404));
                active.fetch_sub(1, Ordering::SeqCst);
                response
            }
        }));
        let mut files = BTreeMap::new();
        files.insert("bin".to_owned(), RuntimeFileKind::Directory);
        for (path, url_path, executable) in [
            ("bin/java.exe".to_owned(), "/bin-java.exe".to_owned(), true),
            (
                "bin/javaw.exe".to_owned(),
                "/bin-javaw.exe".to_owned(),
                true,
            ),
        ]
        .into_iter()
        .chain((0..128u32).map(|number| {
            (
                format!("lib/file-{number:04}"),
                format!("/file-{number:04}"),
                false,
            )
        })) {
            let body = &bodies[&url_path];
            files.insert(
                path,
                RuntimeFileKind::File {
                    executable,
                    downloads: RuntimeDownloads {
                        raw: RuntimeDownload {
                            sha1: Sha1Digest::compute(body).as_hex(),
                            size: body.len() as u64,
                            url: format!("{}{}", server.base_url(), url_path),
                        },
                        lzma: None,
                    },
                },
            );
        }
        let plan = JavaRuntimePlan::from_metadata(
            "java-runtime-epsilon",
            25,
            RuntimePlatform::new(RuntimeOperatingSystem::Windows, RuntimeArchitecture::X86_64),
            RuntimeSelection {
                version_name: "25.0.1".to_owned(),
                released: "2025-10-12".to_owned(),
                manifest_sha1: Sha1Digest::compute(server.base_url().as_bytes()),
            },
            RuntimeFileDocument { files },
        )
        .unwrap();
        let root =
            std::env::temp_dir().join(format!("aurora-runtime-bench-{}", uuid::Uuid::new_v4()));
        let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();
        for scenario in ["cold", "warm"] {
            maximum.store(0, Ordering::SeqCst);
            let before = server.request_count();
            let start = Instant::now();
            ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
                .await
                .unwrap();
            println!(
                "RUNTIME BENCH {scenario} total_ms={} requests={} max_concurrency={}",
                start.elapsed().as_millis(),
                server.request_count() - before,
                maximum.load(Ordering::SeqCst)
            );
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn test_plan(base: &str, java: &[u8], javaw: &[u8]) -> JavaRuntimePlan {
        let mut files = BTreeMap::new();
        files.insert("bin".to_owned(), RuntimeFileKind::Directory);
        for (name, body) in [("bin/java.exe", java), ("bin/javaw.exe", javaw)] {
            files.insert(
                name.to_owned(),
                RuntimeFileKind::File {
                    executable: true,
                    downloads: RuntimeDownloads {
                        raw: RuntimeDownload {
                            sha1: Sha1Digest::compute(body).as_hex(),
                            size: body.len() as u64,
                            url: format!("{base}/{}", name.replace('/', "-")),
                        },
                        lzma: None,
                    },
                },
            );
        }
        JavaRuntimePlan::from_metadata(
            "java-runtime-epsilon",
            25,
            RuntimePlatform::new(RuntimeOperatingSystem::Windows, RuntimeArchitecture::X86_64),
            RuntimeSelection {
                version_name: "25.0.1".to_owned(),
                released: "2025-10-12".to_owned(),
                manifest_sha1: Sha1Digest::compute(base.as_bytes()),
            },
            RuntimeFileDocument { files },
        )
        .unwrap()
    }

    fn options() -> DownloadOptions {
        DownloadOptions {
            connect_timeout: Duration::from_secs(5),
            idle_read_timeout: Duration::from_secs(5),
            max_redirects: crate::downloads::MAX_REDIRECTS,
        }
    }
    fn managed(name: &str) -> ManagedPaths {
        ManagedPaths::from_app_local_data_dir(std::env::temp_dir().join(format!(
            "aurora-runtime-install-{}-{name}",
            std::process::id()
        )))
        .unwrap()
    }

    #[tokio::test]
    async fn installs_validates_reuses_and_repairs_from_verified_cache() {
        let server = TestServer::spawn(Arc::new(|request| match request.path.as_str() {
            "/bin-java.exe" => TestResponse::ok(b"java-runtime"),
            "/bin-javaw.exe" => TestResponse::ok(b"javaw-runtime"),
            _ => TestResponse::status(404),
        }));
        let managed = managed("reuse");
        let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
        let first = ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap();
        assert!(!first.reused());
        assert_eq!(server.request_count(), 2);
        let validation = validate_runtime(&managed, &plan, false, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(validation.status, RuntimeValidationStatus::Ready);
        assert_eq!(validation.checked_files, 2);
        let second = ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap();
        assert!(second.reused());
        assert_eq!(server.request_count(), 2);
        let state_path = plan
            .installation_dir(&managed)
            .join(RUNTIME_STATE_FILE_NAME);
        let mut edited: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
        edited["entries"].as_array_mut().unwrap().remove(0);
        std::fs::write(&state_path, serde_json::to_string_pretty(&edited).unwrap()).unwrap();
        assert_eq!(
            validate_runtime(&managed, &plan, false, Duration::from_secs(1))
                .await
                .unwrap()
                .status,
            RuntimeValidationStatus::Damaged,
            "an edited state cannot omit a planned entry"
        );
        ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap();
        std::fs::write(plan.diagnostic_executable_path(&managed), b"damage").unwrap();
        assert_eq!(
            validate_runtime(&managed, &plan, false, Duration::from_secs(1))
                .await
                .unwrap()
                .status,
            RuntimeValidationStatus::Damaged
        );
        let repaired = ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap();
        assert!(!repaired.reused());
        assert_eq!(
            server.request_count(),
            2,
            "repair must use revalidated cache objects"
        );
        assert_eq!(
            validate_runtime(&managed, &plan, false, Duration::from_secs(1))
                .await
                .unwrap()
                .status,
            RuntimeValidationStatus::Ready
        );
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[tokio::test]
    async fn acquisition_failure_never_exposes_a_complete_runtime() {
        let server = TestServer::spawn(Arc::new(|request| {
            if request.path == "/bin-java.exe" {
                TestResponse::ok(b"java-runtime")
            } else {
                TestResponse::status(404)
            }
        }));
        let managed = managed("failure");
        let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
        assert!(
            ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
                .await
                .is_err()
        );
        assert!(!plan.installation_dir(&managed).exists());
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[tokio::test]
    async fn wrong_runtime_digest_and_size_fail_acquisition() {
        for (name, java_response) in [
            ("wrong-digest", b"same-length!".as_slice()),
            ("wrong-size", b"short".as_slice()),
        ] {
            let response = java_response.to_vec();
            let server = TestServer::spawn(Arc::new(move |request| match request.path.as_str() {
                "/bin-java.exe" => TestResponse::ok(&response),
                "/bin-javaw.exe" => TestResponse::ok(b"javaw-runtime"),
                _ => TestResponse::status(404),
            }));
            let managed = managed(name);
            let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
            let error = ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
                .await
                .unwrap_err();
            assert!(matches!(error, RuntimeInstallError::Acquisition(_)));
            assert!(!plan.installation_dir(&managed).exists());
            let _ = std::fs::remove_dir_all(managed.data_root());
        }
    }

    #[tokio::test]
    async fn corrupt_cache_object_is_revalidated_and_reacquired() {
        let server = TestServer::spawn(Arc::new(|request| match request.path.as_str() {
            "/bin-java.exe" => TestResponse::ok(b"java-runtime"),
            "/bin-javaw.exe" => TestResponse::ok(b"javaw-runtime"),
            _ => TestResponse::status(404),
        }));
        let managed = managed("cache-corruption");
        let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
        ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap();
        let java_digest = Sha1Digest::compute(b"java-runtime");
        let cache_path = ArtifactCache::new(managed.clone()).verified_sha1_path(&java_digest);
        std::fs::write(&cache_path, b"cache-damage").unwrap();
        std::fs::write(plan.diagnostic_executable_path(&managed), b"runtime-damage").unwrap();
        ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap();
        assert_eq!(
            server.request_count(),
            3,
            "only the corrupt cache object redownloads"
        );
        assert_eq!(
            validate_runtime(&managed, &plan, false, Duration::from_secs(1))
                .await
                .unwrap()
                .status,
            RuntimeValidationStatus::Ready
        );
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[tokio::test]
    async fn staging_faults_never_commit_and_retry_recovers_stale_staging() {
        let server = TestServer::spawn(Arc::new(|request| match request.path.as_str() {
            "/bin-java.exe" => TestResponse::ok(b"java-runtime"),
            "/bin-javaw.exe" => TestResponse::ok(b"javaw-runtime"),
            _ => TestResponse::status(404),
        }));
        for (name, faults) in [
            (
                "materialization-fault",
                RuntimeInstallFaults {
                    fail_during_materialization: true,
                    fail_before_commit: false,
                },
            ),
            (
                "precommit-fault",
                RuntimeInstallFaults {
                    fail_during_materialization: false,
                    fail_before_commit: true,
                },
            ),
        ] {
            let managed = managed(name);
            let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
            let error =
                ensure_runtime_with_faults(&managed, &plan, &options(), false, &mut |_| {}, faults)
                    .await
                    .unwrap_err();
            assert!(matches!(
                error,
                RuntimeInstallError::Materialization { .. } | RuntimeInstallError::Commit(_)
            ));
            assert!(
                !plan.installation_dir(&managed).exists(),
                "staging must never appear complete at the final path"
            );
            ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
                .await
                .unwrap();
            let staging = managed
                .runtimes_dir()
                .join(plan.component())
                .join(format!("{STAGING_PREFIX}{}", plan.identity()));
            assert!(
                !staging.exists(),
                "successful retry cleans the fixed staging tree"
            );
            let _ = std::fs::remove_dir_all(managed.data_root());
        }
    }

    #[tokio::test]
    async fn failed_download_can_be_retried_deterministically() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handler_attempts = Arc::clone(&attempts);
        let server = TestServer::spawn(Arc::new(move |request| match request.path.as_str() {
            "/bin-java.exe" => TestResponse::ok(b"java-runtime"),
            "/bin-javaw.exe"
                if handler_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 =>
            {
                TestResponse::status(500)
            }
            "/bin-javaw.exe" => TestResponse::ok(b"javaw-runtime"),
            _ => TestResponse::status(404),
        }));
        let managed = managed("network-retry");
        let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
        assert!(
            ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
                .await
                .is_err()
        );
        assert!(!plan.installation_dir(&managed).exists());
        ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap();
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[tokio::test]
    async fn malformed_or_foreign_final_tree_is_never_overwritten() {
        let server = TestServer::spawn(Arc::new(|_| TestResponse::status(404)));
        let managed = managed("foreign");
        let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
        std::fs::create_dir_all(plan.installation_dir(&managed)).unwrap();
        std::fs::write(plan.installation_dir(&managed).join("foreign.txt"), b"keep").unwrap();
        let error = ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeInstallError::TargetConflict { .. }));
        assert!(plan.installation_dir(&managed).join("foreign.txt").exists());
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[tokio::test]
    async fn unsupported_runtime_state_is_reported_and_preserved() {
        let server = TestServer::spawn(Arc::new(|request| match request.path.as_str() {
            "/bin-java.exe" => TestResponse::ok(b"java-runtime"),
            "/bin-javaw.exe" => TestResponse::ok(b"javaw-runtime"),
            _ => TestResponse::status(404),
        }));
        let managed = managed("unsupported-state");
        let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
        ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap();
        let state_path = plan
            .installation_dir(&managed)
            .join(RUNTIME_STATE_FILE_NAME);
        let unsupported = std::fs::read_to_string(&state_path).unwrap().replacen(
            "\"schemaVersion\": 1",
            "\"schemaVersion\": 99",
            1,
        );
        std::fs::write(&state_path, &unsupported).unwrap();
        let validation = validate_runtime(&managed, &plan, false, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(validation.status, RuntimeValidationStatus::Damaged);
        let error = ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap_err();
        assert!(matches!(error, RuntimeInstallError::State(_)));
        assert_eq!(std::fs::read_to_string(&state_path).unwrap(), unsupported);
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[tokio::test]
    async fn overlapping_same_runtime_installs_fail_immediately() {
        let server = TestServer::spawn(Arc::new(|request| {
            let response = match request.path.as_str() {
                "/bin-java.exe" => TestResponse::ok(b"java-runtime"),
                "/bin-javaw.exe" => TestResponse::ok(b"javaw-runtime"),
                _ => TestResponse::status(404),
            };
            response.with_drip_delay(Duration::from_millis(10))
        }));
        let managed = managed("concurrent");
        let plan = test_plan(server.base_url(), b"java-runtime", b"javaw-runtime");
        let first_managed = managed.clone();
        let first_plan = plan.clone();
        let first_options = options();
        let first = tokio::spawn(async move {
            ensure_runtime(
                &first_managed,
                &first_plan,
                &first_options,
                false,
                &mut |_| {},
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let second = ensure_runtime(&managed, &plan, &options(), false, &mut |_| {})
            .await
            .unwrap_err();
        assert!(matches!(
            second,
            RuntimeInstallError::AlreadyInProgress { .. }
        ));
        first.await.unwrap().unwrap();
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[test]
    fn version_output_is_parsed_without_launching_a_shell() {
        assert_eq!(
            parse_reported_java_major("openjdk version \"25.0.1\" 2025-10-21"),
            Some(25)
        );
        assert_eq!(
            parse_reported_java_major("java version \"1.8.0_51\""),
            Some(8)
        );
        assert_eq!(parse_reported_java_major("not java"), None);
        assert!(validate_reported_major(25, 25).is_ok());
        assert_eq!(
            validate_reported_major(21, 25).unwrap_err(),
            "managed Java reported major version 21, expected 25"
        );
    }

    #[test]
    #[ignore]
    fn slow_process_helper() {
        std::thread::sleep(Duration::from_secs(3));
    }

    #[tokio::test]
    async fn process_timeout_is_bounded_and_structured() {
        let executable = std::env::current_exe().unwrap();
        let result = run_process(
            &executable,
            &[
                "--exact",
                "runtime::install::tests::slow_process_helper",
                "--ignored",
            ],
            Duration::from_millis(50),
        )
        .await;
        assert!(result.unwrap_err().contains("timeout"));
    }

    #[tokio::test]
    async fn failed_java_execution_is_structured() {
        let executable = std::env::current_exe().unwrap();
        let error = run_java_diagnostic(&executable, 25, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(error.contains("exited with status"), "{error}");
    }

    /// Controlled Phase 7 live verification. It resolves Minecraft 26.2's
    /// Java requirement from official metadata, installs the official runtime
    /// into one disposable managed root, executes it, proves warm reuse, and
    /// removes only that exact root.
    #[tokio::test]
    #[ignore = "downloads and installs the live official Mojang Java runtime"]
    async fn live_mojang_java_25_cold_and_warm() {
        let disposable = std::env::temp_dir().join(format!(
            "aurora-phase7-live-{}-{}",
            std::process::id(),
            now_unix_seconds()
        ));
        assert!(disposable.starts_with(std::env::temp_dir()));
        assert!(
            disposable
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("aurora-phase7-live-"))
        );
        let managed = ManagedPaths::from_app_local_data_dir(disposable.clone()).unwrap();
        let result: Result<(), String> = async {
            let minecraft = crate::minecraft::metadata::MinecraftVersionId::new("26.2")
                .map_err(|error| error.to_string())?;
            let game_platform = crate::minecraft::rules::PlatformProfile::current()
                .map_err(|error| error.to_string())?;
            let minecraft_plan = crate::minecraft::resolve_install_plan(
                &crate::minecraft::metadata::MetadataEndpoints::official(),
                &minecraft,
                game_platform,
                &DownloadOptions::default(),
            )
            .await
            .map_err(|error| error.to_string())?;
            assert_eq!(minecraft_plan.java().component(), "java-runtime-epsilon");
            assert_eq!(minecraft_plan.java().major_version(), 25);
            let runtime_plan = crate::runtime::metadata::resolve_runtime_plan(
                &managed,
                &crate::runtime::metadata::RuntimeMetadataEndpoints::official(),
                minecraft_plan.java().component(),
                minecraft_plan.java().major_version(),
                RuntimePlatform::current().map_err(|error| error.to_string())?,
                &DownloadOptions::default(),
            )
            .await
            .map_err(|error| error.to_string())?;
            let cold_start = std::time::Instant::now();
            let cold = ensure_runtime(
                &managed,
                &runtime_plan,
                &DownloadOptions::default(),
                true,
                &mut |progress| {
                    if progress.completed_items % 50 == 0
                        || progress.completed_items == progress.total_items
                    {
                        eprintln!(
                            "live runtime {} {}/{} {}",
                            progress.phase.as_str(),
                            progress.completed_items,
                            progress.total_items,
                            progress.current_item.as_deref().unwrap_or("")
                        );
                    }
                },
            )
            .await
            .map_err(|error| error.to_string())?;
            let cold_elapsed = cold_start.elapsed();
            assert!(!cold.reused());
            let validation = validate_runtime(
                &managed,
                &runtime_plan,
                true,
                DEFAULT_JAVA_DIAGNOSTIC_TIMEOUT,
            )
            .await
            .map_err(|error| error.to_string())?;
            assert_eq!(validation.status, RuntimeValidationStatus::Ready);
            assert_eq!(
                validation
                    .diagnostic
                    .as_ref()
                    .map(|value| value.reported_major_version),
                Some(25)
            );
            let warm_start = std::time::Instant::now();
            let warm = ensure_runtime(
                &managed,
                &runtime_plan,
                &DownloadOptions::default(),
                true,
                &mut |_| {},
            )
            .await
            .map_err(|error| error.to_string())?;
            let warm_elapsed = warm_start.elapsed();
            assert!(warm.reused());
            eprintln!(
                "LIVE_RESULT component={} version={} identity={} platform={} files={} bytes={} cold_ms={} warm_ms={} executable={} diagnostic={}",
                runtime_plan.component(),
                runtime_plan.runtime_version(),
                runtime_plan.identity(),
                runtime_plan.platform_key(),
                validation.checked_files,
                validation.verified_bytes,
                cold_elapsed.as_millis(),
                warm_elapsed.as_millis(),
                warm.launch_executable().display(),
                validation
                    .diagnostic
                    .as_ref()
                    .map(|value| value.summary.as_str())
                    .unwrap_or("missing")
            );
            assert!(
                !managed.instances_dir().exists(),
                "runtime verification must not create or change instance game files"
            );
            Ok(())
        }
        .await;
        if disposable.exists() {
            std::fs::remove_dir_all(&disposable).expect("proven disposable managed root cleanup");
        }
        result.unwrap();
        assert!(!disposable.exists());
    }
}
