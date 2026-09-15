//! Isolated game installation execution.
//!
//! The installer consumes one fully resolved [`GameInstallPlan`] plus a
//! validated [`InstanceId`] and materializes the complete, isolated game
//! into launcher-managed instance storage. It never parses raw Mojang or
//! Fabric metadata, never decides versions, and never re-evaluates rules:
//! the plan is authoritative, and the only external document the installer
//! touches is the asset index — acquired and verified like any artifact
//! first, then parsed by the DTO boundary in `minecraft::metadata`.
//!
//! ## Execution model
//!
//! ```text
//! GameInstallPlan
//!   ↓ acquire per trust policy (sha256 / sha1 / transport-observed stores)
//! verified cache objects
//!   ↓ copy + extract into instance staging  (instances/<id>/.install-staging/game)
//! staged installation
//!   ↓ validate staged files against their recorded trust
//!   ↓ write installed-state manifest LAST inside staging
//!   ↓ promote the staged tree onto the final game directory by rename
//! InstalledGame (instances/<id>/game + installed-game.json)
//! ```
//!
//! ## Atomicity
//!
//! Nothing is ever written into the final `game/` directory directly. The
//! complete tree — including the manifest that marks completion — is built
//! under `.install-staging/` and becomes visible through one directory
//! rename. An interrupted installation therefore always leaves the instance
//! either without a game directory or with its previous complete one; a
//! staging directory is never mistaken for an installed game, and the next
//! attempt removes it (after proving it is the fixed, derived staging path).
//!
//! ## User-data boundary
//!
//! Only `instances/<id>/game/` is launcher-managed and reconstructable.
//! `mods/`, `config/`, `logs/`, and any future user content at the instance
//! root are never read, written, or removed by installation. Replacing an
//! existing installation only ever moves a manifest-proven `game/` directory
//! into staging before deletion — never a recursive wipe of the instance.
//!
//! ## Concurrency
//!
//! One installation may run per instance at a time; a second attempt fails
//! immediately with `installation_already_in_progress`. There is no global
//! queue and no cross-process locking.

pub mod assets;
pub mod natives;
pub mod state;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::cache::{
    AcquisitionError, ArtifactCache, ObservedArtifact, VerifiedArtifact, VerifiedSha1Artifact,
};
use crate::downloads::{
    ArtifactSource, DownloadOptions, InvalidArtifactSource, ObservedArtifactSource,
    Sha1ArtifactSource,
};
use crate::fabric::plan::{GameInstallPlan, GameLibrary};
use crate::instances::InstanceId;
use crate::integrity::{
    ArtifactDigest, ArtifactTrust, DigestAlgorithm, Sha1Digest, verify_file, verify_file_sha1,
};
use crate::minecraft::metadata::AssetIndexObjectsDocument;
use crate::minecraft::plan::{ArtifactRequirement, AssetIndexRequirement, LibraryKind};
use crate::paths::ManagedPaths;

use assets::AssetObjectEndpoints;
use natives::NativeExtractionError;
use state::{
    INSTALLED_GAME_FILE_NAME, InstalledFile, InstalledFileRole, InstalledGameManifest,
    InstalledStateError, NativesRecord, load_installed_state,
};

/// The fixed staging directory name beside the game directory.
const STAGING_DIR_NAME: &str = ".install-staging";

/// Where a replaced (proven-managed) previous game directory waits inside
/// staging until the new installation has been promoted.
const REPLACED_GAME_DIR_NAME: &str = "replaced-game";

/// The maximum asset-index document size the installer will parse (real
/// indexes are well under one megabyte).
const MAX_ASSET_INDEX_BYTES: usize = 8 * 1024 * 1024;

/// Everything the installer needs beyond the plan itself.
#[derive(Debug, Clone)]
pub struct InstallContext {
    download_options: DownloadOptions,
    asset_endpoints: AssetObjectEndpoints,
}

impl InstallContext {
    /// Production context: the launcher's transport defaults and Mojang's
    /// official asset host.
    pub fn official() -> Self {
        Self {
            download_options: DownloadOptions::default(),
            asset_endpoints: AssetObjectEndpoints::official(),
        }
    }

    /// Test context with explicit transport limits and a loopback asset
    /// root (the launcher's documented test-transport pattern).
    pub fn loopback_for_testing(
        download_options: DownloadOptions,
        asset_endpoints: AssetObjectEndpoints,
    ) -> Self {
        Self {
            download_options,
            asset_endpoints,
        }
    }

    pub fn download_options(&self) -> &DownloadOptions {
        &self.download_options
    }

    pub fn asset_endpoints(&self) -> &AssetObjectEndpoints {
        &self.asset_endpoints
    }
}

/// One coarse installation phase, reported as work proceeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    Acquiring,
    Materializing,
    ExtractingNatives,
    Validating,
    Committing,
}

impl InstallPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acquiring => "acquiring",
            Self::Materializing => "materializing",
            Self::ExtractingNatives => "extractingNatives",
            Self::Validating => "validating",
            Self::Committing => "committing",
        }
    }
}

/// A narrow typed progress report owned by Rust; the frontend only displays
/// it. Progress never implies completion — completion is the committed
/// installed-state manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub completed_items: u32,
    pub total_items: u32,
    /// The logical name (Maven coordinate, asset hash, role) of the item in
    /// flight — never a remote-controlled filename or arbitrary path.
    pub current_item: Option<String>,
}

/// Deterministic failure injection for transaction tests. Production always
/// passes the default (no injected failures).
#[derive(Debug, Clone, Copy, Default)]
pub struct InstallFaults {
    /// Fail immediately before the staged tree is promoted, after the
    /// manifest is written into staging.
    pub fail_before_commit: bool,
    /// Fail partway through materialization, after the first staged copy.
    pub fail_during_materialization: bool,
}

/// The result of a committed installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledGame {
    game_directory: PathBuf,
    manifest: InstalledGameManifest,
    total_bytes: u64,
}

impl InstalledGame {
    pub fn game_directory(&self) -> &Path {
        &self.game_directory
    }

    pub fn manifest(&self) -> &InstalledGameManifest {
        &self.manifest
    }

    pub fn total_byte_count(&self) -> u64 {
        self.total_bytes
    }
}

/// One acquired-and-materialized file pairing the verified cache source
/// with its staged destination and recorded trust.
struct PlannedFile {
    source: PathBuf,
    relative: String,
    trust: ArtifactTrust,
    size_bytes: u64,
    role: InstalledFileRole,
}

/// Executes one complete installation of `plan` into `instance`.
///
/// On success the instance owns a validated, committed game directory whose
/// installed-state manifest describes every managed file and how it is
/// trusted. On failure the instance never gains (or loses) a complete
/// installation; leftover staging is deliberate recoverable state that the
/// next attempt removes.
pub async fn install_game(
    managed: &ManagedPaths,
    instance: &InstanceId,
    plan: &GameInstallPlan,
    context: &InstallContext,
    progress: &mut (dyn FnMut(InstallProgress) + Send),
    faults: InstallFaults,
) -> Result<InstalledGame, InstallError> {
    let lock = instance_installation_lock(instance);
    let _guard = lock
        .try_lock()
        .map_err(|_| InstallError::AlreadyInProgress {
            instance_id: instance.as_str().to_owned(),
        })?;

    let instance_paths = managed.instance_paths(instance);
    let instance_root = instance_paths.root().to_path_buf();
    let game_directory = instance_paths.game().to_path_buf();
    let staging_directory = instance_root.join(STAGING_DIR_NAME);
    let staging_game = staging_directory.join("game");

    let replacing = decide_replacement(&game_directory)?;

    prepare_staging(&instance_root, &staging_directory, &staging_game)?;

    let minecraft = plan.minecraft();
    let cache = ArtifactCache::new(managed.clone());

    // ---- Acquisition: everything flows through the verified stores first.
    let mut report = |phase, completed, total, item| {
        progress(InstallProgress {
            phase,
            completed_items: completed,
            total_items: total,
            current_item: item,
        })
    };

    let index_requirement = minecraft.asset_index();
    report(
        InstallPhase::Acquiring,
        0,
        acquisition_total(plan, &UnknownObjects::Pending),
        Some("asset index".to_owned()),
    );

    let index_artifact = acquire_mojang(&cache, index_requirement.artifact(), context).await?;
    let index_document = parse_verified_asset_index(&index_artifact, index_requirement)?;
    let objects = assets::plan_asset_objects(&index_document).map_err(|error| {
        InstallError::AssetInvalid {
            name: error.name,
            reason: error.reason,
        }
    })?;

    let total_items = acquisition_total(plan, &UnknownObjects::Known(objects.len()));
    let mut completed = 1u32;
    let mut files: Vec<PlannedFile> = Vec::new();

    // Client jar.
    report(
        InstallPhase::Acquiring,
        completed,
        total_items,
        Some(format!("{} client", minecraft.minecraft_version())),
    );
    let client = acquire_mojang(&cache, minecraft.client(), context).await?;
    completed += 1;
    files.push(PlannedFile {
        source: client.path.clone(),
        relative: format!("versions/{}/client.jar", minecraft.minecraft_version()),
        trust: client.trust(),
        size_bytes: client.bytes,
        role: InstalledFileRole::Client,
    });

    // Logging configuration, when the version requires one.
    if let Some(logging) = minecraft.logging() {
        report(
            InstallPhase::Acquiring,
            completed,
            total_items,
            Some(format!("logging configuration {}", logging.file_name())),
        );
        let artifact = acquire_mojang(&cache, logging.artifact(), context).await?;
        completed += 1;
        files.push(PlannedFile {
            source: artifact.path.clone(),
            relative: format!(
                "versions/{}/{}",
                minecraft.minecraft_version(),
                logging.file_name()
            ),
            trust: artifact.trust(),
            size_bytes: artifact.bytes,
            role: InstalledFileRole::LoggingConfig,
        });
    }

    // The verified index itself is an installed file.
    files.push(PlannedFile {
        source: index_artifact.path.clone(),
        relative: format!(
            "assets/indexes/{}.json",
            validated_index_file_stem(index_requirement)?
        ),
        trust: index_artifact.trust(),
        size_bytes: index_artifact.bytes,
        role: InstalledFileRole::AssetIndex,
    });

    // Composed libraries, Mojang then Fabric, in classpath order.
    for library in plan.libraries() {
        report(
            InstallPhase::Acquiring,
            completed,
            total_items,
            Some(library.coordinate_string()),
        );
        let (source, trust, size, role) = acquire_library(&cache, library, context).await?;
        completed += 1;
        files.push(PlannedFile {
            source,
            relative: format!("libraries/{}", library.repository_path()),
            trust,
            size_bytes: size,
            role,
        });
    }

    // Asset objects, deduplicated by hash.
    for object in &objects {
        let hex = object.sha1().as_hex();
        report(
            InstallPhase::Acquiring,
            completed,
            total_items,
            Some(format!("asset {hex}")),
        );
        let source = Sha1ArtifactSource::https_or_loopback(
            context.asset_endpoints().object_url(object.sha1()).as_str(),
            &hex,
            Some(object.size_bytes()),
        )
        .map_err(|error| InstallError::InvalidSource(error.to_string()))?;
        let artifact = cache
            .acquire_sha1(&source, context.download_options())
            .await?;
        completed += 1;
        files.push(PlannedFile {
            source: artifact.path.clone(),
            relative: object.game_relative_path(),
            trust: artifact.trust(),
            size_bytes: artifact.bytes,
            role: InstalledFileRole::AssetObject,
        });
    }

    // ---- Materialization: verified cache objects are copied into staging.
    let total_files = files.len() as u32;
    for (index, file) in files.iter().enumerate() {
        report(
            InstallPhase::Materializing,
            index as u32,
            total_files,
            Some(file.relative.clone()),
        );
        materialize_file(&staging_game, file)?;
        if faults.fail_during_materialization {
            return Err(InstallError::Materialization {
                path: file.relative.clone(),
                source: std::io::Error::other(
                    "deterministic fault injected during materialization",
                ),
            });
        }
    }

    // ---- Native extraction from the verified native archives.
    let natives_relative = format!("natives/{}", minecraft.minecraft_version());
    let natives_root = staging_game.join(&natives_relative);
    let native_libraries: Vec<&GameLibrary> = plan
        .libraries()
        .iter()
        .filter(|library| {
            matches!(
                library,
                GameLibrary::Minecraft(entry) if entry.kind() == LibraryKind::NativeArtifact)
        })
        .collect();
    for (index, library) in native_libraries.iter().enumerate() {
        report(
            InstallPhase::ExtractingNatives,
            index as u32,
            native_libraries.len() as u32,
            Some(library.coordinate_string()),
        );
        let GameLibrary::Minecraft(entry) = library else {
            unreachable!("the filter above selects only Mojang native libraries")
        };
        let archive_path = files
            .iter()
            .find(|file| file.relative == format!("libraries/{}", entry.path()))
            .map(|file| file.source.clone())
            .expect("every planned library was acquired and materialized");
        natives::extract_native_archive(
            &archive_path,
            &entry.coordinate().as_maven_string(),
            &natives_root,
        )
        .map_err(InstallError::Native)?;
    }
    if native_libraries.is_empty() {
        return Err(InstallError::Validation {
            path: natives_relative,
            reason: "the plan selected no native libraries for this platform".to_owned(),
        });
    }

    // ---- Staged validation: every file re-verified before completion.
    report(InstallPhase::Validating, 0, total_files, None);
    let installed_files = validate_staged_files(&staging_game, &files, &natives_relative)?;

    // ---- Completion record, written last inside staging.
    let installation_id = format!(
        "install-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default(),
        std::process::id()
    );
    let installed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let manifest = InstalledGameManifest::new(
        minecraft.minecraft_version(),
        plan.loader().loader_version(),
        installation_id,
        installed_at,
        installed_files,
        NativesRecord::new(natives_relative),
    );
    let manifest_path = staging_game.join(INSTALLED_GAME_FILE_NAME);
    std::fs::write(&manifest_path, manifest.to_json()).map_err(|error| InstallError::Commit {
        context: "writing the installed-state manifest".to_owned(),
        source: error,
    })?;

    if faults.fail_before_commit {
        return Err(InstallError::Commit {
            context: "deterministic fault injected immediately before commit".to_owned(),
            source: std::io::Error::other("injected pre-commit failure"),
        });
    }

    // ---- Commit: promote the staged tree with one rename.
    report(InstallPhase::Committing, 0, 1, None);
    promote_staged_game(
        &instance_root,
        &game_directory,
        &staging_directory,
        &staging_game,
        replacing,
    )?;

    let total_bytes = files.iter().map(|file| file.size_bytes).sum();
    Ok(InstalledGame {
        game_directory,
        manifest,
        total_bytes,
    })
}

/// The outcome of [`validate_installed_game`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// No complete installation exists in this instance.
    NotInstalled,
    Installed(InstalledGameValidation),
}

/// The deterministic validation report of one installed game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledGameValidation {
    pub status: ValidationStatus,
    pub problems: Vec<ValidationProblem>,
    pub minecraft_version: String,
    pub fabric_loader_version: String,
    pub installation_id: String,
    pub checked_files: usize,
    pub verified_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationStatus {
    Valid,
    Damaged,
}

/// One concrete validation problem (missing file, size drift, digest drift,
/// missing natives).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationProblem {
    pub path: String,
    pub reason: String,
}

/// Validates one instance's installed game against its own installed-state
/// record: every managed file must exist with its recorded size and must
/// re-verify according to its recorded trust.
///
/// This is validation, not repair: nothing is downloaded and nothing is
/// mutated. A game directory without a manifest is `NotInstalled`; a
/// malformed manifest is a deliberate error.
pub fn validate_installed_game(
    managed: &ManagedPaths,
    instance: &InstanceId,
) -> Result<ValidationOutcome, InstallError> {
    let game_directory = managed.instance_paths(instance).game().to_path_buf();
    if !game_directory.is_dir() {
        return Ok(ValidationOutcome::NotInstalled);
    }

    let Some(manifest) = load_installed_state(&game_directory).map_err(InstallError::State)? else {
        return Ok(ValidationOutcome::NotInstalled);
    };

    let mut problems = Vec::new();
    let mut verified_bytes = 0u64;

    for file in manifest.files() {
        match verify_managed_file(&game_directory, file) {
            Ok(bytes) => verified_bytes += bytes,
            Err(reason) => problems.push(ValidationProblem {
                path: file.path().to_owned(),
                reason,
            }),
        }
    }

    let natives_directory = game_directory.join(manifest.natives().directory());
    let natives_ok = natives_directory.is_dir()
        && std::fs::read_dir(&natives_directory)
            .map(|entries| entries.filter_map(Result::ok).count() > 0)
            .unwrap_or(false);
    if !natives_ok {
        problems.push(ValidationProblem {
            path: manifest.natives().directory().to_owned(),
            reason: "the extracted natives directory is missing or empty".to_owned(),
        });
    }

    let status = if problems.is_empty() {
        ValidationStatus::Valid
    } else {
        ValidationStatus::Damaged
    };

    Ok(ValidationOutcome::Installed(InstalledGameValidation {
        status,
        problems,
        minecraft_version: manifest.minecraft_version().to_owned(),
        fabric_loader_version: manifest.fabric_loader_version().to_owned(),
        installation_id: manifest.installation_id().to_owned(),
        checked_files: manifest.files().len(),
        verified_bytes,
    }))
}

/// Re-verifies one installed file against its recorded trust and size.
fn verify_managed_file(game_root: &Path, file: &InstalledFile) -> Result<u64, String> {
    let path = game_root.join(file.path());
    match file.trust() {
        ArtifactTrust::ExpectedDigestVerified { algorithm, digest } => match algorithm {
            DigestAlgorithm::Sha256 => {
                let expected = ArtifactDigest::parse(digest).map_err(|e| e.to_string())?;
                verify_file(&path, &expected, Some(file.size_bytes()))
                    .map_err(|error| error.to_string())
            }
            DigestAlgorithm::Sha1 => {
                let expected = Sha1Digest::parse(digest).map_err(|e| e.to_string())?;
                verify_file_sha1(&path, &expected, Some(file.size_bytes()))
                    .map_err(|error| error.to_string())
            }
        },
        ArtifactTrust::SecureTransportObserved { observed_sha256 } => {
            let expected = ArtifactDigest::parse(observed_sha256).map_err(|e| e.to_string())?;
            verify_file(&path, &expected, Some(file.size_bytes()))
                .map_err(|error| error.to_string())
        }
    }
}

enum UnknownObjects {
    Pending,
    Known(usize),
}

fn acquisition_total(plan: &GameInstallPlan, objects: &UnknownObjects) -> u32 {
    let mut total = 2; // asset index + client
    if plan.minecraft().logging().is_some() {
        total += 1;
    }
    total += plan.libraries().len() as u32;
    if let UnknownObjects::Known(count) = objects {
        total += *count as u32;
    }
    total
}

/// Decides whether installation replaces a proven existing installation or
/// conflicts with unrecognizable existing state.
///
/// - no `game/` directory → fresh installation;
/// - `game/` with a valid installed-state manifest → deliberate replacement;
/// - `game/` without one (partial prior install, foreign files, a stray
///   file) → a hard conflict; Aurora neither wipes nor guesses.
fn decide_replacement(game_directory: &Path) -> Result<bool, InstallError> {
    if std::fs::symlink_metadata(game_directory).is_err() {
        return Ok(false);
    }
    if !game_directory.is_dir() {
        return Err(InstallError::TargetConflict {
            path: game_directory.display().to_string(),
            reason: "the instance's game location exists but is not a directory".to_owned(),
        });
    }
    match load_installed_state(game_directory) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Err(InstallError::TargetConflict {
            path: game_directory.display().to_string(),
            reason: "the instance's game directory exists without a valid installed-state manifest; it may be a partial or foreign tree, and Aurora does not wipe it"
                .to_owned(),
        }),
        Err(error) => Err(InstallError::State(error)),
    }
}

/// Resets the staging tree: stale staging from a failed attempt is removed
/// only after proving the path is the fixed, derived staging location, then a
/// fresh staging game directory is created.
fn prepare_staging(
    instance_root: &Path,
    staging_directory: &Path,
    staging_game: &Path,
) -> Result<(), InstallError> {
    prove_staging_path(instance_root, staging_directory)?;

    if staging_directory.exists() {
        std::fs::remove_dir_all(staging_directory).map_err(InstallError::Storage)?;
    }
    std::fs::create_dir_all(staging_game).map_err(InstallError::Storage)?;
    Ok(())
}

/// The containment proof required before any recursive removal: staging must
/// be exactly `<instance-root>/.install-staging`.
fn prove_staging_path(instance_root: &Path, staging_directory: &Path) -> Result<(), InstallError> {
    let is_derived_staging = staging_directory.parent() == Some(instance_root)
        && staging_directory.file_name().and_then(|name| name.to_str()) == Some(STAGING_DIR_NAME);
    if !is_derived_staging || !staging_directory.starts_with(instance_root) {
        return Err(InstallError::Storage(std::io::Error::other(format!(
            "refusing to modify {} : it is not the derived instance staging directory",
            staging_directory.display()
        ))));
    }
    Ok(())
}

/// Promotes the staged game tree onto the final location.
///
/// Fresh install: one rename of `staging/game` onto `game`. Replacement: the
/// proven-managed existing `game` is moved into staging first, the new tree
/// is renamed into place, and staging (now holding the old tree) is removed.
/// If the process dies between the two renames the instance has *no* game
/// directory — visibly incomplete, never a mixture — and the next attempt
/// rebuilds from scratch.
fn promote_staged_game(
    instance_root: &Path,
    game_directory: &Path,
    staging_directory: &Path,
    staging_game: &Path,
    replacing: bool,
) -> Result<(), InstallError> {
    std::fs::create_dir_all(instance_root).map_err(|error| InstallError::Commit {
        context: "creating the instance directory".to_owned(),
        source: error,
    })?;

    if replacing {
        let retired = staging_directory.join(REPLACED_GAME_DIR_NAME);
        std::fs::rename(game_directory, &retired).map_err(|error| InstallError::Commit {
            context: "moving the previous installation aside".to_owned(),
            source: error,
        })?;
        if let Err(error) = std::fs::rename(staging_game, game_directory) {
            // Put the proven-managed previous installation back before
            // failing; the instance must not lose its game because the new
            // promotion could not complete.
            let _ = std::fs::rename(&retired, game_directory);
            return Err(InstallError::Commit {
                context: "promoting the staged installation".to_owned(),
                source: error,
            });
        }
        prove_staging_path(instance_root, staging_directory)?;
        // The installation is already committed at this point; a cleanup
        // failure is reported but must not fail the committed install.
        if let Err(error) = std::fs::remove_dir_all(staging_directory) {
            eprintln!(
                "[aurora-launcher] committed installation but could not remove the retired previous tree at {}: {error}",
                staging_directory.display()
            );
        }
    } else {
        std::fs::rename(staging_game, game_directory).map_err(|error| InstallError::Commit {
            context: "promoting the staged installation".to_owned(),
            source: error,
        })?;
        // Staging is now an empty husk; keep the instance tidy.
        let _ = std::fs::remove_dir_all(staging_directory);
    }

    Ok(())
}

/// Acquires one official Mojang artifact through the SHA-1 store.
async fn acquire_mojang(
    cache: &ArtifactCache,
    requirement: &ArtifactRequirement,
    context: &InstallContext,
) -> Result<VerifiedSha1Artifact, InstallError> {
    let source = Sha1ArtifactSource::https_or_loopback(
        requirement.url(),
        &requirement.sha1().as_hex(),
        Some(requirement.size_bytes()),
    )
    .map_err(|error| InstallError::InvalidSource(error.to_string()))?;

    cache
        .acquire_sha1(&source, context.download_options())
        .await
        .map_err(InstallError::Acquisition)
}

/// Parses the verified asset-index document.
///
/// The index is a verified cache object by the time this runs; parsing is
/// bounded, and any structural failure is a deliberate hard error.
fn parse_verified_asset_index(
    artifact: &VerifiedSha1Artifact,
    requirement: &AssetIndexRequirement,
) -> Result<AssetIndexObjectsDocument, InstallError> {
    if artifact.bytes as usize > MAX_ASSET_INDEX_BYTES {
        return Err(InstallError::AssetIndexInvalid {
            reason: format!(
                "asset index '{}' is {} bytes, above the parsing bound",
                requirement.id(),
                artifact.bytes
            ),
        });
    }
    let text = std::fs::read_to_string(&artifact.path).map_err(|error| {
        InstallError::AssetIndexInvalid {
            reason: format!("the verified asset index could not be read: {error}"),
        }
    })?;
    AssetIndexObjectsDocument::from_json(&text).map_err(|error| InstallError::AssetIndexInvalid {
        reason: error.to_string(),
    })
}

/// Acquires one composed library through the store its trust class demands.
async fn acquire_library(
    cache: &ArtifactCache,
    library: &GameLibrary,
    context: &InstallContext,
) -> Result<(PathBuf, ArtifactTrust, u64, InstalledFileRole), InstallError> {
    match library {
        GameLibrary::Minecraft(entry) => {
            let artifact = acquire_mojang(cache, entry.artifact(), context).await?;
            let role = if entry.kind() == LibraryKind::NativeArtifact {
                InstalledFileRole::NativeLibrary
            } else {
                InstalledFileRole::Library
            };
            Ok((
                artifact.path.clone(),
                artifact.trust(),
                artifact.bytes,
                role,
            ))
        }
        GameLibrary::Fabric(entry) => {
            let artifact = entry.artifact();
            if let Some(sha256) = artifact.sha256() {
                let source = https_or_loopback_artifact_source(
                    artifact.url().as_str(),
                    sha256,
                    artifact.size_bytes(),
                )
                .map_err(|error| InstallError::InvalidSource(error.to_string()))?;
                let verified: VerifiedArtifact = cache
                    .acquire_with(&source, context.download_options())
                    .await?;
                Ok((
                    verified.path.clone(),
                    verified.trust(),
                    verified.bytes,
                    InstalledFileRole::Library,
                ))
            } else {
                let source = ObservedArtifactSource::https_or_loopback(artifact.url().as_str())
                    .map_err(|error| InstallError::InvalidSource(error.to_string()))?;
                let observed: ObservedArtifact = cache
                    .acquire_observed(&source, context.download_options())
                    .await?;
                Ok((
                    observed.path.clone(),
                    observed.trust(),
                    observed.bytes,
                    InstalledFileRole::Library,
                ))
            }
        }
    }
}

/// Constructs a production HTTPS SHA-256 artifact source, falling back to
/// the explicit loopback test transport for the offline deterministic tests.
fn https_or_loopback_artifact_source(
    url: &str,
    sha256: &ArtifactDigest,
    size_bytes: Option<u64>,
) -> Result<ArtifactSource, InvalidArtifactSource> {
    ArtifactSource::https(url, &sha256.as_hex(), size_bytes)
        .or_else(|_| ArtifactSource::loopback_http_for_testing(url, &sha256.as_hex(), size_bytes))
}

/// Copies one verified cache object into the staged game tree and checks the
/// copied byte count. Content re-verification happens in the staged
/// validation pass.
fn materialize_file(staging_game: &Path, file: &PlannedFile) -> Result<(), InstallError> {
    let mut target = staging_game.to_path_buf();
    for segment in file.relative.split('/') {
        target.push(segment);
    }
    if !target.starts_with(staging_game) {
        return Err(InstallError::Materialization {
            path: file.relative.clone(),
            source: std::io::Error::other("the derived path escaped the staging root"),
        });
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| InstallError::Materialization {
            path: file.relative.clone(),
            source: error,
        })?;
    }
    std::fs::copy(&file.source, &target).map_err(|error| InstallError::Materialization {
        path: file.relative.clone(),
        source: error,
    })?;
    let copied = std::fs::metadata(&target)
        .map_err(|error| InstallError::Materialization {
            path: file.relative.clone(),
            source: error,
        })?
        .len();
    if copied != file.size_bytes {
        return Err(InstallError::Materialization {
            path: file.relative.clone(),
            source: std::io::Error::other(format!(
                "copied {copied} bytes but the verified artifact holds {}",
                file.size_bytes
            )),
        });
    }
    Ok(())
}

/// Re-verifies every staged file against its recorded trust, producing the
/// manifest entries. This is the pass that proves a successful API call
/// really produced valid installed bytes.
fn validate_staged_files(
    staging_game: &Path,
    files: &[PlannedFile],
    natives_relative: &str,
) -> Result<Vec<InstalledFile>, InstallError> {
    let mut installed = Vec::with_capacity(files.len());
    for file in files {
        let record = InstalledFile::new(
            file.role,
            file.relative.clone(),
            file.trust.clone(),
            file.size_bytes,
        );
        verify_managed_file(staging_game, &record).map_err(|reason| InstallError::Validation {
            path: file.relative.clone(),
            reason,
        })?;
        installed.push(record);
    }

    let natives_root = staging_game.join(natives_relative);
    let has_content = natives_root.is_dir()
        && std::fs::read_dir(&natives_root)
            .map(|entries| entries.filter_map(Result::ok).count() > 0)
            .unwrap_or(false);
    if !has_content {
        return Err(InstallError::Validation {
            path: natives_relative.to_owned(),
            reason: "the staged natives directory is missing or empty".to_owned(),
        });
    }

    Ok(installed)
}

/// The asset-index id becomes one installed file name; it is validated as a
/// safe single segment (official ids are numeric strings).
fn validated_index_file_stem(requirement: &AssetIndexRequirement) -> Result<String, InstallError> {
    let id = requirement.id();
    let valid = !id.is_empty()
        && !id.starts_with('.')
        && id == id.trim()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(InstallError::AssetIndexInvalid {
            reason: format!("asset index id '{id}' is not a safe file name"),
        });
    }
    Ok(id.to_owned())
}

/// The per-instance installation exclusion: one shared async mutex per
/// instance id, held for the duration of an installation.
fn instance_installation_lock(instance: &InstanceId) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    let registry = LOCKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()));

    let mut guards = registry.lock().expect("the lock registry is not poisoned");
    guards
        .entry(instance.as_str().to_owned())
        .or_default()
        .clone()
}

/// A failed installation. Every variant fails closed: the instance never
/// ends up with a complete-looking installation it did not earn.
#[derive(Debug)]
pub enum InstallError {
    /// Another installation is already running for this instance.
    AlreadyInProgress { instance_id: String },
    /// Existing state at the target cannot be replaced safely.
    TargetConflict { path: String, reason: String },
    /// The persisted installed state is malformed or unsupported.
    State(InstalledStateError),
    /// The verified asset-index document is unusable.
    AssetIndexInvalid { reason: String },
    /// One asset-object entry is unusable.
    AssetInvalid { name: String, reason: String },
    /// An artifact acquisition failed (transport, integrity, or cache).
    Acquisition(AcquisitionError),
    /// Plan-supplied artifact metadata could not become a valid source.
    InvalidSource(String),
    /// Copying a verified object into staging failed or short-changed.
    Materialization {
        path: String,
        source: std::io::Error,
    },
    /// Native extraction failed (unsafe, conflicting, or unreadable).
    Native(NativeExtractionError),
    /// Staged content failed validation before commit.
    Validation { path: String, reason: String },
    /// The staged installation could not be committed.
    Commit {
        context: String,
        source: std::io::Error,
    },
    /// Staging could not be prepared or cleaned.
    Storage(std::io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInProgress { instance_id } => write!(
                formatter,
                "an installation is already in progress for instance '{instance_id}'"
            ),
            Self::TargetConflict { path, reason } => {
                write!(
                    formatter,
                    "the installation target conflicts with existing state at {path}: {reason}"
                )
            }
            Self::State(error) => write!(formatter, "{error}"),
            Self::AssetIndexInvalid { reason } => {
                write!(formatter, "the verified asset index is unusable: {reason}")
            }
            Self::AssetInvalid { name, reason } => write!(
                formatter,
                "asset object '{name}' cannot be installed: {reason}"
            ),
            Self::Acquisition(error) => write!(formatter, "{error}"),
            Self::InvalidSource(reason) => {
                write!(formatter, "planned artifact metadata is unusable: {reason}")
            }
            Self::Materialization { path, source } => write!(
                formatter,
                "the verified artifact for '{path}' could not be materialized into the installation: {source}"
            ),
            Self::Native(error) => write!(formatter, "{error}"),
            Self::Validation { path, reason } => write!(
                formatter,
                "the staged installation failed validation at '{path}': {reason}"
            ),
            Self::Commit { context, source } => write!(
                formatter,
                "the staged installation could not be committed while {context}: {source}"
            ),
            Self::Storage(error) => write!(
                formatter,
                "the installation staging area could not be managed: {error}"
            ),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<NativeExtractionError> for InstallError {
    fn from(error: NativeExtractionError) -> Self {
        Self::Native(error)
    }
}

impl From<AcquisitionError> for InstallError {
    fn from(error: AcquisitionError) -> Self {
        Self::Acquisition(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::metadata::LoaderProfileDocument;
    use crate::fabric::plan::{compose_game_plan, plan_loader_profile};
    use crate::minecraft::metadata::VersionDocument;
    use crate::minecraft::plan::plan_version_document;
    use crate::minecraft::rules::PlatformProfile;
    use crate::test_support::{TestResponse, TestServer};
    use std::collections::{BTreeMap, HashSet};
    use std::io::Write as _;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Everything the synthetic installation serves, from one loopback
    /// server. Bodies are fixed at build time; selected paths can be made
    /// to fail (HTTP 404) to inject acquisition failures.
    struct Fixture {
        server: TestServer,
        bodies: BTreeMap<String, Vec<u8>>,
        broken: Arc<Mutex<HashSet<String>>>,
    }

    impl Fixture {
        fn break_path(&self, path: &str) {
            self.broken.lock().unwrap().insert(path.to_owned());
        }
    }

    fn sha1_hex(bytes: &[u8]) -> String {
        Sha1Digest::compute(bytes).as_hex()
    }

    fn sha256_digest(bytes: &[u8]) -> ArtifactDigest {
        use sha2::Digest as _;
        ArtifactDigest::from_sha256(sha2::Sha256::digest(bytes).into())
    }

    /// A tiny deterministic native archive: one DLL, one dylib, plus
    /// META-INF content that must not be extracted.
    fn native_zip_bytes() -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            for (name, bytes) in [
                ("META-INF/MANIFEST.MF", &b"Manifest-Version: 1.0"[..]),
                ("lwjgl.dll", b"synthetic native dll bytes"),
                ("liblwjgl.dylib", b"synthetic native dylib bytes"),
            ] {
                writer
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    /// Builds the complete synthetic game world and the composed plan for it.
    fn synthetic_game() -> (Fixture, GameInstallPlan) {
        let client = b"synthetic client jar bytes".to_vec();
        let logging = b"<Configuration status=\"WARN\"></Configuration>".to_vec();
        let mojang_library = b"synthetic mojang library jar".to_vec();
        let native_archive = native_zip_bytes();
        let fabric_common = b"synthetic digested fabric library jar".to_vec();
        let fabric_loader = b"synthetic digest-less fabric loader jar".to_vec();
        let intermediary = b"synthetic digest-less intermediary jar".to_vec();

        let asset_a = b"tiny png bytes a".to_vec();
        let asset_b = b"tiny png bytes b".to_vec();
        let asset_a_hash = sha1_hex(&asset_a);
        let asset_b_hash = sha1_hex(&asset_b);
        let asset_index_body = format!(
            r#"{{"objects": {{
                "icons/icon_16x16.png": {{"hash": "{asset_a_hash}", "size": {}}},
                "icons/icon_16x16_hd.png": {{"hash": "{asset_a_hash}", "size": {}}},
                "minecraft/sounds/click.ogg": {{"hash": "{asset_b_hash}", "size": {}}}
            }}}}"#,
            asset_a.len(),
            asset_a.len(),
            asset_b.len()
        )
        .into_bytes();

        let mut bodies: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        bodies.insert("/mojang/client.jar".to_owned(), client.clone());
        bodies.insert(
            "/mojang/logging/client-1.21.2.xml".to_owned(),
            logging.clone(),
        );
        bodies.insert(
            "/mojang/libraries/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar".to_owned(),
            mojang_library.clone(),
        );
        bodies.insert(
            "/mojang/libraries/org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar".to_owned(),
            native_archive.clone(),
        );
        bodies.insert(
            "/mojang/asset-index/32.json".to_owned(),
            asset_index_body.clone(),
        );
        bodies.insert(
            format!("/assets/{}/{}", &asset_a_hash[..2], asset_a_hash),
            asset_a.clone(),
        );
        bodies.insert(
            format!("/assets/{}/{}", &asset_b_hash[..2], asset_b_hash),
            asset_b.clone(),
        );
        bodies.insert(
            "/fabric-maven/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar".to_owned(),
            fabric_common.clone(),
        );
        bodies.insert(
            "/fabric-maven/net/fabricmc/intermediary/26.2/intermediary-26.2.jar".to_owned(),
            intermediary.clone(),
        );
        bodies.insert(
            "/fabric-maven/net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar".to_owned(),
            fabric_loader.clone(),
        );

        let broken = Arc::new(Mutex::new(HashSet::new()));
        let broken_for_handler = Arc::clone(&broken);
        let bodies_for_handler = Arc::new(bodies.clone());
        let server = TestServer::spawn(Arc::new(move |request| {
            if broken_for_handler.lock().unwrap().contains(&request.path) {
                return TestResponse::status(404);
            }
            match bodies_for_handler.get(&request.path) {
                Some(body) => TestResponse::ok(body),
                None => TestResponse::status(404),
            }
        }));

        let fixture = Fixture {
            server,
            bodies,
            broken,
        };
        let base = fixture.server.base_url().to_owned();
        let plan = synthetic_plan(
            &base,
            &client,
            &logging,
            &mojang_library,
            &native_archive,
            &asset_index_body,
            &fabric_common,
        );
        (fixture, plan)
    }

    #[allow(clippy::too_many_arguments)]
    fn synthetic_plan(
        base: &str,
        client: &[u8],
        logging: &[u8],
        mojang_library: &[u8],
        native_archive: &[u8],
        asset_index_body: &[u8],
        fabric_common: &[u8],
    ) -> GameInstallPlan {
        let version_document_json = format!(
            r#"{{
                "id": "26.2",
                "type": "release",
                "mainClass": "net.minecraft.client.main.Main",
                "javaVersion": {{ "component": "java-runtime-epsilon", "majorVersion": 25 }},
                "assetIndex": {{
                    "id": "32",
                    "sha1": "{}",
                    "size": {},
                    "totalSize": 999999,
                    "url": "{base}/mojang/asset-index/32.json"
                }},
                "downloads": {{
                    "client": {{
                        "sha1": "{}",
                        "size": {},
                        "url": "{base}/mojang/client.jar"
                    }}
                }},
                "logging": {{
                    "client": {{
                        "argument": "-Dlog4j.configurationFile=${{path}}",
                        "file": {{
                            "id": "client-1.21.2.xml",
                            "sha1": "{}",
                            "size": {},
                            "url": "{base}/mojang/logging/client-1.21.2.xml"
                        }},
                        "type": "log4j2-xml"
                    }}
                }},
                "libraries": [
                    {{
                        "downloads": {{ "artifact": {{
                            "path": "com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar",
                            "sha1": "{}",
                            "size": {},
                            "url": "{base}/mojang/libraries/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar"
                        }} }},
                        "name": "com.mojang:brigadier:1.0.18"
                    }},
                    {{
                        "downloads": {{ "artifact": {{
                            "path": "org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar",
                            "sha1": "{}",
                            "size": {},
                            "url": "{base}/mojang/libraries/org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar"
                        }} }},
                        "name": "org.lwjgl:lwjgl:3.4.1:natives-windows"
                    }}
                ],
                "arguments": {{
                    "game": ["--username", "${{auth_player_name}}"],
                    "jvm": ["-Djava.library.path=${{natives_directory}}"]
                }}
            }}"#,
            sha1_hex(asset_index_body),
            asset_index_body.len(),
            sha1_hex(client),
            client.len(),
            sha1_hex(logging),
            logging.len(),
            sha1_hex(mojang_library),
            mojang_library.len(),
            sha1_hex(native_archive),
            native_archive.len(),
        );
        let document = VersionDocument::from_json(&version_document_json).unwrap();
        let minecraft =
            plan_version_document(&document, PlatformProfile::current().unwrap()).unwrap();

        let fabric_common_sha256 = sha256_digest(fabric_common);
        let profile_json = format!(
            r#"{{
                "loader": {{ "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true }},
                "intermediary": {{ "maven": "net.fabricmc:intermediary:26.2", "version": "26.2", "stable": true }},
                "launcherMeta": {{
                    "version": 2,
                    "min_java_version": 8,
                    "libraries": {{
                        "client": [],
                        "common": [
                            {{
                                "name": "org.ow2.asm:asm:9.10.1",
                                "url": "{base}/fabric-maven/",
                                "sha256": "{}",
                                "size": {}
                            }}
                        ],
                        "server": [],
                        "development": []
                    }},
                    "mainClass": {{
                        "client": "net.fabricmc.loader.impl.launch.knot.KnotClient",
                        "server": "net.fabricmc.loader.impl.launch.knot.KnotServer"
                    }}
                }}
            }}"#,
            fabric_common_sha256.as_hex(),
            fabric_common.len(),
        );
        let profile = LoaderProfileDocument::from_json(&profile_json).unwrap();
        let fabric = plan_loader_profile(
            &profile,
            &crate::minecraft::metadata::MinecraftVersionId::new("26.2").unwrap(),
            &crate::fabric::metadata::LoaderVersionId::new("0.19.5").unwrap(),
        )
        .unwrap();

        compose_game_plan(minecraft, fabric).unwrap()
    }

    fn test_context(server_base: &str) -> InstallContext {
        InstallContext {
            download_options: crate::downloads::DownloadOptions {
                connect_timeout: Duration::from_secs(5),
                idle_read_timeout: Duration::from_secs(5),
                max_redirects: crate::downloads::MAX_REDIRECTS,
            },
            asset_endpoints: AssetObjectEndpoints::loopback_for_testing(&format!(
                "{server_base}/assets/"
            )),
        }
    }

    struct TestEnv {
        managed: ManagedPaths,
        instance: InstanceId,
    }

    impl TestEnv {
        /// Each test owns a distinct instance id: the installation exclusion
        /// is process-global, so shared ids would serialize (and collide)
        /// across concurrently running tests.
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join("aurora-install-test")
                .join(std::process::id().to_string())
                .join(name);
            let _ = std::fs::remove_dir_all(&root);
            let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();
            let instance = InstanceId::new(&format!("inst-{name}")).unwrap();
            Self { managed, instance }
        }

        /// Creates user-sensitive content that installation must never touch.
        fn plant_user_data(&self) {
            let paths = self.managed.instance_paths(&self.instance);
            std::fs::create_dir_all(paths.mods()).unwrap();
            std::fs::write(paths.mods().join("user-mod.jar"), b"precious user mod").unwrap();
            std::fs::create_dir_all(paths.root().join("saves")).unwrap();
            std::fs::write(
                paths.root().join("saves").join("world.dat"),
                b"precious save",
            )
            .unwrap();
            std::fs::create_dir_all(paths.config()).unwrap();
            std::fs::write(paths.config().join("options.txt"), b"gamma:1.0").unwrap();
        }

        fn assert_user_data_intact(&self) {
            let paths = self.managed.instance_paths(&self.instance);
            assert_eq!(
                std::fs::read(paths.mods().join("user-mod.jar")).unwrap(),
                b"precious user mod"
            );
            assert_eq!(
                std::fs::read(paths.root().join("saves").join("world.dat")).unwrap(),
                b"precious save"
            );
            assert_eq!(
                std::fs::read(paths.config().join("options.txt")).unwrap(),
                b"gamma:1.0"
            );
        }

        fn staging(&self) -> PathBuf {
            self.managed
                .instance_paths(&self.instance)
                .root()
                .join(STAGING_DIR_NAME)
        }

        fn game(&self) -> PathBuf {
            self.managed
                .instance_paths(&self.instance)
                .game()
                .to_path_buf()
        }
    }

    async fn install(
        env: &TestEnv,
        fixture: &Fixture,
        plan: &GameInstallPlan,
        faults: InstallFaults,
    ) -> Result<InstalledGame, InstallError> {
        install_game(
            &env.managed,
            &env.instance,
            plan,
            &test_context(fixture.server.base_url()),
            &mut |_| {},
            faults,
        )
        .await
    }

    #[tokio::test]
    async fn a_synthetic_game_installs_completely_and_validates() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("happy");
        env.plant_user_data();

        let installed = install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .expect("the synthetic game must install");

        // The committed tree: everything the plan described, in place.
        let game = env.game();
        assert!(game.join("versions/26.2/client.jar").is_file());
        assert!(game.join("versions/26.2/client-1.21.2.xml").is_file());
        assert!(game.join("assets/indexes/32.json").is_file());
        assert!(
            game.join("libraries/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar")
                .is_file()
        );
        assert!(
            game.join("libraries/org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar")
                .is_file()
        );
        assert!(
            game.join("libraries/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar")
                .is_file()
        );
        assert!(
            game.join("libraries/net/fabricmc/intermediary/26.2/intermediary-26.2.jar")
                .is_file()
        );
        assert!(
            game.join("libraries/net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar")
                .is_file()
        );

        // Asset objects: deduplicated by hash (two names, one object).
        let index = AssetIndexObjectsDocument::from_json(
            &std::fs::read_to_string(game.join("assets/indexes/32.json")).unwrap(),
        )
        .unwrap();
        let objects = assets::plan_asset_objects(&index).unwrap();
        assert_eq!(objects.len(), 2);
        for object in &objects {
            assert!(game.join(object.game_relative_path()).is_file());
        }

        // Natives: extracted, without META-INF.
        let natives = game.join("natives/26.2");
        assert!(natives.join("lwjgl.dll").is_file());
        assert!(natives.join("liblwjgl.dylib").is_file());
        assert!(!natives.join("META-INF").exists());

        // The completion record is committed inside the game directory.
        let manifest = load_installed_state(&game)
            .unwrap()
            .expect("manifest committed");
        assert_eq!(manifest.minecraft_version(), "26.2");
        assert_eq!(manifest.fabric_loader_version(), "0.19.5");
        assert!(manifest.files().len() >= 9);

        // Trust classes are distinct and honest.
        let trust: Vec<&ArtifactTrust> = manifest.files().iter().map(|f| f.trust()).collect();
        assert!(trust.iter().any(|t| matches!(
            t,
            ArtifactTrust::ExpectedDigestVerified {
                algorithm: DigestAlgorithm::Sha1,
                ..
            }
        )));
        assert!(trust.iter().any(|t| matches!(
            t,
            ArtifactTrust::ExpectedDigestVerified {
                algorithm: DigestAlgorithm::Sha256,
                ..
            }
        )));
        assert!(
            trust
                .iter()
                .any(|t| matches!(t, ArtifactTrust::SecureTransportObserved { .. }))
        );

        // Deterministic installed-state: validation passes cleanly.
        match validate_installed_game(&env.managed, &env.instance).unwrap() {
            ValidationOutcome::Installed(validation) => {
                assert_eq!(validation.status, ValidationStatus::Valid);
                assert!(validation.problems.is_empty());
                assert_eq!(validation.checked_files, manifest.files().len());
            }
            ValidationOutcome::NotInstalled => panic!("the installed game must validate"),
        }

        // No staging remains; user data untouched.
        assert!(!env.staging().exists());
        env.assert_user_data_intact();
        assert_eq!(installed.total_byte_count() > 0, true);
    }

    #[tokio::test]
    async fn an_acquisition_failure_leaves_no_complete_looking_installation() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("acquisition-failure");
        env.plant_user_data();
        fixture.break_path("/mojang/libraries/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar");

        let error = install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .expect_err("a 404 artifact must fail the installation");

        assert!(matches!(error, InstallError::Acquisition(_)), "{error}");
        assert!(!env.game().exists(), "no game directory may exist");
        env.assert_user_data_intact();
    }

    #[tokio::test]
    async fn a_materialization_failure_leaves_only_staging() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("materialization-failure");
        env.plant_user_data();

        let error = install(
            &env,
            &fixture,
            &plan,
            InstallFaults {
                fail_during_materialization: true,
                ..InstallFaults::default()
            },
        )
        .await
        .expect_err("the injected materialization fault must fail");

        assert!(matches!(error, InstallError::Materialization { .. }));
        assert!(!env.game().exists());
        // Staging exists (deliberate recoverable state) but is clearly not a
        // game directory, and it contains no completion record at its own
        // root or the instance root.
        assert!(env.staging().exists());
        assert!(
            !env.managed
                .instance_paths(&env.instance)
                .root()
                .join(INSTALLED_GAME_FILE_NAME)
                .exists()
        );
        env.assert_user_data_intact();
    }

    #[tokio::test]
    async fn a_malicious_native_archive_fails_extraction_safely() {
        // A traversal-bearing archive whose SHA-1 is computed from its own
        // bytes: it "verifies" as an artifact, so extraction safety must be
        // the line of defense.
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            writer
                .start_file("../evil.dll", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"escape attempt").unwrap();
            writer.finish().unwrap();
        }
        let malicious = cursor.into_inner();

        let (fixture, _plan) = synthetic_game();
        let env = TestEnv::new("malicious-native");
        // Serve the fixture bodies with the native artifact replaced by the
        // malicious archive (whose SHA-1 the plan is rebuilt around, so it
        // "verifies" — extraction safety is the line of defense).
        let mut bodies = fixture.bodies.clone();
        bodies.insert(
            "/mojang/libraries/org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar".to_owned(),
            malicious.clone(),
        );
        let served: Arc<BTreeMap<String, Vec<u8>>> = Arc::new(bodies);
        let server = TestServer::spawn(Arc::new(move |request| {
            served
                .get(&request.path)
                .map(|body| TestResponse::ok(body))
                .unwrap_or(TestResponse::status(404))
        }));

        let plan = synthetic_plan(
            server.base_url(),
            &fixture.bodies["/mojang/client.jar"],
            &fixture.bodies["/mojang/logging/client-1.21.2.xml"],
            &fixture.bodies["/mojang/libraries/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar"],
            &malicious,
            &fixture.bodies["/mojang/asset-index/32.json"],
            &fixture.bodies["/fabric-maven/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar"],
        );

        let error = install_game(
            &env.managed,
            &env.instance,
            &plan,
            &test_context(server.base_url()),
            &mut |_| {},
            InstallFaults::default(),
        )
        .await
        .expect_err("a traversal archive must fail extraction");

        assert!(
            matches!(
                error,
                InstallError::Native(NativeExtractionError::ArchiveInvalid { .. })
            ),
            "{error}"
        );
        assert!(!env.game().exists());
        // Nothing escaped staging.
        let instance_root = env
            .managed
            .instance_paths(&env.instance)
            .root()
            .to_path_buf();
        assert!(!instance_root.join("evil.dll").exists());
    }

    #[tokio::test]
    async fn an_injected_pre_commit_failure_never_completes_the_installation() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("pre-commit-failure");
        env.plant_user_data();

        let error = install(
            &env,
            &fixture,
            &plan,
            InstallFaults {
                fail_before_commit: true,
                ..InstallFaults::default()
            },
        )
        .await
        .expect_err("the injected commit fault must fail");

        assert!(matches!(error, InstallError::Commit { .. }));
        // No game directory: the staged tree (complete, manifest included)
        // lives only under staging and cannot be mistaken for an install.
        assert!(!env.game().exists());
        assert!(
            env.staging()
                .join("game")
                .join(INSTALLED_GAME_FILE_NAME)
                .is_file()
        );
        env.assert_user_data_intact();

        // Retry is deliberate: the next attempt clears staging and succeeds.
        install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .expect("the retry must succeed");
        assert!(env.game().join(INSTALLED_GAME_FILE_NAME).is_file());
        assert!(!env.staging().exists());
    }

    #[tokio::test]
    async fn stale_staging_from_an_interrupted_attempt_is_cleared_and_rebuilt() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("stale-staging");
        let staging = env.staging();
        std::fs::create_dir_all(staging.join("game").join("junk")).unwrap();
        std::fs::write(
            staging.join("game/junk/leftover.bin"),
            b"interrupted debris",
        )
        .unwrap();

        install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .expect("stale staging must not block a fresh install");

        assert!(env.game().join(INSTALLED_GAME_FILE_NAME).is_file());
        assert!(!staging.exists());
    }

    #[tokio::test]
    async fn an_unrecognized_existing_game_directory_is_a_hard_conflict() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("foreign-game");
        let game = env.game();
        std::fs::create_dir_all(game.join("versions")).unwrap();
        std::fs::write(
            game.join("versions").join("foreign.txt"),
            b"someone else's data",
        )
        .unwrap();

        let error = install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .expect_err("unrecognizable existing state must conflict");

        assert!(
            matches!(error, InstallError::TargetConflict { .. }),
            "{error}"
        );
        // The foreign content is untouched.
        assert_eq!(
            std::fs::read(game.join("versions").join("foreign.txt")).unwrap(),
            b"someone else's data"
        );
    }

    #[tokio::test]
    async fn a_valid_installation_is_deliberately_replaced_and_user_data_survives() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("replace");
        env.plant_user_data();

        install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .unwrap();
        let first_manifest = load_installed_state(&env.game()).unwrap().unwrap();

        // A second installation of the same plan replaces the first: the
        // game directory is proven-managed by its manifest.
        install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .expect("replacement must be supported");

        let second_manifest = load_installed_state(&env.game()).unwrap().unwrap();
        assert_ne!(
            first_manifest.installation_id(),
            second_manifest.installation_id(),
            "the replacement is a new revision"
        );
        assert!(!env.staging().exists(), "the retired tree is gone");
        env.assert_user_data_intact();

        match validate_installed_game(&env.managed, &env.instance).unwrap() {
            ValidationOutcome::Installed(validation) => {
                assert_eq!(validation.status, ValidationStatus::Valid);
            }
            ValidationOutcome::NotInstalled => panic!("the replacement must validate"),
        }
    }

    #[tokio::test]
    async fn tampering_is_detected_by_validation_without_mutation() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("tampered");
        install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .unwrap();

        // Corrupt one managed file after installation.
        let client = env.game().join("versions/26.2/client.jar");
        std::fs::write(&client, b"tampered bytes").unwrap();
        // Remove one asset object entirely.
        let index_text =
            std::fs::read_to_string(env.game().join("assets/indexes/32.json")).unwrap();
        let index = AssetIndexObjectsDocument::from_json(&index_text).unwrap();
        let victim = &assets::plan_asset_objects(&index).unwrap()[0];
        std::fs::remove_file(env.game().join(victim.game_relative_path())).unwrap();

        match validate_installed_game(&env.managed, &env.instance).unwrap() {
            ValidationOutcome::Installed(validation) => {
                assert_eq!(validation.status, ValidationStatus::Damaged);
                assert_eq!(validation.problems.len(), 2, "{:?}", validation.problems);
                assert!(
                    validation
                        .problems
                        .iter()
                        .any(|p| p.path.ends_with("client.jar"))
                );
            }
            ValidationOutcome::NotInstalled => panic!("tampering must be reported, not hidden"),
        }

        // Validation mutated nothing: the tampered file is still the tampered
        // bytes, and the manifest still describes the original expectation.
        assert_eq!(std::fs::read(&client).unwrap(), b"tampered bytes");
    }

    #[tokio::test]
    async fn validation_reports_an_absent_installation_as_not_installed() {
        let env = TestEnv::new("not-installed");
        assert_eq!(
            validate_installed_game(&env.managed, &env.instance).unwrap(),
            ValidationOutcome::NotInstalled
        );
    }

    #[tokio::test]
    async fn a_malformed_installed_state_manifest_fails_deliberately() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("malformed-state");
        install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .unwrap();

        let manifest_path = env.game().join(INSTALLED_GAME_FILE_NAME);
        std::fs::write(&manifest_path, "{ not valid json").unwrap();

        let error = validate_installed_game(&env.managed, &env.instance)
            .expect_err("malformed installed state must fail");
        assert!(matches!(error, InstallError::State(_)), "{error}");

        // Reinstallation also refuses to wipe it: the state is the user's to
        // repair, not the installer's to overwrite.
        let error = install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .expect_err("conflicting malformed state must not be replaced");
        assert!(matches!(error, InstallError::State(_)), "{error}");
        assert_eq!(
            std::fs::read_to_string(&manifest_path).unwrap(),
            "{ not valid json"
        );
    }

    #[tokio::test]
    async fn simultaneous_installs_of_one_instance_are_excluded() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("concurrent");

        // Slow the client transfer so the first install is still running
        // when the second attempt arrives. A drip-delayed re-serve of the
        // client body achieves this without touching the fixture map.
        let slow_bodies: Arc<BTreeMap<String, Vec<u8>>> = Arc::new(fixture.bodies.clone());
        let slow_server = TestServer::spawn(Arc::new(move |request| {
            slow_bodies
                .get(&request.path)
                .map(|body| TestResponse::ok(body).with_drip_delay(Duration::from_millis(8)))
                .unwrap_or(TestResponse::status(404))
        }));
        let slow_plan = synthetic_plan(
            slow_server.base_url(),
            &fixture.bodies["/mojang/client.jar"],
            &fixture.bodies["/mojang/logging/client-1.21.2.xml"],
            &fixture.bodies["/mojang/libraries/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar"],
            &native_zip_bytes(),
            &fixture.bodies["/mojang/asset-index/32.json"],
            &fixture.bodies["/fabric-maven/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar"],
        );

        let first = {
            let managed = env.managed.clone();
            let instance = env.instance.clone();
            let context = test_context(slow_server.base_url());
            let plan = slow_plan.clone();
            tokio::spawn(async move {
                install_game(
                    &managed,
                    &instance,
                    &plan,
                    &context,
                    &mut |_| {},
                    InstallFaults::default(),
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(150)).await;

        let second = install(&env, &fixture, &plan, InstallFaults::default()).await;

        assert!(
            matches!(second, Err(InstallError::AlreadyInProgress { .. })),
            "the overlapping attempt must be excluded, got: {:?}",
            second.err()
        );
        first
            .await
            .unwrap()
            .expect("the first installation completes");
        assert!(env.game().join(INSTALLED_GAME_FILE_NAME).is_file());
    }

    #[tokio::test]
    async fn digest_less_fabric_artifacts_are_never_reported_as_digest_verified() {
        let (fixture, plan) = synthetic_game();
        let env = TestEnv::new("trust-honesty");
        install(&env, &fixture, &plan, InstallFaults::default())
            .await
            .unwrap();

        let manifest = load_installed_state(&env.game()).unwrap().unwrap();
        let loader_entry = manifest
            .files()
            .iter()
            .find(|file| file.path().contains("fabric-loader"))
            .unwrap();
        assert_eq!(loader_entry.trust().kind_name(), "secureTransportObserved");

        let asm_entry = manifest
            .files()
            .iter()
            .find(|file| file.path().contains("org/ow2/asm"))
            .unwrap();
        assert_eq!(loader_entry.trust().kind_name(), "secureTransportObserved");
        assert_eq!(asm_entry.trust().kind_name(), "expectedDigestVerified");

        // And the persisted document keeps the distinction machine-readable.
        let text = std::fs::read_to_string(env.game().join(INSTALLED_GAME_FILE_NAME)).unwrap();
        assert!(text.contains("\"secureTransportObserved\""));
        assert!(text.contains("\"expectedDigestVerified\""));
        assert!(text.contains("\"algorithm\": \"sha1\""));
        assert!(text.contains("\"algorithm\": \"sha256\""));
    }

    /// Controlled live installation against the real official Mojang and
    /// Fabric chains, end to end: resolve both plans, compose, install into
    /// a disposable temp-root instance, and validate the result.
    ///
    /// Ignored by default so the offline suite never depends on public
    /// services or downloads hundreds of megabytes; run explicitly with
    /// `cargo test -- --ignored --nocapture` when verifying. It downloads
    /// the real game artifacts into a throwaway directory outside the
    /// launcher's own managed data root — never the user's `.minecraft`,
    /// never the launcher's real instance storage.
    #[tokio::test]
    #[ignore = "downloads the real Minecraft + Fabric artifact set from official sources"]
    async fn live_official_game_installs_and_validates() {
        use crate::fabric;
        use crate::fabric::metadata::{FabricMetaEndpoints, LoaderVersionId};
        use crate::minecraft::metadata::{MetadataEndpoints, MinecraftVersionId};

        let game_version = "26.2";
        let loader_version = "0.19.5";
        let platform = PlatformProfile::current().expect("the host platform must be plannable");
        let options = crate::downloads::DownloadOptions::default();

        let plan = fabric::resolve_game_plan(
            &MetadataEndpoints::official(),
            &FabricMetaEndpoints::official(),
            &MinecraftVersionId::new(game_version).unwrap(),
            &LoaderVersionId::new(loader_version).unwrap(),
            platform,
            &options,
        )
        .await
        .expect("the live plan must resolve");

        // A throwaway managed root under the system temp directory.
        let root = std::env::temp_dir()
            .join("aurora-live-install-test")
            .join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();
        let instance = InstanceId::new("live-verify").unwrap();

        let mut events = 0usize;
        let installed = install_game(
            &managed,
            &instance,
            &plan,
            &InstallContext::official(),
            &mut |progress| {
                events += 1;
                eprintln!(
                    "[live] {:?} {}/{} {:?}",
                    progress.phase,
                    progress.completed_items,
                    progress.total_items,
                    progress.current_item
                );
            },
            InstallFaults::default(),
        )
        .await
        .expect("the live installation must complete");

        eprintln!(
            "[live] installed {} + {} ({} files, {} bytes) into {}",
            installed.manifest().minecraft_version(),
            installed.manifest().fabric_loader_version(),
            installed.manifest().files().len(),
            installed.total_byte_count(),
            installed.game_directory().display()
        );

        // Trust classification of digest-less Fabric artifacts is honest.
        let manifest = load_installed_state(installed.game_directory())
            .unwrap()
            .expect("the manifest must be committed");
        let loader_files: Vec<&InstalledFile> = manifest
            .files()
            .iter()
            .filter(|file| {
                file.path().contains("fabric-loader") || file.path().contains("intermediary")
            })
            .collect();
        assert!(!loader_files.is_empty());
        for file in &loader_files {
            assert_eq!(file.trust().kind_name(), "secureTransportObserved");
            eprintln!(
                "[live] digest-less artifact {} → {}",
                file.path(),
                file.trust()
            );
        }

        // Validation passes against the committed manifest.
        match validate_installed_game(&managed, &instance).unwrap() {
            ValidationOutcome::Installed(validation) => {
                assert_eq!(validation.status, ValidationStatus::Valid);
                eprintln!(
                    "[live] validated {} files ({} bytes), installation {}",
                    validation.checked_files,
                    verification_bytes(&validation),
                    validation.installation_id
                );
            }
            ValidationOutcome::NotInstalled => panic!("the live install must validate"),
        }

        // No staging debris, and no user-data directories were created.
        let instance_paths = managed.instance_paths(&instance);
        let instance_root = instance_paths.root();
        assert!(!instance_root.join(STAGING_DIR_NAME).exists());
        assert!(events > 0, "progress must have been reported");

        fn verification_bytes(validation: &InstalledGameValidation) -> u64 {
            validation.verified_bytes
        }
    }
}
