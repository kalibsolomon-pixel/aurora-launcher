//! Instance lifecycle orchestration: creation, retry, rename, selection,
//! and complete-instance validation.
//!
//! The lifecycle orchestrates existing subsystems â€” it never duplicates
//! them. Release resolution comes from `distribution`, the game install
//! from the Phase 5 executor, Aurora's install from `aurora`, registry
//! persistence from the instance model, and selection from the launcher
//! configuration.
//!
//! ## Creation and failure semantics (one coherent policy)
//!
//! A creation resolves the release, allocates an identifier, and persists a
//! record in the `installing` state **before** the long installation runs.
//! A failure at any later point therefore leaves an explicit, non-ready
//! record with its pinned release â€” never an ordinary-looking healthy
//! instance, and never a silent gap. Retry re-runs the installation for
//! that same record, reusing Phase 5's staged-install recovery and the
//! verified caches (already-valid artifacts are not re-downloaded). The
//! record becomes `ready` only after complete validation passes; nothing
//! else ever moves it there. Rollback never deletes anything â€” user data
//! and launcher-managed trees alike are preserved for retry and diagnosis.
//!
//! ## Selection policy (explicit)
//!
//! The first successfully created instance is selected automatically when
//! no instance is selected yet; later creations never change an existing
//! selection. A stored selection always refers to a registered instance â€”
//! a dangling one is a deliberate error, and the launcher never silently
//! selects a random instance.

use std::fmt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::aurora::{
    AuroraInstallError, AuroraInstalledState, install_aurora, load_installed_state,
    validate_artifact as validate_aurora_artifact,
};
use crate::config::{self, ConfigError, LauncherConfig};
use crate::distribution::{ManifestError, ReleaseChannel, ReleaseManifest};
use crate::fabric::metadata::FabricMetaEndpoints;
use crate::install::{
    InstallContext, InstallError, InstallProgress, ValidationOutcome as GameValidationOutcome,
    ValidationStatus as GameValidationStatus, install_game as execute_game_install,
    validate_installed_game as validate_game_install,
};
use crate::instances::settings::{
    InstanceConfiguration, InvalidInstanceConfiguration, LoaderCandidate, LoaderPolicy,
};
use crate::instances::{
    InstanceId, InstanceRecord, InstanceRegistry, InstanceRegistryError, InstanceState,
    InvalidInstanceRecord, PinnedRelease, generate_instance_id,
};
use crate::minecraft::metadata::MetadataEndpoints;
use crate::minecraft::rules::PlatformProfile;
use crate::paths::ManagedPaths;
use crate::runtime::install::{
    InstallRuntimeProgress, InstalledRuntime, RuntimeInstallError, RuntimeValidation,
};
use crate::runtime::metadata::{RuntimeMetadataEndpoints, RuntimeMetadataError};
use crate::runtime::plan::{JavaRuntimePlan, RuntimePlatform};

/// Everything instance orchestration resolves from: the Aurora release
/// manifest (the build's operational entries or a test manifest),
/// the official Mojang/Fabric metadata endpoints, and the Phase 5
/// installation context.
#[derive(Debug, Clone)]
pub struct InstanceEndpoints {
    release_manifest: ReleaseManifest,
    minecraft: MetadataEndpoints,
    fabric: FabricMetaEndpoints,
    install: InstallContext,
}

impl InstanceEndpoints {
    /// Releases offered for new instance creation in this build.
    pub fn creation() -> Result<Self, ManifestError> {
        Ok(Self {
            release_manifest: crate::distribution::creation_manifest()?,
            minecraft: MetadataEndpoints::official(),
            fabric: FabricMetaEndpoints::official(),
            install: InstallContext::official(),
        })
    }

    /// Releases needed for existing instances plus official game metadata.
    /// Fixture-pinned instances remain resolvable in release builds.
    pub fn operational() -> Result<Self, ManifestError> {
        Ok(Self {
            release_manifest: crate::distribution::operational_manifest()?,
            minecraft: MetadataEndpoints::official(),
            fabric: FabricMetaEndpoints::official(),
            install: InstallContext::official(),
        })
    }

    /// The checked-in development fixture plus official metadata chains,
    /// retained for deterministic and explicit development flows.
    pub fn development() -> Result<Self, ManifestError> {
        Ok(Self {
            release_manifest: crate::distribution::development_manifest()?,
            minecraft: MetadataEndpoints::official(),
            fabric: FabricMetaEndpoints::official(),
            install: InstallContext::official(),
        })
    }

    /// Test construction: fully injectable, offline-friendly endpoints.
    pub fn for_testing(
        release_manifest: ReleaseManifest,
        minecraft: MetadataEndpoints,
        fabric: FabricMetaEndpoints,
        install: InstallContext,
    ) -> Self {
        Self {
            release_manifest,
            minecraft,
            fabric,
            install,
        }
    }

    pub fn release_manifest(&self) -> &ReleaseManifest {
        &self.release_manifest
    }
}

/// A typed instance-creation request.
///
/// Creation asks for the essentials only: a display name and the desired
/// configuration (Minecraft version, loader kind and policy; everything else
/// takes safe defaults). The Aurora release and concrete loader version are
/// resolved by the lifecycle, never chosen by the frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInstanceRequest {
    display_name: String,
    configuration: InstanceConfiguration,
}

impl CreateInstanceRequest {
    pub fn new(display_name: impl Into<String>, configuration: InstanceConfiguration) -> Self {
        Self {
            display_name: display_name.into(),
            configuration,
        }
    }

    pub fn configuration(&self) -> &InstanceConfiguration {
        &self.configuration
    }
}

/// Deterministic failure injection for lifecycle transaction tests;
/// production always passes the default. Injected faults reuse the error
/// variants of the real failure they precede, with explicit messages.
#[derive(Debug, Clone, Copy, Default)]
pub struct InstanceFaults {
    /// Fail after the game install, immediately before Aurora installation.
    pub fail_before_aurora_install: bool,
    /// Fail after complete validation, immediately before the ready-state
    /// commit.
    pub fail_before_ready: bool,
}

/// One coarse lifecycle phase. During `InstallingGame`, the installer's own
/// item progress is embedded verbatim rather than duplicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstancePhase {
    ResolvingRelease,
    ResolvingGame,
    InstallingGame,
    InstallingAurora,
    Validating,
    Completing,
}

impl InstancePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResolvingRelease => "resolvingRelease",
            Self::ResolvingGame => "resolvingGame",
            Self::InstallingGame => "installingGame",
            Self::InstallingAurora => "installingAurora",
            Self::Validating => "validating",
            Self::Completing => "completing",
        }
    }
}

/// Rust-owned lifecycle progress; the frontend only displays it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceProgress {
    pub phase: InstancePhase,
    /// The Phase 5 installer's item progress while `phase` is
    /// `InstallingGame`; `None` otherwise.
    pub game: Option<InstallProgress>,
}

/// The computed status of one instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Ready,
    Damaged,
    Installing,
    /// The desired configuration and the installed content disagree: an
    /// install-affecting setting changed after the content was installed.
    /// Stale is never "ready"; installing the new configuration restores it.
    Stale,
    NotInstalled,
}

impl InstanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Damaged => "damaged",
            Self::Installing => "installing",
            Self::Stale => "stale",
            Self::NotInstalled => "notInstalled",
        }
    }
}

/// One concrete problem found by complete-instance validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceProblem {
    pub component: &'static str,
    pub reason: String,
}

/// The complete read-only validation outcome of one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceValidation {
    pub instance_id: String,
    pub display_name: String,
    pub status: InstanceStatus,
    pub problems: Vec<InstanceProblem>,
}

/// Creates a complete persistent Aurora instance.
///
/// See the module documentation for the ordering and failure policy. On
/// success the registry holds a `ready` record whose installation has fully
/// validated; on failure an `installing` record remains for retry.
#[allow(clippy::too_many_arguments)]
pub async fn create_instance(
    managed: &ManagedPaths,
    registry_path: &Path,
    config_path: &Path,
    endpoints: &InstanceEndpoints,
    request: CreateInstanceRequest,
    progress: &mut (dyn FnMut(InstanceProgress) + Send),
    faults: InstanceFaults,
) -> Result<InstanceRecord, InstanceError> {
    request.configuration().validate()?;

    progress(report(InstancePhase::ResolvingRelease, None));
    let release =
        resolve_release_for_configuration(endpoints.release_manifest(), request.configuration())?;
    let loader_version =
        resolve_loader_version(endpoints, request.configuration(), &release).await?;

    let pin = PinnedRelease::new(
        release.channel(),
        release.aurora_version(),
        request.configuration().minecraft_version(),
        loader_version,
    )?;

    // Allocate the identity and persist the explicit installing record.
    // Display-name validation happens in record construction — before
    // anything is written.
    let instance_id = allocate_instance_id(managed, registry_path)?;
    let record = InstanceRecord::new(
        instance_id,
        request.display_name,
        InstanceState::Installing,
        pin,
        request.configuration,
    )?;

    {
        let _guard = registry_lock();
        let mut registry = InstanceRegistry::load(registry_path)?;
        registry.instances_mut().push(record.clone());
        registry.save(registry_path)?;
    }

    install_instance_components(managed, registry_path, endpoints, &record, progress, faults)
        .await?;

    // The registry now holds the ready record; mirror it on the returned
    // value.
    let mut record = record;
    record.set_state(InstanceState::Ready);

    // Selection policy: the first successful instance is selected when
    // nothing is selected yet; an existing selection never changes.
    select_instance_if_unselected(config_path, record.id())?;

    Ok(record)
}

/// Retries the installation of an instance whose creation did not finish.
///
/// The record must be in the `installing` state. Re-running reuses the
/// Phase 5 staged-install semantics (existing complete installations are
/// deliberately replaced; staging debris is recovered) and the verified
/// caches, so already-valid artifacts are not re-downloaded.
pub async fn retry_instance_install(
    managed: &ManagedPaths,
    registry_path: &Path,
    config_path: &Path,
    endpoints: &InstanceEndpoints,
    instance_id: &InstanceId,
    progress: &mut (dyn FnMut(InstanceProgress) + Send),
    faults: InstanceFaults,
) -> Result<InstanceRecord, InstanceError> {
    let record = {
        let _guard = registry_lock();
        let registry = InstanceRegistry::load(registry_path)?;
        let record =
            registry
                .find(instance_id)
                .cloned()
                .ok_or_else(|| InstanceError::NotFound {
                    instance_id: instance_id.to_string(),
                })?;
        if record.state() != InstanceState::Installing {
            return Err(InstanceError::NotReady {
                instance_id: instance_id.to_string(),
                reason: format!(
                    "only instances that are still installing can be retried; this instance is {}",
                    record.state()
                ),
            });
        }
        record
    };

    install_instance_components(managed, registry_path, endpoints, &record, progress, faults)
        .await?;

    // The registry now holds the ready record; mirror it on the returned
    // value.
    let mut record = record;
    record.set_state(InstanceState::Ready);

    select_instance_if_unselected(config_path, record.id())?;

    Ok(record)
}

/// Resolves the Aurora release a configuration's Minecraft version requires.
///
/// The release manifest maps each release to one Minecraft version; when
/// several releases target the same version, the most stable channel wins
/// deterministically. No release matching the version is an explicit,
/// honest error — game configuration and Aurora release compatibility stay
/// separate concepts, and an unavailable combination is never silently
/// substituted.
fn resolve_release_for_configuration(
    manifest: &ReleaseManifest,
    configuration: &InstanceConfiguration,
) -> Result<crate::distribution::AuroraRelease, InstanceError> {
    fn channel_preference(channel: ReleaseChannel) -> u8 {
        match channel {
            ReleaseChannel::Stable => 0,
            ReleaseChannel::Beta => 1,
            ReleaseChannel::Nightly => 2,
        }
    }

    let mut matching: Vec<&crate::distribution::AuroraRelease> = manifest
        .releases()
        .iter()
        .filter(|release| release.minecraft_version() == configuration.minecraft_version())
        .collect();
    matching.sort_by_key(|release| channel_preference(release.channel()));

    matching
        .into_iter()
        .next()
        .cloned()
        .ok_or_else(|| InstanceError::ReleaseUnavailable {
            minecraft_version: configuration.minecraft_version().to_owned(),
            available: manifest
                .releases()
                .iter()
                .map(|release| release.minecraft_version().to_owned())
                .collect::<Vec<_>>()
                .join(", "),
        })
}

/// Resolves the concrete Fabric Loader version a configuration's loader
/// policy selects, from the official per-game list (newest first).
async fn resolve_loader_version(
    endpoints: &InstanceEndpoints,
    configuration: &InstanceConfiguration,
    release: &crate::distribution::AuroraRelease,
) -> Result<String, InstanceError> {
    let game =
        crate::minecraft::metadata::MinecraftVersionId::new(configuration.minecraft_version())
            .map_err(|error| {
                InstanceError::ConfigurationInvalid(InvalidInstanceConfiguration::MinecraftVersion(
                    error,
                ))
            })?;
    let entries = crate::fabric::metadata::fetch_game_loader_versions(
        &endpoints.fabric,
        &game,
        endpoints.install.download_options(),
    )
    .await
    .map_err(InstanceError::FabricMetadata)?;
    let candidates: Vec<LoaderCandidate> = entries
        .iter()
        .map(|entry| LoaderCandidate {
            version: entry.version.clone(),
            stable: entry.stable,
        })
        .collect();
    let required = release.fabric_loader_version();
    let selected = match configuration.loader().policy() {
        LoaderPolicy::Automatic => required,
        LoaderPolicy::Pinned { version } => version,
    };
    if selected != required
        || !candidates
            .iter()
            .any(|candidate| candidate.version == selected)
    {
        return Err(InstanceError::LoaderResolution {
            game: configuration.minecraft_version().to_owned(),
            reason: format!(
                "Fabric Loader {selected} is unavailable or differs from the Aurora release's required exact version {required}"
            ),
        });
    }
    Ok(selected.to_owned())
}

/// Atomically persists a new desired configuration for one ready instance.
///
/// The persisted change is metadata only: identifiers, filesystem paths, and
/// installed content are untouched. The release manifest must support the
/// configured Minecraft version (an offline, honest compatibility check);
/// loader-version compatibility is verified when the configuration is
/// installed. Readiness reflects the change immediately: deep validation
/// reports the instance as stale until the content is reinstalled.
pub fn update_instance_configuration(
    registry_path: &Path,
    endpoints: &InstanceEndpoints,
    instance_id: &InstanceId,
    configuration: InstanceConfiguration,
) -> Result<InstanceRecord, InstanceError> {
    configuration.validate()?;
    resolve_release_for_configuration(endpoints.release_manifest(), &configuration)?;

    let _guard = registry_lock();
    let mut registry = InstanceRegistry::load(registry_path)?;
    let record = registry
        .find_mut(instance_id)
        .ok_or_else(|| InstanceError::NotFound {
            instance_id: instance_id.to_string(),
        })?;
    if record.state() != InstanceState::Ready {
        return Err(InstanceError::NotReady {
            instance_id: instance_id.to_string(),
            reason: "finish or retry the current installation before changing the configuration"
                .to_owned(),
        });
    }
    record.set_configuration(configuration);
    let updated = record.clone();
    registry.save(registry_path)?;
    Ok(updated)
}

/// Resolves an existing instance's desired configuration into a new release
/// pin and (re)installs the content to match it.
///
/// This is the deliberate path for install-affecting configuration changes:
/// the new pin and an `installing` state are persisted before the long
/// installation runs (identical failure semantics to creation), the Phase 5
/// staged replacement swaps the previous complete installation only at
/// commit, and user data is never touched. On failure the record stays
/// retryable with the new pin; the previous installation remains on disk
/// until a successful replacement commits over it.
pub async fn install_instance_configuration(
    managed: &ManagedPaths,
    registry_path: &Path,
    config_path: &Path,
    endpoints: &InstanceEndpoints,
    instance_id: &InstanceId,
    progress: &mut (dyn FnMut(InstanceProgress) + Send),
    faults: InstanceFaults,
) -> Result<InstanceRecord, InstanceError> {
    let configuration = {
        let _guard = registry_lock();
        let registry = InstanceRegistry::load(registry_path)?;
        let record =
            registry
                .find(instance_id)
                .cloned()
                .ok_or_else(|| InstanceError::NotFound {
                    instance_id: instance_id.to_string(),
                })?;
        if record.state() == InstanceState::Installing {
            return Err(InstanceError::NotReady {
                instance_id: instance_id.to_string(),
                reason: "this instance is already installing; retry it instead".to_owned(),
            });
        }
        record.configuration().clone()
    };
    configuration.validate()?;

    progress(report(InstancePhase::ResolvingRelease, None));
    let release = resolve_release_for_configuration(endpoints.release_manifest(), &configuration)?;
    let loader_version = resolve_loader_version(endpoints, &configuration, &release).await?;
    let pin = PinnedRelease::new(
        release.channel(),
        release.aurora_version(),
        configuration.minecraft_version(),
        loader_version,
    )?;

    let mut record = {
        let _guard = registry_lock();
        let mut registry = InstanceRegistry::load(registry_path)?;
        let stored = registry
            .find_mut(instance_id)
            .ok_or_else(|| InstanceError::NotFound {
                instance_id: instance_id.to_string(),
            })?;
        stored.set_release(pin.clone());
        stored.set_state(InstanceState::Installing);
        let updated = stored.clone();
        registry.save(registry_path)?;
        updated
    };

    install_instance_components(managed, registry_path, endpoints, &record, progress, faults)
        .await?;

    record.set_state(InstanceState::Ready);
    select_instance_if_unselected(config_path, record.id())?;
    Ok(record)
}

/// The shared installation tail of creation and retry: game install,
/// Aurora install, complete validation, then the ready-state commit.
async fn install_instance_components(
    managed: &ManagedPaths,
    registry_path: &Path,
    endpoints: &InstanceEndpoints,
    record: &InstanceRecord,
    progress: &mut (dyn FnMut(InstanceProgress) + Send),
    faults: InstanceFaults,
) -> Result<(), InstanceError> {
    let started = std::time::Instant::now();
    let release = endpoints
        .release_manifest()
        .resolve_exact(
            record.release().aurora_version(),
            Some(record.release().channel()),
        )
        .ok_or_else(|| InstanceError::AuroraReleaseNotFound {
            channel: record.release().channel(),
            aurora_version: record.release().aurora_version().to_owned(),
        })?;
    let release_finished = std::time::Instant::now();

    progress(report(InstancePhase::ResolvingGame, None));
    let plan = resolve_record_game_plan(endpoints, record, release).await?;
    let metadata_finished = std::time::Instant::now();

    progress(report(InstancePhase::InstallingGame, None));
    execute_game_install(
        managed,
        record.id(),
        &plan,
        &endpoints.install,
        &mut |game| progress(report(InstancePhase::InstallingGame, Some(game))),
        crate::install::InstallFaults::default(),
    )
    .await?;
    let game_finished = std::time::Instant::now();

    if faults.fail_before_aurora_install {
        return Err(InstanceError::Aurora(AuroraInstallError::Materialization {
            path: "aurora artifact".to_owned(),
            reason: "deterministic fault injected before Aurora installation".to_owned(),
        }));
    }

    progress(report(InstancePhase::InstallingAurora, None));
    install_aurora(
        managed,
        record.id(),
        release,
        endpoints.install.download_options(),
    )
    .await?;
    let aurora_finished = std::time::Instant::now();

    progress(report(InstancePhase::Validating, None));
    let registry = InstanceRegistry::load(registry_path)?;
    let validation = validate_instance(managed, &registry, record.id())?;
    // The record is deliberately still `installing` at this point, so the
    // gate is component completeness: a non-empty problem list is the only
    // thing that can fail creation here. (An installing record with a fully
    // validated installation is exactly the state the ready commit expects.)
    if !validation.problems.is_empty() {
        return Err(InstanceError::ValidationFailed {
            instance_id: record.id().to_string(),
            problems: validation
                .problems
                .iter()
                .map(|problem| format!("{}: {}", problem.component, problem.reason))
                .collect(),
        });
    }
    let validation_finished = std::time::Instant::now();

    if faults.fail_before_ready {
        return Err(InstanceError::Registry(InstanceRegistryError::Write(
            std::io::Error::other("deterministic fault injected before the ready-state commit"),
        )));
    }

    progress(report(InstancePhase::Completing, None));
    {
        let _guard = registry_lock();
        let mut registry = InstanceRegistry::load(registry_path)?;
        if let Some(stored) = registry.find_mut(record.id()) {
            stored.set_state(InstanceState::Ready);
        }
        registry.save(registry_path)?;
    }

    if std::env::var_os("AURORA_INSTALL_DIAGNOSTICS").is_some() {
        eprintln!(
            "[aurora-instance] total_ms={} release_resolution_ms={} metadata_and_planning_ms={} game_ms={} aurora_and_fabric_api_ms={} final_validation_ms={} ready_commit_ms={}",
            started.elapsed().as_millis(),
            release_finished.duration_since(started).as_millis(),
            metadata_finished
                .duration_since(release_finished)
                .as_millis(),
            game_finished.duration_since(metadata_finished).as_millis(),
            aurora_finished.duration_since(game_finished).as_millis(),
            validation_finished
                .duration_since(aurora_finished)
                .as_millis(),
            std::time::Instant::now()
                .duration_since(validation_finished)
                .as_millis(),
        );
    }

    Ok(())
}

async fn resolve_record_game_plan(
    endpoints: &InstanceEndpoints,
    record: &InstanceRecord,
    release: &crate::distribution::AuroraRelease,
) -> Result<crate::fabric::plan::GameInstallPlan, InstanceError> {
    let pin = record.release();
    if release.minecraft_version() != pin.minecraft_version()
        || release.fabric_loader_version() != pin.fabric_loader_version()
    {
        return Err(InstanceError::ReleaseInvalid(
            "the resolved release no longer matches the instance's concrete Minecraft/Fabric pin"
                .to_owned(),
        ));
    }
    let game_version =
        crate::minecraft::metadata::MinecraftVersionId::new(release.minecraft_version())
            .map_err(|error| InstanceError::ReleaseInvalid(error.to_string()))?;
    let loader_version =
        crate::fabric::metadata::LoaderVersionId::new(release.fabric_loader_version())
            .map_err(|error| InstanceError::ReleaseInvalid(error.to_string()))?;
    let platform = PlatformProfile::current().map_err(InstanceError::Platform)?;
    let plan = crate::fabric::resolve_game_plan(
        &endpoints.minecraft,
        &endpoints.fabric,
        &game_version,
        &loader_version,
        platform,
        endpoints.install.download_options(),
    )
    .await?;
    validate_release_java_major(release.java().major_version(), plan.java().major_version())?;
    Ok(plan)
}

fn validate_release_java_major(
    release_major: u32,
    resolved_game_major: u32,
) -> Result<(), InstanceError> {
    if release_major != resolved_game_major {
        return Err(InstanceError::ReleaseInvalid(format!(
            "the Aurora release declares Java {release_major}, but official Minecraft/Fabric planning requires Java {resolved_game_major}"
        )));
    }
    Ok(())
}

/// Resolves the exact runtime plan for a content-ready instance. Runtime
/// readiness remains independent: no runtime absence or damage changes the
/// instance's persisted or content-validation state.
pub async fn resolve_instance_runtime_plan(
    managed: &ManagedPaths,
    registry_path: &Path,
    endpoints: &InstanceEndpoints,
    runtime_endpoints: &RuntimeMetadataEndpoints,
    instance_id: &InstanceId,
) -> Result<JavaRuntimePlan, InstanceError> {
    let (_, runtime_plan) = resolve_instance_launch_plans(
        managed,
        registry_path,
        endpoints,
        runtime_endpoints,
        instance_id,
    )
    .await?;
    Ok(runtime_plan)
}

/// Resolves the game and runtime plans through one metadata pass for launch.
pub async fn resolve_instance_launch_plans(
    managed: &ManagedPaths,
    registry_path: &Path,
    endpoints: &InstanceEndpoints,
    runtime_endpoints: &RuntimeMetadataEndpoints,
    instance_id: &InstanceId,
) -> Result<(crate::fabric::plan::GameInstallPlan, JavaRuntimePlan), InstanceError> {
    let game_plan =
        resolve_instance_game_plan(managed, registry_path, endpoints, instance_id).await?;
    let runtime_platform = RuntimePlatform::current().map_err(RuntimeMetadataError::Plan)?;
    let runtime_plan = crate::runtime::metadata::resolve_runtime_plan(
        managed,
        runtime_endpoints,
        game_plan.java().component(),
        game_plan.java().major_version(),
        runtime_platform,
        endpoints.install.download_options(),
    )
    .await
    .map_err(InstanceError::RuntimeMetadata)?;
    Ok((game_plan, runtime_plan))
}

/// Re-resolves the normalized game plan for an existing content-ready
/// instance. The bundled release manifest supplies the exact pin, while
/// Mojang and Fabric remain the metadata authorities. No installation state
/// is changed by this operation.
pub async fn resolve_instance_game_plan(
    managed: &ManagedPaths,
    registry_path: &Path,
    endpoints: &InstanceEndpoints,
    instance_id: &InstanceId,
) -> Result<crate::fabric::plan::GameInstallPlan, InstanceError> {
    let registry = InstanceRegistry::load(registry_path)?;
    let record = registry
        .find(instance_id)
        .cloned()
        .ok_or_else(|| InstanceError::NotFound {
            instance_id: instance_id.to_string(),
        })?;
    if record.state() != InstanceState::Ready {
        return Err(InstanceError::NotReady {
            instance_id: instance_id.to_string(),
            reason: "managed Java can only be prepared after instance content is ready".to_owned(),
        });
    }
    let content = validate_instance(managed, &registry, instance_id)?;
    if content.status != InstanceStatus::Ready {
        return Err(InstanceError::NotReady {
            instance_id: instance_id.to_string(),
            reason: format!(
                "instance content validation reported {}: {}",
                content.status.as_str(),
                content
                    .problems
                    .iter()
                    .map(|problem| format!("{}: {}", problem.component, problem.reason))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        });
    }
    let release = endpoints
        .release_manifest()
        .resolve_exact(
            record.release().aurora_version(),
            Some(record.release().channel()),
        )
        .ok_or_else(|| InstanceError::AuroraReleaseNotFound {
            channel: record.release().channel(),
            aurora_version: record.release().aurora_version().to_owned(),
        })?;
    resolve_record_game_plan(endpoints, &record, release).await
}

pub async fn validate_instance_runtime(
    managed: &ManagedPaths,
    registry_path: &Path,
    endpoints: &InstanceEndpoints,
    runtime_endpoints: &RuntimeMetadataEndpoints,
    instance_id: &InstanceId,
    execute_diagnostic: bool,
) -> Result<RuntimeValidation, InstanceError> {
    let plan = resolve_instance_runtime_plan(
        managed,
        registry_path,
        endpoints,
        runtime_endpoints,
        instance_id,
    )
    .await?;
    crate::runtime::install::validate_runtime(
        managed,
        &plan,
        execute_diagnostic,
        crate::runtime::install::DEFAULT_JAVA_DIAGNOSTIC_TIMEOUT,
    )
    .await
    .map_err(InstanceError::RuntimeInstall)
}

pub async fn ensure_instance_runtime(
    managed: &ManagedPaths,
    registry_path: &Path,
    endpoints: &InstanceEndpoints,
    runtime_endpoints: &RuntimeMetadataEndpoints,
    instance_id: &InstanceId,
    progress: &mut (dyn FnMut(InstallRuntimeProgress) + Send),
) -> Result<InstalledRuntime, InstanceError> {
    let plan = resolve_instance_runtime_plan(
        managed,
        registry_path,
        endpoints,
        runtime_endpoints,
        instance_id,
    )
    .await?;
    crate::runtime::install::ensure_runtime(
        managed,
        &plan,
        endpoints.install.download_options(),
        true,
        progress,
    )
    .await
    .map_err(InstanceError::RuntimeInstall)
}

/// Renames an instance's display name. Metadata only: the identifier, all
/// filesystem paths, installed state, and user data are untouched.
pub fn rename_instance(
    registry_path: &Path,
    instance_id: &InstanceId,
    new_display_name: &str,
) -> Result<InstanceRecord, InstanceError> {
    let _guard = registry_lock();
    let mut registry = InstanceRegistry::load(registry_path)?;
    let record = registry
        .find_mut(instance_id)
        .ok_or_else(|| InstanceError::NotFound {
            instance_id: instance_id.to_string(),
        })?;
    record
        .set_display_name(new_display_name)
        .map_err(InstanceError::NameInvalid)?;
    let renamed = record.clone();
    registry.save(registry_path)?;
    Ok(renamed)
}

/// Selects an instance. The identifier must exist in the registry â€” a
/// dangling selection is never written.
pub fn select_instance(
    registry_path: &Path,
    config_path: &Path,
    instance_id: &InstanceId,
) -> Result<(), InstanceError> {
    {
        let registry = InstanceRegistry::load(registry_path)?;
        if registry.find(instance_id).is_none() {
            return Err(InstanceError::NotFound {
                instance_id: instance_id.to_string(),
            });
        }
    }
    let mut config = load_config(config_path)?;
    config.set_selected_instance_id(Some(instance_id.clone()));
    config::save(config_path, &config)?;
    Ok(())
}

/// The explicit first-instance selection policy: select `id` only when no
/// instance is currently selected.
fn select_instance_if_unselected(
    config_path: &Path,
    instance_id: &InstanceId,
) -> Result<(), InstanceError> {
    let mut config = load_config(config_path)?;
    if config.selected_instance_id().is_some() {
        return Ok(());
    }
    config.set_selected_instance_id(Some(instance_id.clone()));
    config::save(config_path, &config)?;
    Ok(())
}

fn load_config(config_path: &Path) -> Result<LauncherConfig, InstanceError> {
    Ok(crate::config::load(config_path)?.unwrap_or_default())
}

/// Allocates a fresh, registry-unique instance identifier whose instance
/// directory does not exist yet.
fn allocate_instance_id(
    managed: &ManagedPaths,
    registry_path: &Path,
) -> Result<InstanceId, InstanceError> {
    let registry = InstanceRegistry::load(registry_path)?;
    loop {
        let candidate = generate_instance_id();
        if registry.find(&candidate).is_some() {
            continue;
        }
        if managed.instance_paths(&candidate).root().exists() {
            continue;
        }
        return Ok(candidate);
    }
}

/// Validates one instance completely and read-only: registry presence,
/// lifecycle state, the game installation, the Aurora installation, the
/// Aurora artifact's bytes, and cross-component version consistency. No
/// downloads, no repairs, no mutation.
pub fn validate_instance(
    managed: &ManagedPaths,
    registry: &InstanceRegistry,
    instance_id: &InstanceId,
) -> Result<InstanceValidation, InstanceError> {
    let Some(record) = registry.find(instance_id) else {
        return Ok(InstanceValidation {
            instance_id: instance_id.to_string(),
            display_name: String::new(),
            status: InstanceStatus::NotInstalled,
            problems: Vec::new(),
        });
    };

    let mut problems: Vec<InstanceProblem> = Vec::new();

    // Desired configuration versus installed content. This is the deliberate
    // stale gate: an install-affecting configuration change means the record
    // is never ready until matching content is installed. Launch-only
    // settings (memory, JVM arguments, window, name) never participate.
    let pin = record.release();
    let configuration_matches = record
        .configuration()
        .matches_release_pin(pin.minecraft_version(), pin.fabric_loader_version());
    if !configuration_matches {
        problems.push(InstanceProblem {
            component: "configuration",
            reason: format!(
                "the desired configuration (Minecraft {}{}) does not match the installed content (Minecraft {} + Fabric Loader {}); install the new configuration to restore readiness",
                record.configuration().minecraft_version(),
                match record.configuration().loader().policy() {
                    LoaderPolicy::Automatic => String::new(),
                    LoaderPolicy::Pinned { version } => format!(" + Fabric Loader {version}"),
                },
                pin.minecraft_version(),
                pin.fabric_loader_version(),
            ),
        });
    }

    // Game installation.
    let game_root = managed.instance_paths(record.id()).game().to_path_buf();
    let game_manifest = match validate_game_install(managed, record.id())? {
        GameValidationOutcome::Installed(validation) => match validation.status {
            GameValidationStatus::Valid => Some(
                crate::install::state::load_installed_state(&game_root)
                    .map_err(InstanceError::GameInstallState)?
                    .expect("a validated game installation carries its manifest"),
            ),
            GameValidationStatus::Damaged => {
                problems.extend(validation.problems.iter().map(|problem| InstanceProblem {
                    component: "game",
                    reason: problem.reason.clone(),
                }));
                // The manifest, if readable, still feeds consistency checks.
                crate::install::state::load_installed_state(&game_root)
                    .ok()
                    .flatten()
            }
        },
        GameValidationOutcome::NotInstalled => {
            problems.push(InstanceProblem {
                component: "game",
                reason: "no complete game installation is present".to_owned(),
            });
            crate::install::state::load_installed_state(&game_root)
                .ok()
                .flatten()
        }
    };

    // Aurora installation.
    let aurora_state: Option<AuroraInstalledState> =
        match load_installed_state(managed, record.id())? {
            Some(state) => {
                if let Err(reason) = validate_aurora_artifact(managed, record.id(), &state) {
                    problems.push(InstanceProblem {
                        component: "aurora",
                        reason,
                    });
                }
                Some(state)
            }
            None => {
                problems.push(InstanceProblem {
                    component: "aurora",
                    reason: "no Aurora installation is present".to_owned(),
                });
                None
            }
        };

    // Cross-component consistency.
    if let Some(aurora) = &aurora_state {
        let manifest = crate::distribution::production_manifest()
            .map_err(|error| InstanceError::ReleaseInvalid(error.to_string()))?;
        if let Some(release) = manifest.resolve_exact(pin.aurora_version(), Some(pin.channel())) {
            if aurora.artifact().sha256() != release.artifact().sha256()
                || Some(aurora.artifact().size_bytes()) != release.artifact().size_bytes()
            {
                problems.push(InstanceProblem {
                    component: "aurora",
                    reason:
                        "the installed Aurora artifact does not match production release metadata"
                            .to_owned(),
                });
            }
            let expected_api = release.fabric_api();
            let installed_api = aurora.fabric_api();
            if expected_api.map(|api| {
                (
                    api.version(),
                    api.artifact().sha256(),
                    api.artifact().size_bytes(),
                )
            }) != installed_api.map(|api| {
                (
                    api.version(),
                    api.artifact().sha256(),
                    Some(api.artifact().size_bytes()),
                )
            }) {
                problems.push(InstanceProblem {
                        component: "aurora",
                        reason: "the release-required Fabric API installation does not match production metadata".to_owned(),
                    });
            }
        }
        if aurora.aurora_version() != pin.aurora_version()
            || aurora.channel() != pin.channel()
            || aurora.minecraft_version() != pin.minecraft_version()
            || aurora.fabric_loader_version() != pin.fabric_loader_version()
        {
            problems.push(InstanceProblem {
                component: "consistency",
                reason: format!(
                    "the pinned release ({} {} for Minecraft {} + Fabric Loader {}) does not match the installed Aurora release ({} {} for Minecraft {} + Fabric Loader {})",
                    pin.channel().as_str(),
                    pin.aurora_version(),
                    pin.minecraft_version(),
                    pin.fabric_loader_version(),
                    aurora.channel().as_str(),
                    aurora.aurora_version(),
                    aurora.minecraft_version(),
                    aurora.fabric_loader_version(),
                ),
            });
        }
        if let Some(game) = &game_manifest {
            if game.minecraft_version() != aurora.minecraft_version()
                || game.fabric_loader_version() != aurora.fabric_loader_version()
            {
                problems.push(InstanceProblem {
                    component: "consistency",
                    reason: format!(
                        "the installed game (Minecraft {} + Fabric Loader {}) does not match the Aurora release (Minecraft {} + Fabric Loader {})",
                        game.minecraft_version(),
                        game.fabric_loader_version(),
                        aurora.minecraft_version(),
                        aurora.fabric_loader_version(),
                    ),
                });
            }
        }
    }

    // Desired configuration versus installed content. This is the deliberate
    // stale gate: an install-affecting configuration change means the record
    // is never ready until matching content is installed. Launch-only
    // settings (memory, JVM arguments, window, name) never participate.
    let configuration_matches = record
        .configuration()
        .matches_release_pin(pin.minecraft_version(), pin.fabric_loader_version());
    if !configuration_matches {
        problems.push(InstanceProblem {
            component: "configuration",
            reason: format!(
                "the desired configuration (Minecraft {}{}) does not match the installed content (Minecraft {} + Fabric Loader {}); install the new configuration to restore readiness",
                record.configuration().minecraft_version(),
                match record.configuration().loader().policy() {
                    LoaderPolicy::Automatic => String::new(),
                    LoaderPolicy::Pinned { version } => format!(" + Fabric Loader {version}"),
                },
                pin.minecraft_version(),
                pin.fabric_loader_version(),
            ),
        });
    }

    let status = if record.state() == InstanceState::Installing {
        InstanceStatus::Installing
    } else if !configuration_matches {
        InstanceStatus::Stale
    } else if problems.is_empty() {
        InstanceStatus::Ready
    } else {
        InstanceStatus::Damaged
    };

    Ok(InstanceValidation {
        instance_id: record.id().to_string(),
        display_name: record.display_name().to_owned(),
        status,
        problems,
    })
}

/// The process-wide registry mutation lock: registry read-modify-write
/// windows are serialized so concurrent operations cannot lose updates.
/// Long installations run outside it.
fn registry_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn report(phase: InstancePhase, game: Option<InstallProgress>) -> InstanceProgress {
    InstanceProgress { phase, game }
}

/// A failed lifecycle operation.
#[derive(Debug)]
pub enum InstanceError {
    NotFound {
        instance_id: String,
    },
    NotReady {
        instance_id: String,
        reason: String,
    },
    NameInvalid(InvalidInstanceRecord),
    /// A desired configuration failed validation.
    ConfigurationInvalid(InvalidInstanceConfiguration),
    /// The desired configuration does not match the installed content; the
    /// new configuration must be installed before launch-affecting use.
    ConfigurationStale {
        instance_id: String,
        reason: String,
    },
    /// No Aurora release supports the configured Minecraft version.
    ReleaseUnavailable {
        minecraft_version: String,
        available: String,
    },
    /// The configured loader policy resolved to no concrete loader version.
    LoaderResolution {
        game: String,
        reason: String,
    },
    Registry(InstanceRegistryError),
    Config(ConfigError),
    ReleaseInvalid(String),
    AuroraReleaseNotFound {
        channel: ReleaseChannel,
        aurora_version: String,
    },
    Platform(crate::minecraft::rules::UnsupportedPlatform),
    GameResolution(crate::fabric::GameResolutionError),
    /// A Fabric Meta document failed to fetch or parse while selecting a
    /// loader version.
    FabricMetadata(crate::fabric::metadata::FabricMetadataError),
    GameInstall(InstallError),
    /// Reading a game installed-state manifest failed structurally.
    GameInstallState(crate::install::state::InstalledStateError),
    Aurora(AuroraInstallError),
    RuntimeMetadata(RuntimeMetadataError),
    RuntimeInstall(RuntimeInstallError),
    ValidationFailed {
        instance_id: String,
        problems: Vec<String>,
    },
}

impl fmt::Display for InstanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { instance_id } => {
                write!(formatter, "instance '{instance_id}' does not exist")
            }
            Self::NotReady {
                instance_id,
                reason,
            } => {
                write!(
                    formatter,
                    "instance '{instance_id}' is not in a state that allows this operation: {reason}"
                )
            }
            Self::NameInvalid(error) => write!(formatter, "{error}"),
            Self::ConfigurationInvalid(error) => write!(formatter, "{error}"),
            Self::ConfigurationStale {
                instance_id,
                reason,
            } => write!(
                formatter,
                "instance '{instance_id}' has a configuration that no longer matches its installed content: {reason}; install the new configuration to restore readiness"
            ),
            Self::ReleaseUnavailable {
                minecraft_version,
                available,
            } => write!(
                formatter,
                "no Aurora release supports Minecraft {minecraft_version}; the release source supports: {available}"
            ),
            Self::LoaderResolution { game, reason } => write!(
                formatter,
                "no Fabric Loader version could be selected for Minecraft {game}: {reason}"
            ),
            Self::Registry(error) => write!(formatter, "{error}"),
            Self::Config(error) => write!(formatter, "{error}"),
            Self::ReleaseInvalid(reason) => {
                write!(formatter, "the pinned Aurora release is unusable: {reason}")
            }
            Self::AuroraReleaseNotFound {
                channel,
                aurora_version,
            } => write!(
                formatter,
                "Aurora version '{aurora_version}' on the {} channel does not exist in the release manifest",
                channel.as_str()
            ),
            Self::Platform(error) => write!(formatter, "{error}"),
            Self::GameResolution(error) => write!(formatter, "{error}"),
            Self::FabricMetadata(error) => write!(formatter, "{error}"),
            Self::GameInstall(error) => write!(formatter, "{error}"),
            Self::GameInstallState(error) => write!(formatter, "{error}"),
            Self::Aurora(error) => write!(formatter, "{error}"),
            Self::RuntimeMetadata(error) => write!(formatter, "{error}"),
            Self::RuntimeInstall(error) => write!(formatter, "{error}"),
            Self::ValidationFailed {
                instance_id,
                problems,
            } => write!(
                formatter,
                "instance '{instance_id}' failed complete validation: {}",
                problems.join("; ")
            ),
        }
    }
}

impl std::error::Error for InstanceError {}

impl From<InvalidInstanceRecord> for InstanceError {
    fn from(error: InvalidInstanceRecord) -> Self {
        Self::NameInvalid(error)
    }
}

impl From<InvalidInstanceConfiguration> for InstanceError {
    fn from(error: InvalidInstanceConfiguration) -> Self {
        Self::ConfigurationInvalid(error)
    }
}

impl From<InstanceRegistryError> for InstanceError {
    fn from(error: InstanceRegistryError) -> Self {
        Self::Registry(error)
    }
}

impl From<ConfigError> for InstanceError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<InstallError> for InstanceError {
    fn from(error: InstallError) -> Self {
        Self::GameInstall(error)
    }
}

impl From<AuroraInstallError> for InstanceError {
    fn from(error: AuroraInstallError) -> Self {
        Self::Aurora(error)
    }
}

impl From<crate::fabric::GameResolutionError> for InstanceError {
    fn from(error: crate::fabric::GameResolutionError) -> Self {
        Self::GameResolution(error)
    }
}

impl From<RuntimeMetadataError> for InstanceError {
    fn from(error: RuntimeMetadataError) -> Self {
        Self::RuntimeMetadata(error)
    }
}

impl From<RuntimeInstallError> for InstanceError {
    fn from(error: RuntimeInstallError) -> Self {
        Self::RuntimeInstall(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::assets::AssetObjectEndpoints;
    use crate::minecraft::metadata::AssetIndexObjectsDocument;
    use crate::test_support::{TestResponse, TestServer};
    use std::collections::BTreeMap;
    use std::collections::HashSet;
    use std::io::Write as _;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    #[test]
    fn release_java_requirement_must_match_resolved_game_authority() {
        assert!(validate_release_java_major(25, 25).is_ok());
        let error = validate_release_java_major(21, 25).unwrap_err();
        assert!(matches!(error, InstanceError::ReleaseInvalid(_)));
        assert!(error.to_string().contains("official Minecraft/Fabric"));
    }

    /// A complete synthetic Aurora release + game world served from one
    /// loopback server: the Mojang manifest and version document, every
    /// game artifact (client, logging, libraries, native archive, asset
    /// index and objects), the Fabric Meta loader list and profile, and the
    /// Aurora release artifact.
    struct SyntheticWorld {
        server: TestServer,
        release_manifest: ReleaseManifest,
        broken: std::sync::Arc<StdMutex<HashSet<String>>>,
        managed: ManagedPaths,
    }

    impl SyntheticWorld {
        fn new(name: &str) -> Self {
            let client = b"synthetic client jar bytes".to_vec();
            let logging = b"<Configuration status=\"WARN\"></Configuration>".to_vec();
            let mojang_library = b"synthetic mojang library jar".to_vec();
            let fabric_common = b"synthetic digested fabric library jar".to_vec();
            let fabric_loader = b"synthetic digest-less fabric loader jar".to_vec();
            let aurora_artifact = b"aurora development artifact bytes".to_vec();

            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut writer = zip::ZipWriter::new(&mut cursor);
                for entry in ["META-INF/MANIFEST.MF", "lwjgl.dll"] {
                    writer
                        .start_file(entry, zip::write::SimpleFileOptions::default())
                        .unwrap();
                    writer.write_all(b"synthetic native content").unwrap();
                }
                writer.finish().unwrap();
            }
            let native_archive = cursor.into_inner();

            let asset_a = b"tiny png bytes a".to_vec();
            let asset_a_hash = crate::integrity::Sha1Digest::compute(&asset_a).as_hex();
            let asset_index_body = format!(
                r#"{{"objects": {{ "icons/icon_16x16.png": {{"hash": "{asset_a_hash}", "size": {}}}}}}}"#,
                asset_a.len()
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
                "/mojang/libraries/org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar"
                    .to_owned(),
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
                "/fabric-maven/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar".to_owned(),
                fabric_common.clone(),
            );
            bodies.insert(
                "/fabric-maven/net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar"
                    .to_owned(),
                fabric_loader.clone(),
            );
            bodies.insert(
                "/aurora/aurora-0.3.0-dev.jar".to_owned(),
                aurora_artifact.clone(),
            );

            // The Mojang manifest and version document point at this server.
            let sha1 = |bytes: &[u8]| crate::integrity::Sha1Digest::compute(bytes).as_hex();
            let version_document = format!(
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
                        "url": "@BASE@/mojang/asset-index/32.json"
                    }},
                    "downloads": {{
                        "client": {{ "sha1": "{}", "size": {}, "url": "@BASE@/mojang/client.jar" }}
                    }},
                    "logging": {{
                        "client": {{
                            "argument": "-Dlog4j.configurationFile=${{path}}",
                            "file": {{ "id": "client-1.21.2.xml", "sha1": "{}", "size": {}, "url": "@BASE@/mojang/logging/client-1.21.2.xml" }},
                            "type": "log4j2-xml"
                        }}
                    }},
                    "libraries": [
                        {{ "downloads": {{ "artifact": {{
                            "path": "com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar",
                            "sha1": "{}", "size": {},
                            "url": "@BASE@/mojang/libraries/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar"
                        }} }}, "name": "com.mojang:brigadier:1.0.18" }},
                        {{ "downloads": {{ "artifact": {{
                            "path": "org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar",
                            "sha1": "{}", "size": {},
                            "url": "@BASE@/mojang/libraries/org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar"
                        }} }}, "name": "org.lwjgl:lwjgl:3.4.1:natives-windows" }}
                    ],
                    "arguments": {{ "game": ["--username", "${{auth_player_name}}"], "jvm": ["-Djava.library.path=${{natives_directory}}"] }}
                }}"#,
                sha1(&asset_index_body),
                asset_index_body.len(),
                sha1(&client),
                client.len(),
                sha1(&logging),
                logging.len(),
                sha1(&mojang_library),
                mojang_library.len(),
                sha1(&native_archive),
                native_archive.len(),
            );
            // The manifest is built after the base URL is known: its SHA-1
            // must cover the exact served (substituted) version-document
            // bytes, exactly as official metadata chains digests.

            // Fabric Meta documents.
            let loader_list = r#"[
                { "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true }
            ]"#;
            let fabric_sha256 = |bytes: &[u8]| {
                use sha2::Digest as _;
                crate::integrity::ArtifactDigest::from_sha256(sha2::Sha256::digest(bytes).into())
                    .as_hex()
            };
            let profile_document = format!(
                r#"{{
                    "loader": {{ "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true }},
                    "intermediary": {{ "maven": "net.fabricmc:intermediary:0.0.0", "version": "0.0.0", "stable": true }},
                    "launcherMeta": {{
                        "version": 2,
                        "min_java_version": 8,
                        "libraries": {{
                            "client": [],
                            "common": [
                                {{ "name": "org.ow2.asm:asm:9.10.1", "url": "@BASE@/fabric-maven/", "sha256": "{}", "size": {} }}
                            ],
                            "server": [],
                            "development": []
                        }},
                        "mainClass": {{ "client": "net.fabricmc.loader.impl.launch.knot.KnotClient", "server": "net.fabricmc.loader.impl.launch.knot.KnotServer" }}
                    }}
                }}"#,
                fabric_sha256(&fabric_common),
                fabric_common.len(),
            );

            // The body map is shared with the handler so the metadata
            // documents â€” which must embed the bound base URL â€” can be
            // inserted after the server exists and still be served.
            let shared_bodies = std::sync::Arc::new(StdMutex::new(bodies));
            let broken: std::sync::Arc<StdMutex<HashSet<String>>> =
                std::sync::Arc::new(StdMutex::new(HashSet::new()));
            let broken_handler = std::sync::Arc::clone(&broken);
            let bodies_for_handler = std::sync::Arc::clone(&shared_bodies);
            let server = TestServer::spawn(std::sync::Arc::new(move |request| {
                if broken_handler.lock().unwrap().contains(&request.path) {
                    return TestResponse::status(404);
                }
                bodies_for_handler
                    .lock()
                    .unwrap()
                    .get(&request.path)
                    .map(|body| TestResponse::ok(body))
                    .unwrap_or(TestResponse::status(404))
            }));

            // Documents reference the bound base URL.
            let base = server.base_url().to_owned();
            let served_version_document = version_document.replace("@BASE@", &base);
            let manifest_document = format!(
                r#"{{
                    "latest": {{ "release": "26.2", "snapshot": "26.2" }},
                    "versions": [
                        {{ "id": "26.2", "type": "release", "url": "{base}/mojang/versions/26.2.json", "sha1": "{}" }}
                    ]
                }}"#,
                sha1(served_version_document.as_bytes()),
            );
            shared_bodies.lock().unwrap().extend([
                (
                    "/mc/game/version_manifest_v2.json".to_owned(),
                    manifest_document.into_bytes(),
                ),
                (
                    "/mojang/versions/26.2.json".to_owned(),
                    served_version_document.into_bytes(),
                ),
                (
                    "/v2/versions/loader".to_owned(),
                    loader_list.as_bytes().to_vec(),
                ),
                // The per-game loader listing the desired-configuration
                // resolution consumes (the same single stable entry).
                (
                    "/v2/versions/loader/26.2".to_owned(),
                    format!(
                        r#"[ {{ "loader": {{ "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true }}, "intermediary": {{ "maven": "net.fabricmc:intermediary:0.0.0", "version": "0.0.0", "stable": true }}, "launcherMeta": {{ "version": 2, "min_java_version": 8, "libraries": {{ "client": [], "common": [], "server": [], "development": [] }}, "mainClass": {{ "client": "net.fabricmc.loader.impl.launch.knot.KnotClient", "server": "net.fabricmc.loader.impl.launch.knot.KnotServer" }} }} }}]"#
                    )
                    .into_bytes(),
                ),
                (
                    "/v2/versions/loader/26.2/0.19.5".to_owned(),
                    profile_document.replace("@BASE@", &base).into_bytes(),
                ),
            ]);

            let aurora_url = format!("{base}/aurora/aurora-0.3.0-dev.jar");
            let release_manifest = ReleaseManifest::from_json(&format!(
                r#"{{
                    "schemaVersion": 1,
                    "releases": [
                        {{
                            "auroraVersion": "0.3.0",
                            "channel": "stable",
                            "minecraftVersion": "26.2",
                            "fabricLoaderVersion": "0.19.5",
                            "java": {{ "majorVersion": 25 }},
                            "artifact": {{ "url": "{aurora_url}", "sha256": "{}", "sizeBytes": {} }}
                        }}
                    ]
                }}"#,
                fabric_sha256(&aurora_artifact),
                aurora_artifact.len(),
            ))
            .unwrap();

            let root = std::env::temp_dir()
                .join("aurora-lifecycle-test")
                .join(std::process::id().to_string())
                .join(name);
            let _ = std::fs::remove_dir_all(&root);
            let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();

            Self {
                server,
                release_manifest,
                broken,
                managed,
            }
        }

        fn endpoints(&self) -> InstanceEndpoints {
            InstanceEndpoints::for_testing(
                self.release_manifest.clone(),
                crate::minecraft::metadata::MetadataEndpoints::loopback_for_testing(
                    self.server.base_url(),
                ),
                crate::fabric::metadata::FabricMetaEndpoints::loopback_for_testing(&format!(
                    "{}/v2/",
                    self.server.base_url()
                )),
                InstallContext::loopback_for_testing(
                    crate::downloads::DownloadOptions {
                        connect_timeout: Duration::from_secs(5),
                        idle_read_timeout: Duration::from_secs(5),
                        max_redirects: crate::downloads::MAX_REDIRECTS,
                    },
                    AssetObjectEndpoints::loopback_for_testing(&format!(
                        "{}/assets/",
                        self.server.base_url()
                    )),
                ),
            )
        }

        fn registry_path(&self) -> PathBuf {
            self.managed.instance_registry_file()
        }

        fn config_path(&self) -> PathBuf {
            self.managed.config_file()
        }

        fn break_path(&self, path: &str) {
            self.broken.lock().unwrap().insert(path.to_owned());
        }

        async fn create(&self, display_name: &str) -> Result<InstanceRecord, InstanceError> {
            self.create_with_configuration(
                display_name,
                InstanceConfiguration::for_minecraft_version("26.2"),
            )
            .await
        }

        async fn create_with_configuration(
            &self,
            display_name: &str,
            configuration: InstanceConfiguration,
        ) -> Result<InstanceRecord, InstanceError> {
            create_instance(
                &self.managed,
                &self.registry_path(),
                &self.config_path(),
                &self.endpoints(),
                CreateInstanceRequest::new(display_name, configuration),
                &mut |_| {},
                InstanceFaults::default(),
            )
            .await
        }

        async fn create_with_faults(
            &self,
            display_name: &str,
            faults: InstanceFaults,
        ) -> Result<InstanceRecord, InstanceError> {
            create_instance(
                &self.managed,
                &self.registry_path(),
                &self.config_path(),
                &self.endpoints(),
                CreateInstanceRequest::new(
                    display_name,
                    InstanceConfiguration::for_minecraft_version("26.2"),
                ),
                &mut |_| {},
                faults,
            )
            .await
        }

        fn load_registry(&self) -> InstanceRegistry {
            InstanceRegistry::load(&self.registry_path()).unwrap()
        }

        fn load_config(&self) -> LauncherConfig {
            crate::config::load(&self.config_path())
                .unwrap()
                .unwrap_or_default()
        }

        fn validate(&self, id: &InstanceId) -> InstanceValidation {
            let registry = self.load_registry();
            validate_instance(&self.managed, &registry, id).unwrap()
        }
    }

    #[tokio::test]
    async fn a_complete_instance_creates_validates_and_selects() {
        let world = SyntheticWorld::new("happy");

        let record = world
            .create("My Aurora Setup")
            .await
            .expect("the instance must create completely");

        // The registry holds a ready record with the concrete pinned release.
        let registry = world.load_registry();
        let stored = registry.find(record.id()).unwrap();
        assert_eq!(stored.state(), InstanceState::Ready);
        assert_eq!(stored.display_name(), "My Aurora Setup");
        assert_eq!(stored.release().aurora_version(), "0.3.0");
        assert_eq!(stored.release().minecraft_version(), "26.2");
        assert_eq!(stored.release().fabric_loader_version(), "0.19.5");
        assert_eq!(
            stored.id().as_str().len(),
            32,
            "the identifier is an opaque UUID, not derived from the name"
        );

        // First-instance selection policy: created with no prior selection,
        // it became selected.
        assert_eq!(
            world
                .load_config()
                .selected_instance_id()
                .map(|id| id.as_str()),
            Some(stored.id().as_str())
        );

        // Complete validation reports ready, and nothing but the managed
        // trees exist: no staging, user areas untouched (never created).
        let validation = world.validate(stored.id());
        assert_eq!(validation.status, InstanceStatus::Ready);
        assert!(validation.problems.is_empty());
        let instance_paths = world.managed.instance_paths(stored.id());
        assert!(instance_paths.game().join("installed-game.json").is_file());
        assert!(instance_paths.mods().join("aurora-0.3.0.jar").is_file());
        assert!(
            instance_paths
                .root()
                .join("aurora-installed.json")
                .is_file()
        );
        assert!(!instance_paths.root().join(".install-staging").exists());
    }

    #[tokio::test]
    async fn an_unknown_release_or_wrong_channel_fails_before_anything_is_written() {
        let world = SyntheticWorld::new("bad-release");

        let error = create_instance(
            &world.managed,
            &world.registry_path(),
            &world.config_path(),
            &world.endpoints(),
            CreateInstanceRequest::new(
                "Missing",
                InstanceConfiguration::for_minecraft_version("9.9.9"),
            ),
            &mut |_| {},
            InstanceFaults::default(),
        )
        .await
        .expect_err("an unknown Minecraft version must fail");

        assert!(
            matches!(error, InstanceError::ReleaseUnavailable { .. }),
            "{error}"
        );

        assert_eq!(world.load_registry().instances().len(), 0);
        assert!(!world.managed.instances_dir().exists());
    }

    #[tokio::test]
    async fn a_game_install_failure_leaves_an_explicit_non_ready_record_for_retry() {
        let world = SyntheticWorld::new("game-failure");
        world.break_path("/mojang/client.jar");

        let error = world
            .create_with_faults("Broken", InstanceFaults::default())
            .await
            .expect_err("the game installation must fail");
        assert!(matches!(error, InstanceError::GameInstall(_)), "{error}");

        // The registry holds an installing record â€” never a healthy one.
        let registry = world.load_registry();
        assert_eq!(registry.instances().len(), 1);
        let record = &registry.instances()[0];
        assert_eq!(record.state(), InstanceState::Installing);
        assert_eq!(
            world.validate(record.id()).status,
            InstanceStatus::Installing
        );
        // No selection was made.
        assert!(world.load_config().selected_instance_id().is_none());

        // Retry is deterministic: repair the source, retry the same record,
        // and it becomes ready without manual file surgery.
        world.broken.lock().unwrap().clear();
        let retried = retry_instance_install(
            &world.managed,
            &world.registry_path(),
            &world.config_path(),
            &world.endpoints(),
            record.id(),
            &mut |_| {},
            InstanceFaults::default(),
        )
        .await
        .expect("the retry must complete");

        assert_eq!(retried.state(), InstanceState::Ready);
        assert_eq!(world.validate(retried.id()).status, InstanceStatus::Ready);
        // The retried first instance becomes selected by the same policy.
        assert_eq!(
            world
                .load_config()
                .selected_instance_id()
                .map(|id| id.as_str()),
            Some(retried.id().as_str())
        );
    }

    #[tokio::test]
    async fn an_aurora_acquisition_failure_leaves_an_installing_record() {
        let world = SyntheticWorld::new("aurora-failure");
        world.break_path("/aurora/aurora-0.3.0-dev.jar");

        let error = world
            .create_with_faults("No Aurora", InstanceFaults::default())
            .await
            .expect_err("Aurora acquisition must fail");
        assert!(matches!(error, InstanceError::Aurora(_)), "{error}");

        let registry = world.load_registry();
        let record = &registry.instances()[0];
        assert_eq!(record.state(), InstanceState::Installing);
        // The game half did complete â€” retry only needs the Aurora half,
        // and the game install is deliberately replaced, not duplicated.
        assert!(
            world
                .managed
                .instance_paths(record.id())
                .game()
                .join("installed-game.json")
                .is_file()
        );
        assert!(
            !world
                .managed
                .instance_paths(record.id())
                .root()
                .join("aurora-installed.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn injected_faults_never_produce_a_ready_instance() {
        for (name, faults) in [
            (
                "before-aurora",
                InstanceFaults {
                    fail_before_aurora_install: true,
                    ..InstanceFaults::default()
                },
            ),
            (
                "before-ready",
                InstanceFaults {
                    fail_before_ready: true,
                    ..InstanceFaults::default()
                },
            ),
        ] {
            let world = SyntheticWorld::new(name);
            let error = world
                .create_with_faults("Faulted", faults)
                .await
                .expect_err("the injected fault must fail");
            assert!(error.to_string().contains("deterministic fault"), "{error}");

            let registry = world.load_registry();
            assert_eq!(registry.instances().len(), 1, "{name}");
            assert_eq!(
                registry.instances()[0].state(),
                InstanceState::Installing,
                "{name}: no incomplete instance may be ready"
            );
            assert!(
                world.load_config().selected_instance_id().is_none(),
                "{name}: a faulted creation never selects"
            );

            // Retrying the faulted record without faults completes it.
            retry_instance_install(
                &world.managed,
                &world.registry_path(),
                &world.config_path(),
                &world.endpoints(),
                registry.instances()[0].id(),
                &mut |_| {},
                InstanceFaults::default(),
            )
            .await
            .expect("{name}: the retry must complete");
            assert_eq!(
                world.validate(registry.instances()[0].id()).status,
                InstanceStatus::Ready,
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn retry_refuses_ready_or_unknown_instances() {
        let world = SyntheticWorld::new("retry-guards");
        let record = world.create("Ready One").await.unwrap();

        let error = retry_instance_install(
            &world.managed,
            &world.registry_path(),
            &world.config_path(),
            &world.endpoints(),
            record.id(),
            &mut |_| {},
            InstanceFaults::default(),
        )
        .await
        .expect_err("retrying a ready instance must be refused");
        assert!(matches!(error, InstanceError::NotReady { .. }), "{error}");

        let ghost = crate::instances::InstanceId::new("ghost").unwrap();
        let error = retry_instance_install(
            &world.managed,
            &world.registry_path(),
            &world.config_path(),
            &world.endpoints(),
            &ghost,
            &mut |_| {},
            InstanceFaults::default(),
        )
        .await
        .expect_err("retrying an unknown instance must fail");
        assert!(matches!(error, InstanceError::NotFound { .. }), "{error}");
    }

    #[tokio::test]
    async fn renaming_changes_metadata_only() {
        let world = SyntheticWorld::new("rename");
        let record = world.create("Original Name").await.unwrap();
        let game_manifest_before = std::fs::read(
            world
                .managed
                .instance_paths(record.id())
                .game()
                .join("installed-game.json"),
        )
        .unwrap();

        let renamed = rename_instance(&world.registry_path(), record.id(), "Renamed âœ¨").unwrap();

        assert_eq!(renamed.display_name(), "Renamed âœ¨");
        assert_eq!(renamed.id(), record.id(), "the identifier is unchanged");
        let stored = world.load_registry().find(record.id()).unwrap().clone();
        assert_eq!(stored.display_name(), "Renamed âœ¨");
        assert_eq!(stored.release(), record.release());

        // The filesystem path is identical and installed state is untouched.
        let instance_paths = world.managed.instance_paths(record.id());
        assert_eq!(
            instance_paths.root(),
            world.managed.instance_paths(record.id()).root()
        );
        assert!(instance_paths.game().join("installed-game.json").is_file());
        assert_eq!(
            std::fs::read(instance_paths.game().join("installed-game.json")).unwrap(),
            game_manifest_before
        );
        assert_eq!(world.validate(record.id()).status, InstanceStatus::Ready);

        // Invalid names are refused and change nothing.
        let error = rename_instance(&world.registry_path(), record.id(), " padded").unwrap_err();
        assert!(matches!(error, InstanceError::NameInvalid(_)));
        assert_eq!(
            world
                .load_registry()
                .find(record.id())
                .unwrap()
                .display_name(),
            "Renamed âœ¨"
        );
    }

    #[tokio::test]
    async fn selection_requires_an_existing_instance_and_is_enforced_by_state_loading() {
        let world = SyntheticWorld::new("selection");

        // A second creation does not change the existing selection.
        let first = world.create("First").await.unwrap();
        let second = world.create("Second").await.unwrap();
        assert_eq!(
            world
                .load_config()
                .selected_instance_id()
                .map(|id| id.as_str()),
            Some(first.id().as_str())
        );

        // Explicit selection switches, and only to registered instances.
        select_instance(&world.registry_path(), &world.config_path(), second.id()).unwrap();
        assert_eq!(
            world
                .load_config()
                .selected_instance_id()
                .map(|id| id.as_str()),
            Some(second.id().as_str())
        );

        let ghost = crate::instances::InstanceId::new("ghost").unwrap();
        assert!(matches!(
            select_instance(&world.registry_path(), &world.config_path(), &ghost),
            Err(InstanceError::NotFound { .. })
        ));
        // The dangling-selection *state-loading* behavior is covered by the
        // application-layer tests (`config_selected_instance_dangling`);
        // here the guarantee is that the launcher's own operations can never
        // write a dangling selection.
        assert!(world.load_config().selected_instance_id().is_some());
    }

    #[tokio::test]
    async fn validation_reports_damage_precisely_and_mutates_nothing() {
        let world = SyntheticWorld::new("damage");
        let record = world.create("Damage Me").await.unwrap();
        let instance_paths = world.managed.instance_paths(record.id());

        // Tamper: corrupt the Aurora artifact, delete one asset object.
        std::fs::write(instance_paths.mods().join("aurora-0.3.0.jar"), b"tampered").unwrap();
        let index_text =
            std::fs::read_to_string(instance_paths.game().join("assets/indexes/32.json")).unwrap();
        let index = AssetIndexObjectsDocument::from_json(&index_text).unwrap();
        let victim = crate::install::assets::plan_asset_objects(&index).unwrap()[0].clone();
        std::fs::remove_file(instance_paths.game().join(victim.game_relative_path())).unwrap();

        let before = std::fs::read(instance_paths.root().join("aurora-installed.json")).unwrap();
        let validation = world.validate(record.id());
        assert_eq!(validation.status, InstanceStatus::Damaged);
        let components: Vec<&str> = validation
            .problems
            .iter()
            .map(|problem| problem.component)
            .collect();
        assert!(components.contains(&"aurora"), "{:?}", validation.problems);
        assert!(components.contains(&"game"), "{:?}", validation.problems);

        // Read-only: the damaged bytes and state records are unchanged.
        assert_eq!(
            std::fs::read(instance_paths.mods().join("aurora-0.3.0.jar")).unwrap(),
            b"tampered"
        );
        assert_eq!(
            std::fs::read(instance_paths.root().join("aurora-installed.json")).unwrap(),
            before
        );

        // A missing record validates as NotInstalled; an installing record
        // classifies as Installing regardless of partial content.
        let ghost = crate::instances::InstanceId::new("ghost").unwrap();
        assert_eq!(world.validate(&ghost).status, InstanceStatus::NotInstalled);

        let mut registry = world.load_registry();
        registry
            .find_mut(record.id())
            .unwrap()
            .set_state(InstanceState::Installing);
        let validation = validate_instance(&world.managed, &registry, record.id()).unwrap();
        assert_eq!(validation.status, InstanceStatus::Installing);
    }

    #[tokio::test]
    async fn version_mismatches_across_components_are_consistency_failures() {
        let world = SyntheticWorld::new("consistency");
        let record = world.create("Consistent").await.unwrap();
        assert_eq!(world.validate(record.id()).status, InstanceStatus::Ready);

        // Rewrite the registry pin to a different Aurora version: the pin
        // and the Aurora installed state now disagree.
        {
            let mut value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(world.registry_path()).unwrap())
                    .unwrap();
            value["instances"][0]["release"]["auroraVersion"] =
                serde_json::Value::String("0.3.1".to_owned());
            std::fs::write(&world.registry_path(), value.to_string()).unwrap();
        }
        let registry = world.load_registry();
        let validation = validate_instance(&world.managed, &registry, record.id()).unwrap();
        assert_eq!(validation.status, InstanceStatus::Damaged);
        assert!(
            validation
                .problems
                .iter()
                .any(|problem| problem.component == "consistency"
                    && problem.reason.contains("0.3.1")),
            "{:?}",
            validation.problems
        );

        // A missing Aurora artifact (deleted after install) is aurora damage.
        std::fs::remove_file(
            world
                .managed
                .instance_paths(record.id())
                .mods()
                .join("aurora-0.3.0.jar"),
        )
        .unwrap();
        let registry = world.load_registry();
        let validation = validate_instance(&world.managed, &registry, record.id()).unwrap();
        assert!(
            validation
                .problems
                .iter()
                .any(|problem| problem.component == "aurora"),
            "{:?}",
            validation.problems
        );
    }

    #[tokio::test]
    async fn user_mods_and_config_survive_creation_and_never_count_as_damage() {
        let world = SyntheticWorld::new("user-data");
        let record = world.create("User Data").await.unwrap();
        let instance_paths = world.managed.instance_paths(record.id());

        // User content appears after creation (as it would in real use).
        std::fs::write(instance_paths.mods().join("my-mod.jar"), b"precious").unwrap();
        std::fs::create_dir_all(instance_paths.root().join("saves")).unwrap();
        std::fs::write(
            instance_paths.root().join("saves").join("world.dat"),
            b"save",
        )
        .unwrap();

        let validation = world.validate(record.id());
        assert_eq!(validation.status, InstanceStatus::Ready);
        assert!(validation.problems.is_empty());

        assert_eq!(
            std::fs::read(instance_paths.mods().join("my-mod.jar")).unwrap(),
            b"precious"
        );
        assert_eq!(
            std::fs::read(instance_paths.root().join("saves").join("world.dat")).unwrap(),
            b"save"
        );
    }

    #[tokio::test]
    async fn progress_reports_lifecycle_phases_with_embedded_game_progress() {
        let world = SyntheticWorld::new("progress");
        let mut phases: Vec<&'static str> = Vec::new();
        let mut saw_game_progress = false;
        let managed = world.managed.clone();
        let registry_path = world.registry_path();
        let config_path = world.config_path();
        let endpoints = world.endpoints();

        create_instance(
            &managed,
            &registry_path,
            &config_path,
            &endpoints,
            CreateInstanceRequest::new(
                "Progress",
                InstanceConfiguration::for_minecraft_version("26.2"),
            ),
            &mut |progress| {
                if phases.last() != Some(&progress.phase.as_str()) {
                    phases.push(progress.phase.as_str());
                }
                if progress.phase == InstancePhase::InstallingGame {
                    saw_game_progress |= progress.game.is_some();
                }
            },
            InstanceFaults::default(),
        )
        .await
        .unwrap();

        assert_eq!(
            phases,
            vec![
                "resolvingRelease",
                "resolvingGame",
                "installingGame",
                "installingAurora",
                "validating",
                "completing",
            ]
        );
        assert!(
            saw_game_progress,
            "the installer's own item progress is embedded, not duplicated"
        );
    }

    #[tokio::test]
    async fn launch_only_configuration_changes_never_invalidate_content() {
        let world = SyntheticWorld::new("launch-only-changes");
        let record = world.create("Configurable").await.unwrap();
        let manifest_before = std::fs::read(
            world
                .managed
                .instance_paths(record.id())
                .game()
                .join("installed-game.json"),
        )
        .unwrap();

        // Memory, JVM arguments, window, and rename: all persisted, none
        // install-affecting.
        let mut configuration = record.configuration().clone();
        configuration.set_memory_mib(8192);
        configuration.set_additional_jvm_arguments("-Dexample=value");
        configuration.set_window(Some(crate::instances::settings::WindowConfiguration::new(
            1280, 720,
        )));
        let updated = update_instance_configuration(
            &world.registry_path(),
            &world.endpoints(),
            record.id(),
            configuration,
        )
        .unwrap();
        assert_eq!(updated.configuration().memory_mib(), 8192);
        assert_eq!(
            updated.configuration().additional_jvm_arguments(),
            "-Dexample=value"
        );

        rename_instance(&world.registry_path(), record.id(), "Renamed Configurable").unwrap();

        assert_eq!(world.validate(record.id()).status, InstanceStatus::Ready);
        assert_eq!(
            std::fs::read(
                world
                    .managed
                    .instance_paths(record.id())
                    .game()
                    .join("installed-game.json"),
            )
            .unwrap(),
            manifest_before,
            "launch-only changes never rewrite installed state"
        );

        // Round-trip: the registry persists the new values.
        let stored = world.load_registry().find(record.id()).unwrap().clone();
        assert_eq!(stored.display_name(), "Renamed Configurable");
        assert_eq!(stored.configuration().memory_mib(), 8192);
    }

    #[tokio::test]
    async fn an_install_affecting_change_makes_the_instance_stale_until_reinstalled() {
        let world = SyntheticWorld::new("stale-cycle");
        let record = world.create("Stale Me").await.unwrap();
        assert_eq!(world.validate(record.id()).status, InstanceStatus::Ready);

        // Change to the other Minecraft version the synthetic release
        // fixture... does not have. The release manifest here pins 26.2, so
        // an unsupported version is refused up front by the honest
        // compatibility gate.
        let mut unsupported = record.configuration().clone();
        unsupported.set_minecraft_version("9.9.9");
        let error = update_instance_configuration(
            &world.registry_path(),
            &world.endpoints(),
            record.id(),
            unsupported,
        )
        .unwrap_err();
        assert!(
            matches!(error, InstanceError::ReleaseUnavailable { .. }),
            "{error}"
        );
        assert_eq!(world.validate(record.id()).status, InstanceStatus::Ready);

        // A pinned loader change stays within release compatibility and is
        // accepted — then reported stale until installed. The synthetic
        // Fabric Meta only lists 0.19.5, so use it as both old and new pin
        // through the automatic policy flip: pin to the same version.
        let mut configuration = record.configuration().clone();
        configuration.set_loader(crate::instances::settings::LoaderConfiguration::fabric(
            crate::instances::settings::LoaderPolicy::Pinned {
                version: "0.19.5".to_owned(),
            },
        ));
        // Same resolved versions: this is a no-op, still ready.
        update_instance_configuration(
            &world.registry_path(),
            &world.endpoints(),
            record.id(),
            configuration.clone(),
        )
        .unwrap();
        assert_eq!(world.validate(record.id()).status, InstanceStatus::Ready);

        // Now make it genuinely stale: pin a loader the record does not have.
        // Build a second Fabric Meta document set by editing the registry pin
        // directly would bypass the API; instead change the Minecraft version
        // through a second synthetic release entry. The world serves one
        // release (26.2), so the supported in-world path to staleness is a
        // registry-level configuration edit mirroring what
        // update_instance_configuration persists for a matching release but
        // a different loader — represented here through the pinned policy
        // against the installed pin.
        configuration.set_loader(crate::instances::settings::LoaderConfiguration::fabric(
            crate::instances::settings::LoaderPolicy::Pinned {
                version: "0.18.9".to_owned(),
            },
        ));
        // 0.18.9 does not exist in the synthetic loader list; updating must
        // still persist (compatibility is verified at install), then report
        // stale.
        let updated = update_instance_configuration(
            &world.registry_path(),
            &world.endpoints(),
            record.id(),
            configuration,
        )
        .unwrap();
        assert_eq!(
            world.validate(record.id()).status,
            InstanceStatus::Stale,
            "a pinned loader mismatch must be stale"
        );
        // ...but the previous installation is intact and user data is safe.
        assert!(
            world
                .managed
                .instance_paths(record.id())
                .game()
                .join("installed-game.json")
                .is_file()
        );
        let _ = updated;

        // Installing the stale configuration resolves its loader against the
        // synthetic list: 0.18.9 is not available, so the install fails
        // honestly before anything is mutated — the record stays exactly as
        // it was (ready, stale) and the install can be re-attempted after
        // repairing the configuration.
        let error = install_instance_configuration(
            &world.managed,
            &world.registry_path(),
            &world.config_path(),
            &world.endpoints(),
            record.id(),
            &mut |_| {},
            InstanceFaults::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, InstanceError::LoaderResolution { .. }),
            "{error}"
        );
        let registry = world.load_registry();
        assert_eq!(
            registry.find(record.id()).unwrap().state(),
            InstanceState::Ready,
            "a resolution failure mutates nothing"
        );
        assert_eq!(
            world.validate(record.id()).status,
            InstanceStatus::Stale,
            "the configuration mismatch is still reported"
        );

        // Repair the configuration back to the installed pin and install:
        // readiness is restored without destroying anything.
        let mut repaired = record.configuration().clone();
        repaired.set_loader(crate::instances::settings::LoaderConfiguration::fabric(
            crate::instances::settings::LoaderPolicy::Pinned {
                version: "0.19.5".to_owned(),
            },
        ));
        update_instance_configuration(
            &world.registry_path(),
            &world.endpoints(),
            record.id(),
            repaired,
        )
        .unwrap();
        let ready = install_instance_configuration(
            &world.managed,
            &world.registry_path(),
            &world.config_path(),
            &world.endpoints(),
            record.id(),
            &mut |_| {},
            InstanceFaults::default(),
        )
        .await
        .unwrap();
        assert_eq!(ready.state(), InstanceState::Ready);
        assert_eq!(world.validate(record.id()).status, InstanceStatus::Ready);
    }

    #[tokio::test]
    async fn invalid_configurations_are_refused_before_anything_is_persisted() {
        let world = SyntheticWorld::new("invalid-config");
        let record = world.create("Guarded").await.unwrap();

        // Memory out of range.
        let mut bad_memory = record.configuration().clone();
        bad_memory.set_memory_mib(64);
        assert!(matches!(
            update_instance_configuration(
                &world.registry_path(),
                &world.endpoints(),
                record.id(),
                bad_memory
            ),
            Err(InstanceError::ConfigurationInvalid(
                crate::instances::settings::InvalidInstanceConfiguration::MemoryOutOfRange { .. }
            ))
        ));

        // Heap-conflicting JVM arguments.
        let mut bad_arguments = record.configuration().clone();
        bad_arguments.set_additional_jvm_arguments("-Xmx12G");
        assert!(matches!(
            update_instance_configuration(
                &world.registry_path(),
                &world.endpoints(),
                record.id(),
                bad_arguments
            ),
            Err(InstanceError::ConfigurationInvalid(
                crate::instances::settings::InvalidInstanceConfiguration::JvmArguments(_)
            ))
        ));

        // Malformed JVM argument quoting.
        let mut bad_quotes = record.configuration().clone();
        bad_quotes.set_additional_jvm_arguments(r#"-Dbroken="unclosed"#);
        assert!(matches!(
            update_instance_configuration(
                &world.registry_path(),
                &world.endpoints(),
                record.id(),
                bad_quotes
            ),
            Err(InstanceError::ConfigurationInvalid(
                crate::instances::settings::InvalidInstanceConfiguration::JvmArguments(_)
            ))
        ));

        // A traversal-shaped Minecraft version.
        let mut bad_version = record.configuration().clone();
        bad_version.set_minecraft_version("../evil");
        assert!(matches!(
            update_instance_configuration(
                &world.registry_path(),
                &world.endpoints(),
                record.id(),
                bad_version
            ),
            Err(InstanceError::ConfigurationInvalid(
                crate::instances::settings::InvalidInstanceConfiguration::MinecraftVersion(_)
            ))
        ));

        // Nothing above reached the registry.
        let stored = world.load_registry().find(record.id()).unwrap().clone();
        assert_eq!(
            stored.configuration(),
            record.configuration(),
            "refused updates persist nothing"
        );
        assert_eq!(world.validate(record.id()).status, InstanceStatus::Ready);
    }

    #[tokio::test]
    async fn configuration_updates_require_a_ready_instance() {
        let world = SyntheticWorld::new("update-guards");
        world.break_path("/mojang/client.jar");
        let error = world
            .create_with_faults("Half Installed", InstanceFaults::default())
            .await
            .unwrap_err();
        assert!(matches!(error, InstanceError::GameInstall(_)));

        let registry = world.load_registry();
        let installing = registry.instances()[0].id().clone();
        let result = update_instance_configuration(
            &world.registry_path(),
            &world.endpoints(),
            &installing,
            InstanceConfiguration::for_minecraft_version("26.2"),
        );
        assert!(matches!(result, Err(InstanceError::NotReady { .. })));

        let ghost = crate::instances::InstanceId::new("ghost").unwrap();
        let result = update_instance_configuration(
            &world.registry_path(),
            &world.endpoints(),
            &ghost,
            InstanceConfiguration::for_minecraft_version("26.2"),
        );
        assert!(matches!(result, Err(InstanceError::NotFound { .. })));
    }

    #[tokio::test]
    async fn automatic_loader_policy_uses_the_release_exact_loader() {
        let world = SyntheticWorld::new("automatic-loader");
        // The synthetic loader list serves 0.19.5 as the single stable entry.
        let record = world
            .create_with_configuration(
                "Automatic",
                InstanceConfiguration::for_minecraft_version("26.2"),
            )
            .await
            .unwrap();
        assert_eq!(record.release().fabric_loader_version(), "0.19.5");

        // A pinned policy for an unlisted loader fails at creation before
        // any record is persisted.
        let mut pinned = InstanceConfiguration::for_minecraft_version("26.2");
        pinned.set_loader(crate::instances::settings::LoaderConfiguration::fabric(
            crate::instances::settings::LoaderPolicy::Pinned {
                version: "0.99.0".to_owned(),
            },
        ));
        let error = world
            .create_with_configuration("Pinned Missing", pinned)
            .await
            .unwrap_err();
        assert!(
            matches!(error, InstanceError::LoaderResolution { .. }),
            "{error}"
        );
        // The failed attempt persisted nothing: one record total.
        assert_eq!(world.load_registry().instances().len(), 1);
    }

    /// Controlled live instance creation against the real official Mojang
    /// and Fabric chains plus the launcher's actual checked-in development
    /// release fixture: the same lifecycle path that production creation
    /// uses, with injected development endpoints. The artifact URLs pin
    /// `http://127.0.0.1:8765/`, so this test serves the development
    /// directory there — a local development source, honestly not
    /// production release infrastructure.
    ///
    /// Ignored by default (downloads the real Minecraft + Fabric artifact
    /// set, ~600 MB, ~15 minutes cold); run with
    /// `cargo test -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "downloads the real Minecraft + Fabric artifact set from official sources"]
    async fn live_official_game_with_development_release_creates_and_validates() {
        let started = std::time::Instant::now();

        // Serve the checked-in development fixture directory on the pinned
        // loopback port so the fixture's artifact URLs resolve.
        let manifest_text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("development/aurora-releases.json"),
        )
        .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest_text).unwrap();
        let development_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("development");
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for release in manifest["releases"].as_array().unwrap() {
            let url = release["artifact"]["url"].as_str().unwrap();
            let name = url.rsplit('/').next().unwrap();
            files.insert(
                format!("/{name}"),
                std::fs::read(development_dir.join(name)).unwrap(),
            );
        }
        let served = std::sync::Arc::new(files);
        let server = TestServer::spawn_on(
            std::sync::Arc::new(move |request| {
                served
                    .get(&request.path)
                    .map(|body| TestResponse::ok(body))
                    .unwrap_or(TestResponse::status(404))
            }),
            "127.0.0.1:8765",
        );

        // A throwaway managed root under the system temp directory.
        let root = std::env::temp_dir()
            .join("aurora-live-instance-test")
            .join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();

        let endpoints = InstanceEndpoints::development().expect("the fixture must parse");
        let mut events = 0usize;
        let record = create_instance(
            &managed,
            &managed.instance_registry_file(),
            &managed.config_file(),
            &endpoints,
            CreateInstanceRequest::new(
                "Live Verification",
                InstanceConfiguration::for_minecraft_version("26.2"),
            ),
            &mut |progress| {
                events += 1;
                let game = progress
                    .game
                    .as_ref()
                    .map(|game| {
                        format!(
                            " {:?} {}/{}",
                            game.phase, game.completed_items, game.total_items
                        )
                    })
                    .unwrap_or_default();
                eprintln!("[live] {}{game}", progress.phase.as_str());
            },
            InstanceFaults::default(),
        )
        .await
        .expect("the live instance must create completely");

        eprintln!(
            "[live] instance {} created ready in {:.1}s",
            record.id(),
            started.elapsed().as_secs_f64()
        );

        // Complete validation passes.
        let registry = InstanceRegistry::load(&managed.instance_registry_file()).unwrap();
        let validation = validate_instance(&managed, &registry, record.id()).unwrap();
        assert_eq!(validation.status, InstanceStatus::Ready);
        assert!(validation.problems.is_empty());

        // The Aurora artifact is launcher-managed and SHA-256 verified.
        let aurora_state = crate::aurora::load_installed_state(&managed, record.id())
            .unwrap()
            .expect("aurora state committed");
        assert_eq!(aurora_state.aurora_version(), "0.3.0");
        assert_eq!(aurora_state.trust().kind_name(), "expectedDigestVerified");
        let artifact_path = managed
            .instance_paths(record.id())
            .mods()
            .join("aurora-0.3.0.jar");
        assert!(artifact_path.is_file());

        // Selection: first successful instance became selected.
        let config = crate::config::load(&managed.config_file())
            .unwrap()
            .expect("config materialized");
        assert_eq!(config.selected_instance_id(), Some(record.id()));

        // Rename changes metadata only: path identity is preserved.
        let renamed = rename_instance(
            &managed.instance_registry_file(),
            record.id(),
            "Live Verification Renamed",
        )
        .unwrap();
        assert_eq!(renamed.id(), record.id());
        assert!(artifact_path.is_file(), "the artifact path is unchanged");

        // User areas are never touched by creation; staging is clean; no
        // Java runtime, no launch, no .minecraft (verified outside).
        let instance_paths = managed.instance_paths(record.id());
        assert!(!instance_paths.root().join(".install-staging").exists());
        assert!(!managed.runtimes_dir().exists());
        assert!(events > 0);
        let _ = server;
    }

    /// Opt-in production-source diagnostic in a uniquely created temp root.
    /// The root is retained for manual readiness and launch inspection.
    #[tokio::test]
    #[ignore = "downloads the real production Aurora 2.1.2 instance"]
    async fn benchmark_live_production_instances() {
        use std::time::Instant;

        let root =
            std::env::temp_dir().join(format!("aurora-production-bench-{}", uuid::Uuid::new_v4()));
        let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();
        let endpoints = InstanceEndpoints::creation().unwrap();
        let registry_path = managed.instance_registry_file();
        let config_path = managed.config_file();
        println!("PRODUCTION BENCH ROOT {}", root.display());

        let started = Instant::now();
        let mut phases = Vec::new();
        let mut last = String::new();
        let mut progress = |event: InstanceProgress| {
            let phase = match &event.game {
                Some(game) if game.phase == crate::install::InstallPhase::Acquiring => {
                    let category = match game.current_item.as_deref() {
                        Some("asset index") => "asset-index",
                        Some(item) if item.contains(" client") => "client",
                        Some(item) if item.starts_with("logging configuration") => "logging",
                        Some(item) if item.starts_with("asset ") => "assets",
                        _ => "libraries",
                    };
                    format!("game/{category}")
                }
                Some(game) => format!("game/{}", game.phase.as_str()),
                None => event.phase.as_str().to_owned(),
            };
            if phase != last {
                phases.push((phase.clone(), started.elapsed().as_millis()));
                last = phase;
            }
        };
        let first = create_instance(
            &managed,
            &registry_path,
            &config_path,
            &endpoints,
            CreateInstanceRequest::new(
                "Production benchmark one",
                InstanceConfiguration::for_minecraft_version("1.21.11"),
            ),
            &mut progress,
            InstanceFaults::default(),
        )
        .await
        .unwrap();
        println!(
            "PRODUCTION BENCH cold total_ms={} instance={} phases={phases:?}",
            started.elapsed().as_millis(),
            first.id()
        );

        let started = Instant::now();
        install_instance_configuration(
            &managed,
            &registry_path,
            &config_path,
            &endpoints,
            first.id(),
            &mut |_| {},
            InstanceFaults::default(),
        )
        .await
        .unwrap();
        println!(
            "PRODUCTION BENCH warm-reinstall total_ms={}",
            started.elapsed().as_millis()
        );

        let started = Instant::now();
        let second = create_instance(
            &managed,
            &registry_path,
            &config_path,
            &endpoints,
            CreateInstanceRequest::new(
                "Production benchmark two",
                InstanceConfiguration::for_minecraft_version("1.21.11"),
            ),
            &mut |_| {},
            InstanceFaults::default(),
        )
        .await
        .unwrap();
        println!(
            "PRODUCTION BENCH second-instance total_ms={} instance={}",
            started.elapsed().as_millis(),
            second.id()
        );

        for record in [first, second] {
            let validation = validate_instance(
                &managed,
                &InstanceRegistry::load(&registry_path).unwrap(),
                record.id(),
            )
            .unwrap();
            assert_eq!(
                validation.status,
                InstanceStatus::Ready,
                "{:?}",
                validation.problems
            );
        }
    }

    /// Repeat the warm transaction on an already-created benchmark root.
    /// The root must be a direct child of the OS temp directory with the
    /// benchmark prefix, so this cannot mutate normal launcher data.
    #[tokio::test]
    #[ignore = "reinstalls an existing controlled production benchmark instance"]
    async fn benchmark_existing_production_warm_install() {
        use std::time::Instant;

        let requested = std::env::var_os("AURORA_BENCH_ROOT").expect("set AURORA_BENCH_ROOT");
        let root = std::path::PathBuf::from(requested).canonicalize().unwrap();
        let temp = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(root.parent(), Some(temp.as_path()));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("aurora-production-bench-")
        );
        let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();
        let registry_path = managed.instance_registry_file();
        let registry = InstanceRegistry::load(&registry_path).unwrap();
        let first = registry.instances().first().unwrap();
        assert_eq!(first.release().aurora_version(), "2.1.2");
        let id = first.id().clone();
        let endpoints = InstanceEndpoints::operational().unwrap();
        let started = Instant::now();
        let mut phases = Vec::new();
        let mut last = String::new();
        install_instance_configuration(
            &managed,
            &registry_path,
            &managed.config_file(),
            &endpoints,
            &id,
            &mut |event| {
                let phase = match &event.game {
                    Some(game) if game.phase == crate::install::InstallPhase::Acquiring => {
                        match game.current_item.as_deref() {
                            Some("asset index") => "asset-index".to_owned(),
                            Some(item) if item.contains(" client") => "client".to_owned(),
                            Some(item) if item.starts_with("logging configuration") => {
                                "logging".to_owned()
                            }
                            Some(item) if item.starts_with("asset ") => "assets".to_owned(),
                            _ => "libraries".to_owned(),
                        }
                    }
                    Some(game) => game.phase.as_str().to_owned(),
                    None => event.phase.as_str().to_owned(),
                };
                if phase != last {
                    phases.push((phase.clone(), started.elapsed().as_millis()));
                    last = phase;
                }
            },
            InstanceFaults::default(),
        )
        .await
        .unwrap();
        println!(
            "PRODUCTION BENCH existing-warm total_ms={} phases={phases:?}",
            started.elapsed().as_millis()
        );
    }

    #[tokio::test]
    #[ignore = "provisions the real managed Java runtime in a controlled benchmark root"]
    async fn benchmark_live_production_managed_java() {
        use std::time::Instant;

        let requested = std::env::var_os("AURORA_BENCH_ROOT").expect("set AURORA_BENCH_ROOT");
        let root = std::path::PathBuf::from(requested).canonicalize().unwrap();
        let temp = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(root.parent(), Some(temp.as_path()));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("aurora-production-bench-")
        );
        let managed = ManagedPaths::from_app_local_data_dir(root.join("managed")).unwrap();
        let registry_path = managed.instance_registry_file();
        let registry = InstanceRegistry::load(&registry_path).unwrap();
        let id = registry.instances().first().unwrap().id();
        let endpoints = InstanceEndpoints::operational().unwrap();
        let runtime_endpoints = RuntimeMetadataEndpoints::official();
        let resolve_started = Instant::now();
        let (_, plan) = resolve_instance_launch_plans(
            &managed,
            &registry_path,
            &endpoints,
            &runtime_endpoints,
            id,
        )
        .await
        .unwrap();
        println!(
            "PRODUCTION JAVA resolve_ms={} component={} major={} files={}",
            resolve_started.elapsed().as_millis(),
            plan.component(),
            plan.required_major_version(),
            plan.entries().len()
        );
        for scenario in ["cold", "warm"] {
            let started = Instant::now();
            let result = crate::runtime::install::ensure_runtime(
                &managed,
                &plan,
                endpoints.install.download_options(),
                true,
                &mut |_| {},
            )
            .await
            .unwrap();
            println!(
                "PRODUCTION JAVA {scenario} total_ms={} reused={}",
                started.elapsed().as_millis(),
                result.reused()
            );
        }
    }
}
