//! The launcher-managed artifact stores and the acquisition pipelines that
//! fill them.
//!
//! Three stores exist, one per acquisition trust class, and their identities
//! are deliberately distinct:
//!
//! - `cache/artifacts/sha256/<digest>` — verified against a pre-known
//!   expected SHA-256 (Aurora's own distribution and Fabric's published
//!   digests). Identity: the expected SHA-256.
//! - `cache/artifacts/sha1/<digest>` — verified against the official
//!   expected SHA-1 of Mojang metadata (client jar, libraries, asset index,
//!   asset objects, logging configuration). Identity: the expected SHA-1,
//!   so cache identity stays aligned with the externally expected digest.
//!   A SHA-1 store object is never addressed by or recorded as SHA-256.
//! - `cache/artifacts/transport-observed/<digest>` — digest-less artifacts
//!   official metadata adds without published digests (the Fabric loader and
//!   intermediary). There is no expected digest to verify against; the
//!   artifact was acquired over the secure transport and its SHA-256 was
//!   *computed locally* as a stable identity. A small provenance sidecar
//!   (`<digest>.json`) records the source URL and the observed digest, so
//!   later acquisitions of the same URL can compare against it as a local
//!   consistency check. This is transport trust plus observation — it is
//!   never equivalent to expected-digest verification and is never reported
//!   as such.
//!
//! Trust boundary: a download completes into an untrusted staging file under
//! `<managed-root>/cache/staging/` and is promoted into a store only after
//! its size (when expected) and digest verify — except the transport-observed
//! store, which promotes the streamed bytes with their computed identity and
//! honestly records how trust was obtained. A file that already occupies a
//! store slot is never trusted because of its name; it is re-validated by
//! hashing before it can be reported as a cache hit, and corrupt objects are
//! replaced through a full verified re-acquisition.
//!
//! Concurrency: staging names are process-unique, so simultaneous downloads
//! never share a staging file. Promotion is a rename of a fully verified
//! file, so a store only ever receives complete objects. Two concurrent
//! acquisitions of the same digest may both download (duplicate work is
//! accepted for simplicity), and the loser of the promotion race observes a
//! valid destination and discards its own staging copy. No lock file or
//! coordination registry exists; the design goal is "never corrupt", not
//! "never duplicate".

use crate::downloads::{
    self, ArtifactSource, DownloadError, DownloadOptions, ObservedArtifactSource,
    Sha1ArtifactSource, Sha512ArtifactSource,
};
use crate::integrity::{
    ArtifactDigest, ArtifactTrust, Sha1Digest, VerifyFileError, verify_file, verify_file_sha1,
    verify_file_sha512,
};
use crate::paths::ManagedPaths;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The store segment holding verified artifacts, keyed by digest encoding.
const ARTIFACTS_DIR: &str = "artifacts";
/// The digest algorithm segment for the SHA-256-addressed verified store.
const SHA256_DIR: &str = "sha256";
/// The digest algorithm segment for the SHA-1-addressed official Mojang
/// store.
const SHA1_DIR: &str = "sha1";
/// The segment holding securely transported artifacts with no published
/// digest, identified by their locally observed SHA-256.
const OBSERVED_DIR: &str = "transport-observed";
/// The untrusted staging area for in-flight downloads.
const STAGING_DIR: &str = "staging";

/// The only provenance-record schema version this launcher understands.
const OBSERVED_PROVENANCE_SCHEMA_VERSION: u32 = 1;

/// A verified artifact resting in the launcher-managed cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArtifact {
    pub path: PathBuf,
    pub sha256: ArtifactDigest,
    pub bytes: u64,
    pub origin: ArtifactOrigin,
}

/// An official Mojang artifact verified against its expected SHA-1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSha1Artifact {
    pub path: PathBuf,
    pub sha1: Sha1Digest,
    pub bytes: u64,
    pub origin: ArtifactOrigin,
}

/// A digest-less artifact acquired over the secure transport, identified by
/// its locally observed SHA-256.
///
/// `trust` is always the transport-observed class; this type can never
/// represent a digest-verified artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedArtifact {
    pub path: PathBuf,
    pub observed_sha256: ArtifactDigest,
    pub bytes: u64,
    pub origin: ArtifactOrigin,
}

/// How the verified artifact came to be in the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactOrigin {
    /// A fresh download that passed verification and was promoted.
    Downloaded,
    /// An existing store object that passed re-validation.
    CacheHit,
}

impl VerifiedSha1Artifact {
    /// The honest trust record for this artifact: verified against an
    /// official expected SHA-1.
    pub fn trust(&self) -> ArtifactTrust {
        ArtifactTrust::verified_sha1(&self.sha1)
    }
}

impl VerifiedArtifact {
    /// The honest trust record for this artifact: verified against a
    /// pre-known expected SHA-256.
    pub fn trust(&self) -> ArtifactTrust {
        ArtifactTrust::verified_sha256(&self.sha256)
    }
}

impl ObservedArtifact {
    /// The honest trust record for this artifact: secure transport with a
    /// locally observed identity — never an expected-digest verification.
    pub fn trust(&self) -> ArtifactTrust {
        ArtifactTrust::transport_observed(&self.observed_sha256)
    }
}

/// A persisted provenance record for one transport-observed acquisition.
///
/// The record preserves the provenance of the observed digest: which URL was
/// acquired, and what SHA-256 was observed when it was first acquired over
/// secure transport. It is a local consistency reference (TOFU-style), not an
/// official Fabric digest, and it never upgrades the artifact's trust class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedProvenanceRecord {
    schema_version: u32,
    source_url: String,
    observed_sha256: String,
}

impl ObservedProvenanceRecord {
    fn new(source_url: &str, observed: &ArtifactDigest) -> Self {
        Self {
            schema_version: OBSERVED_PROVENANCE_SCHEMA_VERSION,
            source_url: source_url.to_owned(),
            observed_sha256: observed.as_hex(),
        }
    }

    /// Parses one provenance record. Unknown schema versions and malformed
    /// records are errors: a record that cannot be understood grants no pin.
    fn from_json(json: &str) -> Result<Self, ObservedRecordError> {
        let record: Self =
            serde_json::from_str(json).map_err(|error| ObservedRecordError::Malformed {
                reason: error.to_string(),
            })?;
        if record.schema_version != OBSERVED_PROVENANCE_SCHEMA_VERSION {
            return Err(ObservedRecordError::UnsupportedSchema {
                found: record.schema_version,
                supported: OBSERVED_PROVENANCE_SCHEMA_VERSION,
            });
        }
        if record.source_url.trim().is_empty() {
            return Err(ObservedRecordError::Malformed {
                reason: "the record has no source URL".to_owned(),
            });
        }
        ArtifactDigest::parse(&record.observed_sha256).map_err(|error| {
            ObservedRecordError::Malformed {
                reason: format!("the observed digest is invalid: {error}"),
            }
        })?;
        Ok(record)
    }

    fn observed_digest(&self) -> ArtifactDigest {
        ArtifactDigest::parse(&self.observed_sha256)
            .expect("validated records carry a canonical digest")
    }
}

#[derive(Debug)]
enum ObservedRecordError {
    Malformed { reason: String },
    UnsupportedSchema { found: u32, supported: u32 },
}

impl fmt::Display for ObservedRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { reason } => {
                write!(
                    formatter,
                    "an observed-digest record is malformed: {reason}"
                )
            }
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "an observed-digest record uses schema version {found}; this launcher understands {supported}"
            ),
        }
    }
}

/// The launcher-managed, content-addressed artifact stores.
#[derive(Debug)]
pub struct ArtifactCache {
    managed: ManagedPaths,
    staging_sequence: AtomicU64,
}

/// The expectation an expected-digest store verifies against. The SHA-256
/// and SHA-1 paths share size enforcement and promotion mechanics; only the
/// digest comparison differs, and the two digest kinds never mix.
#[derive(Clone, Copy)]
enum Expected<'a> {
    Sha256 {
        digest: &'a ArtifactDigest,
        size_bytes: Option<u64>,
    },
    Sha1 {
        digest: &'a Sha1Digest,
        size_bytes: Option<u64>,
    },
}

impl Expected<'_> {
    fn revalidate(&self, path: &Path) -> Result<u64, VerifyFileError> {
        match *self {
            Self::Sha256 { digest, size_bytes } => verify_file(path, digest, size_bytes),
            Self::Sha1 { digest, size_bytes } => verify_file_sha1(path, digest, size_bytes),
        }
    }

    fn log_identity(&self) -> String {
        match *self {
            Self::Sha256 { digest, .. } => format!("sha256:{}", digest.as_hex()),
            Self::Sha1 { digest, .. } => format!("sha1:{}", digest.as_hex()),
        }
    }
}

impl ArtifactCache {
    pub fn new(managed: ManagedPaths) -> Self {
        Self {
            managed,
            staging_sequence: AtomicU64::new(0),
        }
    }

    /// The directory holding all verified artifacts.
    pub fn store_dir(&self) -> PathBuf {
        self.managed
            .cache_dir()
            .join(ARTIFACTS_DIR)
            .join(SHA256_DIR)
    }

    /// The directory holding official Mojang artifacts verified by SHA-1.
    pub fn sha1_store_dir(&self) -> PathBuf {
        self.managed.cache_dir().join(ARTIFACTS_DIR).join(SHA1_DIR)
    }

    /// The directory holding securely transported digest-less artifacts,
    /// identified by their locally observed SHA-256.
    pub fn observed_store_dir(&self) -> PathBuf {
        self.managed
            .cache_dir()
            .join(ARTIFACTS_DIR)
            .join(OBSERVED_DIR)
    }

    /// The untrusted staging area for in-flight downloads.
    pub fn staging_dir(&self) -> PathBuf {
        self.managed.cache_dir().join(STAGING_DIR)
    }

    /// The verified location of one artifact. Derived purely from the
    /// validated digest, so the result always stays inside the store.
    pub fn verified_path(&self, digest: &ArtifactDigest) -> PathBuf {
        self.store_dir().join(digest.as_hex())
    }

    /// The verified location of one official Mojang artifact, addressed by
    /// its expected SHA-1 so cache identity matches the official digest.
    pub fn verified_sha1_path(&self, digest: &Sha1Digest) -> PathBuf {
        self.sha1_store_dir().join(digest.as_hex())
    }

    /// The location of one transport-observed artifact, addressed by its
    /// locally observed SHA-256.
    pub fn observed_path(&self, observed: &ArtifactDigest) -> PathBuf {
        self.observed_store_dir().join(observed.as_hex())
    }

    /// Creates a unique staging file path for one download attempt.
    ///
    /// Names combine the process id, a timestamp, and a process-wide counter;
    /// nothing user-controlled can reach them.
    pub fn new_staging_path(&self) -> PathBuf {
        let sequence = self.staging_sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default();
        let name = format!(
            "download-{}-{timestamp}-{sequence}.part",
            std::process::id()
        );
        self.staging_dir().join(name)
    }

    /// Obtains the artifact described by `source` and returns its verified
    /// cache location.
    ///
    /// The pipeline: re-validate any existing store object, download to an
    /// untrusted staging file, verify while streaming, then promote the
    /// staging file into the store. A failed acquisition never leaves a
    /// trusted object behind.
    pub async fn acquire(
        &self,
        source: &ArtifactSource,
    ) -> Result<VerifiedArtifact, AcquisitionError> {
        self.acquire_with(source, &DownloadOptions::default()).await
    }

    /// Verify a published SHA-512, then address the verified bytes by their
    /// locally computed SHA-256. The small map is only a lookup hint: every
    /// reused object is checked against both digests and the expected size.
    pub async fn acquire_sha512(
        &self,
        source: &Sha512ArtifactSource,
    ) -> Result<VerifiedArtifact, AcquisitionError> {
        let map_dir = self
            .managed
            .cache_dir()
            .join(ARTIFACTS_DIR)
            .join("sha512-map");
        let map_path = map_dir.join(source.sha512().as_hex());
        if let Ok(text) = std::fs::read_to_string(&map_path) {
            if let Ok(sha256) = ArtifactDigest::parse(text.trim()) {
                let path = self.verified_path(&sha256);
                if let Ok((bytes, computed)) =
                    verify_file_sha512(&path, source.sha512(), source.size_bytes())
                {
                    if computed == sha256 {
                        return Ok(VerifiedArtifact {
                            path,
                            sha256,
                            bytes,
                            origin: ArtifactOrigin::CacheHit,
                        });
                    }
                }
            }
        }
        let staging = self.prepare_staging(&self.store_dir())?;
        let downloaded = downloads::download_sha512(source, &staging, &DownloadOptions::default())
            .await
            .map_err(AcquisitionError::Download)?;
        let path = self.verified_path(&downloaded.sha256);
        let expected = Expected::Sha256 {
            digest: &downloaded.sha256,
            size_bytes: source.size_bytes(),
        };
        self.promote(&staging, &path, expected).await?;
        std::fs::create_dir_all(&map_dir).map_err(AcquisitionError::StoreIo)?;
        let temporary = map_dir.join(format!(
            "{}.{}.tmp",
            source.sha512().as_hex(),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&temporary, downloaded.sha256.as_hex())
            .map_err(AcquisitionError::StoreIo)?;
        if let Err(error) = std::fs::rename(&temporary, &map_path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(AcquisitionError::StoreIo(error));
        }
        Ok(VerifiedArtifact {
            path,
            sha256: downloaded.sha256,
            bytes: downloaded.bytes,
            origin: ArtifactOrigin::Downloaded,
        })
    }

    /// [`ArtifactCache::acquire`] with explicit transport limits (used by
    /// tests to keep timeouts short).
    pub async fn acquire_with(
        &self,
        source: &ArtifactSource,
        options: &DownloadOptions,
    ) -> Result<VerifiedArtifact, AcquisitionError> {
        let verified_path = self.verified_path(source.sha256());
        let expected = Expected::Sha256 {
            digest: source.sha256(),
            size_bytes: source.size_bytes(),
        };

        if let Some(bytes) = self.validate_existing(&verified_path, expected).await? {
            return Ok(VerifiedArtifact {
                path: verified_path,
                sha256: *source.sha256(),
                bytes,
                origin: ArtifactOrigin::CacheHit,
            });
        }

        let staging_path = self.prepare_staging(&self.store_dir())?;
        let downloaded = downloads::download(source, &staging_path, options)
            .await
            .map_err(AcquisitionError::Download)?;
        self.promote(&staging_path, &verified_path, expected)
            .await?;

        Ok(VerifiedArtifact {
            path: verified_path,
            sha256: *source.sha256(),
            bytes: downloaded.bytes,
            origin: ArtifactOrigin::Downloaded,
        })
    }

    /// Obtains one official Mojang artifact, verified against its official
    /// expected SHA-1, and returns its SHA-1-addressed cache location.
    ///
    /// Same pipeline and guarantees as [`ArtifactCache::acquire`], with the
    /// digest comparison anchored in Mojang's SHA-1. Existing SHA-256
    /// behavior is untouched.
    pub async fn acquire_sha1(
        &self,
        source: &Sha1ArtifactSource,
        options: &DownloadOptions,
    ) -> Result<VerifiedSha1Artifact, AcquisitionError> {
        let verified_path = self.verified_sha1_path(source.sha1());
        let expected = Expected::Sha1 {
            digest: source.sha1(),
            size_bytes: source.size_bytes(),
        };

        if let Some(bytes) = self.validate_existing(&verified_path, expected).await? {
            return Ok(VerifiedSha1Artifact {
                path: verified_path,
                sha1: *source.sha1(),
                bytes,
                origin: ArtifactOrigin::CacheHit,
            });
        }

        let staging_path = self.prepare_staging(&self.sha1_store_dir())?;
        let downloaded = downloads::download_sha1(source, &staging_path, options)
            .await
            .map_err(AcquisitionError::Download)?;
        self.promote(&staging_path, &verified_path, expected)
            .await?;

        Ok(VerifiedSha1Artifact {
            path: verified_path,
            sha1: *source.sha1(),
            bytes: downloaded.bytes,
            origin: ArtifactOrigin::Downloaded,
        })
    }

    /// Obtains one digest-less artifact over the secure transport and
    /// returns its transport-observed location plus its honest trust record.
    ///
    /// Provenance: a sidecar record maps the source URL to the SHA-256
    /// observed on first acquisition. A later acquisition of the same URL
    /// compares the received bytes against that observation as a local
    /// consistency check — content changing under a stable versioned URL
    /// fails deliberately. The check is a local consistency reference; it is
    /// not an official digest and never upgrades the artifact's trust class.
    pub async fn acquire_observed(
        &self,
        source: &ObservedArtifactSource,
        options: &DownloadOptions,
    ) -> Result<ObservedArtifact, AcquisitionError> {
        let pin = self.observed_pin_for(source.url().as_str())?;

        if let Some(pin) = &pin {
            let object_path = self.observed_path(pin);
            match verify_file(&object_path, pin, None) {
                Ok(bytes) => {
                    return Ok(ObservedArtifact {
                        path: object_path,
                        observed_sha256: *pin,
                        bytes,
                        origin: ArtifactOrigin::CacheHit,
                    });
                }
                Err(VerifyFileError::Mismatch(failure)) => {
                    eprintln!(
                        "[aurora-launcher] transport-observed object {} is corrupt ({}); acquiring a replacement",
                        pin.as_hex(),
                        failure
                    );
                }
                Err(VerifyFileError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                }
                Err(VerifyFileError::Io(error)) => return Err(AcquisitionError::StoreIo(error)),
            }
        }

        std::fs::create_dir_all(self.observed_store_dir()).map_err(AcquisitionError::StoreIo)?;
        std::fs::create_dir_all(self.staging_dir()).map_err(AcquisitionError::StoreIo)?;
        let staging_path = self.new_staging_path();

        let downloaded = downloads::download_observed(source, pin.as_ref(), &staging_path, options)
            .await
            .map_err(AcquisitionError::Download)?;

        let observed = downloaded.observed_sha256;
        let object_path = self.observed_path(&observed);

        // Promotion: the staged file's identity is its computed digest, so
        // the destination (if occupied by a concurrent acquisition of the
        // same content) necessarily holds identical bytes.
        if let Err(rename_error) = tokio::fs::rename(&staging_path, &object_path).await {
            match verify_file(&object_path, &observed, None) {
                Ok(_) => {
                    let _ = tokio::fs::remove_file(&staging_path).await;
                }
                Err(_) => {
                    return Err(AcquisitionError::Promotion(PromotionError {
                        context: "storing a transport-observed artifact",
                        source: rename_error,
                    }));
                }
            }
        }

        if pin.is_none() {
            self.write_observed_record(source.url().as_str(), &observed)?;
        }

        Ok(ObservedArtifact {
            path: object_path,
            observed_sha256: observed,
            bytes: downloaded.bytes,
            origin: ArtifactOrigin::Downloaded,
        })
    }

    /// The locally recorded observed digest for one source URL, when a prior
    /// acquisition left a provenance record.
    ///
    /// Records that cannot be read or understood grant no pin (first-use
    /// semantics); they never grant trust either. Cache-internal sidecars
    /// are reconstructable hints, not persisted launcher state, so an
    /// unreadable record is skipped rather than failing the acquisition.
    fn observed_pin_for(
        &self,
        source_url: &str,
    ) -> Result<Option<ArtifactDigest>, AcquisitionError> {
        let directory = self.observed_store_dir();
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(AcquisitionError::StoreIo(error)),
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(record) = ObservedProvenanceRecord::from_json(&text) else {
                continue;
            };
            if record.source_url == source_url {
                return Ok(Some(record.observed_digest()));
            }
        }

        Ok(None)
    }

    /// Persists the provenance record for a first observed acquisition,
    /// atomically (sibling temporary file plus rename).
    fn write_observed_record(
        &self,
        source_url: &str,
        observed: &ArtifactDigest,
    ) -> Result<(), AcquisitionError> {
        let record = ObservedProvenanceRecord::new(source_url, observed);
        let mut json = serde_json::to_string_pretty(&record)
            .expect("observed provenance serialization cannot fail");
        json.push('\n');

        let target = self
            .observed_store_dir()
            .join(format!("{}.json", observed.as_hex()));
        let mut temporary = target.clone().into_os_string();
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);

        std::fs::write(&temporary, json).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            AcquisitionError::StoreIo(error)
        })?;
        std::fs::rename(&temporary, &target).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            AcquisitionError::StoreIo(error)
        })
    }

    fn prepare_staging(&self, store_dir: &Path) -> Result<PathBuf, AcquisitionError> {
        std::fs::create_dir_all(store_dir).map_err(AcquisitionError::StoreIo)?;
        std::fs::create_dir_all(self.staging_dir()).map_err(AcquisitionError::StoreIo)?;
        Ok(self.new_staging_path())
    }

    /// Re-validates an existing store object by hashing its bytes.
    ///
    /// Returns the verified byte count when the object is a usable cache
    /// hit, and `Ok(None)` when the slot is empty or corrupt so the caller
    /// proceeds with a full verified acquisition. Unexpected filesystem
    /// failures are errors: the cache never destroys an object it could not
    /// understand.
    async fn validate_existing(
        &self,
        verified_path: &Path,
        expected: Expected<'_>,
    ) -> Result<Option<u64>, AcquisitionError> {
        match expected.revalidate(verified_path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(VerifyFileError::Mismatch(failure)) => {
                eprintln!(
                    "[aurora-launcher] verified-cache object {} is corrupt ({}); acquiring a verified replacement",
                    expected.log_identity(),
                    failure
                );
                Ok(None)
            }
            Err(VerifyFileError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(VerifyFileError::Io(error)) => Err(AcquisitionError::StoreIo(error)),
        }
    }

    /// Promotes a fully verified staging file into its store slot.
    ///
    /// The rename replaces any existing object. When the rename itself fails
    /// (for example, a concurrent acquisition just promoted the same digest,
    /// or a reader briefly locked the destination), the current destination
    /// is validated: a valid object means the acquisition was won by someone
    /// else; a corrupt object is deliberately removed so the verified file
    /// can take its place; anything unexpected fails without destroying the
    /// destination.
    async fn promote(
        &self,
        staging_path: &Path,
        verified_path: &Path,
        expected: Expected<'_>,
    ) -> Result<(), AcquisitionError> {
        match tokio::fs::rename(staging_path, verified_path).await {
            Ok(()) => return Ok(()),
            Err(rename_error) => {
                match expected.revalidate(verified_path) {
                    Ok(_) => {
                        // Another acquisition completed this digest first.
                        let _ = tokio::fs::remove_file(staging_path).await;
                        return Ok(());
                    }
                    Err(VerifyFileError::Io(not_found))
                        if not_found.kind() == std::io::ErrorKind::NotFound =>
                    {
                        // The destination vanished after the rename failed;
                        // the slot is free, so try the promotion again.
                        return tokio::fs::rename(staging_path, verified_path)
                            .await
                            .map_err(|error| {
                                AcquisitionError::Promotion(PromotionError {
                                    context: "replacing a vanished cache object",
                                    source: error,
                                })
                            });
                    }
                    Err(VerifyFileError::Mismatch(_)) => {
                        // The occupying object is proven corrupt: remove it
                        // deliberately so the verified replacement can land.
                        tokio::fs::remove_file(verified_path)
                            .await
                            .map_err(|error| {
                                AcquisitionError::Promotion(PromotionError {
                                    context: "removing a corrupt cache object for replacement",
                                    source: error,
                                })
                            })?;
                        return tokio::fs::rename(staging_path, verified_path)
                            .await
                            .map_err(|error| {
                                AcquisitionError::Promotion(PromotionError {
                                    context: "replacing a corrupt cache object",
                                    source: error,
                                })
                            });
                    }
                    Err(VerifyFileError::Io(unexpected)) => {
                        // An unexpected failure while the destination could
                        // not be validated: never destroy a possibly valid
                        // object on a hunch.
                        let _ = tokio::fs::remove_file(staging_path).await;
                        return Err(AcquisitionError::PromotionBlocked {
                            rename: rename_error,
                            validation: unexpected,
                        });
                    }
                }
            }
        }
    }
}

/// A failed acquisition. Variants map to stable command error categories.
#[derive(Debug)]
pub enum AcquisitionError {
    /// The transfer failed or the received bytes did not verify.
    Download(DownloadError),
    /// Managing the store or staging area (or reading an existing object)
    /// failed.
    StoreIo(std::io::Error),
    /// A verified staging file could not be moved into the store.
    Promotion(PromotionError),
    /// Promotion failed and the existing destination could not be validated,
    /// so it was left untouched.
    PromotionBlocked {
        rename: std::io::Error,
        validation: std::io::Error,
    },
}

#[derive(Debug)]
pub struct PromotionError {
    /// What the launcher was doing when the promotion failed.
    pub context: &'static str,
    pub source: std::io::Error,
}

impl fmt::Display for AcquisitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Download(error) => write!(formatter, "{error}"),
            Self::StoreIo(error) => write!(
                formatter,
                "the launcher-managed artifact cache could not be used: {error}"
            ),
            Self::Promotion(error) => write!(
                formatter,
                "the verified artifact could not be moved into the launcher's cache while trying to {}: {}",
                error.context, error.source
            ),
            Self::PromotionBlocked { rename, validation } => write!(
                formatter,
                "the verified artifact could not replace the existing cache object (move failed: {rename}; existing object could not be validated: {validation})"
            ),
        }
    }
}

impl std::error::Error for AcquisitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Download(error) => Some(error),
            Self::StoreIo(error) => Some(error),
            Self::Promotion(error) => Some(&error.source),
            Self::PromotionBlocked { rename, .. } => Some(rename),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::StreamingVerifier;
    use crate::test_support::{TestResponse, TestServer};
    use sha2::{Digest as _, Sha256};
    use std::sync::Arc;
    use std::time::Duration;

    fn digest_of(bytes: &[u8]) -> ArtifactDigest {
        ArtifactDigest::from_sha256(Sha256::digest(bytes).into())
    }

    fn test_cache(name: &str) -> (PathBuf, ArtifactCache) {
        let root = std::env::temp_dir()
            .join("aurora-cache-test")
            .join(std::process::id().to_string())
            .join(name);
        let managed =
            ManagedPaths::from_app_local_data_dir(root.join("managed")).expect("absolute root");
        (root, ArtifactCache::new(managed))
    }

    fn serve(
        handler: impl Fn(&crate::test_support::TestRequest) -> TestResponse + Send + Sync + 'static,
    ) -> TestServer {
        TestServer::spawn(Arc::new(handler))
    }

    fn source_for(
        server: &TestServer,
        path: &str,
        sha256: &ArtifactDigest,
        size_bytes: Option<u64>,
    ) -> ArtifactSource {
        ArtifactSource::loopback_http_for_testing(
            &format!("{}{path}", server.base_url()),
            &sha256.as_hex(),
            size_bytes,
        )
        .expect("test source must be valid")
    }

    fn quick_options() -> DownloadOptions {
        DownloadOptions {
            connect_timeout: Duration::from_secs(5),
            idle_read_timeout: Duration::from_secs(5),
            max_redirects: downloads::MAX_REDIRECTS,
        }
    }

    fn stored_files(cache: &ArtifactCache) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(cache.store_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn staging_files(cache: &ArtifactCache) -> Vec<String> {
        std::fs::read_dir(cache.staging_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn a_verified_download_is_promoted_and_leaves_no_staging_file() {
        let body = b"a tiny aurora artifact".to_vec();
        let expected_digest = digest_of(&body);
        let expected_body = body.clone();
        let server = serve(move |_request| TestResponse::ok(&body));
        let (_root, cache) = test_cache("promoted");
        let source = source_for(&server, "/aurora-0.1.0.zip", &expected_digest, None);

        let artifact = cache.acquire_with(&source, &quick_options()).await.unwrap();

        assert_eq!(artifact.origin, ArtifactOrigin::Downloaded);
        assert_eq!(artifact.bytes, expected_body.len() as u64);
        assert_eq!(
            artifact.path,
            cache.verified_path(&expected_digest),
            "the returned path must be the verified store object"
        );
        assert_eq!(std::fs::read(&artifact.path).unwrap(), expected_body);
        assert_eq!(stored_files(&cache), vec![expected_digest.as_hex()]);
        assert!(staging_files(&cache).is_empty());
    }

    #[tokio::test]
    async fn a_valid_cache_object_is_revalidated_and_reused() {
        let body = b"cache-hit payload".to_vec();
        let expected_digest = digest_of(&body);
        let server = serve(move |_request| TestResponse::ok(&body));
        let (_root, cache) = test_cache("cache-hit");
        let source = source_for(&server, "/aurora.jar", &expected_digest, None);

        let first = cache.acquire_with(&source, &quick_options()).await.unwrap();
        let second = cache.acquire_with(&source, &quick_options()).await.unwrap();

        assert_eq!(first.origin, ArtifactOrigin::Downloaded);
        assert_eq!(second.origin, ArtifactOrigin::CacheHit);
        assert_eq!(first.path, second.path);
        assert_eq!(server.request_count(), 1, "the hit must not re-download");
        assert_eq!(stored_files(&cache).len(), 1);
    }

    #[tokio::test]
    async fn a_corrupted_cache_object_is_not_trusted_and_is_replaced() {
        let body = b"the one true artifact".to_vec();
        let expected_digest = digest_of(&body);
        let expected_body = body.clone();
        let server = serve(move |_request| TestResponse::ok(&body));
        let (_root, cache) = test_cache("corrupt-replaced");
        let source = source_for(&server, "/aurora.jar", &expected_digest, None);
        std::fs::create_dir_all(cache.store_dir()).unwrap();
        std::fs::write(cache.verified_path(&expected_digest), b"corrupted bytes").unwrap();

        let artifact = cache.acquire_with(&source, &quick_options()).await.unwrap();

        assert_eq!(artifact.origin, ArtifactOrigin::Downloaded);
        assert_eq!(std::fs::read(&artifact.path).unwrap(), expected_body);
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn a_corrupted_cache_object_fails_deliberately_when_replacement_is_impossible() {
        let body = b"unreachable host payload".to_vec();
        let digest = digest_of(&body);
        // A port with no listener: replacement cannot succeed.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let (_root, cache) = test_cache("corrupt-unreachable");
        let source = ArtifactSource::loopback_http_for_testing(
            &format!("http://127.0.0.1:{port}/aurora.jar"),
            &digest.as_hex(),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(cache.store_dir()).unwrap();
        std::fs::write(cache.verified_path(&digest), b"corrupted bytes").unwrap();

        let result = cache.acquire_with(&source, &quick_options()).await;

        assert!(
            matches!(result, Err(AcquisitionError::Download(_))),
            "the acquisition must fail instead of trusting the corrupt object"
        );
        // The corrupt file is still not a verified artifact, but the failure
        // never destroyed it silently behind the caller's back.
        assert!(cache.verified_path(&digest).exists());
    }

    #[tokio::test]
    async fn a_failed_verification_never_promotes_an_object() {
        let body = b"bytes that will not match their manifest digest".to_vec();
        let server = serve(move |_request| TestResponse::ok(&body));
        let (_root, cache) = test_cache("mismatch-no-promotion");
        // The manifest digest belongs to different bytes entirely.
        let source = source_for(&server, "/aurora.jar", &digest_of(b"other bytes"), None);

        let error = cache
            .acquire_with(&source, &quick_options())
            .await
            .expect_err("a digest mismatch must fail the acquisition");

        assert!(matches!(
            error,
            AcquisitionError::Download(DownloadError::Sha256Mismatch { .. })
        ));
        assert!(
            stored_files(&cache).is_empty(),
            "no trusted object may exist"
        );
        assert!(
            staging_files(&cache).is_empty(),
            "staging debris must be gone"
        );
    }

    #[tokio::test]
    async fn a_size_mismatch_never_promotes_an_object() {
        let body = b"0123456789".to_vec();
        let expected_digest = digest_of(&body);
        let declared_size = body.len() as u64 + 5;
        let server = serve(move |_request| TestResponse::ok(&body));
        let (_root, cache) = test_cache("size-no-promotion");
        let source = source_for(
            &server,
            "/aurora.jar",
            &expected_digest,
            Some(declared_size),
        );

        let error = cache
            .acquire_with(&source, &quick_options())
            .await
            .expect_err("a size mismatch must fail the acquisition");

        assert!(matches!(
            error,
            AcquisitionError::Download(DownloadError::SizeMismatch { .. })
        ));
        assert!(stored_files(&cache).is_empty());
        assert!(staging_files(&cache).is_empty());
    }

    #[tokio::test]
    async fn the_digest_alone_identifies_the_cache_object() {
        let body = b"identical bytes served under two names".to_vec();
        let expected_digest = digest_of(&body);
        let server = serve(move |request| {
            let mut response = TestResponse::ok(&body);
            if request.path == "/aurora-0.1.0.zip" {
                response = response.with_header(
                    "Content-Disposition",
                    "attachment; filename=\"..\\..\\totally different name.zip\"",
                );
            }
            response
        });
        let (_root, cache) = test_cache("content-addressing");
        let first = source_for(&server, "/aurora-0.1.0.zip", &expected_digest, None);
        let second = source_for(
            &server,
            "/mirror/some-other-name.bin",
            &expected_digest,
            None,
        );

        let downloaded = cache.acquire_with(&first, &quick_options()).await.unwrap();
        let hit = cache.acquire_with(&second, &quick_options()).await.unwrap();

        assert_eq!(hit.origin, ArtifactOrigin::CacheHit);
        assert_eq!(hit.path, downloaded.path);
        assert_eq!(server.request_count(), 1, "identical content is one object");
        assert_eq!(
            stored_files(&cache),
            vec![expected_digest.as_hex()],
            "remote file names must not appear in the store"
        );
    }

    #[tokio::test]
    async fn concurrent_acquisitions_of_one_digest_never_corrupt_the_object() {
        let body: Vec<u8> = b"concurrent-contention-payload!".repeat(20);
        let expected_digest = digest_of(&body);
        let server = serve(move |_request| {
            TestResponse::ok(&body).with_drip_delay(Duration::from_millis(5))
        });
        let (_root, cache) = test_cache("concurrent");
        let source = source_for(&server, "/aurora.jar", &expected_digest, None);
        let options = quick_options();

        let (first, second) = tokio::join!(
            cache.acquire_with(&source, &options),
            cache.acquire_with(&source, &options)
        );

        let first = first.expect("both acquisitions must succeed");
        let second = second.expect("both acquisitions must succeed");
        assert_eq!(first.path, second.path);

        // Whatever interleaving happened, the stored object must be exactly
        // the verified content, and no staging debris may survive.
        let stored = std::fs::read(&first.path).unwrap();
        let mut verifier = StreamingVerifier::new(Some(stored.len() as u64));
        verifier.update(&stored).unwrap();
        verifier
            .finish(&expected_digest)
            .expect("stored object must verify");
        assert!(staging_files(&cache).is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_store_entry_fails_deliberately_without_deleting_it() {
        let body = b"payload behind a blocked store slot".to_vec();
        let expected_digest = digest_of(&body);
        let server = serve(move |_request| TestResponse::ok(&body));
        let (_root, cache) = test_cache("blocked-slot");
        std::fs::create_dir_all(cache.store_dir()).unwrap();
        // A directory where a file must be: unreadable as an artifact, but
        // not something the cache may silently destroy.
        std::fs::create_dir(cache.verified_path(&expected_digest)).unwrap();

        let error = cache
            .acquire_with(
                &source_for(&server, "/aurora.jar", &expected_digest, None),
                &quick_options(),
            )
            .await
            .expect_err("an unreadable store slot must fail deliberately");

        assert!(matches!(error, AcquisitionError::StoreIo(_)));
        assert!(
            cache.verified_path(&expected_digest).is_dir(),
            "the cache must not delete what it could not understand"
        );
    }

    #[test]
    fn cache_paths_stay_inside_managed_storage() {
        let (_root, cache) = test_cache("path-containment");
        let managed_root = cache.managed.data_root().to_path_buf();
        let digest = digest_of(b"any");

        let verified = cache.verified_path(&digest);
        assert!(verified.starts_with(&managed_root));
        assert!(verified.starts_with(cache.store_dir()));
        assert_eq!(verified.extension(), None);
        assert_eq!(
            verified.file_name().unwrap().to_string_lossy(),
            digest.as_hex()
        );

        assert!(cache.store_dir().starts_with(&managed_root));
        assert!(cache.staging_dir().starts_with(&managed_root));
    }

    #[test]
    fn staging_names_are_unique_and_not_user_controlled() {
        let (_root, cache) = test_cache("staging-names");

        let first = cache.new_staging_path();
        let second = cache.new_staging_path();

        assert_ne!(first, second);
        for path in [&first, &second] {
            assert!(path.starts_with(cache.staging_dir()));
            assert_eq!(path.extension().unwrap(), "part");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("download-"), "{name} is self-descriptive");
        }
    }

    #[tokio::test]
    async fn a_mojang_artifact_verifies_by_sha1_and_reuses_on_the_second_pass() {
        let body = b"official client bytes".to_vec();
        let expected = crate::integrity::Sha1Digest::compute(&body);
        let byte_count = body.len() as u64;
        let server = serve(move |_request| TestResponse::ok(&body));
        let (_root, cache) = test_cache("sha1-store");
        let source = Sha1ArtifactSource::loopback_http_for_testing(
            &format!("{}/client.jar", server.base_url()),
            &expected.as_hex(),
            Some(byte_count),
        )
        .unwrap();

        let first = cache.acquire_sha1(&source, &quick_options()).await.unwrap();
        let second = cache.acquire_sha1(&source, &quick_options()).await.unwrap();

        assert_eq!(first.origin, ArtifactOrigin::Downloaded);
        assert_eq!(second.origin, ArtifactOrigin::CacheHit);
        assert_eq!(first.path, cache.verified_sha1_path(&expected));
        assert_eq!(
            std::fs::read(&first.path).unwrap(),
            b"official client bytes"
        );
        assert_eq!(server.request_count(), 1);
        // The honest trust record anchors in Mojang's SHA-1.
        assert_eq!(
            first.trust(),
            crate::integrity::ArtifactTrust::ExpectedDigestVerified {
                algorithm: crate::integrity::DigestAlgorithm::Sha1,
                digest: expected.as_hex(),
            }
        );
    }

    #[tokio::test]
    async fn a_wrong_sha1_never_promotes_and_a_corrupt_sha1_object_is_replaced() {
        let body = b"one true library".to_vec();
        let expected = crate::integrity::Sha1Digest::compute(&body);
        let server = serve(move |_request| TestResponse::ok(&body));
        let (_root, cache) = test_cache("sha1-integrity");

        // Wrong expectation: hard failure, empty store.
        let wrong = Sha1ArtifactSource::loopback_http_for_testing(
            &format!("{}/lib.jar", server.base_url()),
            &crate::integrity::Sha1Digest::compute(b"other bytes").as_hex(),
            None,
        )
        .unwrap();
        let error = cache
            .acquire_sha1(&wrong, &quick_options())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AcquisitionError::Download(DownloadError::Sha1Mismatch { .. })
        ));
        assert!(std::fs::read_dir(cache.sha1_store_dir()).unwrap().count() == 0);

        // Corrupt occupant: re-acquired and replaced.
        let source = Sha1ArtifactSource::loopback_http_for_testing(
            &format!("{}/lib.jar", server.base_url()),
            &expected.as_hex(),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(cache.sha1_store_dir()).unwrap();
        std::fs::write(cache.verified_sha1_path(&expected), b"corrupted").unwrap();

        let artifact = cache.acquire_sha1(&source, &quick_options()).await.unwrap();

        assert_eq!(artifact.origin, ArtifactOrigin::Downloaded);
        assert_eq!(std::fs::read(&artifact.path).unwrap(), b"one true library");
    }

    #[tokio::test]
    async fn a_digest_less_artifact_is_stored_with_observed_identity_and_provenance() {
        let body = b"fabric loader jar bytes".to_vec();
        let observed = digest_of(&body);
        let served_body = body.clone();
        let server = serve(move |_request| TestResponse::ok(&served_body));
        let url = format!(
            "{}/net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar",
            server.base_url()
        );
        let (_root, cache) = test_cache("observed-store");
        let source = ObservedArtifactSource::loopback_http_for_testing(&url).unwrap();

        let first = cache
            .acquire_observed(&source, &quick_options())
            .await
            .unwrap();
        let second = cache
            .acquire_observed(&source, &quick_options())
            .await
            .unwrap();

        assert_eq!(first.origin, ArtifactOrigin::Downloaded);
        assert_eq!(second.origin, ArtifactOrigin::CacheHit);
        assert_eq!(first.observed_sha256, observed);
        assert_eq!(first.path, cache.observed_path(&observed));
        assert_eq!(std::fs::read(&first.path).unwrap(), body);
        assert_eq!(
            server.request_count(),
            1,
            "the pin makes the second pass a hit"
        );

        // The trust record is the transport-observed class, never
        // expected-digest verification.
        assert_eq!(
            first.trust(),
            crate::integrity::ArtifactTrust::SecureTransportObserved {
                observed_sha256: observed.as_hex(),
            }
        );

        // The provenance sidecar maps the URL to the observed digest.
        let record_path = cache
            .observed_store_dir()
            .join(format!("{}.json", observed.as_hex()));
        let record = std::fs::read_to_string(&record_path).unwrap();
        assert!(record.contains(&url));
        assert!(record.contains(&observed.as_hex()));
    }

    #[tokio::test]
    async fn changed_content_under_a_pinned_url_fails_deliberately() {
        // One server, one URL: the first request serves one payload, later
        // requests serve different bytes under the same stable URL.
        let first_body = b"loader build 1".to_vec();
        let first_observed = digest_of(&first_body);
        let served = first_body.clone();
        let later = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let later_body = b"loader build 2".to_vec();
        let later_ref = later.clone();
        let server = serve(move |_request| {
            if later_ref.load(Ordering::SeqCst) {
                TestResponse::ok(&later_body)
            } else {
                TestResponse::ok(&served)
            }
        });
        let url = format!("{}/fabric-loader.jar", server.base_url());
        let (_root, cache) = test_cache("observed-drift");
        let source = ObservedArtifactSource::loopback_http_for_testing(&url).unwrap();

        let first = cache
            .acquire_observed(&source, &quick_options())
            .await
            .unwrap();
        assert_eq!(first.observed_sha256, first_observed);

        // Same URL, different bytes: remove the stored object so a
        // re-acquisition must download again, this time against the pin.
        later.store(true, Ordering::SeqCst);
        std::fs::remove_file(cache.observed_path(&first_observed)).unwrap();

        let error = cache
            .acquire_observed(&source, &quick_options())
            .await
            .expect_err("drifted content must fail the local pin");

        assert!(
            matches!(
                error,
                AcquisitionError::Download(DownloadError::ObservedDigestDrift { .. })
            ),
            "unexpected error: {error}"
        );
        assert!(staging_files(&cache).is_empty());
        // A new object for the drifted bytes was never stored: only the
        // original pin remains, and no record was rewritten.
        assert!(!cache.observed_path(&digest_of(b"loader build 2")).exists());
    }

    #[test]
    fn all_store_directories_derive_inside_managed_storage() {
        let (_root, cache) = test_cache("store-paths");
        let managed_root = cache.managed.data_root().to_path_buf();

        for directory in [
            cache.store_dir(),
            cache.sha1_store_dir(),
            cache.observed_store_dir(),
            cache.staging_dir(),
        ] {
            assert!(
                directory.starts_with(&managed_root),
                "{}",
                directory.display()
            );
        }

        let digest = digest_of(b"any");
        let sha1 = crate::integrity::Sha1Digest::compute(b"any");
        assert_eq!(
            cache.verified_sha1_path(&sha1),
            cache.sha1_store_dir().join(sha1.as_hex())
        );
        assert_eq!(
            cache.observed_path(&digest),
            cache.observed_store_dir().join(digest.as_hex())
        );
    }
}
