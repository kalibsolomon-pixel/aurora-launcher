//! Typed data model for future Aurora release manifests.
//!
//! This phase only represents and validates release metadata locally.
//! Fetching manifests, verifying artifact hashes, and installing releases are
//! deferred; nothing in this module performs I/O.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The only release-manifest schema version this launcher understands.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// A versioned collection of Aurora releases.
///
/// A manifest deliberately maps each Aurora release independently to its
/// compatible Minecraft version, Fabric Loader version, and Java requirement;
/// no release property is derived from any other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    schema_version: u32,
    releases: Vec<AuroraRelease>,
}

impl ReleaseManifest {
    /// The manifest schema version this parser enforces.
    pub const SCHEMA_VERSION: u32 = MANIFEST_SCHEMA_VERSION;

    /// Parses and validates a manifest from JSON text.
    ///
    /// Unknown schema versions and structurally invalid releases fail
    /// deliberately with a descriptive error.
    pub fn from_json(json: &str) -> Result<Self, ManifestError> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|error| ManifestError::Json(error.to_string()))?;

        if manifest.schema_version != Self::SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema {
                found: manifest.schema_version,
                expected: Self::SCHEMA_VERSION,
            });
        }

        for (index, release) in manifest.releases.iter().enumerate() {
            release
                .validate()
                .map_err(|reason| ManifestError::InvalidRelease {
                    index,
                    aurora_version: release.aurora_version.clone(),
                    reason,
                })?;
        }

        Ok(manifest)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn releases(&self) -> &[AuroraRelease] {
        &self.releases
    }
}

/// A single released Aurora build and its compatibility mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraRelease {
    aurora_version: String,
    channel: ReleaseChannel,
    minecraft_version: String,
    fabric_loader_version: String,
    java: JavaRequirement,
    artifact: ReleaseArtifact,
}

impl AuroraRelease {
    pub fn aurora_version(&self) -> &str {
        &self.aurora_version
    }

    pub fn channel(&self) -> ReleaseChannel {
        self.channel
    }

    pub fn minecraft_version(&self) -> &str {
        &self.minecraft_version
    }

    pub fn fabric_loader_version(&self) -> &str {
        &self.fabric_loader_version
    }

    pub fn java(&self) -> &JavaRequirement {
        &self.java
    }

    pub fn artifact(&self) -> &ReleaseArtifact {
        &self.artifact
    }

    fn validate(&self) -> Result<(), String> {
        validate_version_field("Aurora version", &self.aurora_version)?;
        validate_version_field("Minecraft version", &self.minecraft_version)?;
        validate_version_field("Fabric Loader version", &self.fabric_loader_version)?;
        self.java.validate()?;
        self.artifact.validate()
    }
}

/// The distribution channel a release was published to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseChannel {
    Stable,
    Beta,
    Nightly,
}

/// The Java runtime a release requires, identified by major version.
///
/// Vendor and distribution specifics are deferred to the Java-management phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaRequirement {
    major_version: u32,
}

impl JavaRequirement {
    pub fn major_version(&self) -> u32 {
        self.major_version
    }

    fn validate(&self) -> Result<(), String> {
        if self.major_version == 0 {
            return Err("Java requirement must specify a major version of at least 1".to_owned());
        }
        Ok(())
    }
}

/// The downloadable Aurora artifact for a release.
///
/// Only the representation is implemented: the URL is validated for shape and
/// the hash for encoding, but nothing is downloaded or verified yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseArtifact {
    url: String,
    sha256: String,
    size_bytes: Option<u64>,
}

impl ReleaseArtifact {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }

    fn validate(&self) -> Result<(), String> {
        validate_https_url(&self.url)?;
        validate_sha256_hex(&self.sha256)?;

        if self.size_bytes == Some(0) {
            return Err("artifact size, when present, must be greater than zero".to_owned());
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    Json(String),
    UnsupportedSchema {
        found: u32,
        expected: u32,
    },
    InvalidRelease {
        index: usize,
        aurora_version: String,
        reason: String,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(detail) => write!(
                formatter,
                "release manifest is not valid JSON or does not match the manifest schema: {detail}"
            ),
            Self::UnsupportedSchema { found, expected } => write!(
                formatter,
                "release manifest schema version {found} is not supported; this launcher understands schema version {expected}"
            ),
            Self::InvalidRelease {
                index,
                aurora_version,
                reason,
            } => write!(
                formatter,
                "release manifest entry {index} (Aurora version {aurora_version}) is invalid: {reason}"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

fn validate_version_field(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.trim() != value {
        return Err(format!(
            "{field} must not have leading or trailing whitespace"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(())
}

fn validate_https_url(value: &str) -> Result<(), String> {
    let rest = value
        .strip_prefix("https://")
        .ok_or_else(|| "artifact URL must use HTTPS".to_owned())?;

    if rest.is_empty() {
        return Err("artifact URL must include a host".to_owned());
    }
    if rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("artifact URL must not contain whitespace or control characters".to_owned());
    }

    Ok(())
}

fn validate_sha256_hex(value: &str) -> Result<(), String> {
    if value.len() != 64 {
        return Err(format!(
            "artifact SHA-256 hash must be exactly 64 hexadecimal characters, but is {} characters",
            value.len()
        ));
    }
    if !value
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'))
    {
        return Err("artifact SHA-256 hash must contain only hexadecimal characters".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_MANIFEST: &str = r#"{
        "schemaVersion": 1,
        "releases": [
            {
                "auroraVersion": "0.3.0",
                "channel": "stable",
                "minecraftVersion": "1.21.11",
                "fabricLoaderVersion": "0.17.3",
                "java": { "majorVersion": 21 },
                "artifact": {
                    "url": "https://releases.example.invalid/aurora/0.3.0/aurora-0.3.0.jar",
                    "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "sizeBytes": 1048576
                }
            },
            {
                "auroraVersion": "0.3.1-beta.2",
                "channel": "beta",
                "minecraftVersion": "1.21.9",
                "fabricLoaderVersion": "0.16.14",
                "java": { "majorVersion": 17 },
                "artifact": {
                    "url": "https://releases.example.invalid/aurora/0.3.1-beta.2/aurora-0.3.1-beta.2.jar",
                    "sha256": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"
                }
            },
            {
                "auroraVersion": "0.4.0-nightly.20260914",
                "channel": "nightly",
                "minecraftVersion": "1.21.11",
                "fabricLoaderVersion": "0.17.3",
                "java": { "majorVersion": 21 },
                "artifact": {
                    "url": "https://releases.example.invalid/aurora/nightly/aurora-0.4.0.jar",
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            }
        ]
    }"#;

    #[test]
    fn parses_a_valid_manifest_with_all_channels() {
        let manifest = ReleaseManifest::from_json(VALID_MANIFEST).unwrap();

        assert_eq!(manifest.schema_version(), 1);
        assert_eq!(manifest.releases().len(), 3);

        let stable = &manifest.releases()[0];
        assert_eq!(stable.channel(), ReleaseChannel::Stable);
        assert_eq!(stable.java().major_version(), 21);
        assert_eq!(stable.artifact().size_bytes(), Some(1048576));

        let beta = &manifest.releases()[1];
        assert_eq!(beta.channel(), ReleaseChannel::Beta);
        assert_eq!(beta.artifact().size_bytes(), None);
        assert_eq!(beta.artifact().sha256(), "FF".repeat(32));
    }

    #[test]
    fn releases_map_to_minecraft_fabric_and_java_independently() {
        let manifest = ReleaseManifest::from_json(VALID_MANIFEST).unwrap();

        let stable = &manifest.releases()[0];
        let nightly = &manifest.releases()[2];
        let beta = &manifest.releases()[1];

        // Two Aurora releases on different channels share one Minecraft version
        // and Fabric Loader version, while the beta release maps elsewhere.
        assert_eq!(stable.minecraft_version(), nightly.minecraft_version());
        assert_eq!(
            stable.fabric_loader_version(),
            nightly.fabric_loader_version()
        );
        assert_ne!(stable.aurora_version(), nightly.aurora_version());
        assert_ne!(stable.minecraft_version(), beta.minecraft_version());
        assert_ne!(stable.java().major_version(), beta.java().major_version());
    }

    #[test]
    fn channels_serialize_to_their_lowercase_names() {
        for (channel, name) in [
            (ReleaseChannel::Stable, "stable"),
            (ReleaseChannel::Beta, "beta"),
            (ReleaseChannel::Nightly, "nightly"),
        ] {
            let json = serde_json::to_string(&channel).unwrap();
            assert_eq!(json, format!("\"{name}\""));
            let parsed: ReleaseChannel = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, channel);
        }
    }

    #[test]
    fn rejects_unsupported_schema_versions() {
        let json = VALID_MANIFEST.replace("\"schemaVersion\": 1", "\"schemaVersion\": 2");

        let error = ReleaseManifest::from_json(&json).unwrap_err();

        assert!(matches!(
            error,
            ManifestError::UnsupportedSchema {
                found: 2,
                expected: 1
            }
        ));
    }

    #[test]
    fn rejects_malformed_json_and_missing_fields() {
        let error = ReleaseManifest::from_json("{ not json").unwrap_err();
        assert!(matches!(error, ManifestError::Json(_)));

        let missing_field = r#"{ "schemaVersion": 1, "releases": [ { "channel": "stable" } ] }"#;
        let error = ReleaseManifest::from_json(missing_field).unwrap_err();
        assert!(matches!(error, ManifestError::Json(_)));
    }

    #[test]
    fn invalid_release_values_fail_with_descriptive_errors() {
        let valid: serde_json::Value = serde_json::from_str(VALID_MANIFEST).unwrap();

        let cases: Vec<(serde_json::Value, &str)> = vec![
            (
                replace_artifact_field(&valid, 0, "url", "http://insecure.example/aurora.jar"),
                "HTTPS",
            ),
            (replace_artifact_field(&valid, 0, "url", "https://"), "host"),
            (
                replace_artifact_field(&valid, 0, "sha256", "deadbeef"),
                "64 hexadecimal",
            ),
            (
                replace_artifact_field(&valid, 0, "sha256", "z".repeat(64).as_str()),
                "hexadecimal",
            ),
            (
                replace_release_field(&valid, 0, "auroraVersion", ""),
                "must not be empty",
            ),
            (
                replace_release_field(&valid, 0, "minecraftVersion", " 1.21.11"),
                "whitespace",
            ),
            (
                replace_release_field(&valid, 1, "fabricLoaderVersion", "0.16.1\u{7}4"),
                "control characters",
            ),
            (replace_java_major_version(&valid, 0, 0), "at least 1"),
            (
                replace_artifact_field(&valid, 0, "sizeBytes", 0),
                "greater than zero",
            ),
            (
                replace_release_field(&valid, 0, "channel", "weekly"),
                "unknown variant",
            ),
        ];

        for (mutant, expected_fragment) in cases {
            let json = serde_json::to_string(&mutant).unwrap();
            let error = ReleaseManifest::from_json(&json).unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains(expected_fragment),
                "expected error for {json} to mention '{expected_fragment}', got: {message}"
            );
        }
    }

    fn replace_release_field(
        manifest: &serde_json::Value,
        index: usize,
        field: &str,
        value: &str,
    ) -> serde_json::Value {
        let mut mutant = manifest.clone();
        mutant["releases"][index][field] = serde_json::Value::String(value.to_owned());
        mutant
    }

    fn replace_artifact_field(
        manifest: &serde_json::Value,
        index: usize,
        field: &str,
        value: impl Into<serde_json::Value>,
    ) -> serde_json::Value {
        let mut mutant = manifest.clone();
        mutant["releases"][index]["artifact"][field] = value.into();
        mutant
    }

    fn replace_java_major_version(
        manifest: &serde_json::Value,
        index: usize,
        major_version: u64,
    ) -> serde_json::Value {
        let mut mutant = manifest.clone();
        mutant["releases"][index]["java"]["majorVersion"] = serde_json::Value::from(major_version);
        mutant
    }
}
