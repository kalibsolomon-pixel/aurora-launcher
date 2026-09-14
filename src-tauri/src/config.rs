//! Versioned launcher configuration and its persistence.
//!
//! The configuration is a small, human-inspectable JSON document stored under
//! the managed-data root. Malformed or unsupported files fail deliberately and
//! are never silently replaced; writes go through a temporary file and a
//! rename so a partially written configuration can never be observed.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::instances::InstanceId;

/// The only launcher-configuration schema version this launcher understands.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// Launcher preferences that are genuinely required now.
///
/// Secrets never belong here. Settings for future features are added by later
/// phases together with an explicit schema-version decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherConfig {
    schema_version: u32,
    selected_instance_id: Option<InstanceId>,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            selected_instance_id: None,
        }
    }
}

impl LauncherConfig {
    /// The configuration schema version this launcher writes and enforces.
    pub const SCHEMA_VERSION: u32 = CONFIG_SCHEMA_VERSION;

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn selected_instance_id(&self) -> Option<&InstanceId> {
        self.selected_instance_id.as_ref()
    }

    /// Parses and validates a configuration from JSON text.
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let config: Self = serde_json::from_str(json)
            .map_err(|error| ConfigError::Malformed(error.to_string()))?;

        if config.schema_version != Self::SCHEMA_VERSION {
            return Err(ConfigError::UnsupportedSchema {
                found: config.schema_version,
                supported: Self::SCHEMA_VERSION,
            });
        }

        Ok(config)
    }

    /// Serializes the configuration as pretty, human-inspectable JSON.
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self)
            .expect("launcher configuration serialization cannot fail");
        json.push('\n');
        json
    }
}

/// The outcome of [`load_or_initialize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLoad {
    Existing(LauncherConfig),
    Initialized(LauncherConfig),
}

impl ConfigLoad {
    pub fn into_config(self) -> LauncherConfig {
        match self {
            Self::Existing(config) | Self::Initialized(config) => config,
        }
    }

    pub fn config(&self) -> &LauncherConfig {
        match self {
            Self::Existing(config) | Self::Initialized(config) => config,
        }
    }
}

/// Loads the configuration. Returns `Ok(None)` when no file exists yet.
pub fn load(path: &Path) -> Result<Option<LauncherConfig>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Err(ConfigError::Malformed(
                "the launcher configuration file is not valid UTF-8 and may be corrupted"
                    .to_owned(),
            ));
        }
        Err(error) => return Err(ConfigError::Read(error)),
    };

    LauncherConfig::from_json(&text).map(Some)
}

/// Loads the configuration, writing safe defaults when no file exists yet.
///
/// A malformed or unsupported existing file is returned as an error and left
/// untouched, so a damaged configuration is never silently replaced.
pub fn load_or_initialize(path: &Path) -> Result<ConfigLoad, ConfigError> {
    match load(path)? {
        Some(config) => Ok(ConfigLoad::Existing(config)),
        None => {
            let config = LauncherConfig::default();
            save(path, &config)?;
            Ok(ConfigLoad::Initialized(config))
        }
    }
}

/// Persists the configuration atomically: write to a sibling temporary file,
/// then rename over the target.
pub fn save(path: &Path, config: &LauncherConfig) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(ConfigError::Write)?;
    }

    let temporary_path = temporary_sibling(path);
    std::fs::write(&temporary_path, config.to_json()).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        ConfigError::Write(error)
    })?;

    match std::fs::rename(&temporary_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_path);
            Err(ConfigError::Write(error))
        }
    }
}

fn temporary_sibling(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    PathBuf::from(temporary)
}

#[derive(Debug)]
pub enum ConfigError {
    Read(std::io::Error),
    Write(std::io::Error),
    Malformed(String),
    UnsupportedSchema { found: u32, supported: u32 },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(
                formatter,
                "the launcher configuration could not be read: {error}"
            ),
            Self::Write(error) => write!(
                formatter,
                "the launcher configuration could not be written: {error}"
            ),
            Self::Malformed(detail) => write!(
                formatter,
                "the launcher configuration is malformed and must be corrected manually: {detail}"
            ),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "the launcher configuration uses schema version {found}, which is not supported; this launcher supports version {supported}"
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) | Self::Write(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir()
            .join(name)
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn default_uses_the_current_schema_version_and_no_selection() {
        let config = LauncherConfig::default();

        assert_eq!(config.schema_version(), CONFIG_SCHEMA_VERSION);
        assert_eq!(config.selected_instance_id(), None);
    }

    #[test]
    fn json_round_trip_preserves_the_selection() {
        let config = LauncherConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            selected_instance_id: Some(InstanceId::new("aurora-default").unwrap()),
        };

        let parsed = LauncherConfig::from_json(&config.to_json()).unwrap();

        assert_eq!(parsed, config);
        assert_eq!(
            parsed.selected_instance_id().map(InstanceId::as_str),
            Some("aurora-default")
        );
    }

    #[test]
    fn serializes_to_inspectable_camel_case_json() {
        let json = LauncherConfig::default().to_json();

        assert!(json.contains("\"schemaVersion\": 1"));
        assert!(json.contains("\"selectedInstanceId\": null"));
    }

    #[test]
    fn malformed_configurations_fail_and_are_reported_verbatim() {
        for json in ["{ not json", "{}", r#"{ "schemaVersion": "one" }"#] {
            let error = LauncherConfig::from_json(json).expect_err("must be rejected");
            assert!(matches!(error, ConfigError::Malformed(_)), "got: {error}");
        }
    }

    #[test]
    fn unsupported_schema_versions_fail_deliberately() {
        let json = r#"{ "schemaVersion": 2, "selectedInstanceId": null }"#;

        let error = LauncherConfig::from_json(json).unwrap_err();

        assert!(matches!(
            error,
            ConfigError::UnsupportedSchema {
                found: 2,
                supported: 1
            }
        ));
    }

    #[test]
    fn load_reports_a_missing_file_instead_of_inventing_one() {
        let directory = test_directory("aurora-config-test-missing");
        let path = directory.join("launcher").join("config.json");

        assert_eq!(load(&path).unwrap(), None);
        assert!(!path.exists());
    }

    #[test]
    fn load_or_initialize_materializes_safe_defaults_once() {
        let directory = test_directory("aurora-config-test-initialize");
        let path = directory.join("launcher").join("config.json");

        let first = load_or_initialize(&path).unwrap();
        assert!(matches!(first, ConfigLoad::Initialized(_)));
        assert_eq!(first.config(), &LauncherConfig::default());
        assert!(path.exists());

        let persisted = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            LauncherConfig::from_json(&persisted).unwrap(),
            LauncherConfig::default()
        );

        let second = load_or_initialize(&path).unwrap();
        assert!(matches!(second, ConfigLoad::Existing(_)));
        assert_eq!(second.config(), &LauncherConfig::default());
    }

    #[test]
    fn malformed_configurations_are_never_overwritten() {
        let directory = test_directory("aurora-config-test-malformed");
        let path = directory.join("launcher").join("config.json");
        let damaged = "{ this is not json";
        std::fs::create_dir_all(directory.join("launcher")).unwrap();
        std::fs::write(&path, damaged).unwrap();

        let result = load_or_initialize(&path);

        assert!(matches!(result, Err(ConfigError::Malformed(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), damaged);
    }

    #[test]
    fn save_replaces_existing_configuration_and_leaves_no_temporary_files() {
        let directory = test_directory("aurora-config-test-save");
        let path = directory.join("launcher").join("config.json");
        save(&path, &LauncherConfig::default()).unwrap();

        let updated = LauncherConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            selected_instance_id: Some(InstanceId::new("beta-playground").unwrap()),
        };
        save(&path, &updated).unwrap();

        assert_eq!(load(&path).unwrap(), Some(updated));
        let names: Vec<_> = std::fs::read_dir(directory.join("launcher"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["config.json".to_owned()]);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let directory = test_directory("aurora-config-test-dirs");
        let path = directory.join("launcher").join("config.json");

        save(&path, &LauncherConfig::default()).unwrap();

        assert!(path.is_file());
    }
}
