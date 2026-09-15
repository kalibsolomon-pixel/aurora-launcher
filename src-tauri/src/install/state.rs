//! The Aurora-owned installed-state record.
//!
//! One small versioned JSON document, `installed-game.json` at the root of
//! the instance's managed `game/` directory, records what a complete
//! installation consists of: every installed file with its logical role, its
//! path relative to the game directory, its size, and — crucially — *how its
//! bytes came to be trusted* (verified against an official expected digest,
//! or acquired over secure transport with a locally observed one).
//!
//! The manifest is the completion marker: it is written last inside the
//! installation staging tree and becomes visible together with the whole
//! tree when staging is promoted. A `game/` directory without a parseable,
//! supported-version manifest is by definition not a complete installation.
//!
//! This record exists so a future repair phase can validate (and re-acquire)
//! every managed file from Aurora's own data, without re-resolving or
//! re-parsing external Mojang or Fabric metadata. Repair itself is not
//! implemented yet.
//!
//! Deliberate failure semantics: an unknown schema version or malformed
//! content is a hard error; the file is never migrated speculatively and
//! never overwritten.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::integrity::{ArtifactDigest, ArtifactTrust, DigestAlgorithm, Sha1Digest};

/// The only installed-state schema version this launcher understands.
pub const INSTALLED_GAME_SCHEMA_VERSION: u32 = 1;

/// The manifest file name at the root of the managed game directory.
pub const INSTALLED_GAME_FILE_NAME: &str = "installed-game.json";

/// The complete installed-state record of one instance's game directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledGameManifest {
    schema_version: u32,
    minecraft_version: String,
    fabric_loader_version: String,
    /// The revision identity of this installation (diagnostics only; two
    /// installs of the same plan are distinct revisions).
    installation_id: String,
    installed_at_unix_seconds: u64,
    files: Vec<InstalledFile>,
    natives: NativesRecord,
}

/// One installed managed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledFile {
    role: InstalledFileRole,
    /// The path relative to the game directory, forward slashes, validated
    /// safe-relative on load. Never absolute, never traversing.
    path: String,
    trust: ArtifactTrust,
    size_bytes: u64,
}

/// The logical role one installed file plays in the installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstalledFileRole {
    Client,
    LoggingConfig,
    Library,
    NativeLibrary,
    AssetIndex,
    AssetObject,
}

/// The extracted native directory (derived content, not digest-tracked).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativesRecord {
    /// The path of the extracted natives directory relative to the game
    /// directory.
    directory: String,
}

impl NativesRecord {
    pub fn new(directory: String) -> Self {
        Self { directory }
    }

    pub fn directory(&self) -> &str {
        &self.directory
    }
}

/// Bound on one relative manifest path; real paths are far shorter.
const MAX_RELATIVE_PATH_LENGTH: usize = 512;

impl InstalledGameManifest {
    /// Builds a manifest from already-validated parts (the executor's
    /// materialization pass constructs the file list as it works).
    pub fn new(
        minecraft_version: impl Into<String>,
        fabric_loader_version: impl Into<String>,
        installation_id: impl Into<String>,
        installed_at_unix_seconds: u64,
        files: Vec<InstalledFile>,
        natives: NativesRecord,
    ) -> Self {
        Self {
            schema_version: INSTALLED_GAME_SCHEMA_VERSION,
            minecraft_version: minecraft_version.into(),
            fabric_loader_version: fabric_loader_version.into(),
            installation_id: installation_id.into(),
            installed_at_unix_seconds,
            files,
            natives,
        }
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn minecraft_version(&self) -> &str {
        &self.minecraft_version
    }

    pub fn fabric_loader_version(&self) -> &str {
        &self.fabric_loader_version
    }

    pub fn installation_id(&self) -> &str {
        &self.installation_id
    }

    pub fn installed_at_unix_seconds(&self) -> u64 {
        self.installed_at_unix_seconds
    }

    pub fn files(&self) -> &[InstalledFile] {
        &self.files
    }

    pub fn natives(&self) -> &NativesRecord {
        &self.natives
    }

    /// Parses and validates an installed-state document.
    ///
    /// Validation is deliberately strict: unknown schema versions fail
    /// instead of migrating, every path must be a safe relative path, every
    /// trust record must carry a canonical digest of its declared algorithm,
    /// and every size must be positive. A malformed manifest is never
    /// silently repaired or replaced by the caller.
    pub fn from_json(json: &str) -> Result<Self, InstalledStateError> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|error| InstalledStateError::Malformed {
                reason: error.to_string(),
            })?;

        if manifest.schema_version != INSTALLED_GAME_SCHEMA_VERSION {
            return Err(InstalledStateError::UnsupportedSchema {
                found: manifest.schema_version,
                supported: INSTALLED_GAME_SCHEMA_VERSION,
            });
        }

        if manifest.minecraft_version.trim().is_empty()
            || manifest.fabric_loader_version.trim().is_empty()
            || manifest.installation_id.trim().is_empty()
        {
            return Err(InstalledStateError::Malformed {
                reason: "the manifest must identify its Minecraft version, Fabric Loader version, and installation id"
                    .to_owned(),
            });
        }

        if manifest.files.is_empty() {
            return Err(InstalledStateError::Malformed {
                reason: "the manifest records no installed files".to_owned(),
            });
        }

        for file in &manifest.files {
            validate_relative_game_path(&file.path).map_err(|reason| {
                InstalledStateError::Malformed {
                    reason: format!("installed file '{}': {reason}", file.path),
                }
            })?;
            if file.size_bytes == 0 {
                return Err(InstalledStateError::Malformed {
                    reason: format!("installed file '{}' records a size of zero", file.path),
                });
            }
            validate_trust_record(&file.trust).map_err(|reason| {
                InstalledStateError::Malformed {
                    reason: format!("installed file '{}': {reason}", file.path),
                }
            })?;
        }

        validate_relative_game_path(&manifest.natives.directory).map_err(|reason| {
            InstalledStateError::Malformed {
                reason: format!(
                    "natives directory '{}': {reason}",
                    manifest.natives.directory
                ),
            }
        })?;

        Ok(manifest)
    }

    /// Serializes as pretty, human-inspectable JSON.
    pub fn to_json(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("installed-state serialization cannot fail");
        json.push('\n');
        json
    }
}

impl InstalledFile {
    pub fn new(
        role: InstalledFileRole,
        path: String,
        trust: ArtifactTrust,
        size_bytes: u64,
    ) -> Self {
        Self {
            role,
            path,
            trust,
            size_bytes,
        }
    }

    pub fn role(&self) -> InstalledFileRole {
        self.role
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn trust(&self) -> &ArtifactTrust {
        &self.trust
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

/// A path is valid if it is relative, forward-slashed, traversal-free, and
/// built from safe filename segments — the same class of path planning
/// already guarantees, re-checked at the persisted boundary.
fn validate_relative_game_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_RELATIVE_PATH_LENGTH {
        return Err("the path must be 1 to 512 characters long".to_owned());
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Err("the path must be relative with forward slashes".to_owned());
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("the path must not traverse or repeat segments".to_owned());
        }
        if !segment.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '(' | ')' | '$')
        }) {
            return Err(
                "the path may only contain letters, digits, '.', '_', '-', '+', '(', ')', and '$' per segment"
                    .to_owned(),
            );
        }
    }
    Ok(())
}

/// A trust record is valid if its digest is a canonical encoding of its
/// declared algorithm — an honest, parseable provenance statement.
fn validate_trust_record(trust: &ArtifactTrust) -> Result<(), String> {
    match trust {
        ArtifactTrust::ExpectedDigestVerified { algorithm, digest } => match algorithm {
            DigestAlgorithm::Sha256 => ArtifactDigest::parse(digest)
                .map(|_| ())
                .map_err(|error| format!("the SHA-256 digest is invalid: {error}")),
            DigestAlgorithm::Sha1 => Sha1Digest::parse(digest)
                .map(|_| ())
                .map_err(|error| format!("the SHA-1 digest is invalid: {error}")),
        },
        ArtifactTrust::SecureTransportObserved { observed_sha256 } => {
            ArtifactDigest::parse(observed_sha256)
                .map(|_| ())
                .map_err(|error| format!("the observed SHA-256 digest is invalid: {error}"))
        }
    }
}

/// Loads the installed-state manifest of one game directory, if present.
///
/// Returns `Ok(None)` when no manifest exists (no installation, or an
/// interrupted one — both are "not installed"). A present-but-malformed
/// manifest is a deliberate error, never overwritten.
pub fn load_installed_state(
    game_dir: &Path,
) -> Result<Option<InstalledGameManifest>, InstalledStateError> {
    let path = game_dir.join(INSTALLED_GAME_FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Err(InstalledStateError::Io(std::io::Error::other(
                "the installed-state manifest is not valid UTF-8 and may be corrupted",
            )));
        }
        Err(error) => return Err(InstalledStateError::Io(error)),
    };

    InstalledGameManifest::from_json(&text).map(Some)
}

/// A malformed or unsupported installed-state document.
#[derive(Debug)]
pub enum InstalledStateError {
    Malformed { reason: String },
    UnsupportedSchema { found: u32, supported: u32 },
    Io(std::io::Error),
}

impl fmt::Display for InstalledStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { reason } => {
                write!(formatter, "the installed game state is malformed: {reason}")
            }
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "the installed game state uses schema version {found}; this launcher supports {supported}"
            ),
            Self::Io(error) => {
                write!(
                    formatter,
                    "the installed game state could not be read: {error}"
                )
            }
        }
    }
}

impl std::error::Error for InstalledStateError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest_json() -> String {
        r#"{
  "schemaVersion": 1,
  "minecraftVersion": "26.2",
  "fabricLoaderVersion": "0.19.5",
  "installationId": "install-1760000000000-4242",
  "installedAtUnixSeconds": 1760000000,
  "files": [
    {
      "role": "client",
      "path": "versions/26.2/client.jar",
      "trust": {
        "kind": "expectedDigestVerified",
        "algorithm": "sha1",
        "digest": "e6e7b5c2f8e0f8e7e6a1b2c3d4e5f60718293a4b"
      },
      "sizeBytes": 33816576
    },
    {
      "role": "library",
      "path": "libraries/net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar",
      "trust": {
        "kind": "secureTransportObserved",
        "observedSha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
      },
      "sizeBytes": 785421
    }
  ],
  "natives": { "directory": "natives/26.2" }
}"#
        .to_owned()
    }

    #[test]
    fn the_manifest_round_trips_and_reports_its_parts() {
        let manifest = InstalledGameManifest::from_json(&sample_manifest_json()).unwrap();

        assert_eq!(manifest.schema_version(), INSTALLED_GAME_SCHEMA_VERSION);
        assert_eq!(manifest.minecraft_version(), "26.2");
        assert_eq!(manifest.fabric_loader_version(), "0.19.5");
        assert_eq!(manifest.installation_id(), "install-1760000000000-4242");
        assert_eq!(manifest.installed_at_unix_seconds(), 1760000000);
        assert_eq!(manifest.files().len(), 2);
        assert_eq!(manifest.files()[0].role(), InstalledFileRole::Client);
        assert_eq!(
            manifest.files()[1].trust().kind_name(),
            "secureTransportObserved"
        );
        assert_eq!(manifest.natives().directory(), "natives/26.2");

        let reparsed = InstalledGameManifest::from_json(&manifest.to_json()).unwrap();
        assert_eq!(reparsed, manifest);
        assert!(manifest.to_json().contains("\"schemaVersion\": 1"));
    }

    #[test]
    fn unsupported_schema_versions_and_malformed_documents_fail_deliberately() {
        let unsupported =
            sample_manifest_json().replace("\"schemaVersion\": 1", "\"schemaVersion\": 2");
        assert!(matches!(
            InstalledGameManifest::from_json(&unsupported),
            Err(InstalledStateError::UnsupportedSchema { found: 2, .. })
        ));

        for broken in [
            "not json",
            "{}",
            &sample_manifest_json().replace(
                "\"path\": \"versions/26.2/client.jar\"",
                "\"path\": \"../outside.jar\"",
            ),
            &sample_manifest_json().replace(
                "\"path\": \"versions/26.2/client.jar\"",
                "\"path\": \"C:/evil/client.jar\"",
            ),
            &sample_manifest_json().replace("\"sizeBytes\": 33816576", "\"sizeBytes\": 0"),
            &sample_manifest_json().replace(
                "\"digest\": \"e6e7b5c2f8e0f8e7e6a1b2c3d4e5f60718293a4b\"",
                "\"digest\": \"deadbeef\"",
            ),
        ] {
            assert!(
                InstalledGameManifest::from_json(broken).is_err(),
                "must be rejected: {}",
                &broken[..broken.len().min(60)]
            );
        }

        // An empty file list is not an installation.
        let empty_files = sample_manifest_json().replace(
            &sample_manifest_json()[sample_manifest_json().find("\"files\"").unwrap()
                ..sample_manifest_json().find("\"natives\"").unwrap()],
            "\"files\": [],\n  ",
        );
        assert!(InstalledGameManifest::from_json(&empty_files).is_err());
    }

    #[test]
    fn loading_reports_a_missing_manifest_as_not_installed() {
        let directory = std::env::temp_dir()
            .join("aurora-installed-state-test")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&directory).unwrap();

        assert!(load_installed_state(&directory).unwrap().is_none());
    }
}
