use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::config::{ConfigError, ConfigLoad};
use crate::distribution::ReleaseChannel;
use crate::instances::{InstanceRecord, InstanceRegistry, InstanceRegistryError};
use crate::paths::ManagedPaths;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationStatus {
    launcher_version: String,
    platform: PlatformInfo,
    managed_data_root: String,
    backend_status: BackendStatus,
}

impl ApplicationStatus {
    fn ready(
        launcher_version: impl Into<String>,
        os: impl Into<String>,
        architecture: impl Into<String>,
        managed_data_root: PathBuf,
    ) -> Self {
        Self {
            launcher_version: launcher_version.into(),
            platform: PlatformInfo {
                os: os.into(),
                architecture: architecture.into(),
            },
            managed_data_root: managed_data_root.to_string_lossy().into_owned(),
            backend_status: BackendStatus::Ready,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    os: String,
    architecture: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendStatus {
    Ready,
}

/// The persisted launcher state shown by the UI: configuration summary and
/// the known instance records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherState {
    config: LauncherConfigSummary,
    instances: Vec<InstanceSummary>,
}

impl LauncherState {
    fn from_parts(config: crate::config::LauncherConfig, registry: InstanceRegistry) -> Self {
        Self {
            config: LauncherConfigSummary {
                schema_version: config.schema_version(),
                selected_instance_id: config.selected_instance_id().map(|id| id.to_string()),
            },
            instances: registry
                .instances()
                .iter()
                .map(InstanceSummary::from_record)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherConfigSummary {
    schema_version: u32,
    selected_instance_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSummary {
    id: String,
    display_name: String,
    channel: ReleaseChannel,
    aurora_version: Option<String>,
}

impl InstanceSummary {
    fn from_record(record: &InstanceRecord) -> Self {
        Self {
            id: record.id().to_string(),
            display_name: record.display_name().to_owned(),
            channel: record.release().channel(),
            aurora_version: record.release().aurora_version().map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    code: String,
    message: String,
}

impl CommandError {
    fn managed_path(message: impl Into<String>) -> Self {
        Self::new("managed_path_unavailable", message)
    }

    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<ConfigError> for CommandError {
    fn from(error: ConfigError) -> Self {
        let code = match &error {
            ConfigError::Malformed(_) => "config_malformed",
            ConfigError::UnsupportedSchema { .. } => "config_unsupported_schema",
            ConfigError::Read(_) | ConfigError::Write(_) => "storage_io_failure",
        };
        Self::new(code, error.to_string())
    }
}

impl From<InstanceRegistryError> for CommandError {
    fn from(error: InstanceRegistryError) -> Self {
        let code = match &error {
            InstanceRegistryError::Malformed(_) => "instances_invalid",
            InstanceRegistryError::UnsupportedSchema { .. } => "instances_unsupported_schema",
            InstanceRegistryError::Read(_) => "storage_io_failure",
        };
        Self::new(code, error.to_string())
    }
}

fn managed_paths(app: &AppHandle) -> Result<ManagedPaths, CommandError> {
    let resolved_root = app.path().app_local_data_dir().map_err(|error| {
        CommandError::managed_path(format!(
            "Aurora's managed data location could not be resolved: {error}"
        ))
    })?;

    ManagedPaths::from_app_local_data_dir(resolved_root).map_err(|error| {
        CommandError::managed_path(format!(
            "Aurora's managed data location is not safe to use: {error}"
        ))
    })
}

#[tauri::command]
pub fn get_application_status(app: AppHandle) -> Result<ApplicationStatus, CommandError> {
    let managed_paths = managed_paths(&app)?;

    eprintln!(
        "[aurora-launcher] backend ready; managed data root: {}",
        managed_paths.data_root().display()
    );

    Ok(ApplicationStatus::ready(
        app.package_info().version.to_string(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        managed_paths.data_root().to_path_buf(),
    ))
}

/// Loads the persisted launcher state.
///
/// On first run this materializes the default configuration under the managed
/// data root; malformed persisted files are reported as structured errors and
/// left untouched.
#[tauri::command]
pub fn get_launcher_state(app: AppHandle) -> Result<LauncherState, CommandError> {
    let managed_paths = managed_paths(&app)?;

    let loaded = crate::config::load_or_initialize(&managed_paths.config_file())?;
    if let ConfigLoad::Initialized(_) = &loaded {
        eprintln!(
            "[aurora-launcher] initialized default launcher configuration at {}",
            managed_paths.config_file().display()
        );
    }

    let registry = InstanceRegistry::load(&managed_paths.instance_registry_file())?;

    Ok(LauncherState::from_parts(loaded.into_config(), registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_status_preserves_structured_platform_metadata() {
        let root = std::env::temp_dir().join("com.aurora.launcher");

        let status = ApplicationStatus::ready("0.1.0", "windows", "x86_64", root.clone());

        assert_eq!(status.launcher_version, "0.1.0");
        assert_eq!(status.platform.os, "windows");
        assert_eq!(status.platform.architecture, "x86_64");
        assert_eq!(status.managed_data_root, root.to_string_lossy());
        assert_eq!(status.backend_status, BackendStatus::Ready);
    }

    #[test]
    fn managed_path_errors_have_a_stable_machine_code() {
        let error = CommandError::managed_path("example failure");

        assert_eq!(error.code, "managed_path_unavailable");
        assert_eq!(error.message, "example failure");
    }

    #[test]
    fn config_errors_map_to_stable_machine_codes() {
        let malformed = CommandError::from(ConfigError::Malformed("broken".to_owned()));
        assert_eq!(malformed.code, "config_malformed");

        let unsupported = CommandError::from(ConfigError::UnsupportedSchema {
            found: 9,
            supported: 1,
        });
        assert_eq!(unsupported.code, "config_unsupported_schema");
        assert!(unsupported.message.contains("9"));

        let io = CommandError::from(ConfigError::Read(std::io::Error::other("disk")));
        assert_eq!(io.code, "storage_io_failure");
    }

    #[test]
    fn instance_registry_errors_map_to_stable_machine_codes() {
        let malformed = CommandError::from(InstanceRegistryError::Malformed("broken".to_owned()));
        assert_eq!(malformed.code, "instances_invalid");

        let unsupported = CommandError::from(InstanceRegistryError::UnsupportedSchema {
            found: 9,
            supported: 1,
        });
        assert_eq!(unsupported.code, "instances_unsupported_schema");

        let io = CommandError::from(InstanceRegistryError::Read(std::io::Error::other("disk")));
        assert_eq!(io.code, "storage_io_failure");
    }

    #[test]
    fn launcher_state_flattens_persisted_records_into_summary_dtos() {
        let config = crate::config::LauncherConfig::from_json(
            r#"{ "schemaVersion": 1, "selectedInstanceId": "aurora-default" }"#,
        )
        .unwrap();
        let registry = InstanceRegistry::from_json(
            r#"{
                "schemaVersion": 1,
                "instances": [
                    {
                        "id": "aurora-default",
                        "displayName": "Aurora Default",
                        "release": { "channel": "stable", "auroraVersion": null }
                    }
                ]
            }"#,
        )
        .unwrap();

        let state = LauncherState::from_parts(config, registry);

        assert_eq!(state.config.schema_version, 1);
        assert_eq!(
            state.config.selected_instance_id.as_deref(),
            Some("aurora-default")
        );
        assert_eq!(state.instances.len(), 1);
        assert_eq!(state.instances[0].id, "aurora-default");
        assert_eq!(state.instances[0].display_name, "Aurora Default");
        assert_eq!(state.instances[0].channel, ReleaseChannel::Stable);
        assert_eq!(state.instances[0].aurora_version, None);
    }
}
