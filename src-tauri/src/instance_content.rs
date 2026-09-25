//! Provider-independent instance content. Filesystem authority stays here: a
//! validated instance id and a closed content type select a direct child of
//! the isolated game directory. Provider records are evidence, never guesses.

use std::collections::HashSet;
use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::cache::ArtifactCache;
use crate::downloads::ArtifactSource;
use crate::instances::InstanceId;
use crate::integrity::{ArtifactDigest, verify_file};
use crate::paths::ManagedPaths;

const SCHEMA_VERSION: u32 = 1;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 4_096;
const MAX_METADATA_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentType {
    Mod,
    ResourcePack,
    ShaderPack,
}

impl ContentType {
    pub fn directory_name(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::ResourcePack => "resourcepacks",
            Self::ShaderPack => "shaderpacks",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentOwnership {
    LauncherManagedRequired,
    ProviderManaged,
    UserManaged,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyKind {
    Required,
    Optional,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderDependency {
    pub kind: DependencyKind,
    pub provider: String,
    pub project_id: String,
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentCompatibility {
    pub minecraft_versions: Vec<String>,
    pub loader: Option<String>,
    pub environment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRecord {
    pub content_type: ContentType,
    pub provider: String,
    pub project_id: String,
    pub version_id: String,
    pub file_id: String,
    pub file_name: String,
    pub sha256: String,
    pub display_version: Option<String>,
    pub compatibility: ContentCompatibility,
    pub dependencies: Vec<ProviderDependency>,
}

impl ProviderRecord {
    fn validate(&self) -> Result<(), ContentError> {
        validate_file_name(&self.file_name)?;
        if !self
            .file_name
            .to_ascii_lowercase()
            .ends_with(match self.content_type {
                ContentType::Mod => ".jar",
                _ => ".zip",
            })
        {
            return Err(ContentError::StateMalformed(
                "provider filename has the wrong extension".into(),
            ));
        }
        for value in [
            &self.provider,
            &self.project_id,
            &self.version_id,
            &self.file_id,
        ] {
            if value.trim().is_empty() || value.len() > 256 {
                return Err(ContentError::StateMalformed(
                    "provider identity is empty or oversized".into(),
                ));
            }
        }
        ArtifactDigest::parse(&self.sha256)
            .map_err(|_| ContentError::StateMalformed("provider digest is invalid".into()))?;
        if self
            .compatibility
            .minecraft_versions
            .iter()
            .any(|value| value.trim().is_empty())
            || self.dependencies.iter().any(|dependency| {
                dependency.provider.trim().is_empty() || dependency.project_id.trim().is_empty()
            })
        {
            return Err(ContentError::StateMalformed(
                "provider compatibility or dependency is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentState {
    schema_version: u32,
    pub entries: Vec<ProviderRecord>,
}

impl ContentState {
    pub fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }

    pub fn from_json(text: &str) -> Result<Self, ContentError> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| ContentError::StateMalformed(error.to_string()))?;
        let version = value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| ContentError::StateMalformed("schemaVersion is required".into()))?;
        if version != u64::from(SCHEMA_VERSION) {
            return Err(ContentError::StateVersion(version));
        }
        let state: Self = serde_json::from_value(value)
            .map_err(|error| ContentError::StateMalformed(error.to_string()))?;
        state.validate()?;
        Ok(state)
    }

    fn validate(&self) -> Result<(), ContentError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ContentError::StateVersion(u64::from(self.schema_version)));
        }
        let mut names = HashSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !names.insert((entry.content_type, entry.file_name.to_lowercase())) {
                return Err(ContentError::StateMalformed(
                    "duplicate provider filename".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn load(managed: &ManagedPaths, instance: &InstanceId) -> Result<Self, ContentError> {
        let path = state_path(managed, instance)?;
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(error) => Err(ContentError::Io(error)),
        }
    }

    pub fn save(&self, managed: &ManagedPaths, instance: &InstanceId) -> Result<(), ContentError> {
        self.validate()?;
        // Refuse to repair or overwrite a damaged or future-version document.
        let _ = Self::load(managed, instance)?;
        let path = state_path(managed, instance)?;
        let temporary = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
        let mut sorted = self.clone();
        sorted.entries.sort_by(|a, b| {
            a.content_type
                .directory_name()
                .cmp(b.content_type.directory_name())
                .then_with(|| a.file_name.cmp(&b.file_name))
        });
        let mut json = serde_json::to_string_pretty(&sorted)
            .map_err(|error| ContentError::StateMalformed(error.to_string()))?;
        json.push('\n');
        std::fs::write(&temporary, json).map_err(ContentError::Io)?;
        if let Err(error) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(ContentError::Io(error));
        }
        Ok(())
    }
}

fn state_path(managed: &ManagedPaths, instance: &InstanceId) -> Result<PathBuf, ContentError> {
    let root = validated_instance_root(managed, instance)?;
    let path = root.join("content-managed.json");
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_file() {
            return Err(ContentError::UnsafePath);
        }
    }
    Ok(path)
}

fn validated_instance_root(
    managed: &ManagedPaths,
    instance: &InstanceId,
) -> Result<PathBuf, ContentError> {
    let root = std::fs::canonicalize(managed.data_root()).map_err(ContentError::Io)?;
    let instances = std::fs::canonicalize(managed.instances_dir()).map_err(ContentError::Io)?;
    let target = managed.instance_paths(instance).root().to_path_buf();
    let canonical = std::fs::canonicalize(&target).map_err(ContentError::Io)?;
    if !instances.starts_with(&root) || canonical != instances.join(instance.as_str()) {
        return Err(ContentError::UnsafePath);
    }
    Ok(target)
}

pub fn validate_directory(
    managed: &ManagedPaths,
    instance: &InstanceId,
    kind: ContentType,
) -> Result<PathBuf, ContentError> {
    let root = validated_instance_root(managed, instance)?;
    let directory = root.join(kind.directory_name());
    match std::fs::symlink_metadata(&directory) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() && !is_reparse_point(&meta) => {
            let canonical_root = std::fs::canonicalize(&root).map_err(ContentError::Io)?;
            let canonical_directory =
                std::fs::canonicalize(&directory).map_err(ContentError::Io)?;
            if canonical_directory != canonical_root.join(kind.directory_name()) {
                return Err(ContentError::UnsafePath);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err(ContentError::UnsafePath),
        Err(error) => return Err(ContentError::Io(error)),
    }
    Ok(directory)
}

pub fn ensure_directory(
    managed: &ManagedPaths,
    instance: &InstanceId,
    kind: ContentType,
) -> Result<PathBuf, ContentError> {
    let path = validate_directory(managed, instance, kind)?;
    if !path.exists() {
        std::fs::create_dir(&path).map_err(ContentError::Io)?;
    }
    validate_directory(managed, instance, kind)
}

pub fn validate_file_name(name: &str) -> Result<(), ContentError> {
    let path = Path::new(name);
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let reserved = ["con", "prn", "aux", "nul"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
        || name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(char::is_control)
        || reserved
        || path.is_absolute()
        || path.components().count() != 1
    {
        return Err(ContentError::UnsafePath);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentWarning {
    pub code: String,
    pub message: String,
}

fn warning(code: &str, message: impl Into<String>) -> ContentWarning {
    ContentWarning {
        code: code.into(),
        message: message.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentEntry {
    pub entry_id: String,
    pub content_type: ContentType,
    pub file_name: String,
    pub display_name: String,
    pub file_type: String,
    pub size_bytes: Option<u64>,
    pub modified_unix_millis: Option<u64>,
    pub ownership: ContentOwnership,
    pub sha256: Option<String>,
    pub provenance: Option<ProviderRecord>,
    pub description: Option<String>,
    pub pack_format: Option<u64>,
    pub warnings: Vec<ContentWarning>,
    pub can_remove: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentInventory {
    pub instance_id: String,
    pub content_type: ContentType,
    pub entries: Vec<ContentEntry>,
    pub missing_managed: Vec<ProviderRecord>,
}

pub fn scan(
    managed: &ManagedPaths,
    instance: &InstanceId,
    kind: ContentType,
) -> Result<ContentInventory, ContentError> {
    let directory = validate_directory(managed, instance, kind)?;
    let state = ContentState::load(managed, instance)?;
    let mut entries = Vec::new();
    if directory.exists() {
        for item in std::fs::read_dir(directory).map_err(ContentError::Io)? {
            let item = item.map_err(ContentError::Io)?;
            let name = item.file_name().to_string_lossy().into_owned();
            let record = state.entries.iter().find(|record| {
                record.content_type == kind && record.file_name.eq_ignore_ascii_case(&name)
            });
            entries.push(inspect(&item.path(), kind, record));
        }
    }
    let present: HashSet<_> = entries
        .iter()
        .map(|entry| entry.file_name.to_lowercase())
        .collect();
    let missing_managed = state
        .entries
        .iter()
        .filter(|record| {
            record.content_type == kind && !present.contains(&record.file_name.to_lowercase())
        })
        .cloned()
        .collect();
    entries.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then_with(|| a.file_name.cmp(&b.file_name))
    });
    Ok(ContentInventory {
        instance_id: instance.to_string(),
        content_type: kind,
        entries,
        missing_managed,
    })
}

fn inspect(path: &Path, kind: ContentType, record: Option<&ProviderRecord>) -> ContentEntry {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let meta = std::fs::symlink_metadata(path);
    let (file_type, size, modified) = match &meta {
        Ok(meta) => {
            let file_type = if meta.file_type().is_symlink() || is_reparse_point(meta) {
                "link"
            } else if meta.is_dir() {
                "directory"
            } else if meta.is_file() && name.to_ascii_lowercase().ends_with(".zip") {
                "zip"
            } else {
                "unexpectedFile"
            };
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| u64::try_from(duration.as_millis()).ok());
            (file_type, meta.is_file().then_some(meta.len()), modified)
        }
        Err(_) => ("unreadable", None, None),
    };
    let mut warnings = Vec::new();
    if file_type == "link" {
        warnings.push(warning(
            "link_not_managed",
            "Links and reparse points are shown but never followed.",
        ));
    }
    if file_type == "unexpectedFile" {
        warnings.push(warning(
            "unexpected_file",
            "This is not a ZIP pack and is left untouched.",
        ));
    }
    if file_type == "unreadable" {
        warnings.push(warning(
            "entry_unreadable",
            "This entry could not be inspected.",
        ));
    }
    let mut description = None;
    let mut pack_format = None;
    if file_type == "zip" {
        let (found_description, found_format, issue) =
            inspect_archive(path, kind, size.unwrap_or_default());
        description = found_description;
        pack_format = found_format;
        if let Some(issue) = issue {
            warnings.push(issue);
        }
    } else if file_type == "directory" && kind == ContentType::ResourcePack {
        let metadata = path.join("pack.mcmeta");
        if let Ok(file_meta) = std::fs::symlink_metadata(&metadata) {
            if file_meta.is_file()
                && !file_meta.file_type().is_symlink()
                && !is_reparse_point(&file_meta)
                && file_meta.len() <= MAX_METADATA_BYTES
            {
                if let Ok(file) = std::fs::File::open(metadata) {
                    let mut bytes = Vec::new();
                    if file
                        .take(MAX_METADATA_BYTES + 1)
                        .read_to_end(&mut bytes)
                        .is_ok()
                        && bytes.len() as u64 <= MAX_METADATA_BYTES
                    {
                        (description, pack_format) = parse_pack_metadata(&bytes);
                    } else {
                        warnings.push(warning(
                            "pack_metadata_unreadable",
                            "Pack metadata exceeded the safe read limit.",
                        ));
                    }
                }
            } else {
                warnings.push(warning(
                    "pack_metadata_unsafe",
                    "Pack metadata could not be read safely.",
                ));
            }
        } else {
            warnings.push(warning(
                "pack_metadata_missing",
                "No root-level pack.mcmeta was found.",
            ));
        }
    } else if file_type == "directory" && kind == ContentType::ShaderPack {
        let shaders = path.join("shaders");
        if !std::fs::symlink_metadata(shaders).is_ok_and(|meta| {
            meta.is_dir() && !meta.file_type().is_symlink() && !is_reparse_point(&meta)
        }) {
            warnings.push(warning(
                "shader_structure_unknown",
                "No safe root shaders/ directory was found; compatibility is unknown.",
            ));
        }
    }
    let mut ownership = if file_type == "zip" || file_type == "directory" {
        ContentOwnership::UserManaged
    } else {
        ContentOwnership::Unknown
    };
    let mut sha256 = None;
    let mut provenance = None;
    if let Some(record) = record {
        if file_type == "zip" {
            match ArtifactDigest::parse(&record.sha256)
                .ok()
                .and_then(|digest| verify_file(path, &digest, None).ok())
            {
                Some(_) => {
                    ownership = ContentOwnership::ProviderManaged;
                    sha256 = Some(record.sha256.clone());
                    provenance = Some(record.clone());
                }
                None => {
                    ownership = ContentOwnership::Unknown;
                    warnings.push(warning("content_hash_mismatch", "This file no longer matches its provider-managed record. Actions are blocked."));
                }
            }
        } else {
            ownership = ContentOwnership::Unknown;
            warnings.push(warning(
                "content_record_mismatch",
                "The provider record no longer describes a regular ZIP file.",
            ));
        }
    }
    let material = format!("{kind:?}|{name}|{file_type}|{size:?}|{modified:?}");
    let entry_id: String = sha2::Sha256::digest(material.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let display_name = name.strip_suffix(".zip").unwrap_or(&name).to_owned();
    ContentEntry {
        entry_id,
        content_type: kind,
        file_name: name,
        display_name,
        file_type: file_type.into(),
        size_bytes: size,
        modified_unix_millis: modified,
        ownership,
        sha256,
        provenance,
        description,
        pack_format,
        warnings,
        can_remove: file_type == "zip"
            && matches!(
                ownership,
                ContentOwnership::UserManaged | ContentOwnership::ProviderManaged
            ),
    }
}

fn inspect_archive(
    path: &Path,
    kind: ContentType,
    size: u64,
) -> (Option<String>, Option<u64>, Option<ContentWarning>) {
    if size > MAX_ARCHIVE_BYTES {
        return (
            None,
            None,
            Some(warning(
                "archive_too_large",
                "Archive inspection exceeds the 512 MiB safety bound.",
            )),
        );
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => {
            return (
                None,
                None,
                Some(warning(
                    "archive_unreadable",
                    "Archive could not be opened.",
                )),
            );
        }
    };
    let mut zip = match zip::ZipArchive::new(file) {
        Ok(zip) => zip,
        Err(_) => {
            return (
                None,
                None,
                Some(warning(
                    "archive_malformed",
                    "This is not a readable ZIP archive.",
                )),
            );
        }
    };
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return (
            None,
            None,
            Some(warning(
                "archive_entry_limit",
                "Archive has too many entries for safe inspection.",
            )),
        );
    }
    if kind == ContentType::ShaderPack {
        let has_shaders = (0..zip.len()).any(|i| {
            zip.by_index(i)
                .is_ok_and(|entry| entry.name().starts_with("shaders/"))
        });
        return (
            None,
            None,
            (!has_shaders).then(|| {
                warning(
                    "shader_structure_unknown",
                    "No root shaders/ directory was found; compatibility is unknown.",
                )
            }),
        );
    }
    let matches: Vec<_> = (0..zip.len())
        .filter(|index| {
            zip.by_index(*index)
                .is_ok_and(|entry| entry.name() == "pack.mcmeta")
        })
        .collect();
    if matches.len() != 1 {
        return (
            None,
            None,
            Some(warning(
                "pack_metadata_missing",
                "Expected one root-level pack.mcmeta.",
            )),
        );
    }
    let mut entry = match zip.by_index(matches[0]) {
        Ok(entry) => entry,
        Err(_) => {
            return (
                None,
                None,
                Some(warning(
                    "pack_metadata_unreadable",
                    "Pack metadata could not be opened.",
                )),
            );
        }
    };
    if entry.size() > MAX_METADATA_BYTES {
        return (
            None,
            None,
            Some(warning(
                "pack_metadata_too_large",
                "Pack metadata exceeds the 256 KiB safety bound.",
            )),
        );
    }
    let mut bytes = Vec::new();
    if entry
        .by_ref()
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_METADATA_BYTES
    {
        return (
            None,
            None,
            Some(warning(
                "pack_metadata_unreadable",
                "Pack metadata could not be read within the safety bound.",
            )),
        );
    }
    let (description, pack_format) = parse_pack_metadata(&bytes);
    let issue = (description.is_none() && pack_format.is_none()).then(|| {
        warning(
            "pack_metadata_malformed",
            "Pack metadata is malformed or has no usable pack fields.",
        )
    });
    (description, pack_format, issue)
}

fn parse_pack_metadata(bytes: &[u8]) -> (Option<String>, Option<u64>) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return (None, None);
    };
    let Some(pack) = value.get("pack") else {
        return (None, None);
    };
    let description = pack
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(|text| text.chars().take(500).collect());
    let format = pack.get("pack_format").and_then(serde_json::Value::as_u64);
    (description, format)
}

fn locks() -> &'static Mutex<std::collections::HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<std::collections::HashMap<String, Arc<Mutex<()>>>>> =
        OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

pub fn with_instance_lock<T>(
    instance: &InstanceId,
    operation: impl FnOnce() -> Result<T, ContentError>,
) -> Result<T, ContentError> {
    let lock = locks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(instance.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = lock
        .try_lock()
        .map_err(|_| ContentError::OperationInProgress)?;
    operation()
}

pub fn remove(
    managed: &ManagedPaths,
    instance: &InstanceId,
    kind: ContentType,
    entry_id: &str,
) -> Result<ContentInventory, ContentError> {
    with_instance_lock(instance, || {
        let inventory = scan(managed, instance, kind)?;
        let entry = inventory
            .entries
            .iter()
            .find(|entry| entry.entry_id == entry_id)
            .ok_or(ContentError::ChangedSinceScan)?;
        if !entry.can_remove {
            return Err(ContentError::UnsupportedAction);
        }
        let directory = validate_directory(managed, instance, kind)?;
        validate_file_name(&entry.file_name)?;
        let target = directory.join(&entry.file_name);
        let meta = std::fs::symlink_metadata(&target).map_err(ContentError::Io)?;
        if !meta.is_file() || meta.file_type().is_symlink() || is_reparse_point(&meta) {
            return Err(ContentError::UnsafePath);
        }
        if std::fs::canonicalize(&target)
            .map_err(ContentError::Io)?
            .parent()
            != Some(
                std::fs::canonicalize(&directory)
                    .map_err(ContentError::Io)?
                    .as_path(),
            )
        {
            return Err(ContentError::UnsafePath);
        }
        let temporary = directory.join(format!(".content-removing-{}", uuid::Uuid::new_v4()));
        std::fs::rename(&target, &temporary).map_err(ContentError::Io)?;
        if entry.ownership == ContentOwnership::ProviderManaged {
            let mut state = ContentState::load(managed, instance)?;
            state.entries.retain(|record| {
                !(record.content_type == kind
                    && record.file_name.eq_ignore_ascii_case(&entry.file_name))
            });
            if let Err(error) = state.save(managed, instance) {
                std::fs::rename(&temporary, &target).map_err(ContentError::Io)?;
                return Err(error);
            }
        }
        std::fs::remove_file(temporary).map_err(ContentError::Io)?;
        scan(managed, instance, kind)
    })
}

/// Backend-only normalized plan. A future provider adapter resolves its own
/// metadata into this type; no frontend command accepts a source URL or path.
pub struct ProviderInstallPlan {
    pub record: ProviderRecord,
    pub source: ArtifactSource,
}

pub async fn install_provider_artifact(
    managed: &ManagedPaths,
    instance: &InstanceId,
    plan: ProviderInstallPlan,
) -> Result<ContentInventory, ContentError> {
    plan.record.validate()?;
    if plan.record.sha256 != plan.source.sha256().as_hex() {
        return Err(ContentError::HashMismatch);
    }
    // Cache acquisition revalidates hits and verifies fresh downloads before
    // anything may be materialized inside the instance.
    let artifact = ArtifactCache::new(managed.clone())
        .acquire(&plan.source)
        .await
        .map_err(|error| ContentError::Acquisition(error.to_string()))?;
    activate_verified(
        managed,
        instance,
        plan.record,
        &artifact.path,
        plan.source.size_bytes(),
    )
}

fn activate_verified(
    managed: &ManagedPaths,
    instance: &InstanceId,
    record: ProviderRecord,
    verified_path: &Path,
    expected_size: Option<u64>,
) -> Result<ContentInventory, ContentError> {
    activate_verified_with_commit(
        managed,
        instance,
        record,
        verified_path,
        expected_size,
        |state| state.save(managed, instance),
    )
}

fn activate_verified_with_commit(
    managed: &ManagedPaths,
    instance: &InstanceId,
    record: ProviderRecord,
    verified_path: &Path,
    expected_size: Option<u64>,
    commit: impl FnOnce(&ContentState) -> Result<(), ContentError>,
) -> Result<ContentInventory, ContentError> {
    with_instance_lock(instance, || {
        record.validate()?;
        let digest =
            ArtifactDigest::parse(&record.sha256).map_err(|_| ContentError::HashMismatch)?;
        verify_file(verified_path, &digest, expected_size)
            .map_err(|_| ContentError::HashMismatch)?;
        let kind = record.content_type;
        if kind == ContentType::Mod {
            let required = crate::instance_mods::managed_artifact_file_name(managed, instance)
                .map_err(|error| ContentError::StateMalformed(error.to_string()))?;
            if required.is_some_and(|files| {
                files
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&record.file_name))
            }) {
                return Err(ContentError::Collision);
            }
        }
        let directory = ensure_directory(managed, instance, kind)?;
        let mut state = ContentState::load(managed, instance)?;
        let target = directory.join(&record.file_name);
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err(ContentError::Collision);
        }
        if state.entries.iter().any(|entry| {
            entry.content_type == kind && entry.file_name.eq_ignore_ascii_case(&record.file_name)
        }) {
            return Err(ContentError::Collision);
        }
        let temporary = directory.join(format!(".content-installing-{}", uuid::Uuid::new_v4()));
        if let Err(error) = std::fs::copy(verified_path, &temporary) {
            let _ = std::fs::remove_file(&temporary);
            return Err(ContentError::Io(error));
        }
        if verify_file(&temporary, &digest, expected_size).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return Err(ContentError::HashMismatch);
        }
        // A hard link creates the exact final name without overwriting an
        // entry that raced into place. It links only this private copy, never
        // the shared cache object. The temporary name is then removed.
        if let Err(error) = std::fs::hard_link(&temporary, &target) {
            let _ = std::fs::remove_file(&temporary);
            return if target.exists() {
                Err(ContentError::Collision)
            } else {
                Err(ContentError::Io(error))
            };
        }
        let _ = std::fs::remove_file(&temporary);
        state.entries.push(record);
        if let Err(error) = commit(&state) {
            let _ = std::fs::remove_file(&target);
            return Err(error);
        }
        scan(managed, instance, kind)
    })
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    metadata.file_attributes() & 0x400 != 0
}
#[cfg(not(windows))]
fn is_reparse_point(_: &std::fs::Metadata) -> bool {
    false
}

#[derive(Debug)]
pub enum ContentError {
    Io(std::io::Error),
    UnsafePath,
    StateMalformed(String),
    StateVersion(u64),
    ChangedSinceScan,
    UnsupportedAction,
    Collision,
    HashMismatch,
    OperationInProgress,
    Acquisition(String),
}
impl ContentError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "content_unavailable",
            Self::UnsafePath => "content_unsafe_path",
            Self::StateMalformed(_) | Self::StateVersion(_) => "content_state_malformed",
            Self::ChangedSinceScan => "content_changed_since_scan",
            Self::UnsupportedAction => "unsupported_content_action",
            Self::Collision => "content_collision",
            Self::HashMismatch => "content_hash_mismatch",
            Self::OperationInProgress => "content_operation_in_progress",
            Self::Acquisition(_) => "content_acquisition_failed",
        }
    }
}
impl fmt::Display for ContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "content I/O failed: {error}"),
            Self::UnsafePath => formatter.write_str("content path is unsafe"),
            Self::StateMalformed(reason) => {
                write!(formatter, "content state is malformed: {reason}")
            }
            Self::StateVersion(version) => {
                write!(formatter, "content state schema {version} is unsupported")
            }
            Self::ChangedSinceScan => formatter.write_str("content changed since the scan"),
            Self::UnsupportedAction => formatter.write_str("this content action is unsupported"),
            Self::Collision => formatter.write_str("content target already exists"),
            Self::HashMismatch => formatter.write_str("content hash does not match"),
            Self::OperationInProgress => {
                formatter.write_str("another content operation is in progress")
            }
            Self::Acquisition(reason) => write!(formatter, "content acquisition failed: {reason}"),
        }
    }
}
impl std::error::Error for ContentError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    struct Fixture {
        root: PathBuf,
        managed: ManagedPaths,
        instance: InstanceId,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join("aurora-content-tests")
                .join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(root.join("instances")).unwrap();
            let instance =
                InstanceId::new(format!("safe-{}", uuid::Uuid::new_v4().simple())).unwrap();
            std::fs::create_dir(root.join("instances").join(instance.as_str())).unwrap();
            let managed = ManagedPaths::from_app_local_data_dir(root.clone()).unwrap();
            Self {
                root,
                managed,
                instance,
            }
        }
        fn dir(&self, kind: ContentType) -> PathBuf {
            ensure_directory(&self.managed, &self.instance, kind).unwrap()
        }
        fn zip(&self, kind: ContentType, name: &str, metadata: Option<&str>) -> PathBuf {
            let path = self.dir(kind).join(name);
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            let entry = if kind == ContentType::ShaderPack {
                "shaders/basic.fsh"
            } else {
                "pack.mcmeta"
            };
            writer
                .start_file(entry, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer
                .write_all(metadata.unwrap_or("void main(){}").as_bytes())
                .unwrap();
            writer.finish().unwrap();
            path
        }
        fn record(&self, kind: ContentType, path: &Path) -> ProviderRecord {
            let bytes = std::fs::read(path).unwrap();
            ProviderRecord {
                content_type: kind,
                provider: "synthetic".into(),
                project_id: "opaque-project".into(),
                version_id: "opaque-version".into(),
                file_id: "opaque-file".into(),
                file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
                sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
                display_version: Some("1.2.3".into()),
                compatibility: ContentCompatibility {
                    minecraft_versions: vec!["1.21.11".into()],
                    loader: Some("fabric".into()),
                    environment: Some("client".into()),
                },
                dependencies: vec![ProviderDependency {
                    kind: DependencyKind::Required,
                    provider: "synthetic".into(),
                    project_id: "another-project".into(),
                    version_id: None,
                }],
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn content_directories_are_derived_from_validated_ids_and_closed_types() {
        let fixture = Fixture::new();
        for (kind, name) in [
            (ContentType::Mod, "mods"),
            (ContentType::ResourcePack, "resourcepacks"),
            (ContentType::ShaderPack, "shaderpacks"),
        ] {
            let path = validate_directory(&fixture.managed, &fixture.instance, kind).unwrap();
            assert_eq!(
                path,
                fixture
                    .root
                    .join("instances")
                    .join(fixture.instance.as_str())
                    .join(name)
            );
        }
        assert!(InstanceId::new("../outside").is_err());
        assert!(validate_file_name("../bad.zip").is_err());
    }

    #[test]
    fn manual_packs_and_malformed_archives_are_visible_without_provider_ownership() {
        let fixture = Fixture::new();
        fixture.zip(
            ContentType::ResourcePack,
            "textures.zip",
            Some(r#"{"pack":{"pack_format":64,"description":"Textures"}}"#),
        );
        fixture.zip(ContentType::ShaderPack, "shaders.zip", None);
        std::fs::create_dir(fixture.dir(ContentType::ResourcePack).join("folder pack")).unwrap();
        std::fs::write(
            fixture.dir(ContentType::ShaderPack).join("bad.zip"),
            b"not a ZIP",
        )
        .unwrap();
        let resource = scan(
            &fixture.managed,
            &fixture.instance,
            ContentType::ResourcePack,
        )
        .unwrap();
        assert_eq!(resource.entries.len(), 2);
        assert!(
            resource
                .entries
                .iter()
                .all(|entry| entry.ownership == ContentOwnership::UserManaged)
        );
        assert!(
            resource
                .entries
                .iter()
                .any(|entry| entry.description.as_deref() == Some("Textures"))
        );
        assert!(
            !resource
                .entries
                .iter()
                .find(|entry| entry.file_type == "directory")
                .unwrap()
                .can_remove
        );
        let shader = scan(&fixture.managed, &fixture.instance, ContentType::ShaderPack).unwrap();
        assert_eq!(shader.entries.len(), 2);
        assert!(shader.entries.iter().any(|entry| {
            entry
                .warnings
                .iter()
                .any(|warning| warning.code == "archive_malformed")
        }));
    }

    #[test]
    fn provider_evidence_is_versioned_and_tamper_or_missing_is_reported() {
        let fixture = Fixture::new();
        let path = fixture.zip(
            ContentType::ResourcePack,
            "managed.zip",
            Some(r#"{"pack":{"pack_format":64}}"#),
        );
        let record = fixture.record(ContentType::ResourcePack, &path);
        let mut state = ContentState::empty();
        state.entries.push(record.clone());
        state.save(&fixture.managed, &fixture.instance).unwrap();
        assert_eq!(
            ContentState::load(&fixture.managed, &fixture.instance)
                .unwrap()
                .entries,
            vec![record.clone()]
        );
        let before = scan(
            &fixture.managed,
            &fixture.instance,
            ContentType::ResourcePack,
        )
        .unwrap();
        assert_eq!(
            before.entries[0].ownership,
            ContentOwnership::ProviderManaged
        );
        assert_eq!(
            before.entries[0].provenance.as_ref().unwrap().project_id,
            "opaque-project"
        );
        std::fs::write(&path, b"tampered").unwrap();
        let tampered = scan(
            &fixture.managed,
            &fixture.instance,
            ContentType::ResourcePack,
        )
        .unwrap();
        assert_eq!(tampered.entries[0].ownership, ContentOwnership::Unknown);
        assert!(!tampered.entries[0].can_remove);
        assert!(
            tampered.entries[0]
                .warnings
                .iter()
                .any(|warning| warning.code == "content_hash_mismatch")
        );
        std::fs::remove_file(path).unwrap();
        let missing = scan(
            &fixture.managed,
            &fixture.instance,
            ContentType::ResourcePack,
        )
        .unwrap();
        assert_eq!(missing.missing_managed, vec![record]);
    }

    #[test]
    fn malformed_future_and_duplicate_state_never_get_overwritten() {
        let fixture = Fixture::new();
        let state_file = fixture
            .root
            .join("instances")
            .join(fixture.instance.as_str())
            .join("content-managed.json");
        std::fs::write(&state_file, "broken").unwrap();
        assert!(matches!(
            ContentState::load(&fixture.managed, &fixture.instance),
            Err(ContentError::StateMalformed(_))
        ));
        assert!(
            ContentState::empty()
                .save(&fixture.managed, &fixture.instance)
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&state_file).unwrap(), "broken");
        std::fs::write(&state_file, r#"{"schemaVersion":99,"entries":[]}"#).unwrap();
        assert!(matches!(
            ContentState::load(&fixture.managed, &fixture.instance),
            Err(ContentError::StateVersion(99))
        ));
        std::fs::remove_file(state_file).unwrap();
        let path = fixture.zip(ContentType::ShaderPack, "duplicate.zip", None);
        let record = fixture.record(ContentType::ShaderPack, &path);
        let mut state = ContentState::empty();
        state.entries = vec![record.clone(), record];
        assert!(state.save(&fixture.managed, &fixture.instance).is_err());
    }

    #[test]
    fn activation_refuses_collisions_hash_mismatches_and_state_failures() {
        let fixture = Fixture::new();
        let source = fixture.root.join("verified.zip");
        std::fs::write(&source, b"verified bytes").unwrap();
        let mut record = fixture.record(ContentType::ResourcePack, &source);
        record.file_name = "collision.zip".into();
        let target = fixture
            .dir(ContentType::ResourcePack)
            .join(&record.file_name);
        std::fs::write(&target, b"user bytes").unwrap();
        assert!(matches!(
            activate_verified(
                &fixture.managed,
                &fixture.instance,
                record.clone(),
                &source,
                None
            ),
            Err(ContentError::Collision)
        ));
        assert_eq!(std::fs::read(&target).unwrap(), b"user bytes");
        std::fs::remove_file(&target).unwrap();
        let mut wrong = record.clone();
        wrong.sha256 = "0".repeat(64);
        assert!(matches!(
            activate_verified(&fixture.managed, &fixture.instance, wrong, &source, None),
            Err(ContentError::HashMismatch)
        ));
        assert!(!target.exists());
        let state_file = fixture
            .root
            .join("instances")
            .join(fixture.instance.as_str())
            .join("content-managed.json");
        std::fs::write(&state_file, "broken").unwrap();
        assert!(
            activate_verified(&fixture.managed, &fixture.instance, record, &source, None).is_err()
        );
        assert!(!target.exists());
    }

    #[test]
    fn activation_rolls_back_when_state_commit_fails() {
        let fixture = Fixture::new();
        let source = fixture.root.join("verified.zip");
        std::fs::write(&source, b"verified bytes").unwrap();
        let mut record = fixture.record(ContentType::ShaderPack, &source);
        record.file_name = "rollback.zip".into();
        let target = fixture.dir(ContentType::ShaderPack).join(&record.file_name);
        let result = activate_verified_with_commit(
            &fixture.managed,
            &fixture.instance,
            record,
            &source,
            None,
            |_| {
                Err(ContentError::StateMalformed(
                    "injected commit failure".into(),
                ))
            },
        );
        assert!(
            matches!(result, Err(ContentError::StateMalformed(_))),
            "{result:?}"
        );
        assert!(!target.exists());
        assert!(
            ContentState::load(&fixture.managed, &fixture.instance)
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn provider_remove_updates_state_and_stale_entry_is_refused() {
        let fixture = Fixture::new();
        let path = fixture.zip(
            ContentType::ResourcePack,
            "managed.zip",
            Some(r#"{"pack":{"pack_format":64}}"#),
        );
        let mut state = ContentState::empty();
        state
            .entries
            .push(fixture.record(ContentType::ResourcePack, &path));
        state.save(&fixture.managed, &fixture.instance).unwrap();
        let entry = scan(
            &fixture.managed,
            &fixture.instance,
            ContentType::ResourcePack,
        )
        .unwrap()
        .entries
        .remove(0);
        assert!(matches!(
            remove(
                &fixture.managed,
                &fixture.instance,
                ContentType::ResourcePack,
                "arbitrary-path"
            ),
            Err(ContentError::ChangedSinceScan)
        ));
        let after = remove(
            &fixture.managed,
            &fixture.instance,
            ContentType::ResourcePack,
            &entry.entry_id,
        )
        .unwrap();
        assert!(after.entries.is_empty());
        assert!(
            ContentState::load(&fixture.managed, &fixture.instance)
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn unsafe_directories_are_rejected_for_every_domain() {
        let fixture = Fixture::new();
        for kind in [
            ContentType::Mod,
            ContentType::ResourcePack,
            ContentType::ShaderPack,
        ] {
            let path = fixture
                .root
                .join("instances")
                .join(fixture.instance.as_str())
                .join(kind.directory_name());
            std::fs::write(&path, b"not a directory").unwrap();
            assert!(matches!(
                validate_directory(&fixture.managed, &fixture.instance, kind),
                Err(ContentError::UnsafePath)
            ));
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn substantial_resource_and_shader_inventories_stay_shallow() {
        let fixture = Fixture::new();
        for kind in [ContentType::ResourcePack, ContentType::ShaderPack] {
            let dir = fixture.dir(kind);
            for index in 0..120 {
                std::fs::write(
                    dir.join(format!("pack-{index:03}.zip")),
                    b"malformed but visible",
                )
                .unwrap();
            }
            let entries = scan(&fixture.managed, &fixture.instance, kind)
                .unwrap()
                .entries;
            assert_eq!(entries.len(), 120);
            assert!(
                entries
                    .iter()
                    .all(|entry| entry.ownership == ContentOwnership::UserManaged)
            );
        }
    }

    #[test]
    fn links_are_visible_but_not_followed_or_removed_when_supported() {
        let fixture = Fixture::new();
        let outside = fixture.root.join("outside.zip");
        std::fs::write(&outside, b"external user bytes").unwrap();
        for kind in [
            ContentType::Mod,
            ContentType::ResourcePack,
            ContentType::ShaderPack,
        ] {
            let dir = fixture.dir(kind);
            let link = dir.join("link.zip");
            #[cfg(windows)]
            let result = std::os::windows::fs::symlink_file(&outside, &link);
            #[cfg(unix)]
            let result = std::os::unix::fs::symlink(&outside, &link);
            if result.is_err() {
                continue;
            } // Windows installations without symlink privilege.
            let inventory = scan(&fixture.managed, &fixture.instance, kind).unwrap();
            assert_eq!(inventory.entries[0].file_type, "link");
            assert!(!inventory.entries[0].can_remove);
            assert!(
                remove(
                    &fixture.managed,
                    &fixture.instance,
                    kind,
                    &inventory.entries[0].entry_id
                )
                .is_err()
            );
            assert_eq!(std::fs::read(&outside).unwrap(), b"external user bytes");
        }
    }
}
