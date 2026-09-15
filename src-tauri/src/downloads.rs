//! Transport for trusted artifact acquisition.
//!
//! The transport downloads already-validated artifact metadata into an
//! untrusted staging file while computing its digest and byte count in the
//! same streaming pass. It deliberately does not decide trust: the caller
//! (the verified cache) promotes a staging file only after the streaming
//! verification succeeds.
//!
//! Transport policy:
//!
//! - Production artifact URLs must use HTTPS; cleartext HTTP is accepted only
//!   for explicit loopback hosts (`127.0.0.1`, `::1`, `localhost`), which is
//!   the launcher's scoped test-transport path and never a production source.
//! - Redirects are followed with a hard limit. An HTTPS request is never
//!   downgraded to an insecure scheme, and a loopback-HTTP request is never
//!   redirected off the loopback host in cleartext.
//! - Connection setup and idle reads are bounded; there is no overall
//!   time limit because large artifacts on slow links are legitimate.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use url::Url;

use crate::integrity::{
    ArtifactDigest, InvalidDigest, InvalidSha1Digest, Sha1Digest, StreamingSha1Verifier,
    StreamingVerifier, VerificationFailure,
};

/// Default bound on redirect hops the transport is willing to follow.
pub const MAX_REDIRECTS: usize = 8;

/// Default connection-establishment timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Default maximum time allowed between body chunks once the response starts.
pub const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The single user agent the launcher presents to artifact hosts.
const USER_AGENT: &str = concat!("aurora-launcher/", env!("CARGO_PKG_VERSION"));

/// Validated metadata describing one artifact to acquire.
///
/// Construction is the trust boundary for transport input: the URL must be
/// HTTPS (or an explicitly loopback HTTP test source), and the SHA-256 digest
/// must be a canonical hexadecimal value. Remote file names and other
/// response metadata never influence local paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSource {
    url: Url,
    sha256: ArtifactDigest,
    size_bytes: Option<u64>,
}

impl ArtifactSource {
    /// Creates validated production metadata from an HTTPS artifact URL.
    pub fn https(
        url: &str,
        sha256: &str,
        size_bytes: Option<u64>,
    ) -> Result<Self, InvalidArtifactSource> {
        Self::build(url, sha256, size_bytes, HostPolicy::HttpsOnly)
    }

    /// Creates validated test metadata from a loopback HTTP URL.
    ///
    /// This is the launcher's explicit test transport path: it only accepts
    /// `http://` URLs pointing at `127.0.0.1`, `::1`, or `localhost`, so
    /// deterministic local tests never depend on public internet access and
    /// production URL rules stay strict.
    pub fn loopback_http_for_testing(
        url: &str,
        sha256: &str,
        size_bytes: Option<u64>,
    ) -> Result<Self, InvalidArtifactSource> {
        Self::build(url, sha256, size_bytes, HostPolicy::LoopbackHttpAllowed)
    }

    fn build(
        url: &str,
        sha256: &str,
        size_bytes: Option<u64>,
        host_policy: HostPolicy,
    ) -> Result<Self, InvalidArtifactSource> {
        let parsed = validate_transport_url(url, host_policy)?;

        if size_bytes == Some(0) {
            return Err(InvalidArtifactSource::InvalidSize);
        }

        Ok(Self {
            url: parsed,
            sha256: ArtifactDigest::parse(sha256).map_err(InvalidArtifactSource::InvalidDigest)?,
            size_bytes,
        })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn sha256(&self) -> &ArtifactDigest {
        &self.sha256
    }

    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidArtifactSource {
    UnparsableUrl,
    InsecureUrl(String),
    EmbeddedCredentials,
    InvalidDigest(InvalidDigest),
    InvalidSha1(InvalidSha1Digest),
    InvalidSize,
}

impl fmt::Display for InvalidArtifactSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnparsableUrl => write!(formatter, "the artifact URL is not a valid URL"),
            Self::InsecureUrl(url) => write!(
                formatter,
                "the artifact URL must use HTTPS ({url} is not a secure production source)"
            ),
            Self::EmbeddedCredentials => write!(
                formatter,
                "the artifact URL must not embed user credentials"
            ),
            Self::InvalidDigest(error) => {
                write!(formatter, "the artifact digest is invalid: {error}")
            }
            Self::InvalidSha1(error) => write!(
                formatter,
                "the official expected SHA-1 digest is invalid: {error}"
            ),
            Self::InvalidSize => write!(
                formatter,
                "the artifact size, when provided, must be greater than zero"
            ),
        }
    }
}

impl std::error::Error for InvalidArtifactSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidDigest(error) => Some(error),
            Self::InvalidSha1(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPolicy {
    HttpsOnly,
    LoopbackHttpAllowed,
}

/// Whether the URL host is an explicit loopback address.
pub(crate) fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(address)) => address == std::net::Ipv6Addr::LOCALHOST,
        None => false,
    }
}

/// Tunable transport limits. Tests use shorter timeouts; production uses the
/// documented defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOptions {
    pub connect_timeout: Duration,
    pub idle_read_timeout: Duration,
    pub max_redirects: usize,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            connect_timeout: CONNECT_TIMEOUT,
            idle_read_timeout: IDLE_READ_TIMEOUT,
            max_redirects: MAX_REDIRECTS,
        }
    }
}

/// The measured properties of a fully streamed (but not yet promoted)
/// artifact file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedFile {
    pub bytes: u64,
    pub sha256: ArtifactDigest,
}

/// Streams the artifact described by `source` into `destination`, verifying
/// size and digest while streaming.
///
/// `destination` is an untrusted staging file: on any failure the partial
/// file is removed before this function returns, so a failed download never
/// leaves debris that could later be mistaken for a verified artifact.
pub async fn download(
    source: &ArtifactSource,
    destination: &Path,
    options: &DownloadOptions,
) -> Result<DownloadedFile, DownloadError> {
    let response = send_request(source.url(), options).await?;
    check_declared_length(&response, source.size_bytes())?;

    let mut verifier = StreamingVerifier::new(source.size_bytes());
    stream_body(response, destination, |chunk| verifier.update(chunk)).await?;

    let bytes = match verifier.finish(source.sha256()) {
        Ok(bytes) => bytes,
        Err(failure) => {
            let _ = tokio::fs::remove_file(destination).await;
            return Err(failure.into());
        }
    };

    Ok(DownloadedFile {
        bytes,
        sha256: *source.sha256(),
    })
}

/// Validated metadata describing one official Mojang artifact: an HTTPS (or
/// explicit loopback test) URL, the official expected SHA-1, and the official
/// expected size when published.
///
/// This is the SHA-1 twin of [`ArtifactSource`]: same transport policy, same
/// construction trust boundary, different (officially expected) digest
/// algorithm. A SHA-1 source can never pose as a SHA-256 source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha1ArtifactSource {
    url: Url,
    sha1: Sha1Digest,
    size_bytes: Option<u64>,
}

impl Sha1ArtifactSource {
    /// Creates validated production metadata from an HTTPS artifact URL.
    pub fn https(
        url: &str,
        sha1: &str,
        size_bytes: Option<u64>,
    ) -> Result<Self, InvalidArtifactSource> {
        Self::build(url, sha1, size_bytes, HostPolicy::HttpsOnly)
    }

    /// Creates validated test metadata from a loopback HTTP URL (the
    /// launcher's documented test-transport path).
    pub fn loopback_http_for_testing(
        url: &str,
        sha1: &str,
        size_bytes: Option<u64>,
    ) -> Result<Self, InvalidArtifactSource> {
        Self::build(url, sha1, size_bytes, HostPolicy::LoopbackHttpAllowed)
    }

    /// Creates validated metadata accepting the documented transport policy:
    /// a production HTTPS URL, or an explicit loopback HTTP test URL. This is
    /// the constructor plan-consuming executors use when the plan's URL has
    /// already been policy-validated at planning time.
    pub fn https_or_loopback(
        url: &str,
        sha1: &str,
        size_bytes: Option<u64>,
    ) -> Result<Self, InvalidArtifactSource> {
        Self::build(url, sha1, size_bytes, HostPolicy::LoopbackHttpAllowed)
    }

    fn build(
        url: &str,
        sha1: &str,
        size_bytes: Option<u64>,
        host_policy: HostPolicy,
    ) -> Result<Self, InvalidArtifactSource> {
        let url = validate_transport_url(url, host_policy)?;
        if size_bytes == Some(0) {
            return Err(InvalidArtifactSource::InvalidSize);
        }
        Ok(Self {
            url,
            sha1: Sha1Digest::parse(sha1).map_err(InvalidArtifactSource::InvalidSha1)?,
            size_bytes,
        })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn sha1(&self) -> &Sha1Digest {
        &self.sha1
    }

    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}

/// The measured properties of a fully streamed (but not yet promoted)
/// SHA-1-verified artifact file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedSha1File {
    pub bytes: u64,
    pub sha1: Sha1Digest,
}

/// Streams one official Mojang artifact into `destination`, verifying size
/// and the official expected SHA-1 while streaming.
///
/// Same staging contract as [`download`]: a failed transfer removes its
/// partial file and never leaves debris.
pub async fn download_sha1(
    source: &Sha1ArtifactSource,
    destination: &Path,
    options: &DownloadOptions,
) -> Result<DownloadedSha1File, DownloadError> {
    let response = send_request(source.url(), options).await?;
    check_declared_length(&response, source.size_bytes())?;

    let mut verifier = StreamingSha1Verifier::new(source.size_bytes());
    stream_body(response, destination, |chunk| verifier.update(chunk)).await?;

    let bytes = match verifier.finish(source.sha1()) {
        Ok(bytes) => bytes,
        Err(failure) => {
            let _ = tokio::fs::remove_file(destination).await;
            return Err(failure.into());
        }
    };

    Ok(DownloadedSha1File {
        bytes,
        sha1: *source.sha1(),
    })
}

/// A digest-less artifact source: a URL with no published expected digest,
/// acquired purely over the secure transport.
///
/// This exists for the Fabric artifacts official metadata adds without
/// digests (the loader and the intermediary). Transport policy is unchanged —
/// production HTTPS only, loopback cleartext only for deterministic tests —
/// but there is no expected digest to verify against; a size limit is the
/// only transfer bound, and the SHA-256 is *computed* locally as an observed
/// identity rather than checked against an official expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedArtifactSource {
    url: Url,
    size_limit_bytes: u64,
}

impl ObservedArtifactSource {
    /// The transfer bound for digest-less acquisitions. Fabric loader-sized
    /// artifacts are a few megabytes; the bound exists to keep a broken or
    /// hostile endpoint from streaming unbounded bytes, not to describe the
    /// artifact.
    pub const DEFAULT_SIZE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

    pub fn https(url: &str) -> Result<Self, InvalidArtifactSource> {
        Self::build(url, HostPolicy::HttpsOnly)
    }

    pub fn loopback_http_for_testing(url: &str) -> Result<Self, InvalidArtifactSource> {
        Self::build(url, HostPolicy::LoopbackHttpAllowed)
    }

    /// Creates validated metadata accepting the documented transport policy:
    /// production HTTPS, or an explicit loopback HTTP test URL.
    pub fn https_or_loopback(url: &str) -> Result<Self, InvalidArtifactSource> {
        Self::build(url, HostPolicy::LoopbackHttpAllowed)
    }

    fn build(url: &str, host_policy: HostPolicy) -> Result<Self, InvalidArtifactSource> {
        Ok(Self {
            url: validate_transport_url(url, host_policy)?,
            size_limit_bytes: Self::DEFAULT_SIZE_LIMIT_BYTES,
        })
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn size_limit_bytes(&self) -> u64 {
        self.size_limit_bytes
    }
}

/// The measured properties of a digest-less artifact acquired over secure
/// transport: the byte count and the locally observed SHA-256 identity.
///
/// `observed_sha256` is an observation, not a verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedDownload {
    pub bytes: u64,
    pub observed_sha256: ArtifactDigest,
}

/// Streams one digest-less artifact into `destination`, computing its
/// observed SHA-256 while streaming and enforcing the size limit.
///
/// `local_pin` is an optional *locally recorded* observed digest from a prior
/// acquisition of the same URL (a consistency reference, never an official
/// expectation). When present and the received bytes hash to a different
/// value, the transfer fails: content changing under a stable versioned URL
/// is exactly what the local consistency check exists to surface.
pub async fn download_observed(
    source: &ObservedArtifactSource,
    local_pin: Option<&ArtifactDigest>,
    destination: &Path,
    options: &DownloadOptions,
) -> Result<ObservedDownload, DownloadError> {
    let response = send_request(source.url(), options).await?;
    let limit = source.size_limit_bytes();
    // No exact expected size exists for a digest-less artifact; only reject a
    // server-declared length that already exceeds the transfer bound.
    if let Some(declared) = response.content_length() {
        if declared > limit {
            return Err(DownloadError::SizeMismatch {
                expected: limit,
                actual: declared,
            });
        }
    }

    let mut verifier = StreamingVerifier::new(None);
    let mut streamed = 0u64;
    stream_body(response, destination, |chunk| {
        streamed += chunk.len() as u64;
        if streamed > limit {
            return Err(VerificationFailure::SizeMismatch {
                expected: limit,
                actual: streamed,
            });
        }
        verifier.update(chunk)
    })
    .await?;

    let (bytes, observed) = verifier.finish_observed();
    if let Some(pinned) = local_pin {
        if &observed != pinned {
            let _ = tokio::fs::remove_file(destination).await;
            return Err(DownloadError::ObservedDigestDrift {
                url: source.url().to_string(),
                recorded: pinned.as_hex(),
                received: observed.as_hex(),
            });
        }
    }

    Ok(ObservedDownload {
        bytes,
        observed_sha256: observed,
    })
}

/// Sends the GET request and enforces the success status.
async fn send_request(
    url: &Url,
    options: &DownloadOptions,
) -> Result<reqwest::Response, DownloadError> {
    let client = build_client(options);
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(DownloadError::from_transport)?;

    let status = response.status();
    if !status.is_success() {
        return Err(DownloadError::HttpStatus {
            status: status.as_u16(),
        });
    }

    Ok(response)
}

/// A server-declared length that already disagrees with the expected value is
/// detected before a single byte is written.
fn check_declared_length(
    response: &reqwest::Response,
    expected_size: Option<u64>,
) -> Result<(), DownloadError> {
    if let (Some(expected), Some(declared)) = (expected_size, response.content_length()) {
        if declared != expected {
            return Err(DownloadError::SizeMismatch {
                expected,
                actual: declared,
            });
        }
    }
    Ok(())
}

/// Streams the response body into the staging file while feeding the caller's
/// verification closure, then flushes completely to disk.
///
/// On any failure the partial staging file is removed before returning, so a
/// failed transfer never leaves debris.
async fn stream_body(
    response: reqwest::Response,
    destination: &Path,
    mut verify: impl FnMut(&[u8]) -> Result<(), VerificationFailure>,
) -> Result<(), DownloadError> {
    match stream_body_inner(response, destination, &mut verify).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = tokio::fs::remove_file(destination).await;
            Err(error)
        }
    }
}

async fn stream_body_inner(
    response: reqwest::Response,
    destination: &Path,
    mut verify: impl FnMut(&[u8]) -> Result<(), VerificationFailure>,
) -> Result<(), DownloadError> {
    use tokio::io::AsyncWriteExt;

    let mut response = response;
    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(DownloadError::StagingIo)?;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(DownloadError::from_transport)?
    {
        verify(&chunk)?;
        file.write_all(&chunk)
            .await
            .map_err(DownloadError::StagingIo)?;
    }

    // The staging file must be completely on disk before it can be promoted.
    if let Err(error) = file.sync_all().await {
        return Err(DownloadError::StagingIo(error));
    }

    Ok(())
}

/// Parses and policy-checks one transport URL against the host policy.
fn validate_transport_url(
    url: &str,
    host_policy: HostPolicy,
) -> Result<Url, InvalidArtifactSource> {
    let parsed = Url::parse(url).map_err(|_| InvalidArtifactSource::UnparsableUrl)?;
    if parsed.cannot_be_a_base() {
        return Err(InvalidArtifactSource::UnparsableUrl);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(InvalidArtifactSource::EmbeddedCredentials);
    }

    match host_policy {
        HostPolicy::HttpsOnly => {
            if parsed.scheme() != "https" {
                return Err(InvalidArtifactSource::InsecureUrl(parsed.to_string()));
            }
        }
        HostPolicy::LoopbackHttpAllowed => {
            let loopback_http = parsed.scheme() == "http" && is_loopback_host(&parsed);
            if !loopback_http && parsed.scheme() != "https" {
                return Err(InvalidArtifactSource::InsecureUrl(parsed.to_string()));
            }
        }
    }

    Ok(parsed)
}

/// Builds the launcher's HTTP client with the documented transport policy
/// (user agent, timeouts, redirect rules, rustls provider).
///
/// Shared by the artifact transport and the Minecraft metadata fetch so every
/// outbound request obeys one policy; it confers no trust by itself.
pub(crate) fn build_client(options: &DownloadOptions) -> reqwest::Client {
    ensure_rustls_crypto_provider();

    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(options.connect_timeout)
        .read_timeout(options.idle_read_timeout)
        .redirect(redirect_policy(options.max_redirects))
        .build()
        .expect("download client configuration is valid")
}

/// Installs the rustls crypto provider reqwest requires.
///
/// The launcher pins the `ring` provider because it builds with a plain C
/// compiler everywhere, unlike the default aws-lc-rs provider which needs
/// CMake/NASM on some hosts. Installation is idempotent; a provider already
/// installed by another component is left in place.
fn ensure_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn redirect_policy(max_redirects: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let original = attempt
            .previous()
            .first()
            .expect("redirect attempts always include the original URL");
        let hops_taken = attempt.previous().len().saturating_sub(1);

        match decide_redirect(original, attempt.url(), hops_taken, max_redirects) {
            RedirectDecision::Follow => attempt.follow(),
            RedirectDecision::Refuse(reason) => attempt.error(RedirectPolicyViolation { reason }),
        }
    })
}

/// The pure redirect rules, separated from reqwest so they are deterministically
/// testable without a TLS endpoint.
pub(crate) fn decide_redirect(
    original: &Url,
    candidate: &Url,
    hops_taken: usize,
    max_redirects: usize,
) -> RedirectDecision {
    if hops_taken >= max_redirects {
        return RedirectDecision::Refuse(RedirectRefusal::LimitExceeded {
            limit: max_redirects,
        });
    }

    let candidate_https = candidate.scheme() == "https";
    if original.scheme() == "https" {
        if !candidate_https {
            return RedirectDecision::Refuse(RedirectRefusal::InsecureDowngrade {
                to: candidate.to_string(),
            });
        }
    } else {
        // Cleartext sources only exist for loopback test transports; they may
        // stay on the loopback host in cleartext or upgrade to HTTPS, but must
        // never be redirected to another insecure host.
        let stays_on_loopback_http = candidate.scheme() == "http" && is_loopback_host(candidate);
        if !candidate_https && !stays_on_loopback_http {
            return RedirectDecision::Refuse(RedirectRefusal::LeftLoopbackCleartext {
                to: candidate.to_string(),
            });
        }
    }

    RedirectDecision::Follow
}

#[derive(Debug)]
pub(crate) enum RedirectDecision {
    Follow,
    Refuse(RedirectRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectRefusal {
    LimitExceeded { limit: usize },
    InsecureDowngrade { to: String },
    LeftLoopbackCleartext { to: String },
}

impl fmt::Display for RedirectRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded { limit } => {
                write!(formatter, "the server redirected more than {limit} times")
            }
            Self::InsecureDowngrade { to } => write!(
                formatter,
                "the server redirected a secure download to an insecure location ({to}); the download was stopped"
            ),
            Self::LeftLoopbackCleartext { to } => write!(
                formatter,
                "a local test download was redirected to an insecure remote location ({to}); the download was stopped"
            ),
        }
    }
}

/// The error attached to a refused redirect so the transport failure can be
/// reported with its specific reason.
#[derive(Debug)]
struct RedirectPolicyViolation {
    reason: RedirectRefusal,
}

impl fmt::Display for RedirectPolicyViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.reason)
    }
}

impl std::error::Error for RedirectPolicyViolation {}

/// A failed transfer. Every variant fails closed: the caller must treat the
/// staged bytes as untrusted debris.
#[derive(Debug)]
pub enum DownloadError {
    /// The host could not be reached or the connection broke mid-transfer.
    Network(reqwest::Error),
    /// A configured time limit was exceeded.
    Timeout(reqwest::Error),
    /// A redirect could not be followed within the transport policy.
    Redirect(RedirectRefusal),
    /// The server answered with a non-success status.
    HttpStatus { status: u16 },
    /// The received (or declared) byte count differs from the expected size.
    SizeMismatch { expected: u64, actual: u64 },
    /// The received bytes do not hash to the expected digest.
    Sha256Mismatch { expected: String, actual: String },
    /// The received bytes do not hash to the official expected SHA-1.
    Sha1Mismatch { expected: String, actual: String },
    /// A digest-less artifact's content no longer matches the locally
    /// recorded observation from a prior acquisition of the same URL.
    ObservedDigestDrift {
        url: String,
        recorded: String,
        received: String,
    },
    /// Writing or flushing the staging file failed.
    StagingIo(std::io::Error),
}

impl DownloadError {
    pub(crate) fn from_transport(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            return Self::Timeout(error);
        }

        if error.is_redirect() {
            let reason =
                find_redirect_refusal(&error).unwrap_or_else(|| RedirectRefusal::LimitExceeded {
                    limit: MAX_REDIRECTS,
                });
            return Self::Redirect(reason);
        }

        Self::Network(error)
    }

    pub(crate) fn from_verification(failure: VerificationFailure) -> Self {
        match failure {
            VerificationFailure::SizeMismatch { expected, actual } => {
                Self::SizeMismatch { expected, actual }
            }
            VerificationFailure::Sha256Mismatch { expected, actual } => {
                Self::Sha256Mismatch { expected, actual }
            }
            VerificationFailure::Sha1Mismatch { expected, actual } => {
                Self::Sha1Mismatch { expected, actual }
            }
        }
    }
}

impl From<VerificationFailure> for DownloadError {
    fn from(failure: VerificationFailure) -> Self {
        Self::from_verification(failure)
    }
}

/// Walks the reqwest error source chain to recover the specific redirect
/// refusal attached by the transport policy.
fn find_redirect_refusal(error: &reqwest::Error) -> Option<RedirectRefusal> {
    let mut source = std::error::Error::source(error);
    while let Some(current) = source {
        if let Some(violation) = current.downcast_ref::<RedirectPolicyViolation>() {
            return Some(violation.reason.clone());
        }
        source = current.source();
    }
    None
}

impl fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => {
                write!(formatter, "the artifact host could not be reached: {error}")
            }
            Self::Timeout(error) => write!(
                formatter,
                "the artifact download did not complete in time: {error}"
            ),
            Self::Redirect(reason) => write!(formatter, "{reason}"),
            Self::HttpStatus { status } => write!(
                formatter,
                "the artifact host returned an unusable response (HTTP {status})"
            ),
            Self::SizeMismatch { expected, actual } => write!(
                formatter,
                "the downloaded artifact has the wrong size: expected {expected} bytes but received {actual} bytes"
            ),
            Self::Sha256Mismatch { expected, actual } => write!(
                formatter,
                "the downloaded artifact does not match its expected SHA-256 digest: expected {expected} but computed {actual}"
            ),
            Self::Sha1Mismatch { expected, actual } => write!(
                formatter,
                "the downloaded artifact does not match its official expected SHA-1 digest: expected {expected} but computed {actual}"
            ),
            Self::ObservedDigestDrift {
                url,
                recorded,
                received,
            } => write!(
                formatter,
                "the artifact at {url} no longer matches the SHA-256 observed when it was first acquired over secure transport: recorded {recorded} but received {received}; versioned artifact content should not change"
            ),
            Self::StagingIo(error) => write!(
                formatter,
                "the artifact could not be written to launcher-managed storage: {error}"
            ),
        }
    }
}

impl std::error::Error for DownloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Network(error) | Self::Timeout(error) => Some(error),
            Self::StagingIo(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestRequest, TestResponse, TestServer};
    use sha2::{Digest as _, Sha256};
    use std::sync::Arc;
    use std::thread;

    fn digest_of(bytes: &[u8]) -> ArtifactDigest {
        ArtifactDigest::from_sha256(Sha256::digest(bytes).into())
    }

    fn serve(handler: impl Fn(&TestRequest) -> TestResponse + Send + Sync + 'static) -> TestServer {
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

    fn staging_path(test: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir()
            .join("aurora-downloads-test")
            .join(std::process::id().to_string())
            .join(test);
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("artifact.part")
    }

    fn short_options() -> DownloadOptions {
        DownloadOptions {
            connect_timeout: Duration::from_secs(5),
            idle_read_timeout: Duration::from_millis(500),
            max_redirects: 3,
        }
    }

    #[tokio::test]
    async fn downloads_streamed_bytes_and_reports_the_measured_size_and_digest() {
        let body = b"artifact payload for transport tests".repeat(64);
        let expected_digest = digest_of(&body);
        let expected_body = body.clone();
        let server = serve(move |_request| TestResponse::ok(&body));
        let destination = staging_path("happy-path");
        let source = source_for(&server, "/aurora.jar", &expected_digest, None);

        let downloaded = download(&source, &destination, &short_options())
            .await
            .expect("download must succeed");

        assert_eq!(downloaded.bytes, expected_body.len() as u64);
        assert_eq!(downloaded.sha256, expected_digest);
        assert_eq!(std::fs::read(&destination).unwrap(), expected_body);
    }

    #[tokio::test]
    async fn a_non_success_status_fails_before_any_file_is_created() {
        let server = serve(|_request| TestResponse::status(404));
        let destination = staging_path("http-status");
        let source = source_for(&server, "/missing.jar", &digest_of(b"x"), None);

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("a 404 must fail");

        assert!(matches!(error, DownloadError::HttpStatus { status: 404 }));
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn an_unreachable_host_is_a_closed_transport_failure() {
        // Bind a port, learn its number, then drop the listener so nothing
        // accepts connections there anymore. Depending on the platform's
        // loopback behavior the refusal arrives as a connect error (Windows
        // occasionally drops the initial SYNs instead), so the deterministic
        // assertion is that the transfer fails closed with a transport-class
        // error and never touches local storage.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let url = format!("http://127.0.0.1:{port}/aurora.jar");
        let source =
            ArtifactSource::loopback_http_for_testing(&url, &digest_of(b"x").as_hex(), None)
                .unwrap();
        let destination = staging_path("connection-refused");

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("a dead endpoint must fail");

        assert!(
            matches!(error, DownloadError::Network(_) | DownloadError::Timeout(_)),
            "the transport failure must stay in a transport category, got: {error:?}"
        );
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn wrong_bytes_fail_the_digest_check_and_leave_no_staging_file() {
        let body = b"actual server bytes";
        let server = serve(move |_request| TestResponse::ok(body));
        let destination = staging_path("digest-mismatch");
        let source = source_for(
            &server,
            "/aurora.jar",
            &digest_of(b"digest of different bytes"),
            None,
        );

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("mismatched content must fail");

        assert!(matches!(error, DownloadError::Sha256Mismatch { .. },));
        assert!(
            !destination.exists(),
            "failed staging files must be removed"
        );
    }

    #[tokio::test]
    async fn a_wrong_expected_size_fails_closed() {
        let body = b"0123456789";
        let server = serve(move |_request| TestResponse::ok(body));
        let destination = staging_path("size-mismatch");
        let source = source_for(
            &server,
            "/aurora.jar",
            &digest_of(body),
            Some(body.len() as u64 + 1),
        );

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("a short body must fail the size check");

        assert!(matches!(error, DownloadError::SizeMismatch { .. }));
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn a_close_delimited_body_that_ends_early_fails_the_size_check() {
        let body = b"complete only when the connection says so".to_vec();
        let expected_digest = digest_of(&body);
        let expected_size = body.len() as u64 + 1;
        let server = serve(move |_request| {
            // No Content-Length: the body is delimited by closing the
            // connection, so the transfer "ends cleanly" with fewer bytes
            // than the manifest expects.
            TestResponse::ok(&body).with_close_framing()
        });
        let destination = staging_path("close-framing-short");
        let source = source_for(
            &server,
            "/aurora.jar",
            &expected_digest,
            Some(expected_size),
        );

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("an undersized close-delimited body must fail");

        assert!(matches!(error, DownloadError::SizeMismatch { .. }));
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn a_truncated_response_is_rejected() {
        let body: Vec<u8> = (0..=255u8).collect();
        let expected_digest = digest_of(&body);
        let kept = body.len() / 2;
        let server = serve(move |_request| TestResponse::ok(&body).with_truncated_body(kept));
        let destination = staging_path("truncated");
        let source = source_for(&server, "/aurora.jar", &expected_digest, None);

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("a prematurely closed transfer must fail");

        // The connection breaks mid-body before the size check can run, so
        // the transport reports the broken stream; either way it fails closed.
        assert!(matches!(error, DownloadError::Network(_)));
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn a_stalled_host_is_reported_as_a_timeout() {
        let server = serve(|_request| {
            thread::sleep(Duration::from_millis(2000));
            TestResponse::ok(b"too late")
        });
        let destination = staging_path("timeout");
        let source = source_for(&server, "/aurora.jar", &digest_of(b"too late"), None);

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("a stalled response must time out");

        assert!(matches!(error, DownloadError::Timeout(_)));
    }

    #[tokio::test]
    async fn redirects_within_the_limit_are_followed() {
        let body = b"after three hops";
        let server = serve(move |request| match request.path.as_str() {
            "/start" => TestResponse::redirect_to(format!("{}/hop-1", request.base_url)),
            "/hop-1" => TestResponse::redirect_to(format!("{}/hop-2", request.base_url)),
            "/hop-2" => TestResponse::redirect_to(format!("{}/final", request.base_url)),
            _ => TestResponse::ok(body),
        });
        let destination = staging_path("redirect-followed");
        let source = source_for(&server, "/start", &digest_of(body), None);

        let downloaded = download(&source, &destination, &short_options())
            .await
            .expect("redirects within the limit must be followed");

        assert_eq!(downloaded.bytes, body.len() as u64);
        assert_eq!(server.request_count(), 4);
    }

    #[tokio::test]
    async fn redirects_beyond_the_limit_are_refused() {
        let server = serve(|request| {
            TestResponse::redirect_to(format!("{}/hop{}", request.base_url, request.path.len()))
        });
        let destination = staging_path("redirect-limit");
        let source = source_for(&server, "/hop0", &digest_of(b"never served"), None);

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("endless redirects must be refused");

        assert!(matches!(
            error,
            DownloadError::Redirect(RedirectRefusal::LimitExceeded { limit: 3 })
        ));
    }

    #[tokio::test]
    async fn a_loopback_source_cannot_be_redirected_to_an_insecure_remote() {
        let server = serve(|_request| {
            TestResponse::redirect_to("http://public.example.invalid/evil.jar".to_owned())
        });
        let destination = staging_path("loopback-escape");
        let source = source_for(&server, "/trap", &digest_of(b"never served"), None);

        let error = download(&source, &destination, &short_options())
            .await
            .expect_err("the loopback trampoline must be refused");

        assert!(matches!(
            error,
            DownloadError::Redirect(RedirectRefusal::LeftLoopbackCleartext { .. })
        ));
    }

    #[tokio::test]
    async fn remote_file_names_never_influence_the_local_destination() {
        let body = b"bytes served under a hostile name";
        let server = serve(move |_request| {
            TestResponse::ok(body).with_header(
                "Content-Disposition",
                "attachment; filename=\"..\\..\\evil name.exe\"",
            )
        });
        let destination = staging_path("hostile-name");
        let source = source_for(&server, "/whatever", &digest_of(body), None);

        download(&source, &destination, &short_options())
            .await
            .expect("a hostile file name must not break the download");

        assert_eq!(std::fs::read(&destination).unwrap(), body);
    }

    #[test]
    fn https_sources_require_the_https_scheme() {
        let digest = digest_of(b"x").as_hex();

        assert!(matches!(
            ArtifactSource::https("http://releases.example.invalid/aurora.jar", &digest, None),
            Err(InvalidArtifactSource::InsecureUrl(_))
        ));
        assert!(matches!(
            ArtifactSource::https("ftp://releases.example.invalid/aurora.jar", &digest, None),
            Err(InvalidArtifactSource::InsecureUrl(_))
        ));
        assert!(
            ArtifactSource::https("https://releases.example.invalid/aurora.jar", &digest, None)
                .is_ok()
        );
    }

    #[test]
    fn cleartext_sources_are_accepted_only_for_explicit_loopback_hosts() {
        let digest = digest_of(b"x").as_hex();

        for url in [
            "http://127.0.0.1:9000/aurora.jar",
            "http://localhost:9000/aurora.jar",
            "http://[::1]:9000/aurora.jar",
        ] {
            assert!(
                ArtifactSource::loopback_http_for_testing(url, &digest, None).is_ok(),
                "{url} is a loopback test source"
            );
        }

        for url in [
            "http://releases.example.invalid/aurora.jar",
            "http://192.168.1.10/aurora.jar",
            "http://127.0.0.1.example.invalid/aurora.jar",
        ] {
            assert!(
                matches!(
                    ArtifactSource::loopback_http_for_testing(url, &digest, None),
                    Err(InvalidArtifactSource::InsecureUrl(_))
                ),
                "{url} must not count as a loopback test source"
            );
        }
    }

    #[test]
    fn sources_reject_credentials_bad_digests_and_zero_sizes() {
        let digest = digest_of(b"x").as_hex();

        assert!(matches!(
            ArtifactSource::https(
                "https://user:secret@releases.example.invalid/x",
                &digest,
                None
            ),
            Err(InvalidArtifactSource::EmbeddedCredentials)
        ));
        assert!(matches!(
            ArtifactSource::https("not a url at all", &digest, None),
            Err(InvalidArtifactSource::UnparsableUrl)
        ));
        assert!(matches!(
            ArtifactSource::https("https://releases.example.invalid/x", "deadbeef", None),
            Err(InvalidArtifactSource::InvalidDigest(_))
        ));
        assert!(matches!(
            ArtifactSource::https("https://releases.example.invalid/x", &digest, Some(0)),
            Err(InvalidArtifactSource::InvalidSize)
        ));
    }

    #[test]
    fn redirect_rules_refuse_https_downgrades() {
        let original = Url::parse("https://releases.example.invalid/aurora.jar").unwrap();
        let insecure = Url::parse("http://releases.example.invalid/aurora.jar").unwrap();
        let secure = Url::parse("https://mirror.example.invalid/aurora.jar").unwrap();

        assert!(matches!(
            decide_redirect(&original, &insecure, 0, MAX_REDIRECTS),
            RedirectDecision::Refuse(RedirectRefusal::InsecureDowngrade { .. })
        ));
        assert!(matches!(
            decide_redirect(&original, &secure, 4, MAX_REDIRECTS),
            RedirectDecision::Follow
        ));
    }

    #[test]
    fn redirect_rules_enforce_the_hop_limit() {
        let original = Url::parse("https://releases.example.invalid/a").unwrap();
        let next = Url::parse("https://releases.example.invalid/b").unwrap();

        assert!(matches!(
            decide_redirect(&original, &next, 8, MAX_REDIRECTS),
            RedirectDecision::Refuse(RedirectRefusal::LimitExceeded { limit: 8 })
        ));
        assert!(matches!(
            decide_redirect(&original, &next, 7, MAX_REDIRECTS),
            RedirectDecision::Follow
        ));
    }

    #[test]
    fn redirect_rules_keep_cleartext_sources_on_the_loopback() {
        let original = Url::parse("http://127.0.0.1:9000/a").unwrap();
        let loopback = Url::parse("http://localhost:9001/b").unwrap();
        let escape = Url::parse("http://public.example.invalid/b").unwrap();
        let upgrade = Url::parse("https://releases.example.invalid/b").unwrap();

        assert!(matches!(
            decide_redirect(&original, &loopback, 0, MAX_REDIRECTS),
            RedirectDecision::Follow
        ));
        assert!(matches!(
            decide_redirect(&original, &upgrade, 0, MAX_REDIRECTS),
            RedirectDecision::Follow
        ));
        assert!(matches!(
            decide_redirect(&original, &escape, 0, MAX_REDIRECTS),
            RedirectDecision::Refuse(RedirectRefusal::LeftLoopbackCleartext { .. })
        ));
    }

    fn sha1_of(bytes: &[u8]) -> Sha1Digest {
        Sha1Digest::compute(bytes)
    }

    fn sha1_source_for(server: &TestServer, path: &str, sha1: &Sha1Digest) -> Sha1ArtifactSource {
        Sha1ArtifactSource::loopback_http_for_testing(
            &format!("{}{path}", server.base_url()),
            &sha1.as_hex(),
            None,
        )
        .expect("test source must be valid")
    }

    #[tokio::test]
    async fn sha1_downloads_verify_the_official_digest_or_fail_closed() {
        let body = b"official mojang bytes".to_vec();
        let expected = sha1_of(&body);
        let byte_count = body.len() as u64;
        let server = serve(move |_request| TestResponse::ok(&body));
        let destination = staging_path("sha1-happy");

        let source = sha1_source_for(&server, "/client.jar", &expected);
        let downloaded = download_sha1(&source, &destination, &short_options())
            .await
            .expect("official bytes must verify");
        assert_eq!(downloaded.bytes, byte_count);
        assert_eq!(downloaded.sha1, expected);
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"official mojang bytes"
        );

        // Wrong expectation: hard failure, no debris.
        let wrong = sha1_source_for(&server, "/client.jar", &sha1_of(b"other bytes"));
        let error = download_sha1(&wrong, &destination, &short_options())
            .await
            .expect_err("a SHA-1 mismatch must fail");
        assert!(matches!(error, DownloadError::Sha1Mismatch { .. }));
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn sha1_downloads_enforce_the_declared_size() {
        let body = b"0123456789".to_vec();
        let expected = sha1_of(&body);
        let declared = body.len() as u64 + 1;
        let server = serve(move |_request| TestResponse::ok(&body));
        let destination = staging_path("sha1-size");

        let source = Sha1ArtifactSource::loopback_http_for_testing(
            &format!("{}/client.jar", server.base_url()),
            &expected.as_hex(),
            Some(declared),
        )
        .unwrap();

        let error = download_sha1(&source, &destination, &short_options())
            .await
            .expect_err("a short body must fail the size check");
        assert!(matches!(error, DownloadError::SizeMismatch { .. }));
        assert!(!destination.exists());
    }

    #[tokio::test]
    async fn observed_downloads_compute_their_identity_and_respect_pins() {
        let body = b"digest-less fabric loader bytes".to_vec();
        let expected_digest = digest_of(&body);
        let byte_count = body.len() as u64;
        let server = serve(move |_request| TestResponse::ok(&body));
        let destination = staging_path("observed-happy");

        let source = ObservedArtifactSource::loopback_http_for_testing(&format!(
            "{}/fabric-loader.jar",
            server.base_url()
        ))
        .unwrap();

        // Without a pin: acquisition succeeds and reports the observed digest.
        let first = download_observed(&source, None, &destination, &short_options())
            .await
            .expect("secure transport acquisition must succeed");
        assert_eq!(first.observed_sha256, expected_digest);
        assert_eq!(first.bytes, byte_count);

        // With the recorded pin: identical content passes as a consistency
        // check.
        let second = download_observed(
            &source,
            Some(&expected_digest),
            &destination,
            &short_options(),
        )
        .await
        .expect("identical content satisfies the local pin");
        assert_eq!(second.observed_sha256, expected_digest);

        // With a pin from different bytes: deliberate drift failure.
        let error = download_observed(
            &source,
            Some(&digest_of(b"other")),
            &destination,
            &short_options(),
        )
        .await
        .expect_err("drifted content must fail the local pin");
        assert!(matches!(error, DownloadError::ObservedDigestDrift { .. }));
        assert!(!destination.exists(), "failed staging files are removed");
    }

    #[tokio::test]
    async fn observed_downloads_enforce_the_transfer_size_limit() {
        let body = vec![7u8; 128];
        let server = serve(move |_request| TestResponse::ok(&body));
        let destination = staging_path("observed-limit");

        let url = format!("{}/loader.jar", server.base_url());
        let mut source = ObservedArtifactSource::loopback_http_for_testing(&url).unwrap();
        source.size_limit_bytes = 64;

        let error = download_observed(&source, None, &destination, &short_options())
            .await
            .expect_err("an oversized transfer must fail");
        assert!(matches!(error, DownloadError::SizeMismatch { .. }));
        assert!(!destination.exists());
    }

    #[test]
    fn sha1_and_observed_sources_share_the_transport_url_policy() {
        let sha1 = sha1_of(b"x").as_hex();

        assert!(
            Sha1ArtifactSource::https("http://piston-data.mojang.com/client.jar", &sha1, None)
                .is_err(),
            "production SHA-1 sources must be HTTPS"
        );
        assert!(
            Sha1ArtifactSource::https("https://piston-data.mojang.com/client.jar", &sha1, None)
                .is_ok()
        );
        assert!(
            Sha1ArtifactSource::https(
                "https://piston-data.mojang.com/client.jar",
                "deadbeef",
                None
            )
            .is_err()
        );

        assert!(ObservedArtifactSource::https("https://maven.fabricmc.net/loader.jar").is_ok());
        assert!(ObservedArtifactSource::https("http://maven.fabricmc.net/loader.jar").is_err());
        assert!(
            ObservedArtifactSource::loopback_http_for_testing("http://127.0.0.1:9/loader.jar")
                .is_ok()
        );
        assert!(
            ObservedArtifactSource::loopback_http_for_testing("http://example.invalid/x").is_err()
        );
    }
}
