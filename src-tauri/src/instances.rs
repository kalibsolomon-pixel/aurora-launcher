//! Instance domain model: validated instance identifiers, instance records,
//! and the persisted instance registry.
//!
//! Instances are represented but not created, installed, or deleted in this
//! phase. The registry is therefore read-only: it loads persisted instance
//! records when present and reports an empty registry when the file does not
//! exist yet.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::distribution::ReleaseChannel;

/// The only instance-registry schema version this launcher understands.
pub const INSTANCE_REGISTRY_SCHEMA_VERSION: u32 = 1;

const MAX_INSTANCE_ID_LENGTH: usize = 64;
const MAX_INSTANCE_DISPLAY_NAME_LENGTH: usize = 80;

/// An opaque, filesystem-safe instance identifier.
///
/// Identifiers are the only input to instance path derivation. Display names
/// never influence the filesystem. Validation rejects separators, traversal,
/// absolute paths, and Windows-reserved device names before any path can be
/// constructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InstanceId(String);

impl InstanceId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidInstanceId> {
        let value = value.into();
        validate_instance_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for InstanceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<InstanceId> for String {
    fn from(id: InstanceId) -> Self {
        id.0
    }
}

impl TryFrom<String> for InstanceId {
    type Error = InvalidInstanceId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// A persisted instance record.
///
/// `display_name` is user-facing text only; it is never used to derive paths.
/// `release` pins the Aurora channel and, optionally, an exact Aurora version;
/// a `None` version means "newest release of the channel", resolved at
/// install time in a later phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRecord {
    id: InstanceId,
    display_name: String,
    release: ReleasePin,
}

impl InstanceRecord {
    pub fn new(
        id: InstanceId,
        display_name: impl Into<String>,
        release: ReleasePin,
    ) -> Result<Self, InvalidInstanceRecord> {
        let record = Self {
            id,
            display_name: display_name.into(),
            release,
        };
        record.validate_content()?;
        Ok(record)
    }

    pub fn id(&self) -> &InstanceId {
        &self.id
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn release(&self) -> &ReleasePin {
        &self.release
    }

    /// Validates the parts serde cannot enforce by type.
    fn validate_content(&self) -> Result<(), InvalidInstanceRecord> {
        validate_display_name(&self.display_name)?;
        if let Some(version) = &self.release.aurora_version {
            validate_pinned_version(version)?;
        }
        Ok(())
    }
}

/// The Aurora release an instance tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasePin {
    channel: ReleaseChannel,
    aurora_version: Option<String>,
}

impl ReleasePin {
    pub fn new(
        channel: ReleaseChannel,
        aurora_version: Option<String>,
    ) -> Result<Self, InvalidInstanceRecord> {
        if let Some(version) = &aurora_version {
            validate_pinned_version(version)?;
        }
        Ok(Self {
            channel,
            aurora_version,
        })
    }

    pub fn channel(&self) -> ReleaseChannel {
        self.channel
    }

    pub fn aurora_version(&self) -> Option<&str> {
        self.aurora_version.as_deref()
    }
}

/// The persisted list of known instances.
///
/// Stored as JSON by the launcher; this phase only loads it. A missing file is
/// an empty registry, while malformed or unsupported data is a deliberate
/// error that is never silently repaired or overwritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRegistry {
    schema_version: u32,
    instances: Vec<InstanceRecord>,
}

impl InstanceRegistry {
    /// The registry schema version this loader enforces.
    pub const SCHEMA_VERSION: u32 = INSTANCE_REGISTRY_SCHEMA_VERSION;

    pub fn empty() -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            instances: Vec::new(),
        }
    }

    pub fn instances(&self) -> &[InstanceRecord] {
        &self.instances
    }

    /// Loads the registry from disk. A missing file loads as empty.
    pub fn load(path: &Path) -> Result<Self, InstanceRegistryError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::empty());
            }
            Err(error) => return Err(InstanceRegistryError::Read(error)),
        };

        Self::from_json(&text)
    }

    /// Parses and validates registry data from JSON text.
    pub fn from_json(text: &str) -> Result<Self, InstanceRegistryError> {
        let registry: Self = serde_json::from_str(text)
            .map_err(|error| InstanceRegistryError::Malformed(error.to_string()))?;

        if registry.schema_version != Self::SCHEMA_VERSION {
            return Err(InstanceRegistryError::UnsupportedSchema {
                found: registry.schema_version,
                supported: Self::SCHEMA_VERSION,
            });
        }

        for (index, record) in registry.instances.iter().enumerate() {
            record.validate_content().map_err(|error| {
                InstanceRegistryError::Malformed(format!(
                    "instance entry {index} ({}): {error}",
                    record.id
                ))
            })?;
        }

        if let Some(duplicate) = find_duplicate_id(&registry.instances) {
            return Err(InstanceRegistryError::Malformed(format!(
                "instance identifier '{duplicate}' is registered more than once"
            )));
        }

        Ok(registry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidInstanceId {
    Empty,
    TooLong(usize),
    ForbiddenCharacter(char),
    InvalidBoundary,
    ReservedName(String),
}

impl fmt::Display for InvalidInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "instance identifier must not be empty"),
            Self::TooLong(length) => write!(
                formatter,
                "instance identifier must be at most {MAX_INSTANCE_ID_LENGTH} characters, but is {length}"
            ),
            Self::ForbiddenCharacter(character) => write!(
                formatter,
                "instance identifier contains '{}'; only lowercase letters, digits, hyphens, and underscores are allowed",
                character.escape_default()
            ),
            Self::InvalidBoundary => write!(
                formatter,
                "instance identifier must begin and end with a lowercase letter or digit"
            ),
            Self::ReservedName(name) => write!(
                formatter,
                "'{name}' is a reserved system device name and cannot be used as an instance identifier"
            ),
        }
    }
}

impl std::error::Error for InvalidInstanceId {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidInstanceRecord {
    EmptyDisplayName,
    DisplayNameTooLong(usize),
    DisplayNameWhitespacePadding,
    DisplayNameControlCharacter,
    EmptyAuroraVersion,
    AuroraVersionWhitespacePadding,
    AuroraVersionControlCharacter,
}

impl fmt::Display for InvalidInstanceRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDisplayName => write!(formatter, "instance display name must not be empty"),
            Self::DisplayNameTooLong(length) => write!(
                formatter,
                "instance display name must be at most {MAX_INSTANCE_DISPLAY_NAME_LENGTH} characters, but is {length}"
            ),
            Self::DisplayNameWhitespacePadding => write!(
                formatter,
                "instance display name must not have leading or trailing whitespace"
            ),
            Self::DisplayNameControlCharacter => write!(
                formatter,
                "instance display name must not contain control characters"
            ),
            Self::EmptyAuroraVersion => {
                write!(formatter, "pinned Aurora version must not be empty")
            }
            Self::AuroraVersionWhitespacePadding => write!(
                formatter,
                "pinned Aurora version must not have leading or trailing whitespace"
            ),
            Self::AuroraVersionControlCharacter => write!(
                formatter,
                "pinned Aurora version must not contain control characters"
            ),
        }
    }
}

impl std::error::Error for InvalidInstanceRecord {}

#[derive(Debug)]
pub enum InstanceRegistryError {
    Read(std::io::Error),
    Malformed(String),
    UnsupportedSchema { found: u32, supported: u32 },
}

impl fmt::Display for InstanceRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(
                formatter,
                "the instance registry could not be read: {error}"
            ),
            Self::Malformed(detail) => write!(
                formatter,
                "the instance registry is malformed and must be corrected manually: {detail}"
            ),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "the instance registry uses schema version {found}, which is not supported; this launcher supports version {supported}"
            ),
        }
    }
}

impl std::error::Error for InstanceRegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            _ => None,
        }
    }
}

fn validate_instance_id(value: &str) -> Result<(), InvalidInstanceId> {
    if value.is_empty() {
        return Err(InvalidInstanceId::Empty);
    }

    if value.len() > MAX_INSTANCE_ID_LENGTH {
        return Err(InvalidInstanceId::TooLong(value.len()));
    }

    for character in value.chars() {
        if !matches!(character, 'a'..='z' | '0'..='9' | '-' | '_') {
            return Err(InvalidInstanceId::ForbiddenCharacter(character));
        }
    }

    let first = value
        .chars()
        .next()
        .expect("non-empty string has a first character");
    let last = value
        .chars()
        .last()
        .expect("non-empty string has a last character");
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        return Err(InvalidInstanceId::InvalidBoundary);
    }

    if is_windows_reserved_name(value) {
        return Err(InvalidInstanceId::ReservedName(value.to_owned()));
    }

    Ok(())
}

fn is_windows_reserved_name(value: &str) -> bool {
    if matches!(value, "con" | "prn" | "aux" | "nul") {
        return true;
    }

    value.len() == 4
        && (value.starts_with("com") || value.starts_with("lpt"))
        && value.as_bytes()[3].is_ascii_digit()
}

fn validate_display_name(value: &str) -> Result<(), InvalidInstanceRecord> {
    if value.is_empty() {
        return Err(InvalidInstanceRecord::EmptyDisplayName);
    }
    if value.len() > MAX_INSTANCE_DISPLAY_NAME_LENGTH {
        return Err(InvalidInstanceRecord::DisplayNameTooLong(value.len()));
    }
    if value.trim() != value {
        return Err(InvalidInstanceRecord::DisplayNameWhitespacePadding);
    }
    if value.chars().any(char::is_control) {
        return Err(InvalidInstanceRecord::DisplayNameControlCharacter);
    }
    Ok(())
}

fn validate_pinned_version(value: &str) -> Result<(), InvalidInstanceRecord> {
    if value.is_empty() {
        return Err(InvalidInstanceRecord::EmptyAuroraVersion);
    }
    if value.trim() != value {
        return Err(InvalidInstanceRecord::AuroraVersionWhitespacePadding);
    }
    if value.chars().any(char::is_control) {
        return Err(InvalidInstanceRecord::AuroraVersionControlCharacter);
    }
    Ok(())
}

fn find_duplicate_id(instances: &[InstanceRecord]) -> Option<String> {
    for (position, record) in instances.iter().enumerate() {
        if instances[position + 1..]
            .iter()
            .any(|other| other.id == record.id)
        {
            return Some(record.id.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_REGISTRY: &str = r#"{
        "schemaVersion": 1,
        "instances": [
            {
                "id": "aurora-default",
                "displayName": "Aurora Default",
                "release": { "channel": "stable", "auroraVersion": null }
            },
            {
                "id": "beta_playground",
                "displayName": "Beta Playground",
                "release": { "channel": "beta", "auroraVersion": "0.3.1-beta.2" }
            }
        ]
    }"#;

    #[test]
    fn accepts_constrained_identifiers() {
        let long_but_allowed = "a".repeat(MAX_INSTANCE_ID_LENGTH);

        for value in [
            "a",
            "default",
            "aurora-default",
            "beta_playground",
            "minecraft123",
            long_but_allowed.as_str(),
        ] {
            let id = InstanceId::new(value).expect("identifier should be accepted");
            assert_eq!(id.as_str(), value);
        }
    }

    #[test]
    fn rejects_traversal_separators_and_absolute_paths() {
        for value in [
            "..",
            "../evil",
            "a/b",
            "a\\b",
            "/absolute",
            "C:\\temp",
            ".",
            "id/../../escape",
        ] {
            let error = InstanceId::new(value).expect_err("traversal must be rejected");
            assert!(
                matches!(
                    error,
                    InvalidInstanceId::ForbiddenCharacter(_) | InvalidInstanceId::InvalidBoundary
                ),
                "'{value}' should be rejected as unsafe, got: {error}"
            );
        }
    }

    #[test]
    fn rejects_empty_padding_and_forbidden_characters() {
        for value in [
            "",
            "has space",
            "Aurora",
            "café",
            "tab\tname",
            "-leading",
            "trailing-",
            "_under_",
            "line\nbreak",
        ] {
            assert!(
                InstanceId::new(value).is_err(),
                "'{value}' should not be a valid instance identifier"
            );
        }
    }

    #[test]
    fn rejects_overlong_identifiers() {
        let too_long = "a".repeat(MAX_INSTANCE_ID_LENGTH + 1);

        assert!(matches!(
            InstanceId::new(too_long),
            Err(InvalidInstanceId::TooLong(65))
        ));
    }

    #[test]
    fn rejects_windows_reserved_device_names() {
        for value in [
            "con", "prn", "aux", "nul", "com1", "com9", "com0", "lpt1", "lpt9",
        ] {
            assert!(
                matches!(InstanceId::new(value), Err(InvalidInstanceId::ReservedName(name)) if name == value),
                "'{value}' is a reserved device name and must be rejected"
            );
        }
    }

    #[test]
    fn display_names_are_validated_but_never_used_for_paths() {
        let id = InstanceId::new("default").unwrap();

        let valid = InstanceRecord::new(
            id.clone(),
            "My Auröra ✨ Setup",
            ReleasePin {
                channel: ReleaseChannel::Stable,
                aurora_version: None,
            },
        )
        .unwrap();
        assert_eq!(valid.display_name(), "My Auröra ✨ Setup");

        for display_name in [
            "",
            " padded",
            "padded ",
            "a".repeat(81).as_str(),
            "bad\nline",
        ] {
            let result = InstanceRecord::new(
                id.clone(),
                display_name,
                ReleasePin {
                    channel: ReleaseChannel::Stable,
                    aurora_version: None,
                },
            );
            assert!(result.is_err(), "'{display_name}' should be rejected");
        }

        // Path derivation only accepts identifiers; the display name with
        // spaces and non-ASCII text could never reach the filesystem.
        assert_eq!(valid.id().as_str(), "default");
    }

    #[test]
    fn release_pins_require_non_empty_versions_when_present() {
        let result = ReleasePin::new(ReleaseChannel::Beta, Some(" 0.3.1".to_owned()));

        assert!(matches!(
            result,
            Err(InvalidInstanceRecord::AuroraVersionWhitespacePadding)
        ));
    }

    #[test]
    fn missing_registry_file_loads_as_empty() {
        let path = std::env::temp_dir()
            .join("aurora-instances-test-missing")
            .join("instances.json");

        let registry = InstanceRegistry::load(&path).unwrap();

        assert_eq!(registry, InstanceRegistry::empty());
        assert_eq!(registry.instances().len(), 0);
    }

    #[test]
    fn parses_valid_registry_fixtures() {
        let path = persist_fixture("aurora-instances-test-valid", VALID_REGISTRY);

        let registry = InstanceRegistry::load(&path).unwrap();

        assert_eq!(registry.instances().len(), 2);

        let first = &registry.instances()[0];
        assert_eq!(first.id().as_str(), "aurora-default");
        assert_eq!(first.display_name(), "Aurora Default");
        assert_eq!(first.release().channel(), ReleaseChannel::Stable);
        assert_eq!(first.release().aurora_version(), None);

        let second = &registry.instances()[1];
        assert_eq!(second.id().as_str(), "beta_playground");
        assert_eq!(second.release().channel(), ReleaseChannel::Beta);
        assert_eq!(second.release().aurora_version(), Some("0.3.1-beta.2"));
    }

    #[test]
    fn invalid_registry_data_fails_deliberately() {
        let cases: Vec<(&str, &str)> = vec![
            ("{ not json", "malformed"),
            ("{}", "malformed"),
            (
                r#"{ "schemaVersion": 2, "instances": [] }"#,
                "schema version 2",
            ),
            (
                r#"{ "schemaVersion": 1, "instances": [ { "id": "../evil", "displayName": "Evil", "release": { "channel": "stable", "auroraVersion": null } } ] }"#,
                "only lowercase letters",
            ),
            (
                r#"{ "schemaVersion": 1, "instances": [ { "id": "ok-id", "displayName": "", "release": { "channel": "stable", "auroraVersion": null } } ] }"#,
                "display name must not be empty",
            ),
            (
                r#"{ "schemaVersion": 1, "instances": [ { "id": "ok-id", "displayName": "Ok", "release": { "channel": "weekly", "auroraVersion": null } } ] }"#,
                "unknown variant",
            ),
            (
                r#"{ "schemaVersion": 1, "instances": [ { "id": "ok-id", "displayName": "Ok", "release": { "channel": "stable", "auroraVersion": " " } } ] }"#,
                "pinned Aurora version",
            ),
            (
                r#"{ "schemaVersion": 1, "instances": [
                    { "id": "dupe", "displayName": "First", "release": { "channel": "stable", "auroraVersion": null } },
                    { "id": "dupe", "displayName": "Second", "release": { "channel": "beta", "auroraVersion": null } }
                ] }"#,
                "more than once",
            ),
        ];

        for (fixture, expected_fragment) in cases {
            let path = persist_fixture("aurora-instances-test-invalid", fixture);
            let error = InstanceRegistry::load(&path).expect_err("registry must be rejected");
            let message = error.to_string();
            assert!(
                message.contains(expected_fragment),
                "expected '{expected_fragment}' in: {message}"
            );
        }
    }

    fn persist_fixture(directory_name: &str, contents: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(directory_name);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("registry-{}.json", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }
}
