//! Official Minecraft metadata: discovery, external DTOs, and the narrow
//! fetch boundary.
//!
//! This module owns everything that still speaks Mojang's JSON shapes:
//!
//! - the version-manifest discovery document and its per-version entries;
//! - the per-version document (external DTOs for the fields resolution needs);
//! - the metadata transport, which is deliberately separate from the Phase 2
//!   verified-artifact pipeline and never pretends to be it.
//!
//! ## Trust model
//!
//! The version manifest is *bootstrap discovery metadata*: no prior digest
//! exists before it is fetched, so its trust is HTTPS transport plus parsing
//! and validation — it is never called a verified artifact. Every manifest
//! entry, however, carries the official SHA-1 of its version document, so
//! version documents are hash-addressable: they are fetched from the
//! manifest-provided URL and verified against the manifest-provided SHA-1
//! before parsing. This uses Mojang's own integrity data without distorting
//! the SHA-256-addressed verified cache, which remains reserved for product
//! artifact acquisition.
//!
//! ## Modern-document policy
//!
//! Aurora targets modern Minecraft. Documents are expected to carry the
//! `arguments` object, self-contained release metadata, and per-platform
//! classifier libraries instead of legacy `natives`/`classifiers`/`extract`
//! structures; historical shapes (`inheritsFrom`, `minecraftArguments`,
//! old_beta/old_alpha) are rejected deliberately as unsupported rather than
//! half-resolved.

use std::fmt;

use serde::Deserialize;
use url::Url;

use crate::downloads::{self, DownloadError, DownloadOptions, is_loopback_host};
use crate::integrity::Sha1Digest;
use crate::minecraft::rules::Rule;

/// The official Mojang version-manifest endpoint this launcher resolves from.
pub const OFFICIAL_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Metadata documents are small (the current manifest is well under one
/// megabyte); a hard cap keeps a hostile or broken endpoint from buffering
/// unbounded memory. Product artifacts never flow through this path.
const MAX_METADATA_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// Maximum version-identifier length; real ids ("26.3-rc-3") are far shorter.
const MAX_VERSION_ID_LENGTH: usize = 64;

/// An exact Minecraft version identifier as published by Mojang.
///
/// Version identifiers are lookup keys, not filesystem components. Requests
/// are validated for shape (non-empty, bounded, limited charset) so a
/// malformed request fails at the boundary instead of inside resolution.
/// Historical manifest entries use looser spellings (for example
/// `1.14.2 Pre-Release 4` contains spaces), so listing-level validation
/// checks only presence and length; strict charset validation applies to the
/// version the launcher resolves.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MinecraftVersionId(String);

impl MinecraftVersionId {
    pub fn new(id: &str) -> Result<Self, InvalidMinecraftVersion> {
        if id.is_empty() || id.len() > MAX_VERSION_ID_LENGTH {
            return Err(InvalidMinecraftVersion {
                reason: format!(
                    "a Minecraft version id must be 1 to {MAX_VERSION_ID_LENGTH} characters long"
                ),
            });
        }

        let valid = id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
        if !valid {
            return Err(InvalidMinecraftVersion {
                reason: format!(
                    "a Minecraft version id may only contain letters, digits, '.', '_', '+', and '-' ('{id}' does not)"
                ),
            });
        }

        Ok(Self(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MinecraftVersionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A malformed version identifier request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidMinecraftVersion {
    pub reason: String,
}

impl fmt::Display for InvalidMinecraftVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for InvalidMinecraftVersion {}

/// The metadata roots this launcher resolves Minecraft versions from.
///
/// Production always uses the pinned official manifest endpoint. A loopback
/// constructor exists for deterministic tests, mirroring the launcher's
/// established loopback test-transport policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataEndpoints {
    manifest_url: Url,
}

impl MetadataEndpoints {
    /// The official Mojang metadata roots.
    pub fn official() -> Self {
        Self {
            manifest_url: Url::parse(OFFICIAL_MANIFEST_URL)
                .expect("the official manifest URL is a valid HTTPS URL"),
        }
    }

    /// Test endpoints served from an explicit loopback base URL.
    pub fn loopback_for_testing(base_url: &str) -> Self {
        let mut manifest_url = Url::parse(base_url).expect("test base URL must parse");
        manifest_url.set_path("/mc/game/version_manifest_v2.json");
        assert!(
            is_loopback_host(&manifest_url),
            "test metadata endpoints must stay on the loopback"
        );
        Self { manifest_url }
    }

    pub fn manifest_url(&self) -> &Url {
        &self.manifest_url
    }
}

/// The publication type of a Minecraft version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionType {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
}

impl VersionType {
    /// Whether this type carries modern launcher metadata semantics.
    ///
    /// Historical types (old_beta, old_alpha) use argument and library shapes
    /// this launcher deliberately does not support.
    pub fn is_modern(self) -> bool {
        matches!(self, Self::Release | Self::Snapshot)
    }

    pub fn as_mojang_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Snapshot => "snapshot",
            Self::OldBeta => "old_beta",
            Self::OldAlpha => "old_alpha",
        }
    }
}

impl fmt::Display for VersionType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_mojang_str())
    }
}

/// The version manifest: Mojang's discovery document listing every version.
///
/// Only the fields resolution needs are represented; `latest`, timestamps,
/// and compliance levels are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionManifest {
    pub versions: Vec<ManifestVersionEntry>,
}

/// One version listed by the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestVersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: VersionType,
    /// The version document's URL; provided by official metadata.
    pub url: String,
    /// The version document's official SHA-1; provided by official metadata.
    pub sha1: String,
}

impl VersionManifest {
    /// Parses and validates a manifest document.
    ///
    /// Validation is deliberate: entry ids must be present and bounded (their
    /// spellings may be historical), document URLs must be HTTPS (or loopback
    /// for the test transport), and digests must be canonical SHA-1
    /// encodings. Malformed manifests are rejected wholesale rather than
    /// partially trusted.
    pub fn from_json(json: &str) -> Result<Self, MetadataError> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|error| MetadataError::ManifestInvalid {
                reason: error.to_string(),
            })?;

        for entry in &manifest.versions {
            if entry.id.trim().is_empty() || entry.id.len() > MAX_VERSION_ID_LENGTH {
                return Err(MetadataError::ManifestInvalid {
                    reason: format!(
                        "manifest entry id must be 1 to {MAX_VERSION_ID_LENGTH} non-whitespace characters ('{}' is not)",
                        entry.id
                    ),
                });
            }
            validate_metadata_url(&entry.url).map_err(|reason| MetadataError::ManifestInvalid {
                reason: format!("entry '{}' has an unusable URL: {reason}", entry.id),
            })?;
            Sha1Digest::parse(&entry.sha1).map_err(|error| MetadataError::ManifestInvalid {
                reason: format!("entry '{}' has an invalid digest: {error}", entry.id),
            })?;
        }

        Ok(manifest)
    }

    /// Locates one exact version. There is no fuzzy search or aliasing.
    pub fn find(&self, id: &MinecraftVersionId) -> Option<&ManifestVersionEntry> {
        self.versions.iter().find(|entry| entry.id == id.as_str())
    }
}

/// The per-version metadata document (external DTO shape).
///
/// Only fields needed for planning are represented; irrelevant fields are
/// ignored. Legacy structures (`inheritsFrom`, `minecraftArguments`,
/// library `natives`/`extract`/`classifiers`) are parsed as detection
/// sentinels and rejected deliberately, so a historical document can never be
/// silently half-resolved.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDocument {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: VersionType,
    pub main_class: String,
    pub asset_index: AssetIndexDocument,
    pub downloads: DownloadsDocument,
    pub java_version: JavaVersionDocument,
    pub libraries: Vec<LibraryDocument>,
    pub arguments: ArgumentsDocument,
    #[serde(default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
}

/// The asset-index requirement published by a version document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexDocument {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub total_size: u64,
    pub url: String,
}

/// The downloadable objects published by a version document. Only the client
/// artifact is relevant to this launcher; the server jar is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DownloadsDocument {
    pub client: ArtifactDocument,
}

/// One downloadable object in Mojang metadata: URL, official SHA-1, and size.
/// Library artifacts additionally carry their repository path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ArtifactDocument {
    #[serde(default)]
    pub path: Option<String>,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

/// The Java runtime component a version requires.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersionDocument {
    pub component: String,
    pub major_version: u32,
}

/// One library entry: a Maven coordinate plus its downloadable artifact and
/// optional platform rules.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LibraryDocument {
    pub name: String,
    pub downloads: LibraryDownloadsDocument,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub natives: Option<serde_json::Value>,
    #[serde(default)]
    pub extract: Option<serde_json::Value>,
}

/// The downloads block of a library.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LibraryDownloadsDocument {
    pub artifact: Option<ArtifactDocument>,
    #[serde(default)]
    pub classifiers: Option<serde_json::Value>,
}

/// The modern launch-argument structure.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ArgumentsDocument {
    #[serde(default)]
    pub game: Vec<ArgumentDocument>,
    #[serde(default)]
    pub jvm: Vec<ArgumentDocument>,
}

/// A launch argument: a plain value or a rule-conditioned value.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ArgumentDocument {
    Plain(String),
    Conditional {
        rules: Vec<Rule>,
        value: ArgumentValueDocument,
    },
}

/// An argument value: one string or a list of strings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ArgumentValueDocument {
    One(String),
    Many(Vec<String>),
}

impl ArgumentValueDocument {
    /// The argument strings this value contributes, in document order.
    pub fn values(&self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value.clone()],
            Self::Many(values) => values.clone(),
        }
    }
}

impl VersionDocument {
    /// Parses and validates a version document.
    ///
    /// Structural validation happens here (identity, modern shapes, absence of
    /// legacy structures); semantic normalization into Aurora's install plan
    /// happens in [`crate::minecraft::plan`].
    pub fn from_json(json: &str) -> Result<Self, MetadataError> {
        let document: Self =
            serde_json::from_str(json).map_err(|error| MetadataError::DocumentInvalid {
                reason: error.to_string(),
            })?;

        if document.id.trim().is_empty() || document.id.len() > MAX_VERSION_ID_LENGTH {
            return Err(MetadataError::DocumentInvalid {
                reason: format!(
                    "document id must be 1 to {MAX_VERSION_ID_LENGTH} non-whitespace characters"
                ),
            });
        }
        let id = document.id.as_str();

        if !document.kind.is_modern() {
            return Err(MetadataError::Unsupported(format!(
                "Minecraft version '{id}' has historical type '{}' ; Aurora supports modern release and snapshot metadata only",
                document.kind.as_mojang_str()
            )));
        }

        if let Some(parent) = &document.inherits_from {
            return Err(MetadataError::Unsupported(format!(
                "Minecraft version '{id}' inherits from '{parent}'; version inheritance is a historical launcher mechanism Aurora does not support"
            )));
        }

        if let Some(_legacy) = &document.minecraft_arguments {
            return Err(MetadataError::Unsupported(format!(
                "Minecraft version '{id}' uses the legacy 'minecraftArguments' string; Aurora supports modern structured 'arguments' only"
            )));
        }

        if document.main_class.trim().is_empty() {
            return Err(MetadataError::DocumentInvalid {
                reason: format!("version '{id}' has no main class"),
            });
        }

        if document.asset_index.id.trim().is_empty() {
            return Err(MetadataError::DocumentInvalid {
                reason: format!("version '{id}' has no asset index id"),
            });
        }

        if document.java_version.component.trim().is_empty() {
            return Err(MetadataError::DocumentInvalid {
                reason: format!("version '{id}' has no Java runtime component"),
            });
        }
        if document.java_version.major_version == 0 {
            return Err(MetadataError::DocumentInvalid {
                reason: format!("version '{id}' declares a Java major version of zero"),
            });
        }

        if document.libraries.is_empty() {
            return Err(MetadataError::DocumentInvalid {
                reason: format!("version '{id}' declares no libraries"),
            });
        }
        for (index, library) in document.libraries.iter().enumerate() {
            if library.name.trim().is_empty() {
                return Err(MetadataError::DocumentInvalid {
                    reason: format!("version '{id}' has an unnamed library at index {index}"),
                });
            }
            if library.natives.is_some() || library.extract.is_some() {
                return Err(MetadataError::Unsupported(format!(
                    "version '{id}' uses legacy native-classifier libraries ({}); Aurora supports the modern per-platform library model only",
                    library.name
                )));
            }
            if library.downloads.classifiers.is_some() {
                return Err(MetadataError::Unsupported(format!(
                    "version '{id}' library {} publishes classifier downloads; Aurora supports the modern per-platform library model only",
                    library.name
                )));
            }
        }

        Ok(document)
    }
}

/// A failure while discovering or fetching official metadata.
#[derive(Debug)]
pub enum MetadataError {
    /// The requested version does not exist in the official manifest.
    VersionNotFound { requested: String },
    /// The metadata transport failed or returned an unusable response.
    Network(DownloadError),
    /// A metadata document exceeded the size the launcher will buffer.
    ResponseTooLarge { limit_bytes: usize },
    /// The version manifest could not be parsed or failed validation.
    ManifestInvalid { reason: String },
    /// The selected version document could not be parsed or failed validation.
    DocumentInvalid { reason: String },
    /// A fetched document did not match its official SHA-1.
    Integrity {
        context: String,
        expected: String,
        actual: String,
    },
    /// The document uses metadata semantics this launcher deliberately does
    /// not support.
    Unsupported(String),
}

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionNotFound { requested } => write!(
                formatter,
                "Minecraft version '{requested}' does not exist in the official version manifest"
            ),
            Self::Network(error) => write!(
                formatter,
                "official Minecraft metadata could not be fetched: {error}"
            ),
            Self::ResponseTooLarge { limit_bytes } => write!(
                formatter,
                "official Minecraft metadata exceeded the {limit_bytes}-byte limit the launcher will buffer"
            ),
            Self::ManifestInvalid { reason } => write!(
                formatter,
                "the official Minecraft version manifest is unusable: {reason}"
            ),
            Self::DocumentInvalid { reason } => write!(
                formatter,
                "the selected Minecraft version metadata is unusable: {reason}"
            ),
            Self::Integrity {
                context,
                expected,
                actual,
            } => write!(
                formatter,
                "{context} does not match its official SHA-1 digest: expected {expected} but computed {actual}"
            ),
            Self::Unsupported(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for MetadataError {}

/// Validates one metadata URL against the metadata transport policy.
///
/// Metadata URLs must be HTTPS, or cleartext HTTP only for explicit loopback
/// hosts (the established test-transport path); embedded credentials are
/// rejected. This is the same host policy the artifact transport enforces,
/// applied to the documents official metadata itself references.
fn validate_metadata_url(url_text: &str) -> Result<Url, String> {
    let parsed = Url::parse(url_text).map_err(|_| "the URL is not valid".to_owned())?;
    if parsed.cannot_be_a_base() {
        return Err("the URL is not valid".to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("the URL must not embed user credentials".to_owned());
    }
    if parsed.scheme() == "https" {
        return Ok(parsed);
    }
    if parsed.scheme() == "http" && is_loopback_host(&parsed) {
        return Ok(parsed);
    }
    Err(format!(
        "the URL must use HTTPS ('{url_text}' is not a secure metadata source)"
    ))
}

/// Fetches the official version manifest.
pub async fn fetch_manifest(
    endpoints: &MetadataEndpoints,
    options: &DownloadOptions,
) -> Result<VersionManifest, MetadataError> {
    let bytes = fetch_document(endpoints.manifest_url(), None, "version manifest", options).await?;
    let text = bytes_to_document_text(&bytes, "version manifest")?;
    VersionManifest::from_json(&text)
}

/// Fetches, verifies, and parses one version document.
///
/// The document is fetched from the manifest-provided URL and verified against
/// the manifest-provided SHA-1 before parsing, so a corrupted or truncated
/// transfer never reaches the DTO layer.
pub async fn fetch_version_document(
    entry: &ManifestVersionEntry,
    options: &DownloadOptions,
) -> Result<VersionDocument, MetadataError> {
    let url =
        validate_metadata_url(&entry.url).map_err(|reason| MetadataError::ManifestInvalid {
            reason: format!("entry '{}' has an unusable URL: {reason}", entry.id),
        })?;
    let expected =
        Sha1Digest::parse(&entry.sha1).map_err(|error| MetadataError::ManifestInvalid {
            reason: format!("entry '{}' has an invalid digest: {error}", entry.id),
        })?;

    let bytes = fetch_document(&url, Some(expected), &entry.id, options).await?;
    let text = bytes_to_document_text(&bytes, &entry.id)?;
    VersionDocument::from_json(&text)
}

/// Streams one metadata document into memory under a hard size cap, verifying
/// its official SHA-1 when one is known.
///
/// This is the narrow metadata fetch boundary. It is not the Phase 2
/// verified-artifact pipeline: metadata is bounded, buffered in memory, never
/// promoted into the content-addressed store, and never persisted. The only
/// documents that flow through here are the ones official metadata itself
/// chains to (the manifest, and version documents addressed by manifest URL
/// and SHA-1).
async fn fetch_document(
    url: &Url,
    expected_sha1: Option<Sha1Digest>,
    context: &str,
    options: &DownloadOptions,
) -> Result<Vec<u8>, MetadataError> {
    let client = downloads::build_client(options);
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| MetadataError::Network(DownloadError::from_transport(error)))?;

    let status = response.status();
    if !status.is_success() {
        return Err(MetadataError::Network(DownloadError::HttpStatus {
            status: status.as_u16(),
        }));
    }

    if response
        .content_length()
        .is_some_and(|declared| declared as usize > MAX_METADATA_DOCUMENT_BYTES)
    {
        return Err(MetadataError::ResponseTooLarge {
            limit_bytes: MAX_METADATA_DOCUMENT_BYTES,
        });
    }

    let mut document = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| MetadataError::Network(DownloadError::from_transport(error)))?
    {
        if document.len() + chunk.len() > MAX_METADATA_DOCUMENT_BYTES {
            return Err(MetadataError::ResponseTooLarge {
                limit_bytes: MAX_METADATA_DOCUMENT_BYTES,
            });
        }
        document.extend_from_slice(&chunk);
    }

    if let Some(expected) = expected_sha1 {
        let actual = Sha1Digest::compute(&document);
        if actual != expected {
            return Err(MetadataError::Integrity {
                context: format!("Minecraft version document '{context}'"),
                expected: expected.as_hex(),
                actual: actual.as_hex(),
            });
        }
    }

    Ok(document)
}

/// Decodes fetched bytes as document text.
fn bytes_to_document_text(bytes: &[u8], context: &str) -> Result<String, MetadataError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| MetadataError::DocumentInvalid {
        reason: format!("document '{context}' is not valid UTF-8"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::rules::RuleAction;
    use crate::test_support::{TestResponse, TestServer};
    use sha1::{Digest as _, Sha1};
    use std::sync::Arc;
    use std::time::Duration;

    fn quick_options() -> DownloadOptions {
        DownloadOptions {
            connect_timeout: Duration::from_secs(5),
            idle_read_timeout: Duration::from_secs(5),
            max_redirects: downloads::MAX_REDIRECTS,
        }
    }

    fn sha1_of(bytes: &[u8]) -> String {
        let digest: [u8; 20] = Sha1::digest(bytes).into();
        Sha1Digest::from_sha1(digest).as_hex()
    }

    const MANIFEST_FIXTURE: &str = r#"{
        "latest": { "release": "26.2", "snapshot": "26.3-rc-3" },
        "versions": [
            {
                "id": "26.2",
                "type": "release",
                "url": "https://piston-meta.mojang.com/v1/packages/1595470509933451a460bd157624e6e4f083890b/26.2.json",
                "time": "2026-09-14T06:40:38+00:00",
                "releaseTime": "2026-06-16T12:03:33+00:00",
                "sha1": "1595470509933451a460bd157624e6e4f083890b",
                "complianceLevel": 1
            },
            {
                "id": "1.21.11",
                "type": "release",
                "url": "https://piston-meta.mojang.com/v1/packages/1832adbc4eee60faa097bb1409be305a0abbf3d2/1.21.11.json",
                "sha1": "1832adbc4eee60faa097bb1409be305a0abbf3d2"
            }
        ]
    }"#;

    #[test]
    fn the_manifest_parses_and_finds_exact_versions() {
        let manifest = VersionManifest::from_json(MANIFEST_FIXTURE).unwrap();

        assert_eq!(manifest.versions.len(), 2);
        let id = MinecraftVersionId::new("1.21.11").unwrap();
        let entry = manifest.find(&id).expect("the exact version must be found");
        assert_eq!(entry.kind, VersionType::Release);
        assert_eq!(entry.sha1, "1832adbc4eee60faa097bb1409be305a0abbf3d2");

        assert!(
            manifest
                .find(&MinecraftVersionId::new("1.21.12").unwrap())
                .is_none()
        );
        assert!(
            manifest
                .find(&MinecraftVersionId::new("26.2").unwrap())
                .is_some()
        );

        // Historical spellings are listable entries even though the launcher
        // only requests modern ids.
        let historical = VersionManifest::from_json(
            r#"{ "versions": [ { "id": "1.14.2 Pre-Release 4", "type": "snapshot", "url": "https://piston-meta.mojang.com/v1/packages/1832adbc4eee60faa097bb1409be305a0abbf3d2/x.json", "sha1": "1832adbc4eee60faa097bb1409be305a0abbf3d2" } ] }"#,
        );
        assert!(historical.is_ok(), "{historical:?}");
    }

    #[test]
    fn malformed_manifests_are_rejected_deliberately() {
        let cases = [
            ("not json", "the manifest must be JSON"),
            (
                r#"{ "versions": [ { "id": "1.21.11", "type": "release", "url": "http://meta.example.invalid/x.json", "sha1": "1832adbc4eee60faa097bb1409be305a0abbf3d2" } ] }"#,
                "cleartext non-loopback metadata URLs are rejected",
            ),
            (
                r#"{ "versions": [ { "id": "1.21.11", "type": "release", "url": "https://meta.example.invalid/x.json", "sha1": "deadbeef" } ] }"#,
                "digests must be canonical SHA-1 encodings",
            ),
            (
                r#"{ "versions": [ { "id": "", "type": "release", "url": "https://meta.example.invalid/x.json", "sha1": "1832adbc4eee60faa097bb1409be305a0abbf3d2" } ] }"#,
                "entry ids must be present",
            ),
        ];

        for (json, why) in cases {
            assert!(
                matches!(
                    VersionManifest::from_json(json),
                    Err(MetadataError::ManifestInvalid { .. })
                ),
                "{why}"
            );
        }
    }

    /// A minimal but structurally faithful modern version document covering
    /// the shapes planning relies on.
    fn version_document_fixture() -> String {
        r#"{
            "id": "26.2",
            "type": "release",
            "time": "2026-09-14T06:40:38+00:00",
            "releaseTime": "2026-06-16T12:03:33+00:00",
            "complianceLevel": 1,
            "minimumLauncherVersion": 21,
            "assets": "32",
            "mainClass": "net.minecraft.client.main.Main",
            "javaVersion": { "component": "java-runtime-epsilon", "majorVersion": 25 },
            "assetIndex": {
                "id": "32",
                "sha1": "958c0c70425f7802b1c21ce25b84f3ffd66f778c",
                "size": 586366,
                "totalSize": 480492719,
                "url": "https://piston-meta.mojang.com/v1/packages/958c0c70425f7802b1c21ce25b84f3ffd66f778c/32.json"
            },
            "downloads": {
                "client": {
                    "sha1": "e6e7b5c2f8e0f8e7e6a1b2c3d4e5f60718293a4b",
                    "size": 33816576,
                    "url": "https://piston-data.mojang.com/v1/objects/e6e7b5c2f8e0f8e7e6a1b2c3d4e5f60718293a4b/client.jar"
                },
                "server": {
                    "sha1": "0000000000000000000000000000000000000000",
                    "size": 1,
                    "url": "https://piston-data.mojang.com/v1/objects/0000000000000000000000000000000000000000/server.jar"
                }
            },
            "libraries": [
                {
                    "downloads": {
                        "artifact": {
                            "path": "at/yawk/lz4/lz4-java/1.10.1/lz4-java-1.10.1.jar",
                            "sha1": "f541d7f910fe3d76f38f799c507c48cc81b12ecb",
                            "size": 910232,
                            "url": "https://libraries.minecraft.net/at/yawk/lz4/lz4-java/1.10.1/lz4-java-1.10.1.jar"
                        }
                    },
                    "name": "at.yawk.lz4:lz4-java:1.10.1"
                },
                {
                    "downloads": {
                        "artifact": {
                            "path": "com/mojang/jtracy/1.0.37/jtracy-1.0.37-natives-linux.jar",
                            "sha1": "e1b4395227af41195da9e2ead13c48ee0b7d31cd",
                            "size": 193951,
                            "url": "https://libraries.minecraft.net/com/mojang/jtracy/1.0.37/jtracy-1.0.37-natives-linux.jar"
                        }
                    },
                    "name": "com.mojang:jtracy:1.0.37:natives-linux",
                    "rules": [ { "action": "allow", "os": { "name": "linux" } } ]
                }
            ],
            "arguments": {
                "game": [
                    "--username", "${auth_player_name}",
                    { "rules": [ { "action": "allow", "features": { "is_demo_user": true } } ], "value": "--demo" }
                ],
                "jvm": [
                    { "rules": [ { "action": "allow", "os": { "name": "osx" } } ], "value": "-XstartOnFirstThread" },
                    { "rules": [ { "action": "allow", "os": { "arch": "x86" } } ], "value": ["-Xss1M"] },
                    "-Djava.library.path=${natives_directory}"
                ]
            },
            "logging": {
                "client": {
                    "argument": "-Dlog4j.configurationFile=${path}",
                    "file": { "id": "client-1.21.2.xml", "sha1": "39384bd14c0606d812afec88d8aff595b2587dd9", "size": 1073, "url": "https://piston-data.mojang.com/v1/objects/39384bd14c0606d812afec88d8aff595b2587dd9/client-1.21.2.xml" },
                    "type": "log4j2-xml"
                }
            }
        }"#
        .to_owned()
    }

    #[test]
    fn a_modern_version_document_parses_with_its_relevant_shapes() {
        let document = VersionDocument::from_json(&version_document_fixture()).unwrap();

        assert_eq!(document.id, "26.2");
        assert_eq!(document.kind, VersionType::Release);
        assert_eq!(document.main_class, "net.minecraft.client.main.Main");
        assert_eq!(document.java_version.component, "java-runtime-epsilon");
        assert_eq!(document.java_version.major_version, 25);
        assert_eq!(document.asset_index.id, "32");
        assert_eq!(document.downloads.client.size, 33816576);
        assert_eq!(document.libraries.len(), 2);
        assert_eq!(
            document.libraries[1].rules.as_ref().unwrap()[0].action,
            RuleAction::Allow
        );
        assert!(document.arguments.game.iter().any(|argument| matches!(
            argument,
            ArgumentDocument::Plain(value) if value == "--username"
        )));
        // Irrelevant fields (logging, minimumLauncherVersion, server) are
        // ignored without failing the parse.
    }

    #[test]
    fn historical_and_legacy_document_shapes_are_rejected_deliberately() {
        let base = version_document_fixture();

        let inherits = base.replacen(
            "\"id\": \"26.2\",",
            "\"id\": \"26.2\",\n\"inheritsFrom\": \"26.1\",",
            1,
        );
        assert!(matches!(
            VersionDocument::from_json(&inherits),
            Err(MetadataError::Unsupported(_))
        ));

        let legacy_args = base.replacen(
            "\"arguments\": {",
            "\"minecraftArguments\": \"--username ${auth_player_name}\",\n\"arguments\": {",
            1,
        );
        assert!(matches!(
            VersionDocument::from_json(&legacy_args),
            Err(MetadataError::Unsupported(_))
        ));

        let old_beta = base.replacen("\"type\": \"release\"", "\"type\": \"old_beta\"", 1);
        assert!(matches!(
            VersionDocument::from_json(&old_beta),
            Err(MetadataError::Unsupported(_))
        ));

        let natives = base.replacen(
            "\"name\": \"at.yawk.lz4:lz4-java:1.10.1\"",
            "\"name\": \"at.yawk.lz4:lz4-java:1.10.1\",\n\"natives\": { \"linux\": \"natives-linux\" }, \"extract\": { \"exclude\": [ \"META-INF/\" ] }",
            1,
        );
        assert!(matches!(
            VersionDocument::from_json(&natives),
            Err(MetadataError::Unsupported(_))
        ));
    }

    #[test]
    fn malformed_version_documents_are_rejected_deliberately() {
        let base = version_document_fixture();

        for (broken, why) in [
            (
                base.replacen("\"mainClass\": \"net.minecraft.client.main.Main\",", "", 1),
                "a missing main class is invalid",
            ),
            (
                base.replacen("\"arguments\": {", "\"argumentsX\": {", 1),
                "missing modern arguments are invalid",
            ),
        ] {
            assert!(
                matches!(
                    VersionDocument::from_json(&broken),
                    Err(MetadataError::DocumentInvalid { .. })
                ),
                "{why}"
            );
        }

        assert!(matches!(
            VersionDocument::from_json("not json"),
            Err(MetadataError::DocumentInvalid { .. })
        ));
    }

    #[tokio::test]
    async fn fetching_resolves_the_manifest_and_a_sha1_verified_version_document() {
        let document_body = version_document_fixture();
        let document_sha1 = sha1_of(document_body.as_bytes());
        let server = TestServer::spawn(Arc::new(move |request| {
            match request.path.as_str() {
            "/mc/game/version_manifest_v2.json" => TestResponse::ok(
                format!(
                    r#"{{"latest": {{"release": "26.2"}}, "versions": [{{"id": "26.2", "type": "release", "url": "{}/v1/packages/{}/26.2.json", "sha1": "{document_sha1}"}}]}}"#,
                    request.base_url, document_sha1
                )
                .as_bytes(),
            ),
            "/v1/packages" => unreachable!("never requested exactly"),
            _ if request.path.starts_with(&format!("/v1/packages/{document_sha1}")) => {
                TestResponse::ok(document_body.as_bytes())
            }
            _ => TestResponse::status(404),
        }
        }));

        let endpoints = MetadataEndpoints::loopback_for_testing(server.base_url());
        let manifest = fetch_manifest(&endpoints, &quick_options()).await.unwrap();
        let entry = manifest
            .find(&MinecraftVersionId::new("26.2").unwrap())
            .expect("the version must be listed");

        let document = fetch_version_document(entry, &quick_options())
            .await
            .unwrap();
        assert_eq!(document.id, "26.2");
        assert_eq!(document.java_version.major_version, 25);
    }

    #[tokio::test]
    async fn a_tampered_version_document_fails_its_sha1_verification() {
        let document_body = version_document_fixture();
        let honest_sha1 = sha1_of(document_body.as_bytes());
        let server = TestServer::spawn(Arc::new(move |request| {
            match request.path.as_str() {
            "/mc/game/version_manifest_v2.json" => TestResponse::ok(
                format!(
                    r#"{{"versions": [{{"id": "26.2", "type": "release", "url": "{}/26.2.json", "sha1": "{honest_sha1}"}}]}}"#,
                    request.base_url
                )
                .as_bytes(),
            ),
            "/26.2.json" => TestResponse::ok(b"tampered document bytes"),
            _ => TestResponse::status(404),
        }
        }));

        let endpoints = MetadataEndpoints::loopback_for_testing(server.base_url());
        let manifest = fetch_manifest(&endpoints, &quick_options()).await.unwrap();
        let entry = manifest
            .find(&MinecraftVersionId::new("26.2").unwrap())
            .unwrap();

        let error = fetch_version_document(entry, &quick_options())
            .await
            .expect_err("tampered bytes must fail verification");

        assert!(matches!(error, MetadataError::Integrity { .. }), "{error}");
    }

    #[tokio::test]
    async fn transport_failures_are_structured_network_errors() {
        let server = TestServer::spawn(Arc::new(|_request| TestResponse::status(500)));
        let endpoints = MetadataEndpoints::loopback_for_testing(server.base_url());

        let error = fetch_manifest(&endpoints, &quick_options())
            .await
            .expect_err("a server error must fail the fetch");

        assert!(
            matches!(
                error,
                MetadataError::Network(DownloadError::HttpStatus { status: 500 })
                    | MetadataError::Network(DownloadError::Network(_))
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn version_ids_validate_shape_at_the_boundary() {
        for valid in ["1.21.11", "26.2", "26.3-rc-3", "25w14craftmine"] {
            assert!(MinecraftVersionId::new(valid).is_ok(), "{valid} is valid");
        }

        // "latest" is charset-valid but no such version exists; resolution
        // reports it as not found rather than aliasing anything.
        assert!(MinecraftVersionId::new("latest").is_ok());

        for invalid in ["", "1.21.11 ", "../evil", "a\u{301}"] {
            assert!(
                MinecraftVersionId::new(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
    }

    /// Controlled live verification against the real official metadata chain.
    ///
    /// Ignored by default so the offline suite never depends on Mojang; run
    /// explicitly with `cargo test -- --ignored --nocapture` when verifying
    /// drift. Only metadata documents are fetched: no client jar, library,
    /// native, asset, or runtime is downloaded.
    #[tokio::test]
    #[ignore = "fetches live official Mojang metadata"]
    async fn live_official_metadata_resolves_a_modern_release() {
        use crate::minecraft::plan::plan_version_document;
        use crate::minecraft::rules::PlatformProfile as Profile;

        let endpoints = MetadataEndpoints::official();
        let manifest = fetch_manifest(&endpoints, &DownloadOptions::default())
            .await
            .expect("the official manifest must resolve");

        for version in ["1.21.11", "26.2"] {
            let entry = manifest
                .find(&MinecraftVersionId::new(version).unwrap())
                .unwrap_or_else(|| panic!("{version} must exist in the official manifest"));

            let document = fetch_version_document(entry, &DownloadOptions::default())
                .await
                .unwrap_or_else(|error| panic!("{version} must resolve: {error}"));

            assert_eq!(document.id, version);

            let platform = Profile::current().expect("the host platform must be plannable");
            let plan = plan_version_document(&document, platform)
                .unwrap_or_else(|error| panic!("{version} must plan: {error}"));

            eprintln!(
                "[live] Minecraft {} ({}) on {}-{}: {} libraries ({} native), Java {} ({}), asset index {}, client {} bytes, {} game args, {} JVM args",
                plan.minecraft_version(),
                plan.version_type(),
                platform.os(),
                platform.arch(),
                plan.libraries().len(),
                plan.native_library_count(),
                plan.java().component(),
                plan.java().major_version(),
                plan.asset_index().id(),
                plan.client().size_bytes(),
                plan.launch().game_arguments().len(),
                plan.launch().jvm_arguments().len(),
            );

            assert!(!plan.libraries().is_empty());
            assert!(plan.client().size_bytes() > 0);
            assert!(plan.client().url().starts_with("https://"));
            assert!(!plan.asset_index().id().is_empty());
            assert!(plan.asset_index().artifact().size_bytes() > 0);
            assert!(!plan.java().component().is_empty());
            assert!(plan.java().major_version() >= 21);
            assert_eq!(plan.launch().main_class(), "net.minecraft.client.main.Main");
            assert!(!plan.launch().game_arguments().is_empty());
            assert!(!plan.launch().jvm_arguments().is_empty());
            // Placeholders stay unresolved.
            assert!(plan.launch().game_arguments().iter().any(|argument| {
                argument
                    .values
                    .iter()
                    .any(|value| value == "${auth_player_name}")
            }));
        }
    }
}
