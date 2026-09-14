//! The verified-artifact cache: content-addressed storage and the acquisition
//! pipeline that fills it.
//!
//! Identity: a verified artifact is identified by its SHA-256 digest alone.
//! The cache object lives at `<managed-root>/cache/artifacts/sha256/<digest>`;
//! remote URLs and file names never influence local paths.
//!
//! Trust boundary: a download completes into an untrusted staging file under
//! `<managed-root>/cache/staging/` and is promoted into the store only after
//! its size (when expected) and digest verify. A file that already occupies a
//! store slot is never trusted because of its name; it is re-validated by
//! hashing before it can be reported as a cache hit, and corrupt objects are
//! replaced through a full verified re-acquisition.
//!
//! Concurrency: staging names are process-unique, so simultaneous downloads
//! never share a staging file. Promotion is a rename of a fully verified
//! file, so the store only ever receives complete objects. Two concurrent
//! acquisitions of the same digest may both download (duplicate work is
//! accepted for simplicity), and the loser of the promotion race observes a
//! valid destination and discards its own staging copy. No lock file or
//! coordination registry exists; the design goal is "never corrupt", not
//! "never duplicate".

use crate::downloads::{self, ArtifactSource, DownloadError, DownloadOptions};
use crate::integrity::{ArtifactDigest, VerifyFileError, verify_file};
use crate::paths::ManagedPaths;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The store segment holding verified artifacts, keyed by digest encoding.
const ARTIFACTS_DIR: &str = "artifacts";
/// The digest algorithm segment; digests and paths are tied to SHA-256.
const SHA256_DIR: &str = "sha256";
/// The untrusted staging area for in-flight downloads.
const STAGING_DIR: &str = "staging";

/// A verified artifact resting in the launcher-managed cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArtifact {
    pub path: PathBuf,
    pub sha256: ArtifactDigest,
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

/// The launcher-managed, content-addressed verified-artifact cache.
#[derive(Debug)]
pub struct ArtifactCache {
    managed: ManagedPaths,
    staging_sequence: AtomicU64,
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

    /// The untrusted staging area for in-flight downloads.
    pub fn staging_dir(&self) -> PathBuf {
        self.managed.cache_dir().join(STAGING_DIR)
    }

    /// The verified location of one artifact. Derived purely from the
    /// validated digest, so the result always stays inside the store.
    pub fn verified_path(&self, digest: &ArtifactDigest) -> PathBuf {
        self.store_dir().join(digest.as_hex())
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

    /// [`ArtifactCache::acquire`] with explicit transport limits (used by
    /// tests to keep timeouts short).
    pub async fn acquire_with(
        &self,
        source: &ArtifactSource,
        options: &DownloadOptions,
    ) -> Result<VerifiedArtifact, AcquisitionError> {
        let verified_path = self.verified_path(source.sha256());

        match self.validate_existing(&verified_path, source).await? {
            Some(bytes) => {
                return Ok(VerifiedArtifact {
                    path: verified_path,
                    sha256: *source.sha256(),
                    bytes,
                    origin: ArtifactOrigin::CacheHit,
                });
            }
            None => {}
        }

        std::fs::create_dir_all(self.store_dir()).map_err(AcquisitionError::StoreIo)?;
        std::fs::create_dir_all(self.staging_dir()).map_err(AcquisitionError::StoreIo)?;
        let staging_path = self.new_staging_path();

        let downloaded = downloads::download(source, &staging_path, options)
            .await
            .map_err(AcquisitionError::Download)?;

        self.promote(&staging_path, &verified_path, source).await?;

        Ok(VerifiedArtifact {
            path: verified_path,
            sha256: *source.sha256(),
            bytes: downloaded.bytes,
            origin: ArtifactOrigin::Downloaded,
        })
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
        source: &ArtifactSource,
    ) -> Result<Option<u64>, AcquisitionError> {
        match verify_file(verified_path, source.sha256(), source.size_bytes()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(VerifyFileError::Mismatch(failure)) => {
                eprintln!(
                    "[aurora-launcher] verified-cache object {} is corrupt ({}); acquiring a verified replacement",
                    source.sha256(),
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
        source: &ArtifactSource,
    ) -> Result<(), AcquisitionError> {
        match tokio::fs::rename(staging_path, verified_path).await {
            Ok(()) => return Ok(()),
            Err(rename_error) => {
                match verify_file(verified_path, source.sha256(), source.size_bytes()) {
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
}
