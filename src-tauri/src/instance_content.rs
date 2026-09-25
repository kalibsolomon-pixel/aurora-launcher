//! Provider-independent instance content. Filesystem authority stays here: a
//! validated instance id and a closed content type select a direct child of
//! the isolated game directory. Provider records are evidence, never guesses.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use futures_util::{StreamExt as _, stream};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::cache::ArtifactCache;
use crate::downloads::{ArtifactSource, Sha512ArtifactSource};
use crate::instances::InstanceId;
use crate::integrity::{ArtifactDigest, verify_file};
use crate::paths::ManagedPaths;

const SCHEMA_VERSION: u32 = 2;
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
    /// A direct user choice survives even while another installed item needs
    /// this artifact. Legacy records are migrated conservatively to true.
    pub explicitly_retained: bool,
    /// Required edges that were actually satisfied at installation time.
    /// Remote dependency metadata above is descriptive, not ownership.
    pub requires: Vec<ProviderIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderIdentity {
    pub content_type: ContentType,
    pub provider: String,
    pub project_id: String,
}

impl ProviderRecord {
    pub fn identity(&self) -> ProviderIdentity {
        ProviderIdentity {
            content_type: self.content_type,
            provider: self.provider.clone(),
            project_id: self.project_id.clone(),
        }
    }
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
        if self.requires.iter().any(|edge| {
            edge.provider.trim().is_empty()
                || edge.provider.len() > 256
                || edge.project_id.trim().is_empty()
                || edge.project_id.len() > 256
        }) {
            return Err(ContentError::StateMalformed(
                "provider edge is invalid".into(),
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
        let mut value = value;
        if version == 1 {
            let entries = value
                .get_mut("entries")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or_else(|| ContentError::StateMalformed("entries are required".into()))?;
            for entry in entries {
                let fields = entry.as_object_mut().ok_or_else(|| {
                    ContentError::StateMalformed("provider record is invalid".into())
                })?;
                if fields.contains_key("explicitlyRetained") || fields.contains_key("requires") {
                    return Err(ContentError::StateMalformed(
                        "v1 record contains v2 fields".into(),
                    ));
                }
                fields.insert("explicitlyRetained".into(), serde_json::Value::Bool(true));
                fields.insert("requires".into(), serde_json::json!([]));
            }
            value["schemaVersion"] = serde_json::json!(SCHEMA_VERSION);
        } else if version != u64::from(SCHEMA_VERSION) {
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
        let mut identities = HashSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !names.insert((entry.content_type, entry.file_name.to_lowercase())) {
                return Err(ContentError::StateMalformed(
                    "duplicate provider filename".into(),
                ));
            }
            if !identities.insert(entry.identity()) {
                return Err(ContentError::StateMalformed(
                    "duplicate provider project".into(),
                ));
            }
        }
        for entry in &self.entries {
            let mut edges = HashSet::new();
            for edge in &entry.requires {
                if !edges.insert(edge) || edge == &entry.identity() || !identities.contains(edge) {
                    return Err(ContentError::StateMalformed(
                        "invalid installed dependency edge".into(),
                    ));
                }
            }
        }
        let graph: HashMap<_, _> = self
            .entries
            .iter()
            .map(|entry| (entry.identity(), entry.requires.as_slice()))
            .collect();
        fn cycle(
            node: &ProviderIdentity,
            graph: &HashMap<ProviderIdentity, &[ProviderIdentity]>,
            active: &mut HashSet<ProviderIdentity>,
            done: &mut HashSet<ProviderIdentity>,
        ) -> bool {
            if done.contains(node) {
                return false;
            }
            if !active.insert(node.clone()) {
                return true;
            }
            if graph
                .get(node)
                .is_some_and(|edges| edges.iter().any(|edge| cycle(edge, graph, active, done)))
            {
                return true;
            }
            active.remove(node);
            done.insert(node.clone());
            false
        }
        let mut done = HashSet::new();
        for identity in &identities {
            if cycle(identity, &graph, &mut HashSet::new(), &mut done) {
                return Err(ContentError::StateMalformed(
                    "installed dependency graph contains a cycle".into(),
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

    pub fn load_and_migrate(
        managed: &ManagedPaths,
        instance: &InstanceId,
    ) -> Result<Self, ContentError> {
        with_instance_lock(instance, || {
            Self::load_and_migrate_locked(managed, instance)
        })
    }

    fn load_and_migrate_locked(
        managed: &ManagedPaths,
        instance: &InstanceId,
    ) -> Result<Self, ContentError> {
        let path = state_path(managed, instance)?;
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::empty()),
            Err(error) => return Err(ContentError::Io(error)),
        };
        let legacy = serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|error| ContentError::StateMalformed(error.to_string()))?
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            == Some(1);
        let state = Self::from_json(&text)?;
        if legacy {
            state.save(managed, instance)?;
        }
        Ok(state)
    }

    pub fn required_by(&self, identity: &ProviderIdentity) -> Vec<ProviderIdentity> {
        self.entries
            .iter()
            .filter(|record| record.requires.contains(identity))
            .map(ProviderRecord::identity)
            .collect()
    }

    pub fn find(&self, identity: &ProviderIdentity) -> Option<&ProviderRecord> {
        self.entries
            .iter()
            .find(|record| record.identity() == *identity)
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

pub fn validate_provider_file(
    managed: &ManagedPaths,
    instance: &InstanceId,
    record: &ProviderRecord,
) -> Result<PathBuf, ContentError> {
    let directory = validate_directory(managed, instance, record.content_type)?;
    let path = directory.join(&record.file_name);
    let metadata = std::fs::symlink_metadata(&path).map_err(ContentError::Io)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(ContentError::UnsafePath);
    }
    let digest = ArtifactDigest::parse(&record.sha256).map_err(|_| ContentError::HashMismatch)?;
    verify_file(&path, &digest, None).map_err(|_| ContentError::HashMismatch)?;
    Ok(path)
}

/// Promote an already installed dependency when the user chooses it directly.
/// The exact bytes are revalidated and no network acquisition occurs.
pub fn retain_provider(
    managed: &ManagedPaths,
    instance: &InstanceId,
    identity: &ProviderIdentity,
) -> Result<ProviderRecord, ContentError> {
    with_instance_lock(instance, || {
        let mut state = ContentState::load(managed, instance)?;
        let record = state
            .entries
            .iter_mut()
            .find(|record| record.identity() == *identity)
            .ok_or(ContentError::ChangedSinceScan)?;
        validate_provider_file(managed, instance, record)?;
        if !record.explicitly_retained {
            record.explicitly_retained = true;
            let updated = record.clone();
            state.save(managed, instance)?;
            Ok(updated)
        } else {
            Ok(record.clone())
        }
    })
}

fn required_edges(
    record: &ProviderRecord,
    identities: &[ProviderIdentity],
) -> Vec<ProviderIdentity> {
    record
        .dependencies
        .iter()
        .filter(|dependency| dependency.kind == DependencyKind::Required)
        .filter_map(|dependency| {
            identities
                .iter()
                .find(|identity| {
                    identity.provider == dependency.provider
                        && identity.project_id == dependency.project_id
                })
                .cloned()
        })
        .collect()
}

fn prune_orphans(state: &mut ContentState) {
    loop {
        let removable: HashSet<_> = state
            .entries
            .iter()
            .filter(|record| {
                !record.explicitly_retained && state.required_by(&record.identity()).is_empty()
            })
            .map(ProviderRecord::identity)
            .collect();
        if removable.is_empty() {
            break;
        }
        state
            .entries
            .retain(|record| !removable.contains(&record.identity()));
    }
}

pub fn removal_state(
    current: &ContentState,
    identity: &ProviderIdentity,
) -> Result<ContentState, ContentError> {
    let mut next = current.clone();
    let record = next
        .entries
        .iter_mut()
        .find(|record| record.identity() == *identity)
        .ok_or(ContentError::ChangedSinceScan)?;
    if !record.explicitly_retained && !current.required_by(identity).is_empty() {
        return Err(ContentError::RequiredByInstalledContent);
    }
    record.explicitly_retained = false;
    prune_orphans(&mut next);
    next.validate()?;
    Ok(next)
}

pub fn updated_state(
    current: &ContentState,
    root: &ProviderIdentity,
    mut replacements: Vec<ProviderRecord>,
) -> Result<ContentState, ContentError> {
    let old = current.find(root).ok_or(ContentError::ChangedSinceScan)?;
    if !old.explicitly_retained || !current.required_by(root).is_empty() {
        return Err(ContentError::RequiredByInstalledContent);
    }
    if replacements.last().map(ProviderRecord::identity).as_ref() != Some(root) {
        return Err(ContentError::StateMalformed(
            "update graph has no root".into(),
        ));
    }
    let mut next = current.clone();
    next.entries.retain(|record| record.identity() != *root);
    let mut identities: Vec<_> = next.entries.iter().map(ProviderRecord::identity).collect();
    for replacement in &replacements {
        let identity = replacement.identity();
        if identities.contains(&identity) {
            return Err(ContentError::Collision);
        }
        identities.push(identity);
    }
    for replacement in &mut replacements {
        replacement.explicitly_retained = replacement.identity() == *root;
        replacement.requires = required_edges(replacement, &identities);
    }
    let new_identities: HashSet<_> = replacements.iter().map(ProviderRecord::identity).collect();
    next.entries.extend(replacements);
    prune_orphans(&mut next);
    if new_identities
        .iter()
        .any(|identity| next.find(identity).is_none())
    {
        return Err(ContentError::StateMalformed(
            "update graph contains an unowned artifact".into(),
        ));
    }
    next.validate()?;
    Ok(next)
}

pub fn update_preview_state(
    current: &ContentState,
    root: &ProviderIdentity,
    plans: &[ProviderInstallPlan],
) -> Result<ContentState, ContentError> {
    updated_state(
        current,
        root,
        plans
            .iter()
            .map(|plan| plan.record("0".repeat(64)))
            .collect(),
    )
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
        can_remove: file_type == "zip" && ownership == ContentOwnership::UserManaged,
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

/// Backend-only normalized plan. Source URL authority belongs to its adapter.
/// The installed SHA-256 is filled only after expected-digest acquisition.
pub struct ProviderInstallPlan {
    pub content_type: ContentType,
    pub provider: String,
    pub project_id: String,
    pub version_id: String,
    pub file_id: String,
    pub file_name: String,
    pub display_version: Option<String>,
    pub compatibility: ContentCompatibility,
    pub dependencies: Vec<ProviderDependency>,
    pub source: ProviderArtifactSource,
}

pub enum ProviderArtifactSource {
    Sha256(ArtifactSource),
    Sha512(Sha512ArtifactSource),
}

impl ProviderInstallPlan {
    fn record(&self, sha256: String) -> ProviderRecord {
        ProviderRecord {
            content_type: self.content_type,
            provider: self.provider.clone(),
            project_id: self.project_id.clone(),
            version_id: self.version_id.clone(),
            file_id: self.file_id.clone(),
            file_name: self.file_name.clone(),
            sha256,
            display_version: self.display_version.clone(),
            compatibility: self.compatibility.clone(),
            dependencies: self.dependencies.clone(),
            explicitly_retained: true,
            requires: Vec::new(),
        }
    }
}

pub async fn install_provider_artifact(
    managed: &ManagedPaths,
    instance: &InstanceId,
    plan: ProviderInstallPlan,
) -> Result<ContentInventory, ContentError> {
    let cache = ArtifactCache::new(managed.clone());
    let artifact = match &plan.source {
        ProviderArtifactSource::Sha256(source) => cache.acquire(source).await,
        ProviderArtifactSource::Sha512(source) => cache.acquire_sha512(source).await,
    }
    .map_err(|error| ContentError::Acquisition(error.to_string()))?;
    let record = plan.record(artifact.sha256.as_hex());
    record.validate()?;
    activate_verified(
        managed,
        instance,
        record,
        &artifact.path,
        Some(artifact.bytes),
    )
}

/// Acquire a dependency graph with bounded concurrency, then activate every
/// file under one instance lock. State is written only after all names exist;
/// any failure before that removes only names created by this transaction.
pub async fn install_provider_plans(
    managed: &ManagedPaths,
    instance: &InstanceId,
    plans: Vec<ProviderInstallPlan>,
) -> Result<Vec<ProviderRecord>, ContentError> {
    let acquired = acquire_provider_plans(managed, plans).await?;
    activate_provider_transaction(managed, instance, acquired, |state| {
        state.save(managed, instance)
    })
}

async fn acquire_provider_plans(
    managed: &ManagedPaths,
    plans: Vec<ProviderInstallPlan>,
) -> Result<Vec<(ProviderRecord, PathBuf, u64)>, ContentError> {
    if plans.is_empty() || plans.len() > 64 {
        return Err(ContentError::StateMalformed(
            "provider plan size is invalid".into(),
        ));
    }
    for plan in &plans {
        validate_file_name(&plan.file_name)?;
        let provisional = plan.record("0".repeat(64));
        provisional.validate()?;
    }
    let cache = Arc::new(ArtifactCache::new(managed.clone()));
    let acquired = stream::iter(plans.into_iter().map(|plan| {
        let cache = cache.clone();
        async move {
            let artifact = match &plan.source {
                ProviderArtifactSource::Sha256(source) => cache.acquire(source).await,
                ProviderArtifactSource::Sha512(source) => cache.acquire_sha512(source).await,
            }
            .map_err(|error| ContentError::Acquisition(error.to_string()))?;
            let record = plan.record(artifact.sha256.as_hex());
            record.validate()?;
            Ok::<_, ContentError>((record, artifact.path, artifact.bytes))
        }
    }))
    .buffered(8)
    .collect::<Vec<_>>()
    .await;
    acquired.into_iter().collect::<Result<Vec<_>, _>>()
}

fn activate_provider_transaction(
    managed: &ManagedPaths,
    instance: &InstanceId,
    mut acquired: Vec<(ProviderRecord, PathBuf, u64)>,
    commit: impl FnOnce(&ContentState) -> Result<(), ContentError>,
) -> Result<Vec<ProviderRecord>, ContentError> {
    with_instance_lock(instance, || {
        let mut state = ContentState::load(managed, instance)?;
        let direct = acquired
            .last()
            .map(|(record, _, _)| record.identity())
            .ok_or_else(|| ContentError::StateMalformed("empty provider graph".into()))?;
        let identities: Vec<_> = state
            .entries
            .iter()
            .map(ProviderRecord::identity)
            .chain(acquired.iter().map(|(record, _, _)| record.identity()))
            .collect();
        for (record, _, _) in &mut acquired {
            record.explicitly_retained = record.identity() == direct;
            record.requires = record
                .dependencies
                .iter()
                .filter(|dependency| dependency.kind == DependencyKind::Required)
                .filter_map(|dependency| {
                    identities
                        .iter()
                        .find(|identity| {
                            identity.provider == dependency.provider
                                && identity.project_id == dependency.project_id
                        })
                        .cloned()
                })
                .collect();
        }
        let mut keys = HashSet::new();
        let mut targets = Vec::new();
        let required = crate::instance_mods::managed_artifact_file_name(managed, instance)
            .map_err(|error| ContentError::StateMalformed(error.to_string()))?
            .unwrap_or_default();
        for (record, _, _) in &acquired {
            let key = (record.content_type, record.file_name.to_lowercase());
            if !keys.insert(key.clone())
                || state.entries.iter().any(|entry| {
                    entry.content_type == record.content_type
                        && entry.file_name.eq_ignore_ascii_case(&record.file_name)
                })
                || (record.content_type == ContentType::Mod
                    && required
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&record.file_name)))
            {
                return Err(ContentError::Collision);
            }
            let directory = ensure_directory(managed, instance, record.content_type)?;
            let target = directory.join(&record.file_name);
            if std::fs::symlink_metadata(&target).is_ok()
                || std::fs::read_dir(&directory)
                    .map_err(ContentError::Io)?
                    .any(|item| {
                        item.is_ok_and(|item| {
                            item.file_name()
                                .to_string_lossy()
                                .eq_ignore_ascii_case(&record.file_name)
                        })
                    })
            {
                return Err(ContentError::Collision);
            }
            targets.push(target);
        }
        let mut created = Vec::new();
        let result = (|| {
            for ((record, source, bytes), target) in acquired.iter().zip(targets.iter()) {
                let digest = ArtifactDigest::parse(&record.sha256)
                    .map_err(|_| ContentError::HashMismatch)?;
                verify_file(source, &digest, Some(*bytes))
                    .map_err(|_| ContentError::HashMismatch)?;
                validate_provider_mod_artifact(record, source, *bytes)?;
                let temporary =
                    target.with_file_name(format!(".content-installing-{}", uuid::Uuid::new_v4()));
                if let Err(error) = std::fs::copy(source, &temporary) {
                    let _ = std::fs::remove_file(&temporary);
                    return Err(ContentError::Io(error));
                }
                if verify_file(&temporary, &digest, Some(*bytes)).is_err() {
                    let _ = std::fs::remove_file(&temporary);
                    return Err(ContentError::HashMismatch);
                }
                let linked = std::fs::hard_link(&temporary, target);
                let _ = std::fs::remove_file(&temporary);
                if let Err(error) = linked {
                    return if target.exists() {
                        Err(ContentError::Collision)
                    } else {
                        Err(ContentError::Io(error))
                    };
                }
                created.push(target.clone());
            }
            state
                .entries
                .extend(acquired.iter().map(|(record, _, _)| record.clone()));
            commit(&state)?;
            Ok(())
        })();
        if let Err(error) = result {
            for path in created.into_iter().rev() {
                std::fs::remove_file(path).map_err(ContentError::Io)?;
            }
            return Err(error);
        }
        Ok(acquired.into_iter().map(|(record, _, _)| record).collect())
    })
}

fn same_file(left: &ProviderRecord, right: &ProviderRecord) -> bool {
    left.content_type == right.content_type
        && left.file_name == right.file_name
        && left.sha256 == right.sha256
}

struct RetiredFile {
    target: PathBuf,
    backup: PathBuf,
    rollback_copy: PathBuf,
}

/// Commit one provider-independent lifecycle state transition. Every old file
/// has a verified rollback copy before any target is moved; every new file is
/// copied and verified from the content-addressed store before activation.
fn apply_lifecycle_state(
    managed: &ManagedPaths,
    instance: &InstanceId,
    expected: &ContentState,
    next: &ContentState,
    acquired: &[(ProviderRecord, PathBuf, u64)],
) -> Result<(), ContentError> {
    apply_lifecycle_state_with_hooks(
        managed,
        instance,
        expected,
        next,
        acquired,
        |state| state.save(managed, instance),
        || Ok(()),
    )
}

fn apply_lifecycle_state_with_hooks(
    managed: &ManagedPaths,
    instance: &InstanceId,
    expected: &ContentState,
    next: &ContentState,
    acquired: &[(ProviderRecord, PathBuf, u64)],
    commit: impl FnOnce(&ContentState) -> Result<(), ContentError>,
    before_cleanup: impl FnOnce() -> Result<(), ContentError>,
) -> Result<(), ContentError> {
    with_instance_lock(instance, || {
        let current = ContentState::load(managed, instance)?;
        if &current != expected {
            return Err(ContentError::ChangedSinceScan);
        }
        next.validate()?;
        let required = crate::instance_mods::managed_artifact_file_name(managed, instance)
            .map_err(|error| ContentError::StateMalformed(error.to_string()))?
            .unwrap_or_default();
        let protected = |record: &ProviderRecord| {
            record.content_type == ContentType::Mod
                && required
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&record.file_name))
        };
        for record in &current.entries {
            if protected(record) {
                return Err(ContentError::Collision);
            }
            validate_provider_file(managed, instance, record)?;
        }
        let retired: Vec<_> = current
            .entries
            .iter()
            .filter(|record| {
                next.find(&record.identity())
                    .is_none_or(|updated| !same_file(record, updated))
            })
            .collect();
        let incoming: Vec<_> = next
            .entries
            .iter()
            .filter(|record| {
                current
                    .find(&record.identity())
                    .is_none_or(|old| !same_file(old, record))
            })
            .collect();
        if incoming.len() != acquired.len() {
            return Err(ContentError::StateMalformed(
                "acquired graph does not match lifecycle state".into(),
            ));
        }
        let retired_paths: HashSet<_> = retired
            .iter()
            .map(|record| {
                validate_directory(managed, instance, record.content_type)
                    .map(|dir| dir.join(&record.file_name))
            })
            .collect::<Result<_, _>>()?;
        let mut staged = Vec::<(PathBuf, PathBuf)>::new();
        let mut rollback = Vec::<RetiredFile>::new();
        let mut activated = Vec::<PathBuf>::new();
        let mut moved = 0usize;
        let mut state_committed = false;
        let result = (|| {
            let mut targets = HashSet::new();
            for record in &incoming {
                if protected(record)
                    || !targets.insert((record.content_type, record.file_name.to_lowercase()))
                {
                    return Err(ContentError::Collision);
                }
                let (acquired_record, source, size) = acquired
                    .iter()
                    .find(|(item, _, _)| item.identity() == record.identity())
                    .ok_or_else(|| {
                        ContentError::StateMalformed("acquired artifact is missing".into())
                    })?;
                let mut normalized_acquired = acquired_record.clone();
                normalized_acquired.explicitly_retained = record.explicitly_retained;
                normalized_acquired.requires = record.requires.clone();
                if normalized_acquired != **record {
                    return Err(ContentError::StateMalformed(
                        "acquired artifact changed".into(),
                    ));
                }
                let directory = ensure_directory(managed, instance, record.content_type)?;
                let target = directory.join(&record.file_name);
                if !retired_paths.contains(&target)
                    && (std::fs::symlink_metadata(&target).is_ok()
                        || std::fs::read_dir(&directory)
                            .map_err(ContentError::Io)?
                            .any(|entry| {
                                entry.is_ok_and(|entry| {
                                    entry
                                        .file_name()
                                        .to_string_lossy()
                                        .eq_ignore_ascii_case(&record.file_name)
                                })
                            }))
                {
                    return Err(ContentError::Collision);
                }
                let digest = ArtifactDigest::parse(&record.sha256)
                    .map_err(|_| ContentError::HashMismatch)?;
                verify_file(source, &digest, Some(*size))
                    .map_err(|_| ContentError::HashMismatch)?;
                validate_provider_mod_artifact(record, source, *size)?;
                let stage = directory.join(format!(".content-staged-{}", uuid::Uuid::new_v4()));
                staged.push((target, stage.clone()));
                std::fs::copy(source, &stage).map_err(ContentError::Io)?;
                verify_file(&stage, &digest, Some(*size))
                    .map_err(|_| ContentError::HashMismatch)?;
            }
            for record in &retired {
                let target = validate_provider_file(managed, instance, record)?;
                let directory = target.parent().ok_or(ContentError::UnsafePath)?;
                let backup = directory.join(format!(".content-retired-{}", uuid::Uuid::new_v4()));
                let rollback_copy =
                    directory.join(format!(".content-rollback-{}", uuid::Uuid::new_v4()));
                std::fs::copy(&target, &rollback_copy).map_err(ContentError::Io)?;
                rollback.push(RetiredFile {
                    target,
                    backup,
                    rollback_copy,
                });
                let digest = ArtifactDigest::parse(&record.sha256)
                    .map_err(|_| ContentError::HashMismatch)?;
                verify_file(
                    &rollback.last().expect("just pushed").rollback_copy,
                    &digest,
                    None,
                )
                .map_err(|_| ContentError::HashMismatch)?;
            }
            for file in &rollback {
                std::fs::rename(&file.target, &file.backup).map_err(ContentError::Io)?;
                moved += 1;
            }
            for (target, stage) in &staged {
                std::fs::hard_link(stage, target).map_err(|error| {
                    if target.exists() {
                        ContentError::Collision
                    } else {
                        ContentError::Io(error)
                    }
                })?;
                activated.push(target.clone());
            }
            commit(next)?;
            state_committed = true;
            before_cleanup()?;
            for file in &rollback {
                std::fs::remove_file(&file.backup).map_err(ContentError::Io)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let mut rollback_failed = false;
            for target in activated.iter().rev() {
                if std::fs::remove_file(target).is_err() {
                    rollback_failed = true;
                }
            }
            for file in rollback.iter().take(moved).rev() {
                let source = if file.backup.exists() {
                    &file.backup
                } else {
                    &file.rollback_copy
                };
                if std::fs::hard_link(source, &file.target).is_err() {
                    rollback_failed = true;
                }
            }
            if state_committed && current.save(managed, instance).is_err() {
                rollback_failed = true;
            }
            for (_, stage) in &staged {
                let _ = std::fs::remove_file(stage);
            }
            if rollback_failed {
                return Err(ContentError::StateMalformed(
                    "lifecycle rollback could not restore every file".into(),
                ));
            }
            for file in &rollback {
                let _ = std::fs::remove_file(&file.backup);
                let _ = std::fs::remove_file(&file.rollback_copy);
            }
            return Err(error);
        }
        for (_, stage) in &staged {
            let _ = std::fs::remove_file(stage);
        }
        for file in &rollback {
            let _ = std::fs::remove_file(&file.rollback_copy);
        }
        Ok(())
    })
}

pub fn remove_provider_graph(
    managed: &ManagedPaths,
    instance: &InstanceId,
    expected: &ContentState,
    identity: &ProviderIdentity,
) -> Result<ContentState, ContentError> {
    let next = removal_state(expected, identity)?;
    apply_lifecycle_state(managed, instance, expected, &next, &[])?;
    Ok(next)
}

pub async fn update_provider_graph(
    managed: &ManagedPaths,
    instance: &InstanceId,
    expected: &ContentState,
    root: &ProviderIdentity,
    plans: Vec<ProviderInstallPlan>,
) -> Result<ContentState, ContentError> {
    let acquired = acquire_provider_plans(managed, plans).await?;
    let next = updated_state(
        expected,
        root,
        acquired
            .iter()
            .map(|(record, _, _)| record.clone())
            .collect(),
    )?;
    apply_lifecycle_state(managed, instance, expected, &next, &acquired)?;
    Ok(next)
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

fn validate_provider_mod_artifact(
    record: &ProviderRecord,
    path: &Path,
    bytes: u64,
) -> Result<(), ContentError> {
    if record.content_type != ContentType::Mod {
        return Ok(());
    }
    let (metadata, _) = crate::instance_mods::inspect_fabric_metadata(path, bytes);
    let Some(metadata) = metadata else {
        return Err(ContentError::InvalidProviderArtifact);
    };
    if metadata.id.eq_ignore_ascii_case("aurora") || metadata.id.eq_ignore_ascii_case("fabric-api")
    {
        return Err(ContentError::Collision);
    }
    Ok(())
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
        let bytes = std::fs::metadata(verified_path)
            .map_err(ContentError::Io)?
            .len();
        validate_provider_mod_artifact(&record, verified_path, bytes)?;
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
    InvalidProviderArtifact,
    RequiredByInstalledContent,
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
            Self::InvalidProviderArtifact => "content_invalid_artifact",
            Self::RequiredByInstalledContent => "content_required_by_installed",
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
            Self::InvalidProviderArtifact => write!(
                formatter,
                "the provider artifact is not a valid Fabric mod JAR"
            ),
            Self::RequiredByInstalledContent => {
                formatter.write_str("this content is still required by another installed item")
            }
        }
    }
}
impl std::error::Error for ContentError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestRequest, TestResponse, TestServer};
    use sha2::Sha512;
    use std::io::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
                explicitly_retained: true,
                requires: vec![],
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn transaction_record(kind: ContentType, name: &str, bytes: &[u8]) -> ProviderRecord {
        ProviderRecord {
            content_type: kind,
            provider: "modrinth".into(),
            project_id: "AAAABBBB".into(),
            version_id: "11112222".into(),
            file_id: format!("{:x}", Sha512::digest(bytes)),
            file_name: name.into(),
            sha256: format!("{:x}", sha2::Sha256::digest(bytes)),
            display_version: Some("1.0.0".into()),
            compatibility: ContentCompatibility {
                minecraft_versions: vec!["1.21.11".into()],
                loader: (kind == ContentType::Mod).then(|| "fabric".into()),
                environment: Some("client_and_server".into()),
            },
            dependencies: vec![],
            explicitly_retained: true,
            requires: vec![],
        }
    }

    fn fixture_fabric_jar() -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(br#"{"schemaVersion":1,"id":"fixture","version":"1.0.0"}"#)
            .unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn transaction_rolls_back_after_second_file_and_state_failure() {
        let fixture = Fixture::new();
        let bytes = b"verified content";
        let source = fixture.root.join("verified-source");
        std::fs::write(&source, bytes).unwrap();
        let existing = fixture.dir(ContentType::ResourcePack).join("manual.zip");
        std::fs::write(&existing, b"user content").unwrap();
        let first = transaction_record(ContentType::ResourcePack, "first.zip", bytes);
        let second = transaction_record(ContentType::ShaderPack, "second.zip", bytes);
        let result = activate_provider_transaction(
            &fixture.managed,
            &fixture.instance,
            vec![
                (first.clone(), source.clone(), bytes.len() as u64),
                (
                    second.clone(),
                    fixture.root.join("missing"),
                    bytes.len() as u64,
                ),
            ],
            |state| state.save(&fixture.managed, &fixture.instance),
        );
        assert!(result.is_err());
        assert!(
            !fixture
                .dir(ContentType::ResourcePack)
                .join("first.zip")
                .exists()
        );
        assert!(
            !fixture
                .dir(ContentType::ShaderPack)
                .join("second.zip")
                .exists()
        );
        assert_eq!(std::fs::read(&existing).unwrap(), b"user content");
        assert!(
            ContentState::load(&fixture.managed, &fixture.instance)
                .unwrap()
                .entries
                .is_empty()
        );

        let result = activate_provider_transaction(
            &fixture.managed,
            &fixture.instance,
            vec![(first, source, bytes.len() as u64)],
            |_| {
                Err(ContentError::StateMalformed(
                    "injected state failure".into(),
                ))
            },
        );
        assert!(result.is_err());
        assert!(
            !fixture
                .dir(ContentType::ResourcePack)
                .join("first.zip")
                .exists()
        );
        assert_eq!(std::fs::read(existing).unwrap(), b"user content");
    }

    #[test]
    fn transaction_rejects_collision_before_activating_any_file() {
        let fixture = Fixture::new();
        let bytes = b"verified content";
        let source = fixture.root.join("verified-source");
        std::fs::write(&source, bytes).unwrap();
        let existing = fixture.dir(ContentType::ShaderPack).join("taken.zip");
        std::fs::write(&existing, b"user content").unwrap();
        let result = activate_provider_transaction(
            &fixture.managed,
            &fixture.instance,
            vec![
                (
                    transaction_record(ContentType::ResourcePack, "first.zip", bytes),
                    source.clone(),
                    bytes.len() as u64,
                ),
                (
                    transaction_record(ContentType::ShaderPack, "taken.zip", bytes),
                    source,
                    bytes.len() as u64,
                ),
            ],
            |state| state.save(&fixture.managed, &fixture.instance),
        );
        assert!(matches!(result, Err(ContentError::Collision)));
        assert!(
            !fixture
                .dir(ContentType::ResourcePack)
                .join("first.zip")
                .exists()
        );
        assert_eq!(std::fs::read(existing).unwrap(), b"user content");
    }

    #[tokio::test]
    async fn sha512_plans_acquire_concurrently_and_persist_provider_ownership() {
        let fixture = Fixture::new();
        let bytes = fixture_fabric_jar();
        let served_bytes = bytes.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        let handler_active = active.clone();
        let handler_peak = peak.clone();
        let handler_requests = requests.clone();
        let server = TestServer::spawn(Arc::new(move |_request: &TestRequest| {
            handler_requests.fetch_add(1, Ordering::SeqCst);
            let now = handler_active.fetch_add(1, Ordering::SeqCst) + 1;
            handler_peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(35));
            handler_active.fetch_sub(1, Ordering::SeqCst);
            TestResponse::ok(&served_bytes)
        }));
        let sha512 = format!("{:x}", Sha512::digest(&bytes));
        let cases = [
            (ContentType::Mod, "fixture.jar"),
            (ContentType::ResourcePack, "fixture.zip"),
            (ContentType::ShaderPack, "shader.zip"),
        ];
        let plans: Vec<_> = cases
            .iter()
            .enumerate()
            .map(|(index, (kind, name))| {
                let mut record = transaction_record(*kind, name, &bytes);
                record.project_id = format!("project-{index}");
                ProviderInstallPlan {
                    content_type: *kind,
                    provider: record.provider,
                    project_id: record.project_id,
                    version_id: record.version_id,
                    file_id: record.file_id,
                    file_name: record.file_name,
                    display_version: record.display_version,
                    compatibility: record.compatibility,
                    dependencies: vec![],
                    source: ProviderArtifactSource::Sha512(
                        Sha512ArtifactSource::loopback_http_for_testing(
                            &format!("{}/file/{index}", server.base_url()),
                            &sha512,
                            Some(bytes.len() as u64),
                        )
                        .unwrap(),
                    ),
                }
            })
            .collect();
        let records = install_provider_plans(&fixture.managed, &fixture.instance, plans)
            .await
            .unwrap();
        assert_eq!(records.len(), 3);
        assert!(peak.load(Ordering::SeqCst) > 1);
        assert_eq!(
            ContentState::load(&fixture.managed, &fixture.instance)
                .unwrap()
                .entries
                .len(),
            3
        );
        for (kind, name) in cases {
            if kind == ContentType::Mod {
                let inventory =
                    crate::instance_mods::scan(&fixture.managed, &fixture.instance).unwrap();
                assert_eq!(inventory.entries[0].file_name, name);
                assert_eq!(
                    inventory.entries[0].ownership,
                    crate::instance_mods::ModOwnership::ProviderManaged
                );
            } else {
                let inventory = scan(&fixture.managed, &fixture.instance, kind).unwrap();
                assert_eq!(inventory.entries[0].file_name, name);
                assert_eq!(
                    inventory.entries[0].ownership,
                    ContentOwnership::ProviderManaged
                );
            }
        }
        let count = requests.load(Ordering::SeqCst);
        let source = Sha512ArtifactSource::loopback_http_for_testing(
            &format!("{}/file/0", server.base_url()),
            &sha512,
            Some(bytes.len() as u64),
        )
        .unwrap();
        let cached = ArtifactCache::new(fixture.managed.clone())
            .acquire_sha512(&source)
            .await
            .unwrap();
        assert_eq!(cached.origin, crate::cache::ArtifactOrigin::CacheHit);
        assert_eq!(requests.load(Ordering::SeqCst), count);
        std::fs::write(
            fixture.dir(ContentType::ResourcePack).join("fixture.zip"),
            b"tampered",
        )
        .unwrap();
        assert_eq!(
            scan(
                &fixture.managed,
                &fixture.instance,
                ContentType::ResourcePack
            )
            .unwrap()
            .entries[0]
                .ownership,
            ContentOwnership::Unknown
        );
    }

    #[tokio::test]
    async fn wrong_published_sha512_never_activates_content() {
        let fixture = Fixture::new();
        let server = TestServer::spawn(Arc::new(|_| TestResponse::ok(b"actual bytes")));
        let record = transaction_record(ContentType::ResourcePack, "bad.zip", b"expected bytes");
        let plan = ProviderInstallPlan {
            content_type: record.content_type,
            provider: record.provider,
            project_id: record.project_id,
            version_id: record.version_id,
            file_id: record.file_id.clone(),
            file_name: record.file_name,
            display_version: record.display_version,
            compatibility: record.compatibility,
            dependencies: vec![],
            source: ProviderArtifactSource::Sha512(
                Sha512ArtifactSource::loopback_http_for_testing(
                    &format!("{}/bad", server.base_url()),
                    &record.file_id,
                    Some(12),
                )
                .unwrap(),
            ),
        };
        assert!(
            install_provider_plans(&fixture.managed, &fixture.instance, vec![plan])
                .await
                .is_err()
        );
        assert!(
            !fixture
                .dir(ContentType::ResourcePack)
                .join("bad.zip")
                .exists()
        );
        assert!(
            ContentState::load(&fixture.managed, &fixture.instance)
                .unwrap()
                .entries
                .is_empty()
        );
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
        assert!(matches!(
            remove(
                &fixture.managed,
                &fixture.instance,
                ContentType::ResourcePack,
                &entry.entry_id,
            ),
            Err(ContentError::UnsupportedAction)
        ));
        let current = ContentState::load(&fixture.managed, &fixture.instance).unwrap();
        remove_provider_graph(
            &fixture.managed,
            &fixture.instance,
            &current,
            &current.entries[0].identity(),
        )
        .unwrap();
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

    #[test]
    fn v1_migration_retains_all_historical_content_and_rejects_bad_graphs() {
        let fixture = Fixture::new();
        let mut state = ContentState::empty();
        for (kind, project, name) in [
            (ContentType::Mod, "root", "root.jar"),
            (ContentType::Mod, "old-dependency", "dependency.jar"),
            (ContentType::ResourcePack, "pack", "pack.zip"),
            (ContentType::ShaderPack, "shader", "shader.zip"),
        ] {
            let mut record = transaction_record(kind, name, b"bytes");
            record.project_id = project.into();
            state.entries.push(record);
        }
        let mut legacy = serde_json::to_value(&state).unwrap();
        legacy["schemaVersion"] = serde_json::json!(1);
        for entry in legacy["entries"].as_array_mut().unwrap() {
            entry.as_object_mut().unwrap().remove("explicitlyRetained");
            entry.as_object_mut().unwrap().remove("requires");
        }
        let path = state_path(&fixture.managed, &fixture.instance).unwrap();
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let migrated = ContentState::load_and_migrate(&fixture.managed, &fixture.instance).unwrap();
        assert_eq!(migrated.entries.len(), 4);
        assert!(
            migrated
                .entries
                .iter()
                .all(|record| record.explicitly_retained && record.requires.is_empty())
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()["schemaVersion"],
            2
        );
        legacy["entries"][0]["provider"] = serde_json::json!(null);
        assert!(matches!(
            ContentState::from_json(&legacy.to_string()),
            Err(ContentError::StateMalformed(_))
        ));
        let mut broken = migrated.clone();
        broken.entries[0].requires.push(ProviderIdentity {
            content_type: ContentType::Mod,
            provider: "missing".into(),
            project_id: "missing".into(),
        });
        assert!(matches!(
            broken.validate(),
            Err(ContentError::StateMalformed(_))
        ));
        assert!(matches!(
            ContentState::from_json(r#"{"schemaVersion":99,"entries":[]}"#),
            Err(ContentError::StateVersion(99))
        ));
    }

    #[test]
    fn shared_dependencies_and_direct_promotion_control_orphan_cleanup() {
        let mut state = ContentState::empty();
        let mut a = transaction_record(ContentType::ResourcePack, "a.zip", b"a");
        a.project_id = "A".into();
        let mut b = transaction_record(ContentType::ResourcePack, "b.zip", b"b");
        b.project_id = "B".into();
        b.explicitly_retained = false;
        let mut c = transaction_record(ContentType::ResourcePack, "c.zip", b"c");
        c.project_id = "C".into();
        c.explicitly_retained = false;
        let mut d = transaction_record(ContentType::ResourcePack, "d.zip", b"d");
        d.project_id = "D".into();
        a.requires.push(b.identity());
        b.requires.push(c.identity());
        d.requires.push(c.identity());
        state.entries = vec![a.clone(), b.clone(), c.clone(), d.clone()];
        state.validate().unwrap();
        assert!(matches!(
            removal_state(&state, &b.identity()),
            Err(ContentError::RequiredByInstalledContent)
        ));
        let after_a = removal_state(&state, &a.identity()).unwrap();
        assert!(after_a.find(&a.identity()).is_none());
        assert!(after_a.find(&b.identity()).is_none());
        assert!(after_a.find(&c.identity()).is_some());
        let mut promoted = state.clone();
        promoted.find(&b.identity()).unwrap();
        promoted
            .entries
            .iter_mut()
            .find(|record| record.identity() == b.identity())
            .unwrap()
            .explicitly_retained = true;
        let after_promotion = removal_state(&promoted, &a.identity()).unwrap();
        assert!(after_promotion.find(&b.identity()).is_some());
        assert!(after_promotion.find(&c.identity()).is_some());
        let after_d = removal_state(&after_promotion, &d.identity()).unwrap();
        assert!(after_d.find(&c.identity()).is_some());
        let after_b = removal_state(&after_d, &b.identity()).unwrap();
        assert!(after_b.entries.is_empty());
    }

    #[test]
    fn lifecycle_replaces_same_and_changed_names_without_losing_old_bytes_on_failure() {
        let fixture = Fixture::new();
        let dir = fixture.dir(ContentType::ResourcePack);
        let mut old_bytes = b"old working pack".to_vec();
        let mut old = transaction_record(ContentType::ResourcePack, "pack.zip", &old_bytes);
        old.project_id = "PACK".into();
        std::fs::write(dir.join(&old.file_name), &old_bytes).unwrap();
        let mut current = ContentState::empty();
        current.entries.push(old.clone());
        current.save(&fixture.managed, &fixture.instance).unwrap();
        for name in ["pack.zip", "pack-v2.zip"] {
            let new_bytes = format!("new bytes for {name}").into_bytes();
            let source = fixture
                .root
                .join(format!("source-{}", uuid::Uuid::new_v4()));
            std::fs::write(&source, &new_bytes).unwrap();
            let mut new = transaction_record(ContentType::ResourcePack, name, &new_bytes);
            new.project_id = "PACK".into();
            new.version_id = format!("version-{name}");
            let next = updated_state(&current, &old.identity(), vec![new.clone()]).unwrap();
            let wrong_source = fixture.root.join("wrong-source");
            std::fs::write(&wrong_source, b"tampered").unwrap();
            assert!(matches!(
                apply_lifecycle_state(
                    &fixture.managed,
                    &fixture.instance,
                    &current,
                    &next,
                    &[(new.clone(), wrong_source, new_bytes.len() as u64)]
                ),
                Err(ContentError::HashMismatch)
            ));
            assert_eq!(std::fs::read(dir.join(&old.file_name)).unwrap(), old_bytes);
            assert_eq!(
                ContentState::load(&fixture.managed, &fixture.instance).unwrap(),
                current
            );
            apply_lifecycle_state(
                &fixture.managed,
                &fixture.instance,
                &current,
                &next,
                &[(new.clone(), source, new_bytes.len() as u64)],
            )
            .unwrap();
            assert_eq!(std::fs::read(dir.join(name)).unwrap(), new_bytes);
            if name != old.file_name {
                assert!(!dir.join(&old.file_name).exists());
            }
            current = next;
            old = new;
            old_bytes = new_bytes;
        }
    }

    #[test]
    fn lifecycle_restores_old_content_after_state_or_cleanup_failure() {
        let fixture = Fixture::new();
        let directory = fixture.dir(ContentType::ResourcePack);
        let old = transaction_record(ContentType::ResourcePack, "old.zip", b"old verified bytes");
        std::fs::write(directory.join("old.zip"), b"old verified bytes").unwrap();
        let mut current = ContentState::empty();
        current.entries.push(old.clone());
        current.save(&fixture.managed, &fixture.instance).unwrap();
        let mut new =
            transaction_record(ContentType::ResourcePack, "new.zip", b"new verified bytes");
        new.version_id = "22223333".into();
        let source = fixture.root.join("verified-new");
        std::fs::write(&source, b"new verified bytes").unwrap();
        let next = updated_state(&current, &old.identity(), vec![new.clone()]).unwrap();
        let artifact = [(new, source, b"new verified bytes".len() as u64)];
        for cleanup_failure in [false, true] {
            let result = apply_lifecycle_state_with_hooks(
                &fixture.managed,
                &fixture.instance,
                &current,
                &next,
                &artifact,
                |state| {
                    if cleanup_failure {
                        state.save(&fixture.managed, &fixture.instance)
                    } else {
                        Err(ContentError::StateMalformed(
                            "injected state failure".into(),
                        ))
                    }
                },
                || {
                    if cleanup_failure {
                        Err(ContentError::StateMalformed(
                            "injected cleanup failure".into(),
                        ))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
            assert_eq!(
                std::fs::read(directory.join("old.zip")).unwrap(),
                b"old verified bytes"
            );
            assert!(!directory.join("new.zip").exists());
            assert_eq!(
                ContentState::load(&fixture.managed, &fixture.instance).unwrap(),
                current
            );
        }
    }
}
