//! Artifact digests and streaming integrity verification.
//!
//! Download completion does not imply artifact trust: data only becomes a
//! verified artifact when its exact byte size (when expected) and its SHA-256
//! digest match the values provided by trusted metadata. Hashing here is
//! streaming, so artifacts are never buffered into memory for verification.
//!
//! SHA-1 exists here strictly as the digest algorithm of official Minecraft
//! metadata (Mojang publishes SHA-1 values, not SHA-256). Aurora's own
//! artifacts and the verified cache remain SHA-256-addressed; a SHA-1 value
//! can never masquerade as a cache identity or an Aurora artifact digest.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use sha2::{Sha256, Sha512};

/// The SHA-256 digest is represented as exactly 64 hexadecimal characters.
pub const SHA256_HEX_LENGTH: usize = 64;

/// The SHA-1 digest is represented as exactly 40 hexadecimal characters.
pub const SHA1_HEX_LENGTH: usize = 40;
pub const SHA512_HEX_LENGTH: usize = 128;

/// A canonical expected SHA-512 supplied by a content provider.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha512Digest([u8; 64]);

impl Sha512Digest {
    pub fn parse(hex: &str) -> Result<Self, InvalidDigest> {
        if hex.len() != SHA512_HEX_LENGTH {
            return Err(InvalidDigest::InvalidLength(hex.len()));
        }
        let mut raw = [0; 64];
        for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_value(pair[0]).map_err(InvalidDigest::InvalidCharacter)?;
            let low = hex_value(pair[1]).map_err(InvalidDigest::InvalidCharacter)?;
            raw[index] = (high << 4) | low;
        }
        Ok(Self(raw))
    }

    pub fn as_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl fmt::Debug for Sha512Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Sha512Digest({})", self.as_hex())
    }
}

/// Recheck a cache object against both the provider expectation and its
/// locally computed SHA-256 address before it can be reused.
pub fn verify_file_sha512(
    path: &Path,
    expected: &Sha512Digest,
    expected_size: Option<u64>,
) -> Result<(u64, ArtifactDigest), VerifyFileError> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).map_err(VerifyFileError::Io)?;
    let mut sha512 = Sha512::new();
    let mut sha256 = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; STREAM_CHUNK_BYTES];
    loop {
        let count = file.read(&mut buffer).map_err(VerifyFileError::Io)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        sha512.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
    }
    if let Some(size) = expected_size {
        if size != bytes {
            return Err(VerifyFileError::Mismatch(
                VerificationFailure::SizeMismatch {
                    expected: size,
                    actual: bytes,
                },
            ));
        }
    }
    let actual: [u8; 64] = sha512.finalize().into();
    if &actual != expected.as_bytes() {
        return Err(VerifyFileError::Mismatch(
            VerificationFailure::Sha512Mismatch {
                expected: expected.as_hex(),
                actual: actual.iter().map(|byte| format!("{byte:02x}")).collect(),
            },
        ));
    }
    Ok((bytes, ArtifactDigest::from_sha256(sha256.finalize().into())))
}

/// Read/write chunk size used when streaming files for hashing.
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// A canonical SHA-256 digest of an artifact's exact bytes.
///
/// Parsing accepts any hexadecimal casing but stores one canonical form, so
/// digest comparisons are exact byte comparisons of canonical data rather
/// than string comparisons of manifest text.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArtifactDigest([u8; 32]);

impl ArtifactDigest {
    /// Creates a digest from raw SHA-256 output.
    pub fn from_sha256(raw: [u8; 32]) -> Self {
        Self(raw)
    }

    /// Parses a hexadecimal SHA-256 digest. Both cases are accepted; the
    /// canonical lowercase form is stored.
    pub fn parse(hex: &str) -> Result<Self, InvalidDigest> {
        if hex.len() != SHA256_HEX_LENGTH {
            return Err(InvalidDigest::InvalidLength(hex.len()));
        }

        let mut raw = [0u8; 32];
        for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_value(pair[0]).map_err(InvalidDigest::InvalidCharacter)?;
            let low = hex_value(pair[1]).map_err(InvalidDigest::InvalidCharacter)?;
            raw[index] = (high << 4) | low;
        }

        Ok(Self(raw))
    }

    /// The canonical lowercase hexadecimal representation.
    pub fn as_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(SHA256_HEX_LENGTH);
        for byte in &self.0 {
            hex.push(HEX[(byte >> 4) as usize] as char);
            hex.push(HEX[(byte & 0x0f) as usize] as char);
        }
        hex
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ArtifactDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl fmt::Debug for ArtifactDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ArtifactDigest({})", self.as_hex())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidDigest {
    InvalidLength(usize),
    InvalidCharacter(u8),
}

impl fmt::Display for InvalidDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(length) => write!(
                formatter,
                "artifact SHA-256 digest must be exactly {SHA256_HEX_LENGTH} hexadecimal characters, but is {length} characters"
            ),
            Self::InvalidCharacter(byte) => write!(
                formatter,
                "artifact SHA-256 digest must contain only hexadecimal characters, but contains '{}'",
                char::from(*byte).escape_default()
            ),
        }
    }
}

impl std::error::Error for InvalidDigest {}

/// A canonical SHA-1 digest as published by official Minecraft metadata.
///
/// Like [`ArtifactDigest`], parsing accepts any hexadecimal casing and stores
/// one canonical lowercase form, so comparisons are exact byte comparisons of
/// canonical data. SHA-1 values represent official Mojang expectations only;
/// they are weaker than SHA-256 and never become verified-cache identities.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha1Digest([u8; 20]);

impl Sha1Digest {
    /// Creates a digest from raw SHA-1 output.
    pub fn from_sha1(raw: [u8; 20]) -> Self {
        Self(raw)
    }

    /// Parses a hexadecimal SHA-1 digest. Both cases are accepted; the
    /// canonical lowercase form is stored.
    pub fn parse(hex: &str) -> Result<Self, InvalidSha1Digest> {
        if hex.len() != SHA1_HEX_LENGTH {
            return Err(InvalidSha1Digest::InvalidLength(hex.len()));
        }

        let mut raw = [0u8; 20];
        for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_value(pair[0]).map_err(InvalidSha1Digest::InvalidCharacter)?;
            let low = hex_value(pair[1]).map_err(InvalidSha1Digest::InvalidCharacter)?;
            raw[index] = (high << 4) | low;
        }

        Ok(Self(raw))
    }

    /// Computes the SHA-1 digest of a small in-memory document.
    ///
    /// Official metadata documents are the only users: they are fetched in
    /// memory and bounded in size, so a one-shot hash is appropriate here
    /// (product artifacts keep using the streaming verifier).
    pub fn compute(bytes: &[u8]) -> Self {
        Self(Sha1::digest(bytes).into())
    }

    /// The canonical lowercase hexadecimal representation.
    pub fn as_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(SHA1_HEX_LENGTH);
        for byte in &self.0 {
            hex.push(HEX[(byte >> 4) as usize] as char);
            hex.push(HEX[(byte & 0x0f) as usize] as char);
        }
        hex
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl fmt::Display for Sha1Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl fmt::Debug for Sha1Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Sha1Digest({})", self.as_hex())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidSha1Digest {
    InvalidLength(usize),
    InvalidCharacter(u8),
}

impl fmt::Display for InvalidSha1Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(length) => write!(
                formatter,
                "official metadata SHA-1 digest must be exactly {SHA1_HEX_LENGTH} hexadecimal characters, but is {length} characters"
            ),
            Self::InvalidCharacter(byte) => write!(
                formatter,
                "official metadata SHA-1 digest must contain only hexadecimal characters, but contains '{}'",
                char::from(*byte).escape_default()
            ),
        }
    }
}

impl std::error::Error for InvalidSha1Digest {}

fn hex_value(byte: u8) -> Result<u8, u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        other => Err(other),
    }
}

/// The digest algorithm an artifact's trust is anchored in.
///
/// SHA-1 is exclusively the algorithm of official Mojang expectations;
/// SHA-256 is the algorithm of Aurora's own distribution, Fabric's published
/// digests, and the verified cache's content addressing. The two never
/// exchange roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm {
    Sha256,
    Sha1,
}

impl fmt::Display for DigestAlgorithm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Sha256 => "sha256",
            Self::Sha1 => "sha1",
        })
    }
}

/// How an artifact's bytes came to be trusted, and against what.
///
/// The distinction is deliberate and load-bearing:
///
/// - `ExpectedDigestVerified` means a digest was known *before* acquisition
///   from trusted metadata (an Aurora/Fabric SHA-256 or a Mojang SHA-1) and
///   the received bytes were verified against it. This is cryptographic
///   content verification.
/// - `SecureTransportObserved` means no expected digest existed; the artifact
///   was acquired over verified-HTTPS transport and its SHA-256 was computed
///   locally as a stable identity. The digest is an observation, never an
///   externally verified expectation, and the artifact must never be reported
///   as digest-verified.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum ArtifactTrust {
    /// Verified against a pre-known expected digest from trusted metadata.
    ExpectedDigestVerified {
        algorithm: DigestAlgorithm,
        digest: String,
    },
    /// Acquired over secure transport with no published digest; the SHA-256
    /// was computed locally on first acquisition and is recorded as an
    /// observation (a local consistency reference, not official verification).
    SecureTransportObserved { observed_sha256: String },
}

impl ArtifactTrust {
    /// The persisted trust-class name (`expectedDigestVerified` or
    /// `secureTransportObserved`).
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::ExpectedDigestVerified { .. } => "expectedDigestVerified",
            Self::SecureTransportObserved { .. } => "secureTransportObserved",
        }
    }

    /// The canonical trust record for a verified SHA-256 artifact.
    pub fn verified_sha256(digest: &ArtifactDigest) -> Self {
        Self::ExpectedDigestVerified {
            algorithm: DigestAlgorithm::Sha256,
            digest: digest.as_hex(),
        }
    }

    /// The canonical trust record for a verified official Mojang SHA-1
    /// artifact.
    pub fn verified_sha1(digest: &Sha1Digest) -> Self {
        Self::ExpectedDigestVerified {
            algorithm: DigestAlgorithm::Sha1,
            digest: digest.as_hex(),
        }
    }

    /// The canonical trust record for a digest-less artifact acquired over
    /// secure transport.
    pub fn transport_observed(digest: &ArtifactDigest) -> Self {
        Self::SecureTransportObserved {
            observed_sha256: digest.as_hex(),
        }
    }
}

impl fmt::Display for ArtifactTrust {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedDigestVerified { algorithm, digest } => {
                write!(
                    formatter,
                    "verified against expected {algorithm} digest {digest}"
                )
            }
            Self::SecureTransportObserved { observed_sha256 } => write!(
                formatter,
                "secure transport with locally observed SHA-256 {observed_sha256} (no published digest)"
            ),
        }
    }
}

/// A failed integrity comparison. Every variant is a hard failure: the
/// artifact is untrusted and must never be promoted or activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationFailure {
    SizeMismatch { expected: u64, actual: u64 },
    Sha256Mismatch { expected: String, actual: String },
    Sha1Mismatch { expected: String, actual: String },
    Sha512Mismatch { expected: String, actual: String },
}

impl fmt::Display for VerificationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SizeMismatch { expected, actual } => write!(
                formatter,
                "artifact size does not match the expected size: expected {expected} bytes but received {actual} bytes"
            ),
            Self::Sha256Mismatch { expected, actual } => write!(
                formatter,
                "artifact SHA-256 digest does not match the expected digest: expected {expected} but computed {actual}"
            ),
            Self::Sha1Mismatch { expected, actual } => write!(
                formatter,
                "artifact SHA-1 digest does not match the official expected digest: expected {expected} but computed {actual}"
            ),
            Self::Sha512Mismatch { expected, actual } => write!(
                formatter,
                "artifact SHA-512 digest does not match the expected digest: expected {expected} but computed {actual}"
            ),
        }
    }
}

impl std::error::Error for VerificationFailure {}

/// Accumulates the SHA-256 digest and byte count of a streamed artifact and
/// enforces the expected size as the bytes arrive.
pub struct StreamingVerifier {
    hasher: Sha256,
    bytes: u64,
    expected_size: Option<u64>,
}

impl StreamingVerifier {
    pub fn new(expected_size: Option<u64>) -> Self {
        Self {
            hasher: Sha256::new(),
            bytes: 0,
            expected_size,
        }
    }

    /// Feeds the next chunk. Fails closed as soon as the stream grows past
    /// the expected size, so oversized downloads are stopped early.
    pub fn update(&mut self, chunk: &[u8]) -> Result<(), VerificationFailure> {
        if let Some(expected) = self.expected_size {
            let new_total = self.bytes + chunk.len() as u64;
            if new_total > expected {
                return Err(VerificationFailure::SizeMismatch {
                    expected,
                    actual: new_total,
                });
            }
        }

        self.hasher.update(chunk);
        self.bytes += chunk.len() as u64;
        Ok(())
    }

    /// The number of bytes streamed so far.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Completes verification: the streamed size must match the expected
    /// size (when one exists) and the computed digest must equal the
    /// expected digest. Returns the verified byte count.
    pub fn finish(self, expected: &ArtifactDigest) -> Result<u64, VerificationFailure> {
        if let Some(expected_size) = self.expected_size {
            if self.bytes != expected_size {
                return Err(VerificationFailure::SizeMismatch {
                    expected: expected_size,
                    actual: self.bytes,
                });
            }
        }

        let actual = ArtifactDigest::from_sha256(self.hasher.finalize().into());
        if &actual != expected {
            return Err(VerificationFailure::Sha256Mismatch {
                expected: expected.as_hex(),
                actual: actual.as_hex(),
            });
        }

        Ok(self.bytes)
    }

    /// Completes without any expected digest, returning the observed byte
    /// count and the locally computed SHA-256.
    ///
    /// For digest-less artifacts acquired over secure transport only: the
    /// returned digest is an observation recorded for stable local identity,
    /// never a verification against an external expectation.
    pub fn finish_observed(self) -> (u64, ArtifactDigest) {
        let digest = ArtifactDigest::from_sha256(self.hasher.finalize().into());
        (self.bytes, digest)
    }
}

/// Accumulates the SHA-1 digest and byte count of a streamed artifact and
/// enforces the expected size as the bytes arrive.
///
/// This is the Mojang-side twin of [`StreamingVerifier`]: same size
/// enforcement, same fail-closed behavior, different (officially expected)
/// digest algorithm. A SHA-1 verification result never becomes a
/// SHA-256-addressed cache identity.
pub struct StreamingSha1Verifier {
    hasher: Sha1,
    bytes: u64,
    expected_size: Option<u64>,
}

impl StreamingSha1Verifier {
    pub fn new(expected_size: Option<u64>) -> Self {
        Self {
            hasher: Sha1::new(),
            bytes: 0,
            expected_size,
        }
    }

    /// Feeds the next chunk. Fails closed as soon as the stream grows past
    /// the expected size.
    pub fn update(&mut self, chunk: &[u8]) -> Result<(), VerificationFailure> {
        if let Some(expected) = self.expected_size {
            let new_total = self.bytes + chunk.len() as u64;
            if new_total > expected {
                return Err(VerificationFailure::SizeMismatch {
                    expected,
                    actual: new_total,
                });
            }
        }

        self.hasher.update(chunk);
        self.bytes += chunk.len() as u64;
        Ok(())
    }

    /// The number of bytes streamed so far.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Completes verification against the official expected SHA-1 digest.
    pub fn finish(self, expected: &Sha1Digest) -> Result<u64, VerificationFailure> {
        if let Some(expected_size) = self.expected_size {
            if self.bytes != expected_size {
                return Err(VerificationFailure::SizeMismatch {
                    expected: expected_size,
                    actual: self.bytes,
                });
            }
        }

        let actual = Sha1Digest::from_sha1(self.hasher.finalize().into());
        if &actual != expected {
            return Err(VerificationFailure::Sha1Mismatch {
                expected: expected.as_hex(),
                actual: actual.as_hex(),
            });
        }

        Ok(self.bytes)
    }
}

/// The outcome of re-validating an existing file's bytes.
#[derive(Debug)]
pub enum VerifyFileError {
    Io(std::io::Error),
    Mismatch(VerificationFailure),
}

impl fmt::Display for VerifyFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(
                formatter,
                "the artifact file could not be read for verification: {error}"
            ),
            Self::Mismatch(failure) => write!(formatter, "{failure}"),
        }
    }
}

impl std::error::Error for VerifyFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Mismatch(failure) => Some(failure),
        }
    }
}

/// Re-validates an existing file by streaming its bytes through SHA-256.
///
/// An existing cache object is never trusted because of its file name; this
/// is the deliberate validation pass that decides whether the file is a
/// usable verified artifact.
pub fn verify_file(
    path: &Path,
    expected: &ArtifactDigest,
    expected_size: Option<u64>,
) -> Result<u64, VerifyFileError> {
    let mut file = std::fs::File::open(path).map_err(VerifyFileError::Io)?;
    let mut verifier = StreamingVerifier::new(expected_size);
    let mut chunk = vec![0u8; STREAM_CHUNK_BYTES];

    loop {
        let read = std::io::Read::read(&mut file, &mut chunk).map_err(VerifyFileError::Io)?;
        if read == 0 {
            break;
        }
        verifier
            .update(&chunk[..read])
            .map_err(VerifyFileError::Mismatch)?;
    }

    verifier.finish(expected).map_err(VerifyFileError::Mismatch)
}

/// Re-validates an existing file by streaming its bytes through SHA-1 against
/// an official Mojang expectation.
///
/// SHA-1 store objects are re-validated exactly like SHA-256 ones: never by
/// file name, always by re-hashing.
pub fn verify_file_sha1(
    path: &Path,
    expected: &Sha1Digest,
    expected_size: Option<u64>,
) -> Result<u64, VerifyFileError> {
    let mut file = std::fs::File::open(path).map_err(VerifyFileError::Io)?;
    let mut verifier = StreamingSha1Verifier::new(expected_size);
    let mut chunk = vec![0u8; STREAM_CHUNK_BYTES];

    loop {
        let read = std::io::Read::read(&mut file, &mut chunk).map_err(VerifyFileError::Io)?;
        if read == 0 {
            break;
        }
        verifier
            .update(&chunk[..read])
            .map_err(VerifyFileError::Mismatch)?;
    }

    verifier.finish(expected).map_err(VerifyFileError::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const ABC_SHA1: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";

    #[test]
    fn digest_matches_the_reference_sha256_vector() {
        let digest = digest_of(b"abc");

        assert_eq!(digest.as_hex(), ABC_SHA256);
    }

    #[test]
    fn sha1_digests_match_the_reference_vector_and_any_casing_parses_canonically() {
        assert_eq!(Sha1Digest::compute(b"abc").as_hex(), ABC_SHA1);
        assert_eq!(
            Sha1Digest::parse(ABC_SHA1).unwrap(),
            Sha1Digest::parse(&ABC_SHA1.to_uppercase()).unwrap()
        );
        assert_eq!(
            Sha1Digest::parse(ABC_SHA1).unwrap().as_hex(),
            ABC_SHA1,
            "the canonical form is lowercase"
        );
    }

    #[test]
    fn sha1_digest_parsing_rejects_wrong_lengths_and_non_hexadecimal_characters() {
        assert_eq!(
            Sha1Digest::parse("abcd"),
            Err(InvalidSha1Digest::InvalidLength(4))
        );
        assert_eq!(
            Sha1Digest::parse(&"g".repeat(40)),
            Err(InvalidSha1Digest::InvalidCharacter(b'g'))
        );
    }

    #[test]
    fn digest_parsing_accepts_any_casing_and_stores_the_canonical_lowercase_form() {
        let lower = ArtifactDigest::parse(ABC_SHA256).unwrap();
        let upper = ArtifactDigest::parse(&ABC_SHA256.to_uppercase()).unwrap();
        let mixed = ArtifactDigest::parse(
            "BA7816bf8F01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();

        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
        assert_eq!(upper.as_hex(), ABC_SHA256);
    }

    #[test]
    fn digest_parsing_rejects_wrong_lengths_and_non_hexadecimal_characters() {
        assert_eq!(
            ArtifactDigest::parse("abcd"),
            Err(InvalidDigest::InvalidLength(4))
        );
        assert_eq!(
            ArtifactDigest::parse(&"g".repeat(64)),
            Err(InvalidDigest::InvalidCharacter(b'g'))
        );
        // A character outside ASCII is not a hexadecimal digit either.
        assert!(ArtifactDigest::parse(&"é".repeat(32)).is_err());
    }

    #[test]
    fn streaming_verification_accepts_the_expected_bytes_in_any_chunking() {
        let expected = digest_of(b"hello aurora");

        for chunk_size in [1usize, 3, 12, 1024] {
            let mut verifier = StreamingVerifier::new(None);
            for chunk in b"hello aurora".chunks(chunk_size.max(1)) {
                verifier.update(chunk).unwrap();
            }
            let bytes = verifier.finish(&expected).unwrap();

            assert_eq!(bytes, 12);
        }
    }

    #[test]
    fn streaming_verification_fails_on_a_wrong_digest() {
        let mut verifier = StreamingVerifier::new(None);
        verifier.update(b"different bytes").unwrap();

        let failure = verifier.finish(&digest_of(b"expected bytes")).unwrap_err();

        assert!(matches!(
            failure,
            VerificationFailure::Sha256Mismatch { .. }
        ));
    }

    #[test]
    fn streaming_verification_fails_when_the_stream_is_shorter_than_expected() {
        let expected = digest_of(b"complete bytes");

        let mut verifier = StreamingVerifier::new(Some(14));
        verifier.update(b"complete").unwrap();

        let failure = verifier.finish(&expected).unwrap_err();

        assert_eq!(
            failure,
            VerificationFailure::SizeMismatch {
                expected: 14,
                actual: 8
            }
        );
    }

    #[test]
    fn streaming_verification_fails_early_when_the_stream_exceeds_the_expected_size() {
        let mut verifier = StreamingVerifier::new(Some(4));

        assert!(verifier.update(b"four").is_ok());
        assert_eq!(
            verifier.update(b"five!"),
            Err(VerificationFailure::SizeMismatch {
                expected: 4,
                actual: 9
            })
        );
    }

    #[test]
    fn file_verification_accepts_the_verified_content() {
        let directory = test_directory();
        let path = directory.join("artifact.bin");
        std::fs::write(&path, b"hello aurora").unwrap();
        let expected = digest_of(b"hello aurora");

        let bytes = verify_file(&path, &expected, Some(12)).unwrap();

        assert_eq!(bytes, 12);
    }

    #[test]
    fn file_verification_rejects_corrupted_and_truncated_files() {
        let directory = test_directory();
        let expected = digest_of(b"hello aurora");

        let corrupted = directory.join("corrupted.bin");
        std::fs::write(&corrupted, b"hello aurora!").unwrap();
        assert!(matches!(
            verify_file(&corrupted, &expected, None).unwrap_err(),
            VerifyFileError::Mismatch(VerificationFailure::Sha256Mismatch { .. })
        ));

        let truncated = directory.join("truncated.bin");
        std::fs::write(&truncated, b"hello").unwrap();
        assert!(matches!(
            verify_file(&truncated, &expected, Some(12)).unwrap_err(),
            VerifyFileError::Mismatch(VerificationFailure::SizeMismatch { .. })
        ));
    }

    #[test]
    fn file_verification_reports_missing_files_as_io_errors() {
        let path = test_directory().join("missing.bin");
        let expected = digest_of(b"hello aurora");

        assert!(matches!(
            verify_file(&path, &expected, None).unwrap_err(),
            VerifyFileError::Io(_)
        ));
    }

    #[test]
    fn streaming_sha1_verification_accepts_the_expected_bytes_in_any_chunking() {
        let expected = Sha1Digest::parse(ABC_SHA1).unwrap();

        for chunk_size in [1usize, 4, 1024] {
            let mut verifier = StreamingSha1Verifier::new(None);
            for chunk in b"abc".chunks(chunk_size) {
                verifier.update(chunk).unwrap();
            }
            let bytes = verifier.finish(&expected).unwrap();

            assert_eq!(bytes, 3);
        }
    }

    #[test]
    fn streaming_sha1_verification_fails_on_wrong_digest_and_wrong_size() {
        let expected = Sha1Digest::parse(ABC_SHA1).unwrap();

        let mut wrong_digest = StreamingSha1Verifier::new(None);
        wrong_digest.update(b"xyz").unwrap();
        assert_eq!(
            wrong_digest.finish(&expected).unwrap_err(),
            VerificationFailure::Sha1Mismatch {
                expected: ABC_SHA1.to_owned(),
                actual: Sha1Digest::compute(b"xyz").as_hex(),
            }
        );

        let mut short = StreamingSha1Verifier::new(Some(4));
        short.update(b"abc").unwrap();
        assert_eq!(
            short.finish(&expected).unwrap_err(),
            VerificationFailure::SizeMismatch {
                expected: 4,
                actual: 3,
            }
        );

        let mut long = StreamingSha1Verifier::new(Some(2));
        assert_eq!(
            long.update(b"abc").unwrap_err(),
            VerificationFailure::SizeMismatch {
                expected: 2,
                actual: 3,
            }
        );
    }

    #[test]
    fn file_sha1_verification_accepts_official_content_and_rejects_drift() {
        let directory = test_directory();
        let path = directory.join("official.bin");
        std::fs::write(&path, b"abc").unwrap();
        let expected = Sha1Digest::parse(ABC_SHA1).unwrap();

        assert_eq!(verify_file_sha1(&path, &expected, Some(3)).unwrap(), 3);

        let drifted = directory.join("drifted.bin");
        std::fs::write(&drifted, b"abd").unwrap();
        assert!(matches!(
            verify_file_sha1(&drifted, &expected, None).unwrap_err(),
            VerifyFileError::Mismatch(VerificationFailure::Sha1Mismatch { .. })
        ));
    }

    #[test]
    fn artifact_trust_records_preserve_the_verification_distinction() {
        let sha256 = ArtifactDigest::parse(ABC_SHA256).unwrap();
        let sha1 = Sha1Digest::parse(ABC_SHA1).unwrap();

        let verified_256 = ArtifactTrust::verified_sha256(&sha256);
        let verified_1 = ArtifactTrust::verified_sha1(&sha1);
        let observed = ArtifactTrust::transport_observed(&sha256);

        assert_eq!(
            verified_256,
            ArtifactTrust::ExpectedDigestVerified {
                algorithm: DigestAlgorithm::Sha256,
                digest: ABC_SHA256.to_owned(),
            }
        );
        assert_eq!(
            verified_1,
            ArtifactTrust::ExpectedDigestVerified {
                algorithm: DigestAlgorithm::Sha1,
                digest: ABC_SHA1.to_owned(),
            }
        );
        // The same bytes, but the observed form is a different trust class:
        // secure transport with a locally computed identity, never claimed as
        // an expected-digest verification.
        assert_ne!(observed, verified_256);
        assert_eq!(
            observed,
            ArtifactTrust::SecureTransportObserved {
                observed_sha256: ABC_SHA256.to_owned(),
            }
        );
        assert!(observed.to_string().contains("no published digest"));
    }

    fn digest_of(bytes: &[u8]) -> ArtifactDigest {
        ArtifactDigest::from_sha256(Sha256::digest(bytes).into())
    }

    fn test_directory() -> std::path::PathBuf {
        let directory = std::env::temp_dir()
            .join("aurora-integrity-test")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }
}
