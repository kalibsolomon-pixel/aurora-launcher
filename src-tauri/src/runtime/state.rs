//! Versioned completion state for one launcher-managed shared Java runtime.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::integrity::Sha1Digest;
use crate::runtime::plan::{
    JavaRuntimePlan, RuntimeEntry, validate_component, validate_link_target, validate_relative_path,
};

pub const RUNTIME_STATE_SCHEMA_VERSION: u32 = 1;
pub const RUNTIME_STATE_FILE_NAME: &str = "runtime-installed.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInstalledState {
    schema_version: u32,
    component: String,
    required_major_version: u32,
    platform_os: String,
    platform_architecture: String,
    platform_key: String,
    runtime_version: String,
    released: String,
    manifest_sha1: String,
    installed_at_unix_seconds: u64,
    launch_executable: String,
    diagnostic_executable: String,
    entries: Vec<RuntimeStateEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStateEntry {
    kind: RuntimeStateEntryKind,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size_bytes: Option<u64>,
    #[serde(default)]
    executable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeStateEntryKind {
    Directory,
    File,
    Link,
}

impl RuntimeInstalledState {
    pub fn from_plan(plan: &JavaRuntimePlan, installed_at_unix_seconds: u64) -> Self {
        let entries = plan
            .entries()
            .iter()
            .map(|entry| match entry {
                RuntimeEntry::Directory { path } => RuntimeStateEntry {
                    kind: RuntimeStateEntryKind::Directory,
                    path: path.clone(),
                    sha1: None,
                    size_bytes: None,
                    executable: false,
                    target: None,
                },
                RuntimeEntry::File {
                    path,
                    executable,
                    artifact,
                } => RuntimeStateEntry {
                    kind: RuntimeStateEntryKind::File,
                    path: path.clone(),
                    sha1: Some(artifact.sha1().as_hex()),
                    size_bytes: Some(artifact.size_bytes()),
                    executable: *executable,
                    target: None,
                },
                RuntimeEntry::Link { path, target } => RuntimeStateEntry {
                    kind: RuntimeStateEntryKind::Link,
                    path: path.clone(),
                    sha1: None,
                    size_bytes: None,
                    executable: false,
                    target: Some(target.clone()),
                },
            })
            .collect();
        Self {
            schema_version: RUNTIME_STATE_SCHEMA_VERSION,
            component: plan.component().to_owned(),
            required_major_version: plan.required_major_version(),
            platform_os: plan.platform().os().as_str().to_owned(),
            platform_architecture: plan.platform().architecture().as_str().to_owned(),
            platform_key: plan.platform_key().to_owned(),
            runtime_version: plan.runtime_version().to_owned(),
            released: plan.released().to_owned(),
            manifest_sha1: plan.manifest_sha1().as_hex(),
            installed_at_unix_seconds,
            launch_executable: plan.launch_executable().to_owned(),
            diagnostic_executable: plan.diagnostic_executable().to_owned(),
            entries,
        }
    }

    pub fn from_json(json: &str) -> Result<Self, RuntimeStateError> {
        let state: Self = serde_json::from_str(json)
            .map_err(|error| RuntimeStateError::Malformed(error.to_string()))?;
        state.validate()?;
        Ok(state)
    }

    fn validate(&self) -> Result<(), RuntimeStateError> {
        if self.schema_version != RUNTIME_STATE_SCHEMA_VERSION {
            return Err(RuntimeStateError::UnsupportedSchema {
                found: self.schema_version,
                supported: RUNTIME_STATE_SCHEMA_VERSION,
            });
        }
        validate_component(&self.component)
            .map_err(|error| RuntimeStateError::Malformed(error.to_string()))?;
        if self.required_major_version == 0
            || self.platform_os.is_empty()
            || self.platform_architecture.is_empty()
            || self.platform_key.is_empty()
            || self.runtime_version.trim().is_empty()
            || self.released.trim().is_empty()
            || self.entries.is_empty()
        {
            return Err(RuntimeStateError::Malformed(
                "runtime identity and entries must be complete".to_owned(),
            ));
        }
        Sha1Digest::parse(&self.manifest_sha1).map_err(|error| {
            RuntimeStateError::Malformed(format!("manifest SHA-1 is invalid: {error}"))
        })?;
        validate_relative_path(&self.launch_executable)
            .map_err(|error| RuntimeStateError::Malformed(error.to_string()))?;
        validate_relative_path(&self.diagnostic_executable)
            .map_err(|error| RuntimeStateError::Malformed(error.to_string()))?;
        let mut paths = HashSet::new();
        for entry in &self.entries {
            validate_relative_path(&entry.path)
                .map_err(|error| RuntimeStateError::Malformed(error.to_string()))?;
            if !paths.insert(entry.path.clone()) {
                return Err(RuntimeStateError::Malformed(format!(
                    "runtime path '{}' is duplicated",
                    entry.path
                )));
            }
            match entry.kind {
                RuntimeStateEntryKind::Directory
                    if entry.sha1.is_none()
                        && entry.size_bytes.is_none()
                        && entry.target.is_none()
                        && !entry.executable => {}
                RuntimeStateEntryKind::File
                    if entry.target.is_none()
                        && entry.size_bytes.is_some_and(|size| size > 0)
                        && entry.sha1.is_some() =>
                {
                    Sha1Digest::parse(entry.sha1.as_deref().unwrap()).map_err(|error| {
                        RuntimeStateError::Malformed(format!(
                            "runtime file '{}' has invalid SHA-1: {error}",
                            entry.path
                        ))
                    })?;
                }
                RuntimeStateEntryKind::Link
                    if entry.sha1.is_none()
                        && entry.size_bytes.is_none()
                        && !entry.executable
                        && entry.target.is_some() =>
                {
                    validate_link_target(&entry.path, entry.target.as_deref().unwrap())
                        .map_err(|error| RuntimeStateError::Malformed(error.to_string()))?;
                }
                _ => {
                    return Err(RuntimeStateError::Malformed(format!(
                        "runtime entry '{}' has fields inconsistent with its kind",
                        entry.path
                    )));
                }
            }
        }
        for executable in [&self.launch_executable, &self.diagnostic_executable] {
            if !self.entries.iter().any(|entry| {
                entry.kind == RuntimeStateEntryKind::File
                    && entry.executable
                    && &entry.path == executable
            }) {
                return Err(RuntimeStateError::Malformed(format!(
                    "required executable '{executable}' is not recorded as executable"
                )));
            }
        }
        Ok(())
    }

    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("runtime state serializes");
        text.push('\n');
        text
    }
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    pub fn component(&self) -> &str {
        &self.component
    }
    pub fn required_major_version(&self) -> u32 {
        self.required_major_version
    }
    pub fn platform_os(&self) -> &str {
        &self.platform_os
    }
    pub fn platform_architecture(&self) -> &str {
        &self.platform_architecture
    }
    pub fn platform_key(&self) -> &str {
        &self.platform_key
    }
    pub fn runtime_version(&self) -> &str {
        &self.runtime_version
    }
    pub fn released(&self) -> &str {
        &self.released
    }
    pub fn manifest_sha1(&self) -> &str {
        &self.manifest_sha1
    }
    pub fn installed_at_unix_seconds(&self) -> u64 {
        self.installed_at_unix_seconds
    }
    pub fn launch_executable(&self) -> &str {
        &self.launch_executable
    }
    pub fn diagnostic_executable(&self) -> &str {
        &self.diagnostic_executable
    }
    pub fn entries(&self) -> &[RuntimeStateEntry] {
        &self.entries
    }
    pub fn identity(&self) -> String {
        format!("{}-{}", self.platform_key, self.manifest_sha1)
    }

    /// The state must describe the complete exact plan, not merely claim the
    /// same digest identity. This prevents an edited record from omitting a
    /// planned file while still passing validation of the remaining subset.
    pub fn matches_plan(&self, plan: &JavaRuntimePlan) -> bool {
        let expected = Self::from_plan(plan, self.installed_at_unix_seconds);
        self == &expected
    }
}

impl RuntimeStateEntry {
    pub fn kind(&self) -> RuntimeStateEntryKind {
        self.kind
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn sha1(&self) -> Option<&str> {
        self.sha1.as_deref()
    }
    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
    pub fn executable(&self) -> bool {
        self.executable
    }
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }
}

pub fn load_runtime_state(
    runtime_root: &Path,
) -> Result<Option<RuntimeInstalledState>, RuntimeStateError> {
    let path = runtime_root.join(RUNTIME_STATE_FILE_NAME);
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RuntimeStateError::Read(error)),
    };
    RuntimeInstalledState::from_json(&text).map(Some)
}

#[derive(Debug)]
pub enum RuntimeStateError {
    Malformed(String),
    UnsupportedSchema { found: u32, supported: u32 },
    Read(std::io::Error),
    Write(std::io::Error),
}

impl fmt::Display for RuntimeStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(reason) => {
                write!(f, "the installed runtime state is malformed: {reason}")
            }
            Self::UnsupportedSchema { found, supported } => write!(
                f,
                "the installed runtime state uses schema version {found}; this launcher supports {supported}"
            ),
            Self::Read(error) => {
                write!(f, "the installed runtime state could not be read: {error}")
            }
            Self::Write(error) => write!(
                f,
                "the installed runtime state could not be written: {error}"
            ),
        }
    }
}

impl std::error::Error for RuntimeStateError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_json() -> String {
        r#"{"schemaVersion":1,"component":"java-runtime-epsilon","requiredMajorVersion":25,"platformOs":"windows","platformArchitecture":"x86_64","platformKey":"windows-x64","runtimeVersion":"25.0.1","released":"2025-10-12","manifestSha1":"0000000000000000000000000000000000000000","installedAtUnixSeconds":1,"launchExecutable":"bin/javaw.exe","diagnosticExecutable":"bin/java.exe","entries":[{"kind":"file","path":"bin/java.exe","sha1":"0000000000000000000000000000000000000000","sizeBytes":1,"executable":true},{"kind":"file","path":"bin/javaw.exe","sha1":"0000000000000000000000000000000000000000","sizeBytes":1,"executable":true}]}"#.to_owned()
    }

    #[test]
    fn state_round_trips_and_rejects_unknown_schema() {
        let state = RuntimeInstalledState::from_json(&state_json()).unwrap();
        assert_eq!(
            RuntimeInstalledState::from_json(&state.to_json()).unwrap(),
            state
        );
        let unsupported = state_json().replacen("\"schemaVersion\":1", "\"schemaVersion\":9", 1);
        assert!(matches!(
            RuntimeInstalledState::from_json(&unsupported),
            Err(RuntimeStateError::UnsupportedSchema { found: 9, .. })
        ));
    }

    #[test]
    fn malformed_paths_digests_duplicates_and_entry_shapes_fail() {
        for broken in [
            state_json().replacen("bin/java.exe", "../java.exe", 1),
            state_json().replacen("0000000000000000000000000000000000000000", "bad", 1),
            state_json().replace("bin/javaw.exe", "bin/java.exe"),
            state_json().replacen("\"sizeBytes\":1", "\"sizeBytes\":0", 1),
        ] {
            assert!(
                RuntimeInstalledState::from_json(&broken).is_err(),
                "{broken}"
            );
        }
    }
}
