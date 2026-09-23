//! Official Fabric Meta: discovery, external DTOs, and the narrow fetch
//! boundary.
//!
//! This module owns everything that still speaks Fabric Meta's JSON shapes:
//!
//! - the Fabric Loader version list (`/versions/loader`);
//! - the loader profile document for one Minecraft + Loader combination
//!   (`/versions/loader/:game/:loader`), which embeds the loader's own
//!   launcher metadata (`launcherMeta`, published by the fabric-loader
//!   distribution on maven.fabricmc.net and served verbatim by Fabric Meta).
//!
//! ## Verified live behavior (September 2026)
//!
//! - `/versions/loader` lists `{separator, build, maven, version, stable}`
//!   entries, newest first (253 entries; newest stable loader `0.19.5`).
//! - `/versions/loader/:game/:loader` returns `{loader, intermediary,
//!   launcherMeta}` for a supported combination and HTTP 400 with a plain
//!   reason (`no mappings version found for 9.9.9` / `no loader version found
//!   for 0.0.1`) otherwise.
//! - `launcherMeta` is `{version: 2, min_java_version, libraries: {client,
//!   common, server, development}, mainClass}`. `common`/`client` library
//!   entries carry official digests (`md5`, `sha1`, `sha256`, `sha512`) and a
//!   `size`; the loader and intermediary artifacts the endpoint adds carry
//!   only Maven coordinates — no digests.
//! - `mainClass` is an object keyed by side (`client`/`server`); a bare
//!   string is accepted as the historical shape.
//! - The `development` library group is for development environments and is
//!   not part of a launch profile; it is ignored here.
//!
//! ## Trust model
//!
//! Fabric Meta documents are *bootstrap discovery metadata*, the same trust
//! class as Mojang's version manifest: no prior digest exists before the
//! fetch, so trust is HTTPS transport plus deliberate parsing and validation.
//! They never enter the content-addressed verified store. The digests the
//! loader's launcher metadata publishes for its libraries are official
//! Fabric expectations, recorded in the plan for the phase that acquires
//! artifacts — recording a digest is not itself verification.

use std::fmt;

use serde::Deserialize;
use url::Url;

use crate::downloads::{self, DownloadError, DownloadOptions, is_loopback_host};

/// The official Fabric Meta API root this launcher resolves from.
pub const OFFICIAL_META_URL: &str = "https://meta.fabricmc.net/v2/";

/// Fabric Meta documents are small (the loader list is a few dozen kilobytes);
/// a hard cap keeps a hostile or broken endpoint from buffering unbounded
/// memory. Product artifacts never flow through this path.
const MAX_METADATA_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// Maximum Loader version identifier length; real ids (`0.19.5`) are short.
const MAX_LOADER_VERSION_LENGTH: usize = 64;

/// An exact Fabric Loader version identifier as published by Fabric Meta.
///
/// Loader versions are lookup keys, not filesystem components. The charset
/// includes `+` because official loader versions use build-suffixed spellings
/// (for example `0.4.8+build.155`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoaderVersionId(String);

impl LoaderVersionId {
    pub fn new(version: &str) -> Result<Self, InvalidLoaderVersion> {
        if version.is_empty() || version.len() > MAX_LOADER_VERSION_LENGTH {
            return Err(InvalidLoaderVersion {
                reason: format!(
                    "a Fabric Loader version must be 1 to {MAX_LOADER_VERSION_LENGTH} characters long"
                ),
            });
        }

        let valid = version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
        if !valid {
            return Err(InvalidLoaderVersion {
                reason: format!(
                    "a Fabric Loader version may only contain letters, digits, '.', '_', '+', and '-' ('{version}' does not)"
                ),
            });
        }

        Ok(Self(version.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LoaderVersionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A malformed Fabric Loader version request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidLoaderVersion {
    pub reason: String,
}

impl fmt::Display for InvalidLoaderVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for InvalidLoaderVersion {}

/// The Fabric Meta roots this launcher resolves loader metadata from.
///
/// Production always uses the pinned official API. A loopback constructor
/// exists for deterministic tests, mirroring the launcher's established
/// loopback test-transport policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabricMetaEndpoints {
    base_url: Url,
}

impl FabricMetaEndpoints {
    /// The official Fabric Meta API root.
    pub fn official() -> Self {
        Self {
            base_url: Url::parse(OFFICIAL_META_URL)
                .expect("the official Fabric Meta URL is a valid HTTPS URL"),
        }
    }

    /// Test endpoints served from an explicit loopback base URL.
    pub fn loopback_for_testing(base_url: &str) -> Self {
        let base = Url::parse(base_url).expect("test base URL must parse");
        assert!(
            is_loopback_host(&base),
            "test Fabric Meta endpoints must stay on the loopback"
        );
        Self { base_url: base }
    }

    fn loader_versions_url(&self) -> Url {
        self.join("versions/loader")
    }

    /// The per-game loader listing: `/versions/loader/<game>`, whose entries
    /// combine the loader, intermediary, and launcher metadata.
    fn game_loader_versions_url(
        &self,
        game: &crate::minecraft::metadata::MinecraftVersionId,
    ) -> Url {
        let mut url = self.loader_versions_url();
        url.path_segments_mut()
            .expect("the Fabric Meta base has path segments")
            .push(game.as_str());
        url
    }

    fn loader_profile_url(
        &self,
        game: &crate::minecraft::metadata::MinecraftVersionId,
        loader: &LoaderVersionId,
    ) -> Url {
        let mut url = self.loader_versions_url();
        url.path_segments_mut()
            .expect("the Fabric Meta base has path segments")
            .push(game.as_str())
            .push(loader.as_str());
        url
    }

    fn join(&self, path: &str) -> Url {
        self.base_url.join(path).expect(
            "joining the validated Fabric Meta base with a static endpoint path yields a valid URL",
        )
    }
}

/// One Fabric Loader version as listed by `/versions/loader`.
///
/// Only identity fields are represented; `separator` and `build` are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LoaderVersionEntry {
    pub maven: String,
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

impl LoaderVersionEntry {
    /// Parses and validates the loader version list.
    pub fn list_from_json(json: &str) -> Result<Vec<Self>, FabricMetadataError> {
        let entries: Vec<Self> =
            serde_json::from_str(json).map_err(|error| FabricMetadataError::Malformed {
                reason: format!("the Fabric Loader version list is unusable: {error}"),
            })?;

        if entries.is_empty() {
            return Err(FabricMetadataError::Malformed {
                reason: "the Fabric Loader version list is empty".to_owned(),
            });
        }
        for entry in &entries {
            if entry.version.trim().is_empty() || entry.version.len() > MAX_LOADER_VERSION_LENGTH {
                return Err(FabricMetadataError::Malformed {
                    reason: format!(
                        "a listed loader version must be 1 to {MAX_LOADER_VERSION_LENGTH} non-whitespace characters ('{}' is not)",
                        entry.version
                    ),
                });
            }
            if entry.maven.trim().is_empty() {
                return Err(FabricMetadataError::Malformed {
                    reason: format!(
                        "listed loader version '{}' has no Maven coordinate",
                        entry.version
                    ),
                });
            }
        }

        Ok(entries)
    }

    /// Locates one exact loader version. There is no fuzzy search, no
    /// "latest", and no substitution.
    pub fn find<'a>(entries: &'a [Self], version: &LoaderVersionId) -> Option<&'a Self> {
        entries
            .iter()
            .find(|entry| entry.version == version.as_str())
    }
}

/// One library entry inside `launcherMeta.libraries`.
///
/// Official digests and sizes are optional in the external shape: the loader's
/// own libraries publish them, while entries Fabric Meta adds (the loader and
/// intermediary artifacts) publish none. `md5` and `sha512` are ignored;
/// SHA-256 is both the strongest digest Fabric publishes and the launcher's
/// cache identity algorithm.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MetaLibraryDocument {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// The `launcherMeta.libraries` groups. `development` is a development-
/// environment group and deliberately absent; `server` is not consumed
/// because Aurora launches clients.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LauncherLibrariesDocument {
    #[serde(default)]
    pub client: Vec<MetaLibraryDocument>,
    #[serde(default)]
    pub common: Vec<MetaLibraryDocument>,
}

/// The `mainClass` shape: an object keyed by launch side, or the historical
/// bare string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum MainClassDocument {
    Sides { client: String, server: String },
    One(String),
}

impl MainClassDocument {
    /// The client launch entry point.
    pub fn client(&self) -> &str {
        match self {
            Self::Sides { client, .. } => client,
            Self::One(value) => value,
        }
    }
}

/// The loader's launcher metadata, served verbatim by Fabric Meta.
///
/// The official keys mix conventions (`min_java_version` is snake_case,
/// `mainClass` is camelCase); renames are explicit rather than a blanket
/// convention for exactly that reason.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LauncherMetaDocument {
    pub version: u32,
    pub min_java_version: u32,
    pub libraries: LauncherLibrariesDocument,
    #[serde(rename = "mainClass")]
    pub main_class: MainClassDocument,
}

/// The loader and intermediary references of a profile document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MavenReferenceDocument {
    pub maven: String,
    pub version: String,
}

/// The profile document for one Minecraft + Loader combination.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderProfileDocument {
    pub loader: MavenReferenceDocument,
    pub intermediary: MavenReferenceDocument,
    pub launcher_meta: LauncherMetaDocument,
}

/// The `launcherMeta` schema generation this launcher understands.
///
/// Fabric Meta currently serves version 2 (verified live for loader 0.19.5).
/// A future generation must be a deliberate support decision, not a silent
/// partial parse.
const SUPPORTED_LAUNCHER_META_VERSION: u32 = 2;

/// The official no-op intermediary placeholder
/// (`net.fabricmc:intermediary:0.0.0`): Fabric Meta's marker for a Minecraft
/// version that ships unobfuscated names and needs no intermediary artifact.
pub const NOOP_INTERMEDIARY_VERSION: &str = "0.0.0";

impl LoaderProfileDocument {
    /// Parses and validates a loader profile document.
    ///
    /// Structural validation happens here (identity fields, the pinned
    /// launcherMeta generation, non-empty coordinates and entry points);
    /// Maven/repository/digest normalization into Aurora's plan happens in
    /// [`crate::fabric::plan`].
    pub fn from_json(json: &str) -> Result<Self, FabricMetadataError> {
        let document: Self =
            serde_json::from_str(json).map_err(|error| FabricMetadataError::Malformed {
                reason: format!("the Fabric loader profile document is unusable: {error}"),
            })?;

        if document.loader.version.trim().is_empty() {
            return Err(FabricMetadataError::Malformed {
                reason: "the profile document has no loader version".to_owned(),
            });
        }
        if document.loader.maven.trim().is_empty() {
            return Err(FabricMetadataError::Malformed {
                reason: format!(
                    "loader '{}' has no Maven coordinate",
                    document.loader.version
                ),
            });
        }
        if document.intermediary.version.trim().is_empty()
            || document.intermediary.maven.trim().is_empty()
        {
            return Err(FabricMetadataError::Malformed {
                reason: "the profile document has an unusable intermediary reference".to_owned(),
            });
        }

        let meta = &document.launcher_meta;
        if meta.version != SUPPORTED_LAUNCHER_META_VERSION {
            return Err(FabricMetadataError::Unsupported {
                reason: format!(
                    "the profile uses launcherMeta generation {}, but this launcher supports generation {SUPPORTED_LAUNCHER_META_VERSION}; support must be added deliberately",
                    meta.version
                ),
            });
        }
        if meta.min_java_version == 0 {
            return Err(FabricMetadataError::Malformed {
                reason: "the profile declares a minimum Java version of zero".to_owned(),
            });
        }
        if meta.main_class.client().trim().is_empty() {
            return Err(FabricMetadataError::Malformed {
                reason: "the profile has no client main class".to_owned(),
            });
        }
        for (group, libraries) in [
            ("client", &meta.libraries.client),
            ("common", &meta.libraries.common),
        ] {
            for library in libraries {
                if library.name.trim().is_empty() {
                    return Err(FabricMetadataError::Malformed {
                        reason: format!("the {group} library group has an unnamed entry"),
                    });
                }
                if library.url.trim().is_empty() {
                    return Err(FabricMetadataError::Malformed {
                        reason: format!("library '{}' has no repository base URL", library.name),
                    });
                }
            }
        }

        Ok(document)
    }
}

/// A failure while fetching or parsing Fabric Meta documents.
#[derive(Debug)]
pub enum FabricMetadataError {
    /// The metadata transport failed or returned an unusable response.
    Network(DownloadError),
    /// A non-success status, surfaced separately so an unsupported
    /// combination (official Fabric Meta answers HTTP 400) can be
    /// distinguished from transport trouble.
    HttpStatus { status: u16 },
    /// A metadata document exceeded the size the launcher will buffer.
    ResponseTooLarge { limit_bytes: usize },
    /// A document could not be parsed or failed validation.
    Malformed { reason: String },
    /// A document uses semantics this launcher deliberately does not support.
    Unsupported { reason: String },
}

impl fmt::Display for FabricMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => {
                write!(
                    formatter,
                    "official Fabric metadata could not be fetched: {error}"
                )
            }
            Self::HttpStatus { status } => {
                write!(formatter, "official Fabric metadata answered HTTP {status}")
            }
            Self::ResponseTooLarge { limit_bytes } => write!(
                formatter,
                "official Fabric metadata exceeded the {limit_bytes}-byte limit the launcher will buffer"
            ),
            Self::Malformed { reason } => {
                write!(formatter, "official Fabric metadata is unusable: {reason}")
            }
            Self::Unsupported { reason } => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for FabricMetadataError {}

/// Fetches the Fabric Loader version list.
pub async fn fetch_loader_versions(
    endpoints: &FabricMetaEndpoints,
    options: &DownloadOptions,
) -> Result<Vec<LoaderVersionEntry>, FabricMetadataError> {
    let bytes = fetch_document(&endpoints.loader_versions_url(), options).await?;
    let text = bytes_to_document_text(&bytes)?;
    LoaderVersionEntry::list_from_json(&text)
}

/// One entry of the per-game loader listing: only the `loader` identity is
/// consumed; the per-entry `intermediary` and `launcherMeta` blocks are the
/// profile document's business and deliberately ignored here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct GameLoaderEntry {
    loader: LoaderVersionEntry,
}

/// Fetches the loader versions available for one exact Minecraft version,
/// newest first, with the official `stable` markers preserved.
///
/// An unsupported game version is Fabric Meta's own HTTP 400; an empty list
/// is returned honestly when the combination has no loaders.
pub async fn fetch_game_loader_versions(
    endpoints: &FabricMetaEndpoints,
    game: &crate::minecraft::metadata::MinecraftVersionId,
    options: &DownloadOptions,
) -> Result<Vec<LoaderVersionEntry>, FabricMetadataError> {
    let url = endpoints.game_loader_versions_url(game);
    let bytes = fetch_document(&url, options).await?;
    let text = bytes_to_document_text(&bytes)?;
    let entries: Vec<GameLoaderEntry> =
        serde_json::from_str(&text).map_err(|error| FabricMetadataError::Malformed {
            reason: format!(
                "the Fabric Loader list for Minecraft '{}' is unusable: {error}",
                game.as_str()
            ),
        })?;

    let loaders: Vec<LoaderVersionEntry> = entries.into_iter().map(|entry| entry.loader).collect();
    for entry in &loaders {
        if entry.version.trim().is_empty() || entry.version.len() > MAX_LOADER_VERSION_LENGTH {
            return Err(FabricMetadataError::Malformed {
                reason: format!(
                    "a listed loader version must be 1 to {MAX_LOADER_VERSION_LENGTH} non-whitespace characters ('{}' is not)",
                    entry.version
                ),
            });
        }
        if entry.maven.trim().is_empty() {
            return Err(FabricMetadataError::Malformed {
                reason: format!(
                    "listed loader version '{}' has no Maven coordinate",
                    entry.version
                ),
            });
        }
    }

    // Fabric Meta serves the list newest first; document order is preserved
    // so the automatic policy's "first stable entry" means newest stable.
    Ok(loaders)
}

/// Fetches and parses the loader profile document for one exact combination.
pub async fn fetch_loader_profile(
    endpoints: &FabricMetaEndpoints,
    game: &crate::minecraft::metadata::MinecraftVersionId,
    loader: &LoaderVersionId,
    options: &DownloadOptions,
) -> Result<LoaderProfileDocument, FabricMetadataError> {
    let url = endpoints.loader_profile_url(game, loader);
    let bytes = fetch_document(&url, options).await?;
    let text = bytes_to_document_text(&bytes)?;
    LoaderProfileDocument::from_json(&text)
}

/// Streams one Fabric Meta document into memory under a hard size cap.
///
/// This is the same narrow metadata fetch boundary the Mojang resolution
/// uses, minus the SHA-1 verification branch: no Fabric Meta document has a
/// prior digest, so none is pretended to. Documents are buffered in memory,
/// never staged, never promoted into the verified store, and never persisted.
async fn fetch_document(
    url: &Url,
    options: &DownloadOptions,
) -> Result<Vec<u8>, FabricMetadataError> {
    let client = downloads::build_client(options);
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| FabricMetadataError::Network(DownloadError::from_transport(error)))?;

    let status = response.status();
    if !status.is_success() {
        return Err(FabricMetadataError::HttpStatus {
            status: status.as_u16(),
        });
    }

    if response
        .content_length()
        .is_some_and(|declared| declared as usize > MAX_METADATA_DOCUMENT_BYTES)
    {
        return Err(FabricMetadataError::ResponseTooLarge {
            limit_bytes: MAX_METADATA_DOCUMENT_BYTES,
        });
    }

    let mut document = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| FabricMetadataError::Network(DownloadError::from_transport(error)))?
    {
        if document.len() + chunk.len() > MAX_METADATA_DOCUMENT_BYTES {
            return Err(FabricMetadataError::ResponseTooLarge {
                limit_bytes: MAX_METADATA_DOCUMENT_BYTES,
            });
        }
        document.extend_from_slice(&chunk);
    }

    Ok(document)
}

/// Decodes fetched bytes as document text.
fn bytes_to_document_text(bytes: &[u8]) -> Result<String, FabricMetadataError> {
    String::from_utf8(bytes.to_vec()).map_err(|_| FabricMetadataError::Malformed {
        reason: "a Fabric Meta document is not valid UTF-8".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::metadata::MinecraftVersionId;
    use crate::test_support::{TestResponse, TestServer};
    use std::sync::Arc;
    use std::time::Duration;

    fn quick_options() -> DownloadOptions {
        DownloadOptions {
            connect_timeout: Duration::from_secs(5),
            idle_read_timeout: Duration::from_secs(5),
            max_redirects: downloads::MAX_REDIRECTS,
        }
    }

    /// A loader version list representative of live data (values abridged).
    fn loader_list_fixture() -> String {
        r#"[
            { "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true },
            { "separator": ".", "build": 4, "maven": "net.fabricmc:fabric-loader:0.19.4", "version": "0.19.4", "stable": false },
            { "separator": ".", "build": 155, "maven": "net.fabricmc:fabric-loader:0.4.8+build.155", "version": "0.4.8+build.155", "stable": false }
        ]"#
        .to_owned()
    }

    /// A profile document representative of the live 1.21.11 + 0.19.5
    /// response: a real intermediary, digested common libraries, and a
    /// two-sided main class. Abridged to the entries tests reason about.
    fn profile_fixture(intermediary_version: &str) -> String {
        let intermediary_maven = if intermediary_version == NOOP_INTERMEDIARY_VERSION {
            "net.fabricmc:intermediary:0.0.0".to_owned()
        } else {
            format!("net.fabricmc:intermediary:{intermediary_version}")
        };
        format!(
            r#"{{
                "loader": {{ "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true }},
                "intermediary": {{ "maven": "{intermediary_maven}", "version": "{intermediary_version}", "stable": true }},
                "launcherMeta": {{
                    "version": 2,
                    "min_java_version": 8,
                    "libraries": {{
                        "client": [],
                        "common": [
                            {{
                                "name": "org.ow2.asm:asm:9.10.1",
                                "url": "https://maven.fabricmc.net/",
                                "md5": "9cb72080438acf27f2607002e1444a7b",
                                "sha1": "ada2141c0cc52ee8f5c48cd5fa4ce0e794f22236",
                                "sha256": "ed825d10ab1399c8c0cb669e688cf0c8c82629b4c8399b58352b68e92ca10fcb",
                                "sha512": "8db6efa37d4d569bf2b95d90931b8064f87ed928d20bbf8d8e7fa6182e9a11d32e6a2cc034a3d173a38ee70ce2ea9f20f52f9d8730d9401d2bc6060674c18928",
                                "size": 126151
                            }},
                            {{
                                "name": "net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7",
                                "url": "https://maven.fabricmc.net/",
                                "md5": "9e33b145b2a46fbc70daf532e3705066",
                                "sha1": "5f66cc9f59b8efaa942155a3d5a30599bf6640dd",
                                "sha256": "1f0ae44db7295f8626f33b1dc0ad7f043d8954a8d6847247875fcc5dfcecc934",
                                "sha512": "837b479c7e831463e074b56314259952f2f82c2ec8ddd71930514ca04f97bb488f43fdb2d460e8f7f3fad334604a546053256844ea653542bae8d25bbdab66e21",
                                "size": 1539080
                            }}
                        ],
                        "server": [],
                        "development": [
                            {{
                                "name": "io.github.llamalad7:mixinextras-fabric:0.5.5",
                                "url": "https://maven.fabricmc.net/",
                                "sha256": "5da883dc4bfb16e4ceca3f16d8c4ad937bdfb1ce337ed5ba15472d4d81dd6242",
                                "size": 727864
                            }}
                        ]
                    }},
                    "mainClass": {{
                        "client": "net.fabricmc.loader.impl.launch.knot.KnotClient",
                        "server": "net.fabricmc.loader.impl.launch.knot.KnotServer"
                    }}
                }}
            }}"#
        )
    }

    #[test]
    fn loader_versions_parse_and_find_exact_versions() {
        let entries = LoaderVersionEntry::list_from_json(&loader_list_fixture()).unwrap();
        assert_eq!(entries.len(), 3);

        let found =
            LoaderVersionEntry::find(&entries, &LoaderVersionId::new("0.19.5").unwrap()).unwrap();
        assert_eq!(found.maven, "net.fabricmc:fabric-loader:0.19.5");
        assert!(found.stable);

        // Build-suffixed historical spellings are exact-lookup keys too.
        assert!(
            LoaderVersionEntry::find(&entries, &LoaderVersionId::new("0.4.8+build.155").unwrap())
                .is_some()
        );
        assert!(
            LoaderVersionEntry::find(&entries, &LoaderVersionId::new("0.19.6").unwrap()).is_none(),
            "unknown loader versions are not found, never substituted"
        );
    }

    #[test]
    fn malformed_loader_lists_are_rejected_deliberately() {
        for (json, why) in [
            ("not json", "non-JSON input"),
            ("[]", "an empty list is unusable"),
            (
                r#"[{ "maven": "net.fabricmc:fabric-loader:x", "version": "" }]"#,
                "versions must be present",
            ),
        ] {
            assert!(
                matches!(
                    LoaderVersionEntry::list_from_json(json),
                    Err(FabricMetadataError::Malformed { .. })
                ),
                "{why}"
            );
        }
    }

    #[test]
    fn loader_versions_validate_shape_at_the_boundary() {
        for valid in ["0.19.5", "0.4.8+build.155", "1.0.0"] {
            assert!(LoaderVersionId::new(valid).is_ok(), "{valid} is valid");
        }
        for invalid in ["", "0.19.5 ", "../evil", "0.19.5/beta"] {
            assert!(
                LoaderVersionId::new(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_profile_document_parses_with_its_relevant_shapes() {
        let document = LoaderProfileDocument::from_json(&profile_fixture("1.21.11")).unwrap();

        assert_eq!(document.loader.version, "0.19.5");
        assert_eq!(document.intermediary.version, "1.21.11");
        assert_eq!(document.launcher_meta.version, 2);
        assert_eq!(document.launcher_meta.min_java_version, 8);
        assert_eq!(
            document.launcher_meta.main_class.client(),
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_eq!(document.launcher_meta.libraries.common.len(), 2);
        assert_eq!(
            document.launcher_meta.libraries.common[0].sha256.as_deref(),
            Some("ed825d10ab1399c8c0cb669e688cf0c8c82629b4c8399b58352b68e92ca10fcb")
        );
        // The development group is ignored entirely.
        assert_eq!(document.launcher_meta.libraries.client.len(), 0);

        // The no-op intermediary placeholder parses identically.
        let noop =
            LoaderProfileDocument::from_json(&profile_fixture(NOOP_INTERMEDIARY_VERSION)).unwrap();
        assert_eq!(noop.intermediary.version, NOOP_INTERMEDIARY_VERSION);
    }

    #[test]
    fn malformed_and_unsupported_profiles_are_rejected_deliberately() {
        let base = profile_fixture("26.2-placeholder");

        let future_meta = base.replace("\"version\": 2,", "\"version\": 3,");
        assert!(matches!(
            LoaderProfileDocument::from_json(&future_meta),
            Err(FabricMetadataError::Unsupported { .. })
        ));

        let no_main_class = base.replace(
            "\"client\": \"net.fabricmc.loader.impl.launch.knot.KnotClient\",",
            "\"client\": \"\",",
        );
        assert!(matches!(
            LoaderProfileDocument::from_json(&no_main_class),
            Err(FabricMetadataError::Malformed { .. })
        ));

        let unnamed = base.replace("\"name\": \"org.ow2.asm:asm:9.10.1\"", "\"name\": \"\"");
        assert!(matches!(
            LoaderProfileDocument::from_json(&unnamed),
            Err(FabricMetadataError::Malformed { .. })
        ));

        assert!(matches!(
            LoaderProfileDocument::from_json("not json"),
            Err(FabricMetadataError::Malformed { .. })
        ));
    }

    #[tokio::test]
    async fn fetching_resolves_the_loader_list_and_one_exact_profile() {
        let profile_body = profile_fixture("1.21.11");
        let server = TestServer::spawn(Arc::new(move |request| match request.path.as_str() {
            "/v2/versions/loader" => TestResponse::ok(loader_list_fixture().as_bytes()),
            path if path.starts_with("/v2/versions/loader/1.21.11/0.19.5") => {
                TestResponse::ok(profile_body.as_bytes())
            }
            _ => TestResponse::status(400),
        }));

        let endpoints =
            FabricMetaEndpoints::loopback_for_testing(&format!("{}/v2/", server.base_url()));
        let options = quick_options();

        let versions = fetch_loader_versions(&endpoints, &options).await.unwrap();
        assert!(
            LoaderVersionEntry::find(&versions, &LoaderVersionId::new("0.19.5").unwrap()).is_some()
        );

        let document = fetch_loader_profile(
            &endpoints,
            &MinecraftVersionId::new("1.21.11").unwrap(),
            &LoaderVersionId::new("0.19.5").unwrap(),
            &options,
        )
        .await
        .unwrap();
        assert_eq!(document.loader.version, "0.19.5");
    }

    #[tokio::test]
    async fn unsupported_combinations_surface_the_http_status() {
        let server = TestServer::spawn(Arc::new(|_request| TestResponse::status(400)));
        let endpoints =
            FabricMetaEndpoints::loopback_for_testing(&format!("{}/v2/", server.base_url()));

        let error = fetch_loader_profile(
            &endpoints,
            &MinecraftVersionId::new("9.9.9").unwrap(),
            &LoaderVersionId::new("0.19.5").unwrap(),
            &quick_options(),
        )
        .await
        .expect_err("an unsupported combination must fail");

        assert!(
            matches!(error, FabricMetadataError::HttpStatus { status: 400 }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn endpoint_urls_pin_the_official_paths() {
        let endpoints = FabricMetaEndpoints::official();
        assert_eq!(
            endpoints.loader_versions_url().as_str(),
            "https://meta.fabricmc.net/v2/versions/loader"
        );
        assert_eq!(
            endpoints
                .loader_profile_url(
                    &MinecraftVersionId::new("1.21.11").unwrap(),
                    &LoaderVersionId::new("0.19.5").unwrap()
                )
                .as_str(),
            "https://meta.fabricmc.net/v2/versions/loader/1.21.11/0.19.5"
        );

        let loopback = FabricMetaEndpoints::loopback_for_testing("http://127.0.0.1:9/v2/");
        assert_eq!(
            loopback.loader_versions_url().as_str(),
            "http://127.0.0.1:9/v2/versions/loader"
        );
    }
}
