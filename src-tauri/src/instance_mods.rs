//! Local, filesystem-authoritative inventory and safe mutation of one
//! registered instance's Fabric mods.
//!
//! The scanner is deliberately shallow: it reads only direct children of the
//! derived `instances/<id>/mods` directory. JARs are untrusted input. Aurora
//! never executes them, never extracts them, and reads only a root-level
//! `fabric.mod.json` under a 256 KiB uncompressed limit. Archives with more
//! than 4,096 entries or files larger than 512 MiB are reported without
//! metadata inspection. Icons are intentionally not extracted in this phase.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::aurora;
use crate::instances::InstanceId;
use crate::paths::ManagedPaths;

const MAX_METADATA_BYTES: u64 = 256 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 4_096;
const MAX_INSPECTED_JAR_BYTES: u64 = 512 * 1024 * 1024;
const DISABLED_SUFFIX: &str = ".disabled";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInventory {
    pub instance_id: String,
    pub entries: Vec<ModEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModEntry {
    pub entry_id: String,
    pub file_name: String,
    pub display_name: String,
    pub enabled: bool,
    pub file_type: ModFileType,
    pub size_bytes: Option<u64>,
    pub modified_unix_millis: Option<u64>,
    pub ownership: ModOwnership,
    pub metadata: Option<FabricModMetadata>,
    pub warnings: Vec<ModWarning>,
    pub can_toggle: bool,
    pub can_remove: bool,
    pub action_blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ModFileType {
    EnabledJar,
    DisabledJar,
    UnexpectedFile,
    Directory,
    Link,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ModOwnership {
    LauncherManagedRequired,
    UserManaged,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricModMetadata {
    pub id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub environment: Option<String>,
    pub depends: Vec<ModRelation>,
    pub recommends: Vec<ModRelation>,
    pub suggests: Vec<ModRelation>,
    pub conflicts: Vec<ModRelation>,
    pub breaks: Vec<ModRelation>,
    pub has_declared_icon: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModRelation {
    pub mod_id: String,
    pub requirement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModWarning {
    pub code: String,
    pub message: String,
}

impl ModWarning {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FabricMetadataDocument {
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    #[serde(default)]
    authors: Vec<serde_json::Value>,
    environment: Option<String>,
    #[serde(default)]
    depends: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    recommends: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    suggests: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    conflicts: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    breaks: serde_json::Map<String, serde_json::Value>,
    icon: Option<serde_json::Value>,
}

/// Reads the authoritative local inventory for one validated instance.
pub fn scan(managed: &ManagedPaths, instance: &InstanceId) -> Result<ModInventory, ModError> {
    let mods = validate_mods_directory(managed, instance)?;
    let managed_file = managed_artifact_file_name(managed, instance)?;
    let mut entries = Vec::new();
    let children = std::fs::read_dir(&mods).map_err(|source| ModError::DirectoryRead { source })?;

    for child in children {
        match child {
            Ok(child) => entries.push(inspect_entry(&child.path(), managed_file.as_deref())),
            Err(source) => entries.push(ModEntry {
                entry_id: opaque_id(b"unreadable-directory-entry"),
                file_name: "Unreadable entry".to_owned(),
                display_name: "Unreadable entry".to_owned(),
                enabled: false,
                file_type: ModFileType::UnexpectedFile,
                size_bytes: None,
                modified_unix_millis: None,
                ownership: ModOwnership::Unknown,
                metadata: None,
                warnings: vec![ModWarning::new(
                    "entry_unreadable",
                    format!("A directory entry could not be inspected: {source}"),
                )],
                can_toggle: false,
                can_remove: false,
                action_blocked_reason: Some(
                    "Aurora cannot safely identify this directory entry.".to_owned(),
                ),
            }),
        }
    }

    derive_local_warnings(&mut entries);
    entries.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| {
                left.file_name
                    .to_lowercase()
                    .cmp(&right.file_name.to_lowercase())
            })
    });
    Ok(ModInventory {
        instance_id: instance.to_string(),
        entries,
    })
}

/// Proves that the mods directory is the exact derived directory beneath the
/// canonical managed instance. The canonical path is validation evidence;
/// callers keep using the ordinary platform path for OS APIs.
pub fn validate_mods_directory(
    managed: &ManagedPaths,
    instance: &InstanceId,
) -> Result<PathBuf, ModError> {
    let instance_paths = managed.instance_paths(instance);
    let mods = instance_paths.mods().to_path_buf();
    if !mods.is_dir() {
        return Err(ModError::DirectoryMissing);
    }
    let canonical_root = std::fs::canonicalize(managed.data_root())
        .map_err(|source| ModError::Boundary { source })?;
    let canonical_instances = std::fs::canonicalize(managed.instances_dir())
        .map_err(|source| ModError::Boundary { source })?;
    let canonical_instance = std::fs::canonicalize(instance_paths.root())
        .map_err(|source| ModError::Boundary { source })?;
    let canonical_mods =
        std::fs::canonicalize(&mods).map_err(|source| ModError::Boundary { source })?;

    if !canonical_instances.starts_with(&canonical_root)
        || canonical_instance != canonical_instances.join(instance.as_str())
        || canonical_mods != canonical_instance.join("mods")
    {
        return Err(ModError::BoundaryEscape);
    }
    Ok(mods)
}

fn managed_artifact_file_name(
    managed: &ManagedPaths,
    instance: &InstanceId,
) -> Result<Option<Vec<String>>, ModError> {
    let state = aurora::load_installed_state(managed, instance)
        .map_err(|error| ModError::InstalledState(error.to_string()))?;
    Ok(state.map(|state| {
        let mut files = vec![
            state
                .artifact()
                .relative_path()
                .strip_prefix("mods/")
                .expect("validated Aurora state always lives beneath mods")
                .to_owned(),
        ];
        if let Some(fabric_api) = state.fabric_api() {
            files.push(
                fabric_api
                    .artifact()
                    .relative_path()
                    .strip_prefix("mods/")
                    .expect("validated Fabric API state always lives beneath mods")
                    .to_owned(),
            );
        }
        files
    }))
}

fn inspect_entry(path: &Path, managed_files: Option<&[String]>) -> ModEntry {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unreadable entry".to_owned());
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) => {
            return unavailable_entry(file_name, format!("The entry could not be read: {source}"));
        }
    };
    let is_link = metadata.file_type().is_symlink() || is_reparse_point(&metadata);
    let file_type = if is_link {
        ModFileType::Link
    } else if metadata.is_dir() {
        ModFileType::Directory
    } else if metadata.is_file() && is_enabled_jar(&file_name) {
        ModFileType::EnabledJar
    } else if metadata.is_file() && is_disabled_jar(&file_name) {
        ModFileType::DisabledJar
    } else {
        ModFileType::UnexpectedFile
    };
    let enabled = file_type == ModFileType::EnabledJar;
    let is_managed = managed_files.is_some_and(|files| {
        files.iter().any(|managed| {
            file_name.eq_ignore_ascii_case(managed)
                || file_name.eq_ignore_ascii_case(&format!("{managed}{DISABLED_SUFFIX}"))
        })
    });
    let ownership = if is_managed {
        ModOwnership::LauncherManagedRequired
    } else if matches!(
        file_type,
        ModFileType::EnabledJar | ModFileType::DisabledJar
    ) {
        ModOwnership::UserManaged
    } else {
        ModOwnership::Unknown
    };
    let size_bytes = metadata.is_file().then_some(metadata.len());
    let modified_unix_millis = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    let mut id_material = Vec::new();
    id_material.extend_from_slice(file_name.as_bytes());
    id_material.extend_from_slice(
        format!("|{file_type:?}|{:?}|{modified_unix_millis:?}", size_bytes).as_bytes(),
    );
    let entry_id = opaque_id(&id_material);

    let (fabric_metadata, mut warnings) = if matches!(
        file_type,
        ModFileType::EnabledJar | ModFileType::DisabledJar
    ) {
        inspect_fabric_metadata(path, size_bytes.unwrap_or_default())
    } else {
        (None, Vec::new())
    };
    if file_type == ModFileType::Link {
        warnings.push(ModWarning::new(
            "link_not_managed",
            "Links and Windows reparse points are shown but never followed or modified.",
        ));
    } else if file_type == ModFileType::Directory {
        warnings.push(ModWarning::new(
            "nested_directory_ignored",
            "Nested directories are not scanned or managed as mods.",
        ));
    } else if file_type == ModFileType::UnexpectedFile {
        warnings.push(ModWarning::new(
            "unexpected_file",
            "This file is not an enabled or disabled JAR and is left untouched.",
        ));
    }
    if is_managed && !enabled {
        warnings.push(ModWarning::new(
            "required_mod_disabled",
            "A required managed mod is disabled outside the launcher; instance readiness may be damaged.",
        ));
    }
    let blocked_reason = match ownership {
        ModOwnership::LauncherManagedRequired => Some(
            "This mod is required and is maintained by the verified installation system."
                .to_owned(),
        ),
        ModOwnership::Unknown => {
            Some("Aurora cannot prove that this entry is a user-managed mod file.".to_owned())
        }
        ModOwnership::UserManaged => None,
    };
    let display_name = fabric_metadata
        .as_ref()
        .and_then(|metadata| metadata.name.as_deref())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            fabric_metadata
                .as_ref()
                .map(|metadata| metadata.id.trim())
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            if is_managed {
                "Aurora Client".to_owned()
            } else {
                file_name.clone()
            }
        });

    ModEntry {
        entry_id,
        file_name,
        display_name,
        enabled,
        file_type,
        size_bytes,
        modified_unix_millis,
        ownership,
        metadata: fabric_metadata,
        warnings,
        can_toggle: ownership == ModOwnership::UserManaged,
        can_remove: ownership == ModOwnership::UserManaged,
        action_blocked_reason: blocked_reason,
    }
}

fn unavailable_entry(file_name: String, reason: String) -> ModEntry {
    ModEntry {
        entry_id: opaque_id(file_name.as_bytes()),
        display_name: file_name.clone(),
        file_name,
        enabled: false,
        file_type: ModFileType::UnexpectedFile,
        size_bytes: None,
        modified_unix_millis: None,
        ownership: ModOwnership::Unknown,
        metadata: None,
        warnings: vec![ModWarning::new("entry_unreadable", reason)],
        can_toggle: false,
        can_remove: false,
        action_blocked_reason: Some("Aurora cannot safely inspect this entry.".to_owned()),
    }
}

fn inspect_fabric_metadata(
    path: &Path,
    jar_size: u64,
) -> (Option<FabricModMetadata>, Vec<ModWarning>) {
    if jar_size > MAX_INSPECTED_JAR_BYTES {
        return (
            None,
            vec![ModWarning::new(
                "jar_too_large",
                "Metadata inspection was skipped because this JAR exceeds the 512 MiB safety bound.",
            )],
        );
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(source) => {
            return (
                None,
                vec![ModWarning::new(
                    "jar_unreadable",
                    format!("The JAR could not be opened: {source}"),
                )],
            );
        }
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(error) => {
            return (
                None,
                vec![ModWarning::new(
                    "jar_malformed",
                    format!("This file is not a readable ZIP/JAR archive: {error}"),
                )],
            );
        }
    };
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return (
            None,
            vec![ModWarning::new(
                "jar_entry_limit",
                "Metadata inspection was skipped because the archive contains more than 4,096 entries.",
            )],
        );
    }
    let mut metadata_indexes = Vec::new();
    for index in 0..archive.len() {
        match archive.by_index(index) {
            Ok(entry) if entry.name() == "fabric.mod.json" => metadata_indexes.push(index),
            Ok(_) => {}
            Err(error) => {
                return (
                    None,
                    vec![ModWarning::new(
                        "jar_malformed",
                        format!("The JAR directory could not be inspected: {error}"),
                    )],
                );
            }
        }
    }
    if metadata_indexes.is_empty() {
        return (
            None,
            vec![ModWarning::new(
                "fabric_metadata_missing",
                "No root-level fabric.mod.json metadata was found.",
            )],
        );
    }
    if metadata_indexes.len() != 1 {
        return (
            None,
            vec![ModWarning::new(
                "fabric_metadata_duplicate",
                "The JAR contains more than one fabric.mod.json entry.",
            )],
        );
    }
    let mut entry = match archive.by_index(metadata_indexes[0]) {
        Ok(entry) => entry,
        Err(error) => {
            return (
                None,
                vec![ModWarning::new("jar_malformed", error.to_string())],
            );
        }
    };
    if entry.size() > MAX_METADATA_BYTES {
        return (
            None,
            vec![ModWarning::new(
                "fabric_metadata_too_large",
                "fabric.mod.json exceeds the 256 KiB uncompressed safety bound.",
            )],
        );
    }
    let mut bytes = Vec::with_capacity(entry.size().min(MAX_METADATA_BYTES) as usize);
    if let Err(error) = entry
        .by_ref()
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        return (
            None,
            vec![ModWarning::new(
                "fabric_metadata_unreadable",
                format!("fabric.mod.json could not be decompressed: {error}"),
            )],
        );
    }
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return (
            None,
            vec![ModWarning::new(
                "fabric_metadata_too_large",
                "fabric.mod.json exceeded the 256 KiB read limit.",
            )],
        );
    }
    let document: FabricMetadataDocument = match serde_json::from_slice(&bytes) {
        Ok(document) => document,
        Err(error) => {
            return (
                None,
                vec![ModWarning::new(
                    "fabric_metadata_malformed",
                    format!("fabric.mod.json is malformed: {error}"),
                )],
            );
        }
    };
    let Some(id) = document
        .id
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
    else {
        return (
            None,
            vec![ModWarning::new(
                "fabric_metadata_malformed",
                "fabric.mod.json does not contain a usable mod id.",
            )],
        );
    };
    let authors = document
        .authors
        .into_iter()
        .filter_map(|author| match author {
            serde_json::Value::String(name) => Some(name),
            serde_json::Value::Object(fields) => fields
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            _ => None,
        })
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .take(16)
        .collect();
    (
        Some(FabricModMetadata {
            id,
            name: clean_optional(document.name),
            version: clean_optional(document.version),
            description: clean_optional(document.description).map(|value| {
                if value.chars().count() > 2_000 {
                    value.chars().take(2_000).collect()
                } else {
                    value
                }
            }),
            authors,
            environment: clean_optional(document.environment),
            depends: relations(document.depends),
            recommends: relations(document.recommends),
            suggests: relations(document.suggests),
            conflicts: relations(document.conflicts),
            breaks: relations(document.breaks),
            has_declared_icon: document.icon.is_some(),
        }),
        Vec::new(),
    )
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn relations(fields: serde_json::Map<String, serde_json::Value>) -> Vec<ModRelation> {
    let mut relations: Vec<_> = fields
        .into_iter()
        .filter_map(|(mod_id, value)| {
            let mod_id = mod_id.trim().to_owned();
            if mod_id.is_empty() {
                return None;
            }
            let requirement = match value {
                serde_json::Value::String(value) => value,
                serde_json::Value::Array(values) => values
                    .into_iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
                    .join(" or "),
                value => value.to_string(),
            };
            Some(ModRelation {
                mod_id,
                requirement,
            })
        })
        .collect();
    relations.sort_by(|left, right| left.mod_id.cmp(&right.mod_id));
    relations
}

fn derive_local_warnings(entries: &mut [ModEntry]) {
    let enabled_ids: HashSet<String> = entries
        .iter()
        .filter(|entry| entry.enabled)
        .filter_map(|entry| {
            entry
                .metadata
                .as_ref()
                .map(|metadata| metadata.id.to_lowercase())
        })
        .collect();
    let mut id_counts = HashMap::<String, usize>::new();
    for id in entries.iter().filter_map(|entry| {
        entry
            .metadata
            .as_ref()
            .map(|metadata| metadata.id.to_lowercase())
    }) {
        *id_counts.entry(id).or_default() += 1;
    }
    let builtins = ["minecraft", "fabricloader", "java"];
    for entry in entries.iter_mut() {
        let Some(metadata) = &entry.metadata else {
            continue;
        };
        if id_counts
            .get(&metadata.id.to_lowercase())
            .copied()
            .unwrap_or_default()
            > 1
        {
            entry.warnings.push(ModWarning::new(
                "duplicate_mod_id",
                format!(
                    "More than one local artifact declares the mod id '{}'.",
                    metadata.id
                ),
            ));
        }
        if !entry.enabled {
            continue;
        }
        for dependency in &metadata.depends {
            let dependency_id = dependency.mod_id.to_lowercase();
            if builtins.contains(&dependency_id.as_str()) || enabled_ids.contains(&dependency_id) {
                continue;
            }
            entry.warnings.push(ModWarning::new(
                "required_dependency_missing",
                format!(
                    "Required dependency '{}' was not detected among enabled local mods.",
                    dependency.mod_id
                ),
            ));
        }
        for relation in metadata.conflicts.iter().chain(metadata.breaks.iter()) {
            if enabled_ids.contains(&relation.mod_id.to_lowercase()) {
                entry.warnings.push(ModWarning::new(
                    "declared_conflict_present",
                    format!(
                        "Declared conflict '{}' is present and enabled locally.",
                        relation.mod_id
                    ),
                ));
            }
        }
    }
}

/// Atomically renames one current user-owned JAR between `.jar` and
/// `.jar.disabled`, then returns a fresh authoritative inventory.
pub fn set_enabled(
    managed: &ManagedPaths,
    instance: &InstanceId,
    entry_id: &str,
    enabled: bool,
) -> Result<ModInventory, ModError> {
    with_mutation_lock(instance, || {
        let inventory = scan(managed, instance)?;
        let entry = resolve_mutable_entry(&inventory, entry_id)?;
        if entry.enabled == enabled {
            return Ok(inventory);
        }
        let mods = validate_mods_directory(managed, instance)?;
        let source = validate_current_regular_file(&mods, &entry.file_name)?;
        let target_name = if enabled {
            entry
                .file_name
                .strip_suffix(DISABLED_SUFFIX)
                .ok_or_else(|| ModError::State("the disabled filename is malformed".to_owned()))?
                .to_owned()
        } else {
            format!("{}{DISABLED_SUFFIX}", entry.file_name)
        };
        validate_file_name(&target_name)?;
        let target = mods.join(&target_name);
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err(ModError::TargetConflict(target_name));
        }
        std::fs::rename(&source, &target).map_err(|source| ModError::MutationIo { source })?;
        scan(managed, instance)
    })
}

/// Permanently removes one exact current user-owned local JAR, then returns
/// a fresh authoritative inventory. UI confirmation is required by product
/// policy; this backend still revalidates identity and ownership itself.
pub fn remove(
    managed: &ManagedPaths,
    instance: &InstanceId,
    entry_id: &str,
) -> Result<ModInventory, ModError> {
    with_mutation_lock(instance, || {
        let inventory = scan(managed, instance)?;
        let entry = resolve_mutable_entry(&inventory, entry_id)?;
        let mods = validate_mods_directory(managed, instance)?;
        let target = validate_current_regular_file(&mods, &entry.file_name)?;
        std::fs::remove_file(target).map_err(|source| ModError::MutationIo { source })?;
        scan(managed, instance)
    })
}

fn resolve_mutable_entry<'a>(
    inventory: &'a ModInventory,
    entry_id: &str,
) -> Result<&'a ModEntry, ModError> {
    let entry = inventory
        .entries
        .iter()
        .find(|entry| entry.entry_id == entry_id)
        .ok_or(ModError::StaleEntry)?;
    if entry.ownership == ModOwnership::LauncherManagedRequired {
        return Err(ModError::RequiredArtifact);
    }
    if entry.ownership != ModOwnership::UserManaged || !entry.can_remove || !entry.can_toggle {
        return Err(ModError::UnsafeEntry);
    }
    Ok(entry)
}

fn validate_current_regular_file(mods: &Path, file_name: &str) -> Result<PathBuf, ModError> {
    validate_file_name(file_name)?;
    let target = mods.join(file_name);
    let metadata = std::fs::symlink_metadata(&target).map_err(|_| ModError::StaleEntry)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(ModError::UnsafeEntry);
    }
    let canonical_target =
        std::fs::canonicalize(&target).map_err(|source| ModError::Boundary { source })?;
    let canonical_mods =
        std::fs::canonicalize(mods).map_err(|source| ModError::Boundary { source })?;
    if canonical_target.parent() != Some(canonical_mods.as_path()) {
        return Err(ModError::BoundaryEscape);
    }
    Ok(target)
}

fn validate_file_name(file_name: &str) -> Result<(), ModError> {
    let path = Path::new(file_name);
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || path.is_absolute()
        || path.components().count() != 1
        || file_name.contains(['/', '\\', ':'])
    {
        return Err(ModError::BoundaryEscape);
    }
    Ok(())
}

fn mutation_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_mutation_lock<T>(
    instance: &InstanceId,
    operation: impl FnOnce() -> Result<T, ModError>,
) -> Result<T, ModError> {
    let lock = {
        let mut locks = mutation_locks()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(instance.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _guard = lock.try_lock().map_err(|_| ModError::MutationInProgress)?;
    operation()
}

fn is_enabled_jar(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".jar")
}

fn is_disabled_jar(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".jar.disabled")
}

fn opaque_id(material: &[u8]) -> String {
    let digest: [u8; 32] = sha2::Sha256::digest(material).into();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[derive(Debug)]
pub enum ModError {
    DirectoryMissing,
    DirectoryRead { source: std::io::Error },
    Boundary { source: std::io::Error },
    BoundaryEscape,
    InstalledState(String),
    StaleEntry,
    RequiredArtifact,
    UnsafeEntry,
    TargetConflict(String),
    MutationInProgress,
    MutationIo { source: std::io::Error },
    State(String),
}

impl ModError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DirectoryMissing | Self::DirectoryRead { .. } | Self::Boundary { .. } => {
                "mod_inventory_unavailable"
            }
            Self::BoundaryEscape | Self::UnsafeEntry => "mod_entry_unsafe",
            Self::InstalledState(_) => "aurora_installation_invalid",
            Self::StaleEntry => "mod_entry_stale",
            Self::RequiredArtifact => "mod_required_artifact",
            Self::TargetConflict(_) => "mod_target_conflict",
            Self::MutationInProgress => "mod_mutation_in_progress",
            Self::MutationIo { .. } | Self::State(_) => "mod_mutation_failure",
        }
    }
}

impl fmt::Display for ModError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectoryMissing => {
                write!(formatter, "the instance mods directory does not exist")
            }
            Self::DirectoryRead { source } => write!(
                formatter,
                "the instance mods directory could not be read: {source}"
            ),
            Self::Boundary { source } => write!(
                formatter,
                "the managed mods boundary could not be resolved: {source}"
            ),
            Self::BoundaryEscape => write!(
                formatter,
                "the resolved mod entry is outside the managed mods directory"
            ),
            Self::InstalledState(reason) => write!(
                formatter,
                "Aurora ownership could not be established from installed state: {reason}"
            ),
            Self::StaleEntry => write!(
                formatter,
                "this mod changed since the inventory was loaded; refresh and try again"
            ),
            Self::RequiredArtifact => write!(
                formatter,
                "a required launcher-managed mod cannot be disabled or removed from Mods"
            ),
            Self::UnsafeEntry => write!(
                formatter,
                "this entry is not a regular user-managed mod file"
            ),
            Self::TargetConflict(name) => write!(
                formatter,
                "the target filename '{name}' already exists; nothing was overwritten"
            ),
            Self::MutationInProgress => write!(
                formatter,
                "another mod change is already in progress for this instance"
            ),
            Self::MutationIo { source } => {
                write!(formatter, "the local mod change failed: {source}")
            }
            Self::State(reason) => write!(formatter, "the local mod state is invalid: {reason}"),
        }
    }
}

impl std::error::Error for ModError {}

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
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join("aurora-mods-test")
                .join(format!("{name}-{}", uuid::Uuid::new_v4()));
            let managed = ManagedPaths::from_app_local_data_dir(root.clone()).unwrap();
            let instance = InstanceId::new(format!("fixture-{name}")).unwrap();
            let paths = managed.instance_paths(&instance);
            std::fs::create_dir_all(paths.mods()).unwrap();
            std::fs::write(
                paths.root().join(aurora::AURORA_INSTALLED_FILE_NAME),
                r#"{
                  "schemaVersion": 1,
                  "auroraVersion": "0.3.0",
                  "channel": "stable",
                  "minecraftVersion": "26.2",
                  "fabricLoaderVersion": "0.19.5",
                  "artifact": {
                    "relativePath": "mods/aurora-0.3.0.jar",
                    "sizeBytes": 4,
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                  },
                  "installationId": "fixture",
                  "installedAtUnixSeconds": 1
                }"#,
            )
            .unwrap();
            Self {
                root,
                managed,
                instance,
            }
        }

        fn mods(&self) -> PathBuf {
            self.managed
                .instance_paths(&self.instance)
                .mods()
                .to_path_buf()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn jar(path: &Path, metadata: Option<&[u8]>) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(
                "META-INF/MANIFEST.MF",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"Manifest-Version: 1.0").unwrap();
        if let Some(metadata) = metadata {
            writer
                .start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(metadata).unwrap();
        }
        writer.finish().unwrap();
    }

    fn metadata(id: &str, name: &str, depends: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "name": name,
            "version": "1.2.3",
            "description": "Fixture",
            "authors": ["Aurora Test", {"name": "Second"}],
            "environment": "client",
            "depends": serde_json::from_str::<serde_json::Value>(depends).unwrap(),
            "icon": "assets/fixture/icon.png"
        }))
        .unwrap()
    }

    #[test]
    fn inventory_discovers_enabled_disabled_and_resilient_failures() {
        let fixture = Fixture::new("inventory");
        jar(
            &fixture.mods().join("useful.jar"),
            Some(&metadata("useful", "Useful Mod", r#"{"minecraft": ">=1"}"#)),
        );
        jar(
            &fixture.mods().join("sleepy.jar.disabled"),
            Some(&metadata("sleepy", "Sleepy Mod", "{}")),
        );
        jar(&fixture.mods().join("library.jar"), None);
        std::fs::write(fixture.mods().join("broken.jar"), b"not a zip").unwrap();
        std::fs::write(fixture.mods().join("notes.txt"), b"keep me").unwrap();
        std::fs::create_dir(fixture.mods().join("nested")).unwrap();

        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        assert_eq!(inventory.entries.len(), 6);
        let useful = inventory
            .entries
            .iter()
            .find(|entry| entry.file_name == "useful.jar")
            .unwrap();
        assert_eq!(useful.display_name, "Useful Mod");
        assert!(useful.enabled && useful.can_toggle && useful.can_remove);
        assert_eq!(
            useful.metadata.as_ref().unwrap().authors,
            ["Aurora Test", "Second"]
        );
        assert!(useful.metadata.as_ref().unwrap().has_declared_icon);
        let sleepy = inventory
            .entries
            .iter()
            .find(|entry| entry.file_name.ends_with(".disabled"))
            .unwrap();
        assert_eq!(sleepy.display_name, "Sleepy Mod");
        assert!(!sleepy.enabled);
        assert!(
            inventory
                .entries
                .iter()
                .find(|entry| entry.file_name == "broken.jar")
                .unwrap()
                .warnings
                .iter()
                .any(|warning| warning.code == "jar_malformed")
        );
        assert_eq!(
            inventory
                .entries
                .iter()
                .find(|entry| entry.file_name == "nested")
                .unwrap()
                .file_type,
            ModFileType::Directory
        );
    }

    #[test]
    fn installed_state_protects_the_required_artifact() {
        let fixture = Fixture::new("managed");
        std::fs::write(fixture.mods().join("aurora-0.3.0.jar"), b"tiny").unwrap();
        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        let required = &inventory.entries[0];
        assert_eq!(required.ownership, ModOwnership::LauncherManagedRequired);
        assert!(!required.can_toggle && !required.can_remove);
        assert_eq!(
            set_enabled(
                &fixture.managed,
                &fixture.instance,
                &required.entry_id,
                false
            )
            .unwrap_err()
            .code(),
            "mod_required_artifact"
        );
        assert_eq!(
            remove(&fixture.managed, &fixture.instance, &required.entry_id)
                .unwrap_err()
                .code(),
            "mod_required_artifact"
        );
        assert!(fixture.mods().join("aurora-0.3.0.jar").is_file());
    }

    #[test]
    fn installed_state_also_protects_required_fabric_api() {
        let fixture = Fixture::new("fabric-api-managed");
        let state_path = fixture
            .managed
            .instance_paths(&fixture.instance)
            .root()
            .join(aurora::AURORA_INSTALLED_FILE_NAME);
        let mut state: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
        state["fabricApi"] = serde_json::json!({
            "version": "0.141.6+1.21.11",
            "artifact": {
                "relativePath": "mods/fabric-api-0.141.6+1.21.11.jar",
                "sizeBytes": 4,
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        });
        std::fs::write(&state_path, serde_json::to_string(&state).unwrap()).unwrap();
        let path = fixture.mods().join("fabric-api-0.141.6+1.21.11.jar");
        std::fs::write(&path, b"tiny").unwrap();
        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        let required = inventory
            .entries
            .iter()
            .find(|entry| entry.file_name == "fabric-api-0.141.6+1.21.11.jar")
            .unwrap();
        assert_eq!(required.ownership, ModOwnership::LauncherManagedRequired);
        assert!(!required.can_toggle && !required.can_remove);
        assert_eq!(
            remove(&fixture.managed, &fixture.instance, &required.entry_id)
                .unwrap_err()
                .code(),
            "mod_required_artifact"
        );
        assert!(path.is_file());
    }

    #[test]
    fn enable_disable_is_atomic_reversible_and_detects_collisions() {
        let fixture = Fixture::new("toggle");
        jar(
            &fixture.mods().join("toggle.jar"),
            Some(&metadata("toggle", "Toggle", "{}")),
        );
        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        let id = inventory.entries[0].entry_id.clone();
        let disabled = set_enabled(&fixture.managed, &fixture.instance, &id, false).unwrap();
        assert!(fixture.mods().join("toggle.jar.disabled").is_file());
        let disabled_entry = disabled
            .entries
            .iter()
            .find(|entry| entry.file_name == "toggle.jar.disabled")
            .unwrap();
        assert_eq!(disabled_entry.display_name, "Toggle");
        let enabled = set_enabled(
            &fixture.managed,
            &fixture.instance,
            &disabled_entry.entry_id,
            true,
        )
        .unwrap();
        assert!(
            enabled
                .entries
                .iter()
                .any(|entry| entry.file_name == "toggle.jar" && entry.enabled)
        );

        std::fs::rename(
            fixture.mods().join("toggle.jar"),
            fixture.mods().join("toggle.jar.disabled"),
        )
        .unwrap();
        std::fs::write(fixture.mods().join("toggle.jar"), b"collision").unwrap();
        let current = scan(&fixture.managed, &fixture.instance).unwrap();
        let disabled_id = current
            .entries
            .iter()
            .find(|entry| entry.file_name.ends_with(".disabled"))
            .unwrap()
            .entry_id
            .clone();
        assert_eq!(
            set_enabled(&fixture.managed, &fixture.instance, &disabled_id, true)
                .unwrap_err()
                .code(),
            "mod_target_conflict"
        );
    }

    #[test]
    fn remove_revalidates_identity_and_never_accepts_a_path() {
        let fixture = Fixture::new("remove");
        jar(
            &fixture.mods().join("remove.jar"),
            Some(&metadata("remove", "Remove", "{}")),
        );
        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        let id = inventory.entries[0].entry_id.clone();
        std::fs::write(fixture.mods().join("remove.jar"), b"changed after scan").unwrap();
        assert_eq!(
            remove(&fixture.managed, &fixture.instance, &id)
                .unwrap_err()
                .code(),
            "mod_entry_stale"
        );
        let refreshed = scan(&fixture.managed, &fixture.instance).unwrap();
        let fresh_id = refreshed.entries[0].entry_id.clone();
        let empty = remove(&fixture.managed, &fixture.instance, &fresh_id).unwrap();
        assert!(empty.entries.is_empty());
        assert_eq!(
            remove(&fixture.managed, &fixture.instance, "../remove.jar")
                .unwrap_err()
                .code(),
            "mod_entry_stale"
        );
    }

    #[test]
    fn derives_conservative_local_dependency_and_duplicate_warnings() {
        let fixture = Fixture::new("warnings");
        jar(
            &fixture.mods().join("first.jar"),
            Some(&metadata("same", "First", r#"{"missing": "*"}"#)),
        );
        jar(
            &fixture.mods().join("second.jar"),
            Some(&metadata("same", "Second", "{}")),
        );
        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        assert!(inventory.entries.iter().all(|entry| {
            entry
                .warnings
                .iter()
                .any(|warning| warning.code == "duplicate_mod_id")
        }));
        assert!(
            inventory
                .entries
                .iter()
                .find(|entry| entry.display_name == "First")
                .unwrap()
                .warnings
                .iter()
                .any(|warning| warning.code == "required_dependency_missing")
        );
    }

    #[test]
    fn bounds_metadata_reads_and_uses_filename_fallback() {
        let fixture = Fixture::new("bounds");
        let oversized = vec![b' '; MAX_METADATA_BYTES as usize + 1];
        jar(&fixture.mods().join("oversized.jar"), Some(&oversized));
        jar(
            &fixture.mods().join("malformed.jar"),
            Some(br#"{"name":"No id"}"#),
        );
        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        let oversized = inventory
            .entries
            .iter()
            .find(|entry| entry.file_name == "oversized.jar")
            .unwrap();
        assert_eq!(oversized.display_name, "oversized.jar");
        assert!(
            oversized
                .warnings
                .iter()
                .any(|warning| warning.code == "fabric_metadata_too_large")
        );
        let malformed = inventory
            .entries
            .iter()
            .find(|entry| entry.file_name == "malformed.jar")
            .unwrap();
        assert_eq!(malformed.display_name, "malformed.jar");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn links_are_visible_but_never_followed_or_mutated() {
        let fixture = Fixture::new("links");
        let outside = fixture.root.join("outside.jar");
        std::fs::write(&outside, b"outside").unwrap();
        let link = fixture.mods().join("linked.jar");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&outside, &link).is_err() {
            return;
        }
        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        let entry = &inventory.entries[0];
        assert_eq!(entry.file_type, ModFileType::Link);
        assert!(!entry.can_remove && !entry.can_toggle);
        assert_eq!(
            remove(&fixture.managed, &fixture.instance, &entry.entry_id)
                .unwrap_err()
                .code(),
            "mod_entry_unsafe"
        );
        assert_eq!(std::fs::read(outside).unwrap(), b"outside");
    }

    #[test]
    fn shallow_scan_handles_three_hundred_entries() {
        let fixture = Fixture::new("large");
        for index in 0..300 {
            std::fs::write(
                fixture.mods().join(format!("entry-{index:03}.txt")),
                b"fixture",
            )
            .unwrap();
        }
        let inventory = scan(&fixture.managed, &fixture.instance).unwrap();
        assert_eq!(inventory.entries.len(), 300);
        assert_eq!(
            inventory.entries.first().unwrap().file_name,
            "entry-000.txt"
        );
    }
}
