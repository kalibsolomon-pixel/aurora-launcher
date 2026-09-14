//! Artifact digests and streaming integrity verification.
//!
//! Download completion does not imply artifact trust: data only becomes a
//! verified artifact when its exact byte size (when expected) and its SHA-256
//! digest match the values provided by trusted metadata. Hashing here is
//! streaming, so artifacts are never buffered into memory for verification.

use std::fmt;
use std::path::Path;

use sha2::{Digest as _, Sha256};

/// The SHA-256 digest is represented as exactly 64 hexadecimal characters.
pub const SHA256_HEX_LENGTH: usize = 64;

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

fn hex_value(byte: u8) -> Result<u8, u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        other => Err(other),
    }
}

/// A failed integrity comparison. Every variant is a hard failure: the
/// artifact is untrusted and must never be promoted or activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationFailure {
    SizeMismatch { expected: u64, actual: u64 },
    Sha256Mismatch { expected: String, actual: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn digest_matches_the_reference_sha256_vector() {
        let digest = digest_of(b"abc");

        assert_eq!(digest.as_hex(), ABC_SHA256);
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
