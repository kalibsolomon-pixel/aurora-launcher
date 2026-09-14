use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    code: String,
    message: String,
}

impl CommandError {
    fn managed_path(message: impl Into<String>) -> Self {
        Self {
            code: "managed_path_unavailable".to_owned(),
            message: message.into(),
        }
    }
}

#[tauri::command]
pub fn get_application_status(app: AppHandle) -> Result<ApplicationStatus, CommandError> {
    let resolved_root = app.path().app_local_data_dir().map_err(|error| {
        CommandError::managed_path(format!(
            "Aurora's managed data location could not be resolved: {error}"
        ))
    })?;

    let managed_paths = ManagedPaths::from_app_local_data_dir(resolved_root).map_err(|error| {
        CommandError::managed_path(format!(
            "Aurora's managed data location is not safe to use: {error}"
        ))
    })?;

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
}
