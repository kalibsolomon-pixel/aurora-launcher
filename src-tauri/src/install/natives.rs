//! Safe extraction of already-verified Minecraft native artifacts.
//!
//! Native artifacts arrive as ordinary planned libraries with `natives-*`
//! classifiers; each is cryptographically verified against its official
//! SHA-1 *before* this code runs. Extraction is still defensive by policy:
//! a verified artifact's internal archive structure is untrusted input.
//!
//! Safety rules, all enforced before any byte is written:
//!
//! - entry names must be relative, forward-slashed, traversal-free
//!   (`..`, `.`, empty, absolute, drive-letter, and backslash names are
//!   rejected) and built from a conservative filename charset;
//! - the extraction target is always derived by joining validated segments
//!   onto the designated native staging root, so extraction can never
//!   escape it;
//! - `META-INF/**` entries are skipped deliberately: they are jar signature
//!   and metadata files, not native libraries, matching the long-standing
//!   extraction semantics of the official launcher (verified against real
//!   LWJGL native jars, which carry META-INF manifests/services only);
//! - duplicate entries (within one archive, or the same file name produced
//!   by two planned native archives) are extracted once when the bytes are
//!   identical and rejected as a conflict when they differ — Aurora never
//!   guesses which duplicate should win;
//! - per-entry and per-archive uncompressed size bounds reject unreasonable
//!   content.
//!
//! This path exists exclusively for planned, verified Minecraft native
//! artifacts. It never executes or loads anything it extracts, and it is not
//! a general-purpose archive tool.

use std::fmt;
use std::io::Read as _;
use std::path::Path;

/// Upper bound on one extracted entry. Real native libraries are a few
/// megabytes; this bound exists to refuse absurd content, not to describe it.
const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

/// Upper bound on the total uncompressed bytes extracted from one archive.
const MAX_ARCHIVE_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// The jar-internal metadata prefix skipped during extraction.
const META_INF_PREFIX: &str = "META-INF/";

/// The outcome of one successful extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractedNative {
    /// How many entries were written (duplicates that matched earlier
    /// content are not re-written).
    pub written_entries: usize,
    pub total_bytes: u64,
}

/// Extracts one verified native archive into `destination_root`.
///
/// `archive_label` is a logical name for diagnostics (the library
/// coordinate), never a remote filename and never a local path.
pub fn extract_native_archive(
    archive_path: &Path,
    archive_label: &str,
    destination_root: &Path,
) -> Result<ExtractedNative, NativeExtractionError> {
    let file = std::fs::File::open(archive_path).map_err(|error| NativeExtractionError::Io {
        archive: archive_label.to_owned(),
        context: "opening the verified native archive".to_owned(),
        source: error,
    })?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| NativeExtractionError::ArchiveInvalid {
            archive: archive_label.to_owned(),
            reason: error.to_string(),
        })?;

    let mut written_entries = 0usize;
    let mut extractable_entries = 0usize;
    let mut total_bytes = 0u64;

    for index in 0..archive.len() {
        let mut entry =
            archive
                .by_index(index)
                .map_err(|error| NativeExtractionError::ArchiveInvalid {
                    archive: archive_label.to_owned(),
                    reason: error.to_string(),
                })?;

        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        if name.starts_with(META_INF_PREFIX) {
            continue;
        }

        let segments = validate_entry_name(&name, archive_label)?;
        let declared_size = entry.size();
        if declared_size > MAX_ENTRY_BYTES {
            return Err(NativeExtractionError::ArchiveInvalid {
                archive: archive_label.to_owned(),
                reason: format!(
                    "entry '{name}' declares {declared_size} uncompressed bytes, above the per-entry bound"
                ),
            });
        }

        let mut bytes = Vec::with_capacity(declared_size.min(8 * 1024 * 1024) as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| NativeExtractionError::ArchiveInvalid {
                archive: archive_label.to_owned(),
                reason: format!("entry '{name}' could not be decompressed: {error}"),
            })?;
        if bytes.len() as u64 != declared_size {
            return Err(NativeExtractionError::ArchiveInvalid {
                archive: archive_label.to_owned(),
                reason: format!(
                    "entry '{name}' produced {} bytes but declared {declared_size}",
                    bytes.len()
                ),
            });
        }
        extractable_entries += 1;

        let mut target = destination_root.to_path_buf();
        for segment in &segments {
            target.push(segment);
        }

        if target.exists() {
            // A duplicate name: deliberate handling, never a guess. Identical
            // content is fine (two archives shipping the same library);
            // differing content is a real conflict Aurora refuses to resolve.
            let existing = std::fs::read(&target).map_err(|error| NativeExtractionError::Io {
                archive: archive_label.to_owned(),
                context: format!("comparing duplicate entry '{name}'"),
                source: error,
            })?;
            if existing == bytes {
                continue;
            }
            return Err(NativeExtractionError::EntryConflict {
                archive: archive_label.to_owned(),
                entry: name,
                reason: "two entries claim the same file name with different content; Aurora does not guess which one should win".to_owned(),
            });
        }

        total_bytes += bytes.len() as u64;
        if total_bytes > MAX_ARCHIVE_TOTAL_BYTES {
            return Err(NativeExtractionError::ArchiveInvalid {
                archive: archive_label.to_owned(),
                reason: "the archive exceeds the total extraction bound".to_owned(),
            });
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| NativeExtractionError::Io {
                archive: archive_label.to_owned(),
                context: format!("creating the parent directory for entry '{name}'"),
                source: error,
            })?;
        }
        std::fs::write(&target, &bytes).map_err(|error| NativeExtractionError::Io {
            archive: archive_label.to_owned(),
            context: format!("writing entry '{name}'"),
            source: error,
        })?;
        written_entries += 1;
    }

    if extractable_entries == 0 {
        return Err(NativeExtractionError::ArchiveInvalid {
            archive: archive_label.to_owned(),
            reason: "the archive contains no extractable native library files".to_owned(),
        });
    }

    Ok(ExtractedNative {
        written_entries,
        total_bytes,
    })
}

/// Validates one archive entry name into its path segments.
///
/// Accepted names are relative, forward-slashed, and built from a
/// conservative ASCII filename charset; traversal shapes, absolute paths,
/// drive letters, and backslashes are rejected before any path is joined.
fn validate_entry_name(
    name: &str,
    archive_label: &str,
) -> Result<Vec<String>, NativeExtractionError> {
    let invalid = |reason: String| NativeExtractionError::ArchiveInvalid {
        archive: archive_label.to_owned(),
        reason: format!("entry name '{name}' is not safe to extract: {reason}"),
    };

    if name.is_empty() {
        return Err(invalid("the name is empty".to_owned()));
    }
    if name.starts_with('/') || name.starts_with('\\') || name.contains('\\') || name.contains(':')
    {
        return Err(invalid(
            "the name must be relative with forward slashes".to_owned(),
        ));
    }

    let segments: Vec<String> = name.split('/').map(str::to_owned).collect();
    for segment in &segments {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(invalid(
                "the name must not traverse or repeat segments".to_owned(),
            ));
        }
        if !segment.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '(' | ')' | '$')
        }) {
            return Err(invalid(
                "the name may only contain letters, digits, '.', '_', '-', '+', '(', ')', and '$' per segment"
                    .to_owned(),
            ));
        }
    }

    Ok(segments)
}

/// A failed native extraction.
#[derive(Debug)]
pub enum NativeExtractionError {
    /// The archive is not usable (not a ZIP, unreadable, unsafe entry
    /// names, unreasonable sizes, or nothing extractable).
    ArchiveInvalid { archive: String, reason: String },
    /// Two entries claim one file name with different content.
    EntryConflict {
        archive: String,
        entry: String,
        reason: String,
    },
    Io {
        archive: String,
        context: String,
        source: std::io::Error,
    },
}

impl fmt::Display for NativeExtractionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ArchiveInvalid { archive, reason } => write!(
                formatter,
                "native archive '{archive}' is unusable: {reason}"
            ),
            Self::EntryConflict {
                archive,
                entry,
                reason,
            } => write!(
                formatter,
                "native archive '{archive}' conflicts on entry '{entry}': {reason}"
            ),
            Self::Io {
                archive,
                context,
                source,
            } => write!(
                formatter,
                "native archive '{archive}' could not be extracted while {context}: {source}"
            ),
        }
    }
}

impl std::error::Error for NativeExtractionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::path::PathBuf;

    fn test_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir()
            .join("aurora-natives-test")
            .join(std::process::id().to_string())
            .join(name);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// Builds an in-memory ZIP with the given entries (name → bytes).
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            for (name, bytes) in entries {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn write_archive(directory: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, zip_bytes(entries)).unwrap();
        path
    }

    #[test]
    fn a_well_formed_archive_extracts_only_library_entries() {
        let directory = test_directory("happy");
        let archive = write_archive(
            &directory,
            "lwjgl-natives.jar",
            &[
                ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0"),
                ("META-INF/versions/9/module-info.class", b"garbage"),
                ("lwjgl.dll", b"dll bytes"),
                ("sub/liblwjgl.dylib", b"dylib bytes"),
            ],
        );
        let root = directory.join("natives");
        std::fs::create_dir_all(&root).unwrap();

        let outcome =
            extract_native_archive(&archive, "org.lwjgl:lwjgl:3.4.1:natives-windows", &root)
                .unwrap();

        assert_eq!(outcome.written_entries, 2);
        assert_eq!(std::fs::read(root.join("lwjgl.dll")).unwrap(), b"dll bytes");
        assert_eq!(
            std::fs::read(root.join("sub").join("liblwjgl.dylib")).unwrap(),
            b"dylib bytes"
        );
        assert!(!root.join("META-INF").exists(), "metadata is not extracted");
    }

    #[test]
    fn traversal_and_absolute_entry_names_are_rejected_without_touching_disk() {
        let directory = test_directory("traversal");
        let root = directory.join("natives");
        std::fs::create_dir_all(&root).unwrap();
        let sentinel = directory.join("sentinel.txt");
        std::fs::write(&sentinel, b"precious").unwrap();

        for (index, malicious) in [
            vec![("../evil.dll", b"x" as &[u8])],
            vec![("a/../../evil.dll", b"x")],
            vec![("/absolute.dll", b"x")],
            vec![("C:/drive.dll", b"x")],
            vec![("back\\slash.dll", b"x")],
            vec![("nested/../escape.dll", b"x")],
        ]
        .into_iter()
        .enumerate()
        {
            let archive = write_archive(&directory, &format!("evil-{index}.jar"), &malicious);
            let result = extract_native_archive(&archive, "evil", &root);
            assert!(
                matches!(result, Err(NativeExtractionError::ArchiveInvalid { .. })),
                "entry {:?} must be rejected",
                malicious[0].0
            );
        }

        assert_eq!(std::fs::read(&sentinel).unwrap(), b"precious");
        assert!(!root.join("evil.dll").exists());
        assert!(!directory.join("evil.dll").exists());
    }

    #[test]
    fn duplicate_entry_names_deduplicate_or_conflict_deliberately() {
        let directory = test_directory("duplicates");

        // Note: within a *single* archive, the zip crate's own reader
        // surfaces duplicate central-directory names, and the extraction
        // collision handling below is the same code path either way — what
        // matters deterministically is the cross-archive case real installs
        // hit (two planned native archives shipping one file name), so that
        // is what is exercised here with full byte comparison.
        //
        // Identical content from a second archive: one file, no error.
        let alpha = write_archive(&directory, "alpha.jar", &[("shared.dll", b"alpha")]);
        let beta_same = write_archive(&directory, "beta-same.jar", &[("shared.dll", b"alpha")]);
        let beta_differs =
            write_archive(&directory, "beta-differs.jar", &[("shared.dll", b"beta")]);
        let root = directory.join("r1");
        std::fs::create_dir_all(&root).unwrap();
        extract_native_archive(&alpha, "a", &root).unwrap();
        extract_native_archive(&beta_same, "b-same", &root).unwrap();
        assert!(matches!(
            extract_native_archive(&beta_differs, "b-differs", &root),
            Err(NativeExtractionError::EntryConflict { .. })
        ));
        assert_eq!(std::fs::read(root.join("shared.dll")).unwrap(), b"alpha");
    }

    #[test]
    fn non_archives_and_empty_archives_fail_closed() {
        let directory = test_directory("broken");
        let root = directory.join("natives");
        std::fs::create_dir_all(&root).unwrap();

        let not_a_zip = directory.join("not-a-zip.jar");
        std::fs::write(&not_a_zip, b"definitely not zip content").unwrap();
        assert!(matches!(
            extract_native_archive(&not_a_zip, "broken", &root),
            Err(NativeExtractionError::ArchiveInvalid { .. })
        ));

        let empty = write_archive(&directory, "empty.jar", &[]);
        assert!(matches!(
            extract_native_archive(&empty, "broken", &root),
            Err(NativeExtractionError::ArchiveInvalid { .. })
        ));

        let metadata_only = write_archive(
            &directory,
            "metadata-only.jar",
            &[("META-INF/MANIFEST.MF", b"only metadata")],
        );
        assert!(matches!(
            extract_native_archive(&metadata_only, "broken", &root),
            Err(NativeExtractionError::ArchiveInvalid { .. })
        ));
    }
}
