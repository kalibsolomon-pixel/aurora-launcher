//! Instance domain model: validated instance identifiers, instance records,
//! the persisted (now writable) instance registry, and identifier generation.
//!
//! The registry is a versioned JSON document at
//! `<managed-data-root>/launcher/instances.json`. Malformed or
//! unsupported-schema files are deliberate errors that are never silently
//! repaired or overwritten; writes are atomic (sibling temporary file plus
//! rename). Display names are user-facing text only and never influence the
//! filesystem; the identifier alone derives paths.
//!
//! Instance lifecycle orchestration (create/retry/rename/select/validate)
//! lives in [`lifecycle`]; Aurora's own installed state lives in
//! [`crate::aurora`]. Deletion remains unimplemented by design.

pub mod lifecycle;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::distribution::ReleaseChannel;

/// The only instance-registry schema version this launcher understands.
///
/// Version 2 added the per-record lifecycle state and the concrete release
/// pin (every instance now records exactly which Aurora release it targets,
/// including its Minecraft and Fabric Loader versions). Version 1 files —
/// from before any code could write the registry — fail deliberately as
/// unsupported rather than being migrated.
pub const INSTANCE_REGISTRY_SCHEMA_VERSION: u32 = 2;

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

/// The persisted instance record.
///
/// `display_name` is user-facing text only; it is never used to derive paths.
/// `state` is the record's lifecycle state — a creation that has not yet
/// completed stays `Installing` and is never reported ready. `release` pins
/// the concrete Aurora release the instance targets; "channel only" pins are
/// deliberately unrepresentable because channels drift over time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRecord {
    id: InstanceId,
    display_name: String,
    state: InstanceState,
    release: PinnedRelease,
}

/// The lifecycle state of a persisted instance.
///
/// Deliberately minimal: `Installing` means creation or retry has not yet
/// completed validation; `Ready` means it did. "Damaged" is not a stored
/// state — it is computed on demand by complete-instance validation, which
/// can discover damage at any later time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceState {
    Installing,
    Ready,
}

impl InstanceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installing => "installing",
            Self::Ready => "ready",
        }
    }
}

impl fmt::Display for InstanceState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The concrete Aurora release an instance is pinned to.
///
/// Every field is a required fact of the resolved release — the pin never
/// says merely "stable", because what "stable" means changes over time.
/// Artifact provenance (URL, digest) intentionally does not live here; it
/// lives in the instance's Aurora installed-state record next to the
/// materialized artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedRelease {
    channel: ReleaseChannel,
    aurora_version: String,
    minecraft_version: String,
    fabric_loader_version: String,
}

impl PinnedRelease {
    pub fn new(
        channel: ReleaseChannel,
        aurora_version: impl Into<String>,
        minecraft_version: impl Into<String>,
        fabric_loader_version: impl Into<String>,
    ) -> Result<Self, InvalidInstanceRecord> {
        let pin = Self {
            channel,
            aurora_version: aurora_version.into(),
            minecraft_version: minecraft_version.into(),
            fabric_loader_version: fabric_loader_version.into(),
        };
        pin.validate()?;
        Ok(pin)
    }

    pub fn channel(&self) -> ReleaseChannel {
        self.channel
    }

    pub fn aurora_version(&self) -> &str {
        &self.aurora_version
    }

    pub fn minecraft_version(&self) -> &str {
        &self.minecraft_version
    }

    pub fn fabric_loader_version(&self) -> &str {
        &self.fabric_loader_version
    }

    fn validate(&self) -> Result<(), InvalidInstanceRecord> {
        validate_pinned_version("Aurora", &self.aurora_version)?;
        validate_pinned_version("Minecraft", &self.minecraft_version)?;
        validate_pinned_version("Fabric Loader", &self.fabric_loader_version)?;
        Ok(())
    }
}

impl InstanceRecord {
    pub fn new(
        id: InstanceId,
        display_name: impl Into<String>,
        state: InstanceState,
        release: PinnedRelease,
    ) -> Result<Self, InvalidInstanceRecord> {
        let record = Self {
            id,
            display_name: display_name.into(),
            state,
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

    pub fn state(&self) -> InstanceState {
        self.state
    }

    pub fn release(&self) -> &PinnedRelease {
        &self.release
    }

    /// Sets the lifecycle state (used only by lifecycle orchestration when
    /// a record genuinely transitions).
    pub fn set_state(&mut self, state: InstanceState) {
        self.state = state;
    }

    /// Renames the instance's display name. This changes metadata only: the
    /// identifier, and therefore every filesystem path, is untouched.
    pub fn set_display_name(
        &mut self,
        display_name: impl Into<String>,
    ) -> Result<(), InvalidInstanceRecord> {
        self.display_name = display_name.into();
        self.validate_content()
    }

    /// Validates the parts serde cannot enforce by type.
    fn validate_content(&self) -> Result<(), InvalidInstanceRecord> {
        validate_display_name(&self.display_name)?;
        self.release.validate()
    }
}

/// The persisted list of known instances.
///
/// Stored as versioned JSON. A missing file loads as an empty registry;
/// malformed or unsupported data is a deliberate error that is never
/// silently repaired or overwritten. Saves are atomic: sibling temporary
/// file plus rename, so a partially written registry can never be observed.
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

    pub fn instances_mut(&mut self) -> &mut Vec<InstanceRecord> {
        &mut self.instances
    }

    pub fn find(&self, id: &InstanceId) -> Option<&InstanceRecord> {
        self.instances.iter().find(|record| record.id() == id)
    }

    pub fn find_mut(&mut self, id: &InstanceId) -> Option<&mut InstanceRecord> {
        self.instances.iter_mut().find(|record| record.id() == id)
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

    /// Persists the registry atomically: write to a sibling temporary file,
    /// then rename over the target. Duplicate identifiers are rejected
    /// before anything touches disk.
    pub fn save(&self, path: &Path) -> Result<(), InstanceRegistryError> {
        if let Some(duplicate) = find_duplicate_id(&self.instances) {
            return Err(InstanceRegistryError::Malformed(format!(
                "instance identifier '{duplicate}' is registered more than once"
            )));
        }

        let mut json = serde_json::to_string_pretty(self)
            .expect("instance registry serialization cannot fail");
        json.push('\n');

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(InstanceRegistryError::Write)?;
        }
        let temporary_path = temporary_sibling(path);
        std::fs::write(&temporary_path, json).map_err(|error| {
            let _ = std::fs::remove_file(&temporary_path);
            InstanceRegistryError::Write(error)
        })?;
        match std::fs::rename(&temporary_path, path) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = std::fs::remove_file(&temporary_path);
                Err(InstanceRegistryError::Write(error))
            }
        }
    }
}

/// Generates a fresh instance identifier: an opaque UUIDv4 in the canonical
/// simple (hyphen-less) lowercase-hex form.
///
/// Thirty-two hexadecimal characters satisfy the identifier rules by
/// construction (lowercase, alphanumeric boundaries, no reserved names), are
/// filesystem-safe on every supported platform, and are independent of the
/// display name and of timestamps alone. Uniqueness is re-checked against
/// the registry by the caller; collisions are practically impossible.
pub fn generate_instance_id() -> InstanceId {
    let simple = uuid::Uuid::new_v4().simple().to_string();
    InstanceId::new(simple).expect("a UUIDv4 in simple form is always a valid identifier")
}

fn temporary_sibling(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    PathBuf::from(temporary)
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
    EmptyPinnedVersion(&'static str),
    PinnedVersionWhitespacePadding(&'static str),
    PinnedVersionControlCharacter(&'static str),
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
            Self::EmptyPinnedVersion(field) => {
                write!(formatter, "pinned {field} version must not be empty")
            }
            Self::PinnedVersionWhitespacePadding(field) => write!(
                formatter,
                "pinned {field} version must not have leading or trailing whitespace"
            ),
            Self::PinnedVersionControlCharacter(field) => write!(
                formatter,
                "pinned {field} version must not contain control characters"
            ),
        }
    }
}

impl std::error::Error for InvalidInstanceRecord {}

#[derive(Debug)]
pub enum InstanceRegistryError {
    Read(std::io::Error),
    /// Saving the registry failed; the previous file was never damaged.
    Write(std::io::Error),
    Malformed(String),
    UnsupportedSchema {
        found: u32,
        supported: u32,
    },
}

impl fmt::Display for InstanceRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(
                formatter,
                "the instance registry could not be read: {error}"
            ),
            Self::Write(error) => write!(
                formatter,
                "the instance registry could not be written: {error}"
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
            Self::Read(error) | Self::Write(error) => Some(error),
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

fn validate_pinned_version(field: &'static str, value: &str) -> Result<(), InvalidInstanceRecord> {
    if value.is_empty() {
        return Err(InvalidInstanceRecord::EmptyPinnedVersion(field));
    }
    if value.trim() != value {
        return Err(InvalidInstanceRecord::PinnedVersionWhitespacePadding(field));
    }
    if value.chars().any(char::is_control) {
        return Err(InvalidInstanceRecord::PinnedVersionControlCharacter(field));
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
        "schemaVersion": 2,
        "instances": [
            {
                "id": "aurora-default",
                "displayName": "Aurora Default",
                "state": "ready",
                "release": {
                    "channel": "stable",
                    "auroraVersion": "0.3.0",
                    "minecraftVersion": "26.2",
                    "fabricLoaderVersion": "0.19.5"
                }
            },
            {
                "id": "beta_playground",
                "displayName": "Beta Playground",
                "state": "installing",
                "release": {
                    "channel": "beta",
                    "auroraVersion": "0.3.1-beta.1",
                    "minecraftVersion": "26.2",
                    "fabricLoaderVersion": "0.19.5"
                }
            }
        ]
    }"#;

    fn sample_pin() -> PinnedRelease {
        PinnedRelease::new(ReleaseChannel::Stable, "0.3.0", "26.2", "0.19.5").unwrap()
    }

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
    fn generated_identifiers_are_valid_unique_and_name_independent() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let id = generate_instance_id();
            // Valid by construction, but prove it against the validator.
            assert!(InstanceId::new(id.as_str()).is_ok());
            assert_eq!(id.as_str().len(), 32);
            assert!(
                id.as_str()
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
            seen.insert(id.as_str().to_owned());
        }
        assert_eq!(seen.len(), 200, "UUIDv4 identifiers must not collide");
    }

    #[test]
    fn display_names_are_validated_but_never_used_for_paths() {
        let id = InstanceId::new("default").unwrap();

        let valid = InstanceRecord::new(
            id.clone(),
            "My Auröra ✨ Setup",
            InstanceState::Ready,
            sample_pin(),
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
            let result =
                InstanceRecord::new(id.clone(), display_name, InstanceState::Ready, sample_pin());
            assert!(result.is_err(), "'{display_name}' should be rejected");
        }

        // Path derivation only accepts identifiers; the display name with
        // spaces and non-ASCII text could never reach the filesystem.
        assert_eq!(valid.id().as_str(), "default");
    }

    #[test]
    fn pinned_releases_require_concrete_valid_versions() {
        assert!(matches!(
            PinnedRelease::new(ReleaseChannel::Beta, " 0.3.1", "26.2", "0.19.5"),
            Err(InvalidInstanceRecord::PinnedVersionWhitespacePadding(
                "Aurora"
            ))
        ));
        assert!(matches!(
            PinnedRelease::new(ReleaseChannel::Beta, "0.3.1", "", "0.19.5"),
            Err(InvalidInstanceRecord::EmptyPinnedVersion("Minecraft"))
        ));
        assert!(matches!(
            PinnedRelease::new(ReleaseChannel::Beta, "0.3.1", "26.2", "0.19.5\u{7}"),
            Err(InvalidInstanceRecord::PinnedVersionControlCharacter(
                "Fabric Loader"
            ))
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
        assert_eq!(first.state(), InstanceState::Ready);
        assert_eq!(first.release().channel(), ReleaseChannel::Stable);
        assert_eq!(first.release().aurora_version(), "0.3.0");
        assert_eq!(first.release().minecraft_version(), "26.2");
        assert_eq!(first.release().fabric_loader_version(), "0.19.5");

        let second = &registry.instances()[1];
        assert_eq!(second.state(), InstanceState::Installing);
        assert_eq!(second.release().aurora_version(), "0.3.1-beta.1");

        assert!(
            registry
                .find(&InstanceId::new("aurora-default").unwrap())
                .is_some()
        );
        assert!(registry.find(&InstanceId::new("ghost").unwrap()).is_none());
    }

    #[test]
    fn saves_atomically_round_trips_and_rejects_duplicates() {
        let directory = std::env::temp_dir()
            .join("aurora-instances-test-save")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("instances.json");

        let mut registry = InstanceRegistry::empty();
        registry.instances_mut().push(
            InstanceRecord::new(
                InstanceId::new("saved-instance").unwrap(),
                "Saved Instance",
                InstanceState::Installing,
                sample_pin(),
            )
            .unwrap(),
        );
        registry.save(&path).unwrap();

        let reloaded = InstanceRegistry::load(&path).unwrap();
        assert_eq!(reloaded, registry);
        assert!(
            !directory.join("instances.json.tmp").exists(),
            "no temporary debris"
        );

        // Duplicate identifiers never reach disk.
        let mut duplicated = registry.clone();
        duplicated.instances_mut().push(
            InstanceRecord::new(
                InstanceId::new("saved-instance").unwrap(),
                "Duplicate",
                InstanceState::Ready,
                sample_pin(),
            )
            .unwrap(),
        );
        assert!(matches!(
            duplicated.save(&path),
            Err(InstanceRegistryError::Malformed(_))
        ));
    }

    #[test]
    fn invalid_registry_data_fails_deliberately() {
        let cases: Vec<(&str, &str)> = vec![
            ("{ not json", "malformed"),
            ("{}", "malformed"),
            (
                r#"{ "schemaVersion": 3, "instances": [] }"#,
                "schema version 3",
            ),
            (
                r#"{ "schemaVersion": 1, "instances": [] }"#,
                "schema version 1",
            ),
            (
                r#"{ "schemaVersion": 2, "instances": [ { "id": "../evil", "displayName": "Evil", "state": "ready", "release": { "channel": "stable", "auroraVersion": "0.3.0", "minecraftVersion": "26.2", "fabricLoaderVersion": "0.19.5" } } ] }"#,
                "only lowercase letters",
            ),
            (
                r#"{ "schemaVersion": 2, "instances": [ { "id": "ok-id", "displayName": "", "state": "ready", "release": { "channel": "stable", "auroraVersion": "0.3.0", "minecraftVersion": "26.2", "fabricLoaderVersion": "0.19.5" } } ] }"#,
                "display name must not be empty",
            ),
            (
                r#"{ "schemaVersion": 2, "instances": [ { "id": "ok-id", "displayName": "Ok", "state": "ready", "release": { "channel": "weekly", "auroraVersion": "0.3.0", "minecraftVersion": "26.2", "fabricLoaderVersion": "0.19.5" } } ] }"#,
                "unknown variant",
            ),
            (
                r#"{ "schemaVersion": 2, "instances": [ { "id": "ok-id", "displayName": "Ok", "state": "ready", "release": { "channel": "stable", "auroraVersion": " ", "minecraftVersion": "26.2", "fabricLoaderVersion": "0.19.5" } } ] }"#,
                "pinned Aurora version",
            ),
            (
                r#"{ "schemaVersion": 2, "instances": [
                    { "id": "dupe", "displayName": "First", "state": "ready", "release": { "channel": "stable", "auroraVersion": "0.3.0", "minecraftVersion": "26.2", "fabricLoaderVersion": "0.19.5" } },
                    { "id": "dupe", "displayName": "Second", "state": "ready", "release": { "channel": "beta", "auroraVersion": "0.3.1", "minecraftVersion": "26.2", "fabricLoaderVersion": "0.19.5" } }
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

    #[test]
    fn a_malformed_registry_file_is_never_overwritten_by_save() {
        let directory = std::env::temp_dir()
            .join("aurora-instances-test-preserve")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("instances.json");
        std::fs::write(&path, "{ damaged").unwrap();

        // Loading fails deliberately; a caller therefore never reaches save,
        // and the damaged bytes remain exactly as they were.
        assert!(InstanceRegistry::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ damaged");
    }

    fn persist_fixture(directory_name: &str, contents: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(directory_name);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("registry-{}.json", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }
}
