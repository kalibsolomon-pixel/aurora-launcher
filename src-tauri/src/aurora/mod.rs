//! Aurora client-artifact installation: acquisition, managed
//! materialization, and the Aurora installed-state record.
//!
//! Aurora's client mod and any release-pinned required Fabric API mod have
//! pre-known SHA-256 digests in release metadata. Both use the Phase 2
//! verified store, never the transport-observed path for digest-less Fabric
//! loader artifacts.
//!
//! ## Managed ownership
//!
//! Materialized artifacts live at deterministic launcher-managed paths
//! inside the instance's mods directory:
//!
//! ```text
//! instances/<id>/mods/aurora-<validated aurora version>.jar
//! instances/<id>/mods/fabric-api-<validated API version>.jar  # when required
//! ```
//!
//! The filename derives from validated release metadata (never a remote
//! filename), so later update/repair logic knows exactly which file the
//! launcher owns. Every other file under `mods/` — and everything under the
//! instance's user-data areas — is user-owned: installation never
//! enumerates, moves, or removes it, and validation never treats unrelated
//! user mods as damage.
//!
//! ## Installed state
//!
//! `aurora-installed.json` (schema 1) sits at the instance root beside
//! `game/`, is written atomically, and is committed only after the
//! materialized artifact re-verifies. It deliberately does not duplicate
//! the game installed-state manifest; it records Aurora's own facts and is
//! compared against the registry's release pin and the game manifest by
//! complete-instance validation.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cache::ArtifactCache;
use crate::distribution::{AuroraRelease, ReleaseChannel};
use crate::downloads::{ArtifactSource, DownloadOptions};
use crate::instances::InstanceId;
use crate::integrity::{ArtifactDigest, ArtifactTrust, verify_file};
use crate::paths::ManagedPaths;

/// The only Aurora installed-state schema version this launcher understands.
pub const AURORA_INSTALLED_SCHEMA_VERSION: u32 = 1;

/// The installed-state file name at the instance root.
pub const AURORA_INSTALLED_FILE_NAME: &str = "aurora-installed.json";

/// The launcher-managed prefix for Aurora artifacts under the mods
/// directory, distinguishing launcher-owned files from user mods.
const MANAGED_FILE_PREFIX: &str = "aurora-";

/// The Aurora installed-state record of one instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraInstalledState {
    schema_version: u32,
    aurora_version: String,
    channel: ReleaseChannel,
    minecraft_version: String,
    fabric_loader_version: String,
    artifact: AuroraInstalledArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fabric_api: Option<InstalledFabricApi>,
    installation_id: String,
    installed_at_unix_seconds: u64,
}

/// A verified Fabric API mod installed because the Aurora release requires it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledFabricApi {
    version: String,
    artifact: AuroraInstalledArtifact,
}

impl InstalledFabricApi {
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn artifact(&self) -> &AuroraInstalledArtifact {
        &self.artifact
    }
}

/// The materialized launcher-managed artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraInstalledArtifact {
    /// Path relative to the instance root, forward slashes, validated safe
    /// and required to live under `mods/` with the managed prefix.
    relative_path: String,
    size_bytes: u64,
    sha256: String,
}

impl AuroraInstalledState {
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

    pub fn artifact(&self) -> &AuroraInstalledArtifact {
        &self.artifact
    }

    pub fn fabric_api(&self) -> Option<&InstalledFabricApi> {
        self.fabric_api.as_ref()
    }

    pub fn installation_id(&self) -> &str {
        &self.installation_id
    }

    /// Parses and validates an installed-state document. Unknown schema
    /// versions and malformed content fail deliberately; the caller never
    /// repairs or overwrites a damaged record.
    pub fn from_json(json: &str) -> Result<Self, AuroraStateError> {
        let state: Self =
            serde_json::from_str(json).map_err(|error| AuroraStateError::Malformed {
                reason: error.to_string(),
            })?;

        if state.schema_version != AURORA_INSTALLED_SCHEMA_VERSION {
            return Err(AuroraStateError::UnsupportedSchema {
                found: state.schema_version,
                supported: AURORA_INSTALLED_SCHEMA_VERSION,
            });
        }

        for (field, value) in [
            ("Aurora", &state.aurora_version),
            ("Minecraft", &state.minecraft_version),
            ("Fabric Loader", &state.fabric_loader_version),
        ] {
            if value.trim().is_empty() {
                return Err(AuroraStateError::Malformed {
                    reason: format!("the installed state must record a {field} version"),
                });
            }
        }
        if state.installation_id.trim().is_empty() {
            return Err(AuroraStateError::Malformed {
                reason: "the installed state must record an installation id".to_owned(),
            });
        }
        validate_managed_relative_path(&state.artifact.relative_path)
            .map_err(|reason| AuroraStateError::Malformed { reason })?;
        ArtifactDigest::parse(&state.artifact.sha256).map_err(|error| {
            AuroraStateError::Malformed {
                reason: format!("the artifact SHA-256 is invalid: {error}"),
            }
        })?;
        if state.artifact.size_bytes == 0 {
            return Err(AuroraStateError::Malformed {
                reason: "the artifact size must be greater than zero".to_owned(),
            });
        }
        if let Some(fabric_api) = &state.fabric_api {
            let expected_path = format!("mods/fabric-api-{}.jar", fabric_api.version);
            if fabric_api.version.is_empty() || fabric_api.artifact.relative_path != expected_path {
                return Err(AuroraStateError::Malformed {
                    reason: "the Fabric API managed path or version is invalid".to_owned(),
                });
            }
            validate_fabric_api_relative_path(&fabric_api.artifact.relative_path)
                .map_err(|reason| AuroraStateError::Malformed { reason })?;
            ArtifactDigest::parse(&fabric_api.artifact.sha256).map_err(|error| {
                AuroraStateError::Malformed {
                    reason: format!("the Fabric API SHA-256 is invalid: {error}"),
                }
            })?;
            if fabric_api.artifact.size_bytes == 0 {
                return Err(AuroraStateError::Malformed {
                    reason: "the Fabric API size must be greater than zero".to_owned(),
                });
            }
        }

        Ok(state)
    }

    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self)
            .expect("Aurora installed-state serialization cannot fail");
        json.push('\n');
        json
    }

    /// The honest trust record: Aurora artifacts are verified against a
    /// pre-known expected SHA-256 from release metadata.
    pub fn trust(&self) -> ArtifactTrust {
        ArtifactTrust::ExpectedDigestVerified {
            algorithm: crate::integrity::DigestAlgorithm::Sha256,
            digest: self.artifact.sha256.clone(),
        }
    }
}

impl AuroraInstalledArtifact {
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// The managed artifact path for one release:
/// `mods/aurora-<validated version>.jar`.
///
/// The Aurora version must be a safe single filename segment (it becomes
/// part of a launcher-managed path), and the result is checked again on
/// load — belt and braces on both sides of persistence.
pub fn managed_artifact_relative_path(
    release: &AuroraRelease,
) -> Result<String, AuroraInstallError> {
    let version = release.aurora_version();
    if version.is_empty()
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        || version.starts_with('.')
    {
        return Err(AuroraInstallError::ReleaseInvalid(format!(
            "Aurora version '{version}' cannot form a safe managed file name"
        )));
    }
    let path = format!("mods/{MANAGED_FILE_PREFIX}{version}.jar");
    validate_managed_relative_path(&path)
        .map_err(|reason| AuroraInstallError::ReleaseInvalid(reason))?;
    Ok(path)
}

/// A managed path is relative, forward-slashed, under `mods/`, and carries
/// the launcher-owned `aurora-` prefix — exactly the shape installation
/// writes and the only shape validation accepts as launcher-owned.
fn validate_managed_relative_path(path: &str) -> Result<(), String> {
    validate_managed_mod_path(path, MANAGED_FILE_PREFIX)
}

fn validate_fabric_api_relative_path(path: &str) -> Result<(), String> {
    validate_managed_mod_path(path, "fabric-api-")
}

fn validate_managed_mod_path(path: &str, prefix: &str) -> Result<(), String> {
    let rest = path
        .strip_prefix("mods/")
        .ok_or("the managed artifact path must live under mods/")?;
    let file = rest
        .strip_prefix(prefix)
        .ok_or("the managed artifact path must use its launcher-owned prefix")?;
    if !file.ends_with(".jar") || file.len() < 5 {
        return Err("the managed artifact path must be a .jar file".to_owned());
    }
    if !path.split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
    }) {
        return Err(
            "the managed artifact path must be traversal-free with safe segments".to_owned(),
        );
    }
    Ok(())
}

/// The outcome of one Aurora installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAurora {
    pub state: AuroraInstalledState,
    pub artifact_path: PathBuf,
}

/// Acquires the release's artifact through the SHA-256 verified store and
/// materializes it into the instance's managed mods path.
///
/// Order of operations is deliberate: acquire and verify in the cache,
/// copy into the instance with an immediate size check, re-hash the
/// materialized copy against the expected digest, and only then commit the
/// installed-state record. A failure at any point leaves no record, so a
/// partially installed Aurora artifact is never mistaken for installed.
pub async fn install_aurora(
    managed: &ManagedPaths,
    instance: &InstanceId,
    release: &AuroraRelease,
    options: &DownloadOptions,
) -> Result<InstalledAurora, AuroraInstallError> {
    let expected = ArtifactDigest::parse(release.artifact().sha256())
        .map_err(|error| AuroraInstallError::ReleaseInvalid(error.to_string()))?;
    let source = ArtifactSource::https_or_loopback(
        release.artifact().url(),
        &release.artifact().sha256(),
        release.artifact().size_bytes(),
    )
    .map_err(|error| AuroraInstallError::ArtifactInvalid(error.to_string()))?;

    let cache = ArtifactCache::new(managed.clone());
    let verified = cache
        .acquire_with(&source, options)
        .await
        .map_err(AuroraInstallError::Acquisition)?;

    // Acquire every required mod through the verified store before touching
    // the instance. A failed dependency cannot leave a completed state.
    let fabric_api = if let Some(dependency) = release.fabric_api() {
        let digest = ArtifactDigest::parse(dependency.artifact().sha256())
            .map_err(|error| AuroraInstallError::ReleaseInvalid(error.to_string()))?;
        let source = ArtifactSource::https_or_loopback(
            dependency.artifact().url(),
            dependency.artifact().sha256(),
            dependency.artifact().size_bytes(),
        )
        .map_err(|error| AuroraInstallError::ArtifactInvalid(error.to_string()))?;
        let verified = cache
            .acquire_with(&source, options)
            .await
            .map_err(AuroraInstallError::Acquisition)?;
        Some((dependency, digest, verified))
    } else {
        None
    };

    let relative = managed_artifact_relative_path(release)?;
    let instance_paths = managed.instance_paths(instance);
    let artifact_path = instance_paths
        .root()
        .join(relative.split('/').collect::<PathBuf>());
    if !artifact_path.starts_with(instance_paths.mods()) {
        return Err(AuroraInstallError::Materialization {
            path: relative.clone(),
            reason: "the derived path escaped the managed mods directory".to_owned(),
        });
    }

    if let Some(parent) = artifact_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| AuroraInstallError::Materialization {
            path: relative.clone(),
            reason: error.to_string(),
        })?;
    }
    std::fs::copy(&verified.path, &artifact_path).map_err(|error| {
        AuroraInstallError::Materialization {
            path: relative.clone(),
            reason: error.to_string(),
        }
    })?;

    // Re-verify the materialized copy — a successful copy call is not
    // itself trusted content.
    let bytes = verify_file(&artifact_path, &expected, Some(verified.bytes)).map_err(|error| {
        AuroraInstallError::Materialization {
            path: relative.clone(),
            reason: format!("the materialized artifact failed verification: {error}"),
        }
    })?;

    let installed_fabric_api = if let Some((dependency, digest, verified)) = fabric_api {
        let relative_path = format!("mods/fabric-api-{}.jar", dependency.version());
        validate_fabric_api_relative_path(&relative_path)
            .map_err(AuroraInstallError::ReleaseInvalid)?;
        let path = instance_paths
            .root()
            .join(relative_path.split('/').collect::<PathBuf>());
        if !path.starts_with(instance_paths.mods()) {
            return Err(AuroraInstallError::Materialization {
                path: relative_path,
                reason: "the derived path escaped the managed mods directory".to_owned(),
            });
        }
        std::fs::copy(&verified.path, &path).map_err(|error| {
            AuroraInstallError::Materialization {
                path: relative_path.clone(),
                reason: error.to_string(),
            }
        })?;
        let size_bytes = verify_file(&path, &digest, Some(verified.bytes)).map_err(|error| {
            AuroraInstallError::Materialization {
                path: relative_path.clone(),
                reason: format!("the materialized Fabric API failed verification: {error}"),
            }
        })?;
        Some(InstalledFabricApi {
            version: dependency.version().to_owned(),
            artifact: AuroraInstalledArtifact {
                relative_path,
                size_bytes,
                sha256: digest.as_hex(),
            },
        })
    } else {
        None
    };

    let state = AuroraInstalledState {
        schema_version: AURORA_INSTALLED_SCHEMA_VERSION,
        aurora_version: release.aurora_version().to_owned(),
        channel: release.channel(),
        minecraft_version: release.minecraft_version().to_owned(),
        fabric_loader_version: release.fabric_loader_version().to_owned(),
        artifact: AuroraInstalledArtifact {
            relative_path: relative,
            size_bytes: bytes,
            sha256: expected.as_hex(),
        },
        fabric_api: installed_fabric_api,
        installation_id: format!(
            "aurora-install-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_millis())
                .unwrap_or_default(),
            std::process::id()
        ),
        installed_at_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or_default(),
    };
    write_installed_state(managed, instance, &state)?;

    Ok(InstalledAurora {
        state,
        artifact_path,
    })
}

/// Writes the installed-state record atomically (sibling temporary file plus
/// rename) at the instance root.
pub fn write_installed_state(
    managed: &ManagedPaths,
    instance: &InstanceId,
    state: &AuroraInstalledState,
) -> Result<(), AuroraInstallError> {
    let target = managed
        .instance_paths(instance)
        .root()
        .join(AURORA_INSTALLED_FILE_NAME);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| AuroraInstallError::StateWrite(error))?;
    }

    let mut temporary = target.clone().into_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, state.to_json())
        .map_err(|error| AuroraInstallError::StateWrite(error))?;
    std::fs::rename(&temporary, &target).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        AuroraInstallError::StateWrite(error)
    })
}

/// Loads the Aurora installed-state record of one instance, if present.
///
/// A missing record is `Ok(None)`; a present-but-malformed record is a
/// deliberate error that is never overwritten by the caller.
pub fn load_installed_state(
    managed: &ManagedPaths,
    instance: &InstanceId,
) -> Result<Option<AuroraInstalledState>, AuroraInstallError> {
    let path = managed
        .instance_paths(instance)
        .root()
        .join(AURORA_INSTALLED_FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AuroraInstallError::StateRead(error)),
    };
    AuroraInstalledState::from_json(&text)
        .map(Some)
        .map_err(AuroraInstallError::State)
}

/// Re-validates the materialized Aurora artifact against its recorded
/// state: existence, size, and SHA-256. Read-only.
pub fn validate_artifact(
    managed: &ManagedPaths,
    instance: &InstanceId,
    state: &AuroraInstalledState,
) -> Result<(), String> {
    let artifact_path = managed.instance_paths(instance).root().join(
        state
            .artifact()
            .relative_path()
            .split('/')
            .collect::<PathBuf>(),
    );
    let expected =
        ArtifactDigest::parse(state.artifact().sha256()).map_err(|error| error.to_string())?;
    verify_file(
        &artifact_path,
        &expected,
        Some(state.artifact().size_bytes()),
    )
    .map_err(|error| error.to_string())?;
    if let Some(fabric_api) = state.fabric_api() {
        let path = managed.instance_paths(instance).root().join(
            fabric_api
                .artifact()
                .relative_path()
                .split('/')
                .collect::<PathBuf>(),
        );
        let digest = ArtifactDigest::parse(fabric_api.artifact().sha256())
            .map_err(|error| error.to_string())?;
        verify_file(&path, &digest, Some(fabric_api.artifact().size_bytes()))
            .map_err(|error| format!("Fabric API verification failed: {error}"))?;
    }
    Ok(())
}

/// A failed Aurora installation.
#[derive(Debug)]
pub enum AuroraInstallError {
    /// The release entry cannot describe a safe, verifiable artifact.
    ReleaseInvalid(String),
    /// The release artifact metadata could not become a valid source.
    ArtifactInvalid(String),
    /// Acquisition (transport, integrity, or cache) failed.
    Acquisition(crate::cache::AcquisitionError),
    /// Materializing or re-verifying the artifact in the instance failed.
    Materialization { path: String, reason: String },
    /// Reading the persisted installed state failed.
    StateRead(std::io::Error),
    /// Writing the persisted installed state failed.
    StateWrite(std::io::Error),
    /// The persisted installed state is malformed or unsupported.
    State(AuroraStateError),
}

impl fmt::Display for AuroraInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReleaseInvalid(reason) => {
                write!(formatter, "the Aurora release is unusable: {reason}")
            }
            Self::ArtifactInvalid(reason) => write!(
                formatter,
                "the Aurora release artifact metadata is invalid: {reason}"
            ),
            Self::Acquisition(error) => write!(formatter, "{error}"),
            Self::Materialization { path, reason } => write!(
                formatter,
                "the Aurora artifact '{path}' could not be materialized into the instance: {reason}"
            ),
            Self::StateRead(error) => write!(
                formatter,
                "the Aurora installed state could not be read: {error}"
            ),
            Self::StateWrite(error) => write!(
                formatter,
                "the Aurora installed state could not be written: {error}"
            ),
            Self::State(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for AuroraInstallError {}

/// A malformed or unsupported Aurora installed-state document.
#[derive(Debug)]
pub enum AuroraStateError {
    Malformed { reason: String },
    UnsupportedSchema { found: u32, supported: u32 },
}

impl fmt::Display for AuroraStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { reason } => {
                write!(
                    formatter,
                    "the Aurora installed state is malformed: {reason}"
                )
            }
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "the Aurora installed state uses schema version {found}; this launcher supports {supported}"
            ),
        }
    }
}

impl std::error::Error for AuroraStateError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestResponse, TestServer};
    use std::sync::Arc;
    use std::time::Duration;

    fn sha256_of(bytes: &[u8]) -> String {
        use sha2::Digest as _;
        let digest: [u8; 32] = sha2::Sha256::digest(bytes).into();
        ArtifactDigest::from_sha256(digest).as_hex()
    }

    fn release(url: &str, sha256: &str, size: Option<u64>) -> AuroraRelease {
        serde_json::from_value(serde_json::json!({
            "auroraVersion": "0.3.0",
            "channel": "stable",
            "minecraftVersion": "26.2",
            "fabricLoaderVersion": "0.19.5",
            "java": { "majorVersion": 25 },
            "artifact": { "url": url, "sha256": sha256, "sizeBytes": size }
        }))
        .unwrap()
    }

    fn test_managed(name: &str) -> (PathBuf, ManagedPaths, InstanceId) {
        let root = std::env::temp_dir()
            .join("aurora-aurora-test")
            .join(std::process::id().to_string())
            .join(name);
        let _ = std::fs::remove_dir_all(&root);
        let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();
        let instance = InstanceId::new("inst-test").unwrap();
        (root, managed, instance)
    }

    fn quick_options() -> DownloadOptions {
        DownloadOptions {
            connect_timeout: Duration::from_secs(5),
            idle_read_timeout: Duration::from_secs(5),
            max_redirects: crate::downloads::MAX_REDIRECTS,
        }
    }

    #[tokio::test]
    async fn installs_the_verified_artifact_as_a_managed_mod_and_commits_state_last() {
        let body = b"aurora client artifact bytes".to_vec();
        let served = body.clone();
        let server = TestServer::spawn(Arc::new(move |_request| TestResponse::ok(&served)));
        let (root, managed, instance) = test_managed("happy");
        let rel = release(
            &format!("{}/aurora-0.3.0-dev.jar", server.base_url()),
            &sha256_of(&body),
            Some(body.len() as u64),
        );

        // A user mod and config exist before installation; they must survive.
        let instance_paths = managed.instance_paths(&instance);
        std::fs::create_dir_all(instance_paths.mods()).unwrap();
        std::fs::write(
            instance_paths.mods().join("user-mod.jar"),
            b"user's own mod",
        )
        .unwrap();
        std::fs::create_dir_all(instance_paths.config()).unwrap();
        std::fs::write(instance_paths.config().join("options.txt"), b"gamma:1.0").unwrap();

        let installed = install_aurora(&managed, &instance, &rel, &quick_options())
            .await
            .expect("installation must succeed");

        let managed_path = instance_paths.mods().join("aurora-0.3.0.jar");
        assert_eq!(installed.artifact_path, managed_path);
        assert_eq!(std::fs::read(&managed_path).unwrap(), body);
        assert_eq!(
            std::fs::read(instance_paths.mods().join("user-mod.jar")).unwrap(),
            b"user's own mod",
            "user mods are untouched"
        );
        assert_eq!(
            std::fs::read(instance_paths.config().join("options.txt")).unwrap(),
            b"gamma:1.0"
        );

        let state = load_installed_state(&managed, &instance)
            .unwrap()
            .expect("state must be committed");
        assert_eq!(state.aurora_version(), "0.3.0");
        assert_eq!(state.artifact().relative_path(), "mods/aurora-0.3.0.jar");
        assert_eq!(state.artifact().sha256(), sha256_of(&body));
        assert_eq!(state.trust().kind_name(), "expectedDigestVerified");
        assert!(validate_artifact(&managed, &instance, &state).is_ok());
        let _ = root;
    }

    #[tokio::test]
    async fn required_fabric_api_is_verified_protected_by_state_and_detects_damage() {
        let body = b"two verified managed mod artifacts".to_vec();
        let served = body.clone();
        let server = TestServer::spawn(Arc::new(move |_request| TestResponse::ok(&served)));
        let (root, managed, instance) = test_managed("fabric-api");
        let mut value = serde_json::to_value(release(
            &format!("{}/aurora.jar", server.base_url()),
            &sha256_of(&body),
            Some(body.len() as u64),
        ))
        .unwrap();
        value["fabricApi"] = serde_json::json!({
            "version": "0.141.6+1.21.11",
            "artifact": {
                "url": format!("{}/fabric-api.jar", server.base_url()),
                "sha256": sha256_of(&body),
                "sizeBytes": body.len()
            }
        });
        let release: AuroraRelease = serde_json::from_value(value).unwrap();
        install_aurora(&managed, &instance, &release, &quick_options())
            .await
            .unwrap();
        let state = load_installed_state(&managed, &instance).unwrap().unwrap();
        let dependency = state.fabric_api().unwrap();
        assert_eq!(dependency.version(), "0.141.6+1.21.11");
        assert_eq!(
            dependency.artifact().relative_path(),
            "mods/fabric-api-0.141.6+1.21.11.jar"
        );
        assert!(validate_artifact(&managed, &instance, &state).is_ok());
        let path = managed
            .instance_paths(&instance)
            .mods()
            .join("fabric-api-0.141.6+1.21.11.jar");
        std::fs::write(&path, b"damage").unwrap();
        assert!(validate_artifact(&managed, &instance, &state).is_err());
        let _ = root;
    }

    #[tokio::test]
    async fn wrong_fabric_api_hash_never_activates_either_managed_mod() {
        let body = b"untrusted dependency bytes".to_vec();
        let served = body.clone();
        let server = TestServer::spawn(Arc::new(move |_request| TestResponse::ok(&served)));
        let (root, managed, instance) = test_managed("fabric-api-wrong-hash");
        let mut value = serde_json::to_value(release(
            &format!("{}/aurora.jar", server.base_url()),
            &sha256_of(&body),
            Some(body.len() as u64),
        ))
        .unwrap();
        value["fabricApi"] = serde_json::json!({
            "version": "0.141.6+1.21.11",
            "artifact": {
                "url": format!("{}/fabric-api.jar", server.base_url()),
                "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "sizeBytes": body.len()
            }
        });
        let release: AuroraRelease = serde_json::from_value(value).unwrap();
        assert!(
            install_aurora(&managed, &instance, &release, &quick_options())
                .await
                .is_err()
        );
        assert!(load_installed_state(&managed, &instance).unwrap().is_none());
        assert!(!managed.instance_paths(&instance).mods().exists());
        let _ = root;
    }

    #[tokio::test]
    async fn a_hash_mismatch_fails_without_committing_state() {
        let body = b"tampered artifact bytes".to_vec();
        let served = body.clone();
        let server = TestServer::spawn(Arc::new(move |_request| TestResponse::ok(&served)));
        let (root, managed, instance) = test_managed("mismatch");
        let rel = release(
            &format!("{}/aurora.jar", server.base_url()),
            &sha256_of(b"different expected bytes"),
            None,
        );

        let error = install_aurora(&managed, &instance, &rel, &quick_options())
            .await
            .expect_err("a mismatched artifact must fail");

        assert!(
            matches!(error, AuroraInstallError::Acquisition(_)),
            "{error}"
        );
        assert!(load_installed_state(&managed, &instance).unwrap().is_none());
        assert!(
            !managed
                .instance_paths(&instance)
                .root()
                .join(AURORA_INSTALLED_FILE_NAME)
                .exists()
        );
        let _ = root;
    }

    #[tokio::test]
    async fn a_materialization_failure_leaves_no_state() {
        let body = b"artifact behind a blocked mods path".to_vec();
        let served = body.clone();
        let server = TestServer::spawn(Arc::new(move |_request| TestResponse::ok(&served)));
        let (root, managed, instance) = test_managed("blocked-mods");
        let instance_paths = managed.instance_paths(&instance);
        // `mods` exists as a FILE: the managed copy cannot be placed.
        std::fs::create_dir_all(instance_paths.root()).unwrap();
        std::fs::write(instance_paths.mods(), b"not a directory").unwrap();

        let rel = release(
            &format!("{}/aurora.jar", server.base_url()),
            &sha256_of(&body),
            None,
        );
        let error = install_aurora(&managed, &instance, &rel, &quick_options())
            .await
            .expect_err("materialization must fail");

        assert!(
            matches!(error, AuroraInstallError::Materialization { .. }),
            "{error}"
        );
        assert!(load_installed_state(&managed, &instance).unwrap().is_none());
        let _ = root;
    }

    #[test]
    fn managed_paths_derive_from_release_metadata_and_reject_unsafe_versions() {
        let rel = release(
            "https://releases.example.invalid/a.jar",
            &"a".repeat(64),
            None,
        );
        assert_eq!(
            managed_artifact_relative_path(&rel).unwrap(),
            "mods/aurora-0.3.0.jar"
        );

        for broken_version in ["", "../escape", "sub/dir", "weird name", ".hidden"] {
            let mut value = serde_json::to_value(&rel).unwrap();
            value["auroraVersion"] = serde_json::Value::String(broken_version.to_owned());
            let broken: AuroraRelease = serde_json::from_value(value).unwrap();
            assert!(
                managed_artifact_relative_path(&broken).is_err(),
                "{broken_version:?} must not form a managed path"
            );
        }
    }

    #[test]
    fn installed_state_validation_is_strict() {
        let state_json = || {
            r#"{
  "schemaVersion": 1,
  "auroraVersion": "0.3.0",
  "channel": "stable",
  "minecraftVersion": "26.2",
  "fabricLoaderVersion": "0.19.5",
  "artifact": { "relativePath": "mods/aurora-0.3.0.jar", "sizeBytes": 57, "sha256": "41765f8d32372d242b51a5509f467480be661c19d127ea203e5bdf9d5da4f2c3" },
  "installationId": "aurora-install-1-1",
  "installedAtUnixSeconds": 1789455335
}"#
        };

        let state = AuroraInstalledState::from_json(state_json()).unwrap();
        assert_eq!(state.artifact().relative_path(), "mods/aurora-0.3.0.jar");
        assert!(AuroraInstalledState::from_json(&state.to_json()).is_ok());

        for broken in [
            state_json().replace("\"schemaVersion\": 1", "\"schemaVersion\": 2"),
            state_json().replace("mods/aurora-0.3.0.jar", "mods/aurora-0.3.0.json"),
            state_json().replace("mods/aurora-0.3.0.jar", "user-mod.jar"),
            state_json().replace("mods/aurora-0.3.0.jar", "../aurora-0.3.0.jar"),
            state_json().replace(
                "\"sha256\": \"41765f8d32372d242b51a5509f467480be661c19d127ea203e5bdf9d5da4f2c3\"",
                "\"sha256\": \"zz\"",
            ),
            state_json().replace("\"sizeBytes\": 57", "\"sizeBytes\": 0"),
            "not json".to_owned(),
        ] {
            assert!(
                AuroraInstalledState::from_json(&broken).is_err(),
                "must be rejected: {}",
                &broken[..broken.len().min(50)]
            );
        }
    }
}
