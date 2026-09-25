//! Typed data model and resolution for Aurora release manifests.
//!
//! The model parses and validates release metadata locally; resolution
//! selects exact releases from a manifest. Aurora release *distribution*
//! is deliberately gated by the checked-in production manifest, embedded in
//! the application. Development builds also offer the separate loopback
//! fixture for new instances; existing fixture-pinned instances retain a
//! compatibility path in release builds.
//!
//! Manifest authenticity, honestly stated: a manifest fetched over HTTPS
//! would be HTTPS-authenticated release metadata, not independently
//! signature-verified — the SHA-256 values inside it verify downloaded
//! artifacts against the manifest, and compromise of the manifest origin
//! could replace both artifact URL and expected digest together. No signing
//! infrastructure exists, so none is invented.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::downloads::is_loopback_host;

/// The only release-manifest schema version this launcher understands.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// The checked-in debug-only development release fixture.
/// Development artifact URLs are pinned to a developer-run loopback server.
const DEVELOPMENT_MANIFEST_JSON: &str = include_str!("../development/aurora-releases.json");
const PRODUCTION_MANIFEST_JSON: &str = include_str!("../production/aurora-releases.json");

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

        let mut versions = HashSet::new();
        for (index, release) in manifest.releases.iter().enumerate() {
            release
                .validate()
                .map_err(|reason| ManifestError::InvalidRelease {
                    index,
                    aurora_version: release.aurora_version.clone(),
                    reason,
                })?;
            if !versions.insert(release.aurora_version.as_str()) {
                return Err(ManifestError::InvalidRelease {
                    index,
                    aurora_version: release.aurora_version.clone(),
                    reason: "duplicate Aurora version".to_owned(),
                });
            }
        }

        Ok(manifest)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn releases(&self) -> &[AuroraRelease] {
        &self.releases
    }

    /// Resolves one exact release by Aurora version, optionally requiring a
    /// specific channel.
    ///
    /// Selection is exact, mirroring the launcher's version-selection
    /// policy everywhere: no "latest", no channel fallback, no
    /// substitution. Parsing rejects duplicate Aurora versions.
    pub fn resolve_exact(
        &self,
        aurora_version: &str,
        channel: Option<ReleaseChannel>,
    ) -> Option<&AuroraRelease> {
        self.releases.iter().find(|release| {
            release.aurora_version() == aurora_version
                && channel.is_none_or(|required| release.channel() == required)
        })
    }
}

/// The development fixture, parsed and validated like any other manifest.
/// Its artifact URLs are loopback-only; release builds do not offer it for
/// new instance creation.
pub fn development_manifest() -> Result<ReleaseManifest, ManifestError> {
    ReleaseManifest::from_json(DEVELOPMENT_MANIFEST_JSON)
}

/// The manually curated production manifest shipped in this launcher build.
pub fn production_manifest() -> Result<ReleaseManifest, ManifestError> {
    ReleaseManifest::from_json(PRODUCTION_MANIFEST_JSON)
}

/// Releases resolvable for existing instances, including the historical
/// development fixture so a new build does not strand older pinned content.
pub fn operational_manifest() -> Result<ReleaseManifest, ManifestError> {
    let mut manifest = production_manifest()?;
    manifest.releases.extend(development_manifest()?.releases);
    ReleaseManifest::from_json(
        &serde_json::to_string(&manifest)
            .map_err(|error| ManifestError::Json(error.to_string()))?,
    )
}

/// Releases offered for new instance creation. Production builds show only
/// reviewed entries; debug builds also offer explicit development fixtures.
pub fn creation_manifest() -> Result<ReleaseManifest, ManifestError> {
    #[cfg(debug_assertions)]
    {
        operational_manifest()
    }
    #[cfg(not(debug_assertions))]
    {
        production_manifest()
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fabric_api: Option<RequiredFabricApi>,
}

/// A release-pinned Fabric API runtime dependency, when Aurora requires it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequiredFabricApi {
    version: String,
    artifact: ReleaseArtifact,
}

impl RequiredFabricApi {
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn artifact(&self) -> &ReleaseArtifact {
        &self.artifact
    }
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

    pub fn fabric_api(&self) -> Option<&RequiredFabricApi> {
        self.fabric_api.as_ref()
    }

    fn validate(&self) -> Result<(), String> {
        validate_version_field("Aurora version", &self.aurora_version)?;
        validate_version_field("Minecraft version", &self.minecraft_version)?;
        validate_version_field("Fabric Loader version", &self.fabric_loader_version)?;
        self.java.validate()?;
        self.artifact.validate()?;
        if let Some(fabric_api) = &self.fabric_api {
            validate_version_field("Fabric API version", &fabric_api.version)?;
            if !fabric_api
                .version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
            {
                return Err("Fabric API version must be safe for a managed file name".to_owned());
            }
            fabric_api.artifact.validate()?;
        }
        Ok(())
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

impl ReleaseChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
        }
    }
}

impl fmt::Display for ReleaseChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
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
/// The URL and expected digest/size are validated here. The acquisition
/// boundary verifies downloaded bytes before cache or instance activation.
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
    // Production release artifacts must use HTTPS; cleartext is accepted
    // only for explicit loopback hosts — the launcher's documented
    // development/test transport (the checked-in development fixture is
    // served from one), the same policy every other transport boundary
    // applies.
    let parsed = Url::parse(value).map_err(|_| "artifact URL is not valid".to_owned())?;
    if parsed.cannot_be_a_base() {
        return Err("artifact URL is not valid".to_owned());
    }
    if parsed.scheme() == "https" {
        return Ok(());
    }
    if parsed.scheme() == "http" && is_loopback_host(&parsed) {
        return Ok(());
    }
    Err("artifact URL must use HTTPS".to_owned())
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
    fn rejects_duplicate_release_versions_even_across_channels() {
        let mut duplicate: serde_json::Value = serde_json::from_str(VALID_MANIFEST).unwrap();
        let version = duplicate["releases"][0]["auroraVersion"].clone();
        duplicate["releases"][1]["auroraVersion"] = version;
        let error = ReleaseManifest::from_json(&duplicate.to_string()).unwrap_err();
        assert!(matches!(
            error,
            ManifestError::InvalidRelease { index: 1, .. }
        ));
        assert!(error.to_string().contains("duplicate Aurora version"));
    }

    #[test]
    fn invalid_release_values_fail_with_descriptive_errors() {
        let valid: serde_json::Value = serde_json::from_str(VALID_MANIFEST).unwrap();

        let cases: Vec<(serde_json::Value, &str)> = vec![
            (
                replace_artifact_field(&valid, 0, "url", "http://insecure.example/aurora.jar"),
                "HTTPS",
            ),
            (
                replace_artifact_field(&valid, 0, "url", "https://"),
                "not valid",
            ),
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

    #[test]
    fn artifact_urls_accept_https_and_loopback_http_only() {
        assert!(validate_https_url("https://releases.example.invalid/a.jar").is_ok());
        assert!(validate_https_url("http://127.0.0.1:8765/aurora-0.3.0-dev.jar").is_ok());
        assert!(validate_https_url("http://localhost:8765/a.jar").is_ok());
        assert!(validate_https_url("http://[::1]:8765/a.jar").is_ok());

        for rejected in [
            "http://releases.example.invalid/a.jar",
            "http://192.168.0.2/a.jar",
            "ftp://releases.example.invalid/a.jar",
            "not a url",
        ] {
            assert!(
                validate_https_url(rejected).is_err(),
                "{rejected} must be rejected as an artifact URL"
            );
        }
    }

    #[test]
    fn exact_resolution_selects_precisely_without_substitution() {
        let manifest = ReleaseManifest::from_json(VALID_MANIFEST).unwrap();

        let stable = manifest
            .resolve_exact("0.3.0", Some(ReleaseChannel::Stable))
            .expect("the exact stable release must resolve");
        assert_eq!(stable.minecraft_version(), "1.21.11");

        // Same version pinned to the wrong channel does not resolve to it.
        assert!(
            manifest
                .resolve_exact("0.3.0", Some(ReleaseChannel::Nightly))
                .is_none()
        );
        // Unknown versions never fall back to anything.
        assert!(manifest.resolve_exact("0.9.9", None).is_none());
        // Channel-agnostic lookup still requires the exact version.
        let nightly = manifest
            .resolve_exact("0.4.0-nightly.20260914", None)
            .unwrap();
        assert_eq!(nightly.channel(), ReleaseChannel::Nightly);
    }

    #[test]
    fn the_development_fixture_parses_and_resolves() {
        let manifest = development_manifest().expect("the checked-in fixture must parse");

        let release = manifest
            .resolve_exact("0.3.0", Some(ReleaseChannel::Stable))
            .expect("the fixture's stable release must resolve");
        assert_eq!(release.minecraft_version(), "26.2");
        assert_eq!(release.fabric_loader_version(), "0.19.5");
        assert!(
            release
                .artifact()
                .url()
                .starts_with("http://127.0.0.1:8765/")
        );
        assert_eq!(release.artifact().size_bytes(), Some(57));

        // Every channel the model supports is represented by the fixture.
        for channel in [
            ReleaseChannel::Stable,
            ReleaseChannel::Beta,
            ReleaseChannel::Nightly,
        ] {
            assert!(
                manifest
                    .releases()
                    .iter()
                    .any(|release| release.channel() == channel),
                "the development fixture must carry a {channel:?} release"
            );
        }
    }

    #[test]
    fn production_release_is_exact_and_separate_from_the_development_fixture() {
        let production = production_manifest().unwrap();
        assert_eq!(production.schema_version(), 1);
        assert_eq!(production.releases().len(), 1);
        let release = production
            .resolve_exact("2.1.2", Some(ReleaseChannel::Stable))
            .unwrap();
        assert_eq!(release.minecraft_version(), "1.21.11");
        assert_eq!(release.fabric_loader_version(), "0.19.5");
        assert_eq!(release.java().major_version(), 21);
        assert_eq!(
            release.artifact().url(),
            "https://github.com/kalibsolomon-pixel/Aurora-Client/releases/download/v2.1.2/aurora-2.1.2.jar"
        );
        assert_eq!(
            release.artifact().sha256(),
            "55ac97f7494daa3866bb3b4aa8d23e49b240fe5ced00fbf1742f7214fc77c52a"
        );
        assert_eq!(release.artifact().size_bytes(), Some(2450086));
        let fabric_api = release
            .fabric_api()
            .expect("production Aurora requires Fabric API");
        assert_eq!(fabric_api.version(), "0.141.6+1.21.11");
        assert_eq!(
            fabric_api.artifact().sha256(),
            "bdff7fd7e220085cfad2ff9b1f40dde6534ae0b96cf378f97a374bc54cb9ed0f"
        );
        assert_eq!(fabric_api.artifact().size_bytes(), Some(2426039));
        assert!(
            production
                .resolve_exact("2.1.2", Some(ReleaseChannel::Beta))
                .is_none()
        );
        assert!(production.resolve_exact("2.1.1", None).is_none());
        assert!(
            development_manifest()
                .unwrap()
                .resolve_exact("2.1.2", None)
                .is_none()
        );
        let operational = operational_manifest().unwrap();
        assert!(
            operational
                .resolve_exact("2.1.2", Some(ReleaseChannel::Stable))
                .is_some()
        );
        assert!(operational.resolve_exact("0.3.0", None).is_some());
        assert_eq!(
            creation_manifest()
                .unwrap()
                .resolve_exact("0.3.0", None)
                .is_some(),
            cfg!(debug_assertions)
        );
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
