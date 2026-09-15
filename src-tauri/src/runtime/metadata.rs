//! Official Mojang Java-runtime metadata and its fetch boundary.
//!
//! The pinned `all.json` index is bootstrap discovery metadata: HTTPS plus
//! strict parsing, not a cryptographic verification claim. Its selected
//! per-platform manifest carries an official SHA-1 and exact size, so that
//! document is acquired through Aurora's existing SHA-1-addressed verified
//! store before it is parsed. Runtime payload files are described by that
//! verified manifest and are acquired later by the installer.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use url::Url;

use crate::cache::{AcquisitionError, ArtifactCache};
use crate::downloads::{self, DownloadError, DownloadOptions, Sha1ArtifactSource};
use crate::integrity::Sha1Digest;
use crate::paths::ManagedPaths;
use crate::runtime::plan::{
    JavaRuntimePlan, RuntimePlanError, RuntimePlatform, parse_java_major, validate_component,
};

/// The runtime index used by the current official Minecraft Launcher. Mojang
/// publishes this as a content-named product feed; the launcher pins the
/// official URL just as it pins the Minecraft version-manifest root.
pub const OFFICIAL_RUNTIME_INDEX_URL: &str = "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

const MAX_INDEX_BYTES: usize = 1024 * 1024;
const MAX_FILE_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMetadataEndpoints {
    index_url: Url,
}

impl RuntimeMetadataEndpoints {
    pub fn official() -> Self {
        Self {
            index_url: Url::parse(OFFICIAL_RUNTIME_INDEX_URL)
                .expect("official runtime index URL is valid HTTPS"),
        }
    }

    pub fn loopback_for_testing(base_url: &str) -> Self {
        let mut index_url = Url::parse(base_url).expect("test base URL must parse");
        index_url.set_path("/java-runtime/all.json");
        assert!(
            downloads::is_loopback_host(&index_url),
            "test runtime metadata must stay on loopback"
        );
        Self { index_url }
    }

    pub fn index_url(&self) -> &Url {
        &self.index_url
    }
}

#[derive(Debug, Deserialize)]
struct RuntimeIndexDocument {
    #[serde(flatten)]
    platforms: BTreeMap<String, BTreeMap<String, Vec<RuntimeIndexEntry>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeIndexEntry {
    manifest: ManifestDownload,
    version: RuntimeVersion,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestDownload {
    sha1: String,
    size: u64,
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeVersion {
    name: String,
    released: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeSelection {
    pub(crate) version_name: String,
    pub(crate) released: String,
    pub(crate) manifest_sha1: Sha1Digest,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RuntimeFileDocument {
    pub(crate) files: BTreeMap<String, RuntimeFileKind>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum RuntimeFileKind {
    Directory,
    File {
        executable: bool,
        downloads: RuntimeDownloads,
    },
    Link {
        target: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RuntimeDownloads {
    pub(crate) raw: RuntimeDownload,
    #[allow(dead_code)]
    pub(crate) lzma: Option<RuntimeDownload>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RuntimeDownload {
    pub(crate) sha1: String,
    pub(crate) size: u64,
    pub(crate) url: String,
}

/// Resolves an exact official component for an explicit platform and Java
/// major, returning only Aurora's normalized runtime plan.
pub async fn resolve_runtime_plan(
    managed: &ManagedPaths,
    endpoints: &RuntimeMetadataEndpoints,
    component: &str,
    required_major_version: u32,
    platform: RuntimePlatform,
    options: &DownloadOptions,
) -> Result<JavaRuntimePlan, RuntimeMetadataError> {
    validate_component(component).map_err(RuntimeMetadataError::Plan)?;
    let platform_key = platform.mojang_key().map_err(RuntimeMetadataError::Plan)?;
    let index_bytes = fetch_index(endpoints.index_url(), options).await?;
    let index: RuntimeIndexDocument = serde_json::from_slice(&index_bytes)
        .map_err(|error| RuntimeMetadataError::IndexInvalid(error.to_string()))?;

    let platform_components = index.platforms.get(platform_key).ok_or_else(|| {
        RuntimeMetadataError::PlatformUnavailable {
            platform: platform_key.to_owned(),
        }
    })?;
    let candidates = platform_components.get(component).ok_or_else(|| {
        RuntimeMetadataError::ComponentUnavailable {
            platform: platform_key.to_owned(),
            component: component.to_owned(),
        }
    })?;
    let entry = match candidates.as_slice() {
        [] => {
            return Err(RuntimeMetadataError::ComponentUnavailable {
                platform: platform_key.to_owned(),
                component: component.to_owned(),
            });
        }
        [entry] => entry,
        entries => {
            return Err(RuntimeMetadataError::AmbiguousComponent {
                platform: platform_key.to_owned(),
                component: component.to_owned(),
                candidates: entries.len(),
            });
        }
    };
    if entry.version.name.trim().is_empty() || entry.version.released.trim().is_empty() {
        return Err(RuntimeMetadataError::IndexInvalid(format!(
            "runtime component '{component}' has incomplete version identity"
        )));
    }
    if parse_java_major(&entry.version.name) != Some(required_major_version) {
        return Err(RuntimeMetadataError::Plan(
            RuntimePlanError::MajorVersionMismatch {
                required: required_major_version,
                published: parse_java_major(&entry.version.name).unwrap_or(0),
            },
        ));
    }
    if entry.manifest.size == 0 || entry.manifest.size > MAX_FILE_MANIFEST_BYTES {
        return Err(RuntimeMetadataError::IndexInvalid(format!(
            "runtime manifest size {} is outside the supported 1..={MAX_FILE_MANIFEST_BYTES} byte range",
            entry.manifest.size
        )));
    }
    let manifest_sha1 = Sha1Digest::parse(&entry.manifest.sha1).map_err(|error| {
        RuntimeMetadataError::IndexInvalid(format!("runtime manifest SHA-1 is invalid: {error}"))
    })?;
    let source = Sha1ArtifactSource::https_or_loopback(
        &entry.manifest.url,
        &manifest_sha1.as_hex(),
        Some(entry.manifest.size),
    )
    .map_err(|error| {
        RuntimeMetadataError::IndexInvalid(format!("runtime manifest source is unusable: {error}"))
    })?;
    let cache = ArtifactCache::new(managed.clone());
    let verified = cache
        .acquire_sha1(&source, options)
        .await
        .map_err(RuntimeMetadataError::ManifestAcquisition)?;
    let bytes = std::fs::read(&verified.path).map_err(RuntimeMetadataError::ManifestRead)?;
    if bytes.len() as u64 > MAX_FILE_MANIFEST_BYTES {
        return Err(RuntimeMetadataError::IndexInvalid(
            "the verified runtime manifest exceeds the parser bound".to_owned(),
        ));
    }
    let document: RuntimeFileDocument = serde_json::from_slice(&bytes)
        .map_err(|error| RuntimeMetadataError::ManifestInvalid(error.to_string()))?;
    let selection = RuntimeSelection {
        version_name: entry.version.name.clone(),
        released: entry.version.released.clone(),
        manifest_sha1,
    };
    JavaRuntimePlan::from_metadata(
        component,
        required_major_version,
        platform,
        selection,
        document,
    )
    .map_err(RuntimeMetadataError::Plan)
}

async fn fetch_index(
    url: &Url,
    options: &DownloadOptions,
) -> Result<Vec<u8>, RuntimeMetadataError> {
    let client = downloads::build_client(options);
    let response = client.get(url.clone()).send().await.map_err(|error| {
        RuntimeMetadataError::IndexNetwork(DownloadError::from_transport(error))
    })?;
    if !response.status().is_success() {
        return Err(RuntimeMetadataError::IndexNetwork(
            DownloadError::HttpStatus {
                status: response.status().as_u16(),
            },
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_INDEX_BYTES as u64)
    {
        return Err(RuntimeMetadataError::IndexTooLarge {
            limit_bytes: MAX_INDEX_BYTES,
        });
    }
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| RuntimeMetadataError::IndexNetwork(DownloadError::from_transport(error)))?
    {
        if body.len() + chunk.len() > MAX_INDEX_BYTES {
            return Err(RuntimeMetadataError::IndexTooLarge {
                limit_bytes: MAX_INDEX_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Debug)]
pub enum RuntimeMetadataError {
    IndexNetwork(DownloadError),
    IndexTooLarge {
        limit_bytes: usize,
    },
    IndexInvalid(String),
    PlatformUnavailable {
        platform: String,
    },
    ComponentUnavailable {
        platform: String,
        component: String,
    },
    AmbiguousComponent {
        platform: String,
        component: String,
        candidates: usize,
    },
    ManifestAcquisition(AcquisitionError),
    ManifestRead(std::io::Error),
    ManifestInvalid(String),
    Plan(RuntimePlanError),
}

impl fmt::Display for RuntimeMetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndexNetwork(error) => write!(
                f,
                "official Mojang runtime metadata could not be fetched: {error}"
            ),
            Self::IndexTooLarge { limit_bytes } => write!(
                f,
                "official Mojang runtime metadata exceeded the {limit_bytes}-byte safety limit"
            ),
            Self::IndexInvalid(reason) => {
                write!(f, "official Mojang runtime index is invalid: {reason}")
            }
            Self::PlatformUnavailable { platform } => write!(
                f,
                "official Mojang runtime metadata does not contain platform '{platform}'"
            ),
            Self::ComponentUnavailable {
                platform,
                component,
            } => write!(
                f,
                "official Mojang runtime component '{component}' is unavailable for '{platform}'"
            ),
            Self::AmbiguousComponent {
                platform,
                component,
                candidates,
            } => write!(
                f,
                "official Mojang runtime component '{component}' has {candidates} candidates for '{platform}'; exact selection is ambiguous"
            ),
            Self::ManifestAcquisition(error) => write!(
                f,
                "the official Mojang runtime file manifest failed SHA-1 acquisition: {error}"
            ),
            Self::ManifestRead(error) => write!(
                f,
                "the verified Mojang runtime file manifest could not be read: {error}"
            ),
            Self::ManifestInvalid(reason) => write!(
                f,
                "the verified Mojang runtime file manifest is invalid: {reason}"
            ),
            Self::Plan(error) => write!(
                f,
                "the Mojang runtime metadata cannot form a safe plan: {error}"
            ),
        }
    }
}

impl std::error::Error for RuntimeMetadataError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::plan::{RuntimeArchitecture, RuntimeOperatingSystem};
    use crate::test_support::{TestResponse, TestServer};
    use std::sync::Arc;
    use std::time::Duration;

    fn options() -> DownloadOptions {
        DownloadOptions {
            connect_timeout: Duration::from_secs(5),
            idle_read_timeout: Duration::from_secs(5),
            max_redirects: downloads::MAX_REDIRECTS,
        }
    }
    fn platform() -> RuntimePlatform {
        RuntimePlatform::new(RuntimeOperatingSystem::Windows, RuntimeArchitecture::X86_64)
    }

    #[tokio::test]
    async fn resolves_and_sha1_verifies_a_minimal_runtime_manifest() {
        let manifest = br#"{"files":{"bin":{"type":"directory"},"bin/java.exe":{"type":"file","executable":true,"downloads":{"raw":{"sha1":"be76331b95dfc399cd776d2fc68021e0db03cc4f","size":5,"url":"http://127.0.0.1:9/java.exe"}}},"bin/javaw.exe":{"type":"file","executable":true,"downloads":{"raw":{"sha1":"be76331b95dfc399cd776d2fc68021e0db03cc4f","size":5,"url":"http://127.0.0.1:9/javaw.exe"}}}}}"#;
        let sha1 = Sha1Digest::compute(manifest).as_hex();
        let server = TestServer::spawn(Arc::new(move |request| {
            match request.path.as_str() {
            "/java-runtime/all.json" => TestResponse::ok(format!(r#"{{"windows-x64":{{"java-runtime-epsilon":[{{"manifest":{{"sha1":"{sha1}","size":{},"url":"{}/manifest.json"}},"version":{{"name":"25.0.1","released":"2025-10-12"}}}}]}}}}"#, manifest.len(), request.base_url).as_bytes()),
            "/manifest.json" => TestResponse::ok(manifest),
            _ => TestResponse::status(404),
        }
        }));
        let managed = ManagedPaths::from_app_local_data_dir(
            std::env::temp_dir().join(format!("aurora-runtime-meta-{}", std::process::id())),
        )
        .unwrap();
        let plan = resolve_runtime_plan(
            &managed,
            &RuntimeMetadataEndpoints::loopback_for_testing(server.base_url()),
            "java-runtime-epsilon",
            25,
            platform(),
            &options(),
        )
        .await
        .unwrap();
        assert_eq!(plan.runtime_version(), "25.0.1");
        assert_eq!(plan.file_count(), 2);
        assert!(plan.identity().starts_with("windows-x64-"));
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[tokio::test]
    async fn missing_empty_and_ambiguous_components_fail_exactly() {
        for (body, expected) in [
            (r#"{"windows-x64":{}}"#, "unavailable"),
            (
                r#"{"windows-x64":{"java-runtime-epsilon":[]}}"#,
                "unavailable",
            ),
            (
                r#"{"windows-x64":{"java-runtime-epsilon":[{"manifest":{"sha1":"0000000000000000000000000000000000000000","size":1,"url":"https://example.invalid/a"},"version":{"name":"25","released":"x"}},{"manifest":{"sha1":"0000000000000000000000000000000000000000","size":1,"url":"https://example.invalid/b"},"version":{"name":"25","released":"x"}}]}}"#,
                "ambiguous",
            ),
        ] {
            let owned = body.as_bytes().to_vec();
            let server = TestServer::spawn(Arc::new(move |_request| TestResponse::ok(&owned)));
            let managed =
                ManagedPaths::from_app_local_data_dir(std::env::temp_dir().join(format!(
                    "aurora-runtime-error-{}-{}",
                    std::process::id(),
                    server.base_url().rsplit(':').next().unwrap()
                )))
                .unwrap();
            let error = resolve_runtime_plan(
                &managed,
                &RuntimeMetadataEndpoints::loopback_for_testing(server.base_url()),
                "java-runtime-epsilon",
                25,
                platform(),
                &options(),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[tokio::test]
    async fn tampered_runtime_manifest_fails_before_parsing() {
        let server = TestServer::spawn(Arc::new(move |request| {
            match request.path.as_str() {
            "/java-runtime/all.json" => TestResponse::ok(format!(r#"{{"windows-x64":{{"java-runtime-epsilon":[{{"manifest":{{"sha1":"0000000000000000000000000000000000000000","size":8,"url":"{}/manifest.json"}},"version":{{"name":"25.0.1","released":"x"}}}}]}}}}"#, request.base_url).as_bytes()),
            "/manifest.json" => TestResponse::ok(b"tampered"),
            _ => TestResponse::status(404),
        }
        }));
        let managed = ManagedPaths::from_app_local_data_dir(
            std::env::temp_dir().join(format!("aurora-runtime-tamper-{}", std::process::id())),
        )
        .unwrap();
        let error = resolve_runtime_plan(
            &managed,
            &RuntimeMetadataEndpoints::loopback_for_testing(server.base_url()),
            "java-runtime-epsilon",
            25,
            platform(),
            &options(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, RuntimeMetadataError::ManifestAcquisition(_)),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(managed.data_root());
    }

    #[test]
    fn unexpected_manifest_entry_types_are_rejected() {
        let error = serde_json::from_str::<RuntimeFileDocument>(
            r#"{"files":{"bin/java":{"type":"archive","url":"https://example.invalid/java.zip"}}}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown variant"));
    }
}
