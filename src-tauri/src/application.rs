use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::cache::{AcquisitionError, ArtifactCache, ArtifactOrigin};
use crate::config::{ConfigError, ConfigLoad};
use crate::distribution::ReleaseChannel;
use crate::downloads::{ArtifactSource, DownloadError, InvalidArtifactSource};
use crate::fabric::metadata::{FabricMetadataError, InvalidLoaderVersion, LoaderVersionId};
use crate::fabric::{FabricResolutionError, GameResolutionError, resolve_game_plan};
use crate::instances::{InstanceRecord, InstanceRegistry, InstanceRegistryError};
use crate::minecraft::metadata::{
    InvalidMinecraftVersion, MetadataEndpoints, MetadataError, MinecraftVersionId,
};
use crate::minecraft::plan::PlanError;
use crate::minecraft::rules::UnsupportedPlatform;
use crate::minecraft::{MinecraftResolutionError, resolve_install_plan};
use crate::paths::ManagedPaths;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationStatus {
    launcher_version: String,
    platform: PlatformInfo,
    managed_data_root: String,
    backend_status: BackendStatus,
}

impl ApplicationStatus {
    fn ready(
        launcher_version: impl Into<String>,
        os: impl Into<String>,
        architecture: impl Into<String>,
        managed_data_root: PathBuf,
    ) -> Self {
        Self {
            launcher_version: launcher_version.into(),
            platform: PlatformInfo {
                os: os.into(),
                architecture: architecture.into(),
            },
            managed_data_root: managed_data_root.to_string_lossy().into_owned(),
            backend_status: BackendStatus::Ready,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    os: String,
    architecture: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendStatus {
    Ready,
}

/// The persisted launcher state shown by the UI: configuration summary and
/// the known instance records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherState {
    config: LauncherConfigSummary,
    instances: Vec<InstanceSummary>,
}

impl LauncherState {
    fn from_parts(config: crate::config::LauncherConfig, registry: InstanceRegistry) -> Self {
        Self {
            config: LauncherConfigSummary {
                schema_version: config.schema_version(),
                selected_instance_id: config.selected_instance_id().map(|id| id.to_string()),
            },
            instances: registry
                .instances()
                .iter()
                .map(InstanceSummary::from_record)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherConfigSummary {
    schema_version: u32,
    selected_instance_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSummary {
    id: String,
    display_name: String,
    channel: ReleaseChannel,
    aurora_version: Option<String>,
}

impl InstanceSummary {
    fn from_record(record: &InstanceRecord) -> Self {
        Self {
            id: record.id().to_string(),
            display_name: record.display_name().to_owned(),
            channel: record.release().channel(),
            aurora_version: record.release().aurora_version().map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    code: String,
    message: String,
}

impl CommandError {
    fn managed_path(message: impl Into<String>) -> Self {
        Self::new("managed_path_unavailable", message)
    }

    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<ConfigError> for CommandError {
    fn from(error: ConfigError) -> Self {
        let code = match &error {
            ConfigError::Malformed(_) => "config_malformed",
            ConfigError::UnsupportedSchema { .. } => "config_unsupported_schema",
            ConfigError::Read(_) | ConfigError::Write(_) => "storage_io_failure",
        };
        Self::new(code, error.to_string())
    }
}

impl From<InstanceRegistryError> for CommandError {
    fn from(error: InstanceRegistryError) -> Self {
        let code = match &error {
            InstanceRegistryError::Malformed(_) => "instances_invalid",
            InstanceRegistryError::UnsupportedSchema { .. } => "instances_unsupported_schema",
            InstanceRegistryError::Read(_) => "storage_io_failure",
        };
        Self::new(code, error.to_string())
    }
}

impl From<InvalidArtifactSource> for CommandError {
    fn from(error: InvalidArtifactSource) -> Self {
        Self::new("artifact_source_invalid", error.to_string())
    }
}

impl From<InvalidMinecraftVersion> for CommandError {
    fn from(error: InvalidMinecraftVersion) -> Self {
        Self::new("minecraft_version_invalid", error.to_string())
    }
}

impl From<InvalidLoaderVersion> for CommandError {
    fn from(error: InvalidLoaderVersion) -> Self {
        Self::new("fabric_loader_version_invalid", error.to_string())
    }
}

impl From<UnsupportedPlatform> for CommandError {
    fn from(error: UnsupportedPlatform) -> Self {
        Self::new("minecraft_platform_unsupported", error.to_string())
    }
}

impl From<FabricResolutionError> for CommandError {
    fn from(error: FabricResolutionError) -> Self {
        let code = match &error {
            FabricResolutionError::Metadata(metadata) => match metadata {
                FabricMetadataError::Network(_) | FabricMetadataError::ResponseTooLarge { .. } => {
                    "fabric_metadata_network_failure"
                }
                FabricMetadataError::HttpStatus { .. } => "fabric_metadata_network_failure",
                FabricMetadataError::Malformed { .. } => "fabric_metadata_invalid",
                FabricMetadataError::Unsupported { .. } => "fabric_metadata_unsupported",
            },
            FabricResolutionError::LoaderNotFound { .. } => "fabric_loader_not_found",
            FabricResolutionError::CombinationUnsupported { .. } => {
                "fabric_combination_unsupported"
            }
            FabricResolutionError::Planning(planning) => match planning {
                crate::fabric::plan::FabricPlanError::LibraryInvalid { .. } => {
                    "fabric_library_invalid"
                }
                crate::fabric::plan::FabricPlanError::RepositoryInvalid { .. } => {
                    "fabric_repository_invalid"
                }
                crate::fabric::plan::FabricPlanError::ArtifactInvalid { .. } => {
                    "fabric_library_invalid"
                }
            },
        };
        Self::new(code, error.to_string())
    }
}

impl From<GameResolutionError> for CommandError {
    fn from(error: GameResolutionError) -> Self {
        match error {
            GameResolutionError::Minecraft(error) => error.into(),
            GameResolutionError::Fabric(error) => error.into(),
            GameResolutionError::Composition(error) => {
                Self::new("fabric_plan_conflict", error.to_string())
            }
        }
    }
}

impl From<MinecraftResolutionError> for CommandError {
    fn from(error: MinecraftResolutionError) -> Self {
        let code = match &error {
            MinecraftResolutionError::Metadata(metadata) => match metadata {
                MetadataError::VersionNotFound { .. } => "minecraft_version_not_found",
                MetadataError::Network(_) | MetadataError::ResponseTooLarge { .. } => {
                    "minecraft_metadata_network_failure"
                }
                MetadataError::ManifestInvalid { .. } => "minecraft_manifest_invalid",
                MetadataError::DocumentInvalid { .. } => "minecraft_version_metadata_invalid",
                MetadataError::Integrity { .. } => "minecraft_metadata_integrity_failure",
                MetadataError::Unsupported(_) => "minecraft_version_unsupported",
            },
            MinecraftResolutionError::Planning(planning) => match planning {
                PlanError::LibraryInvalid { .. } => "minecraft_library_invalid",
                PlanError::ArtifactInvalid { .. } => "minecraft_artifact_invalid",
                PlanError::Unsupported { .. } => "minecraft_version_unsupported",
            },
        };
        Self::new(code, error.to_string())
    }
}

impl From<AcquisitionError> for CommandError {
    fn from(error: AcquisitionError) -> Self {
        let code = match &error {
            AcquisitionError::Download(download) => match download {
                DownloadError::Network(_) => "network_unavailable",
                DownloadError::Timeout(_) => "download_timeout",
                DownloadError::Redirect(_) => "download_redirect_failure",
                DownloadError::HttpStatus { .. } => "download_http_failure",
                DownloadError::SizeMismatch { .. } => "artifact_size_mismatch",
                DownloadError::Sha256Mismatch { .. } => "artifact_hash_mismatch",
                DownloadError::StagingIo(_) => "cache_io_failure",
            },
            AcquisitionError::StoreIo(_) => "cache_io_failure",
            AcquisitionError::Promotion(_) | AcquisitionError::PromotionBlocked { .. } => {
                "artifact_promotion_failure"
            }
        };
        Self::new(code, error.to_string())
    }
}

fn managed_paths(app: &AppHandle) -> Result<ManagedPaths, CommandError> {
    let resolved_root = app.path().app_local_data_dir().map_err(|error| {
        CommandError::managed_path(format!(
            "Aurora's managed data location could not be resolved: {error}"
        ))
    })?;

    ManagedPaths::from_app_local_data_dir(resolved_root).map_err(|error| {
        CommandError::managed_path(format!(
            "Aurora's managed data location is not safe to use: {error}"
        ))
    })
}

#[tauri::command]
pub fn get_application_status(app: AppHandle) -> Result<ApplicationStatus, CommandError> {
    let managed_paths = managed_paths(&app)?;

    eprintln!(
        "[aurora-launcher] backend ready; managed data root: {}",
        managed_paths.data_root().display()
    );

    Ok(ApplicationStatus::ready(
        app.package_info().version.to_string(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        managed_paths.data_root().to_path_buf(),
    ))
}

/// Loads the persisted launcher state.
///
/// On first run this materializes the default configuration under the managed
/// data root; malformed persisted files are reported as structured errors and
/// left untouched.
#[tauri::command]
pub fn get_launcher_state(app: AppHandle) -> Result<LauncherState, CommandError> {
    let managed_paths = managed_paths(&app)?;

    let loaded = crate::config::load_or_initialize(&managed_paths.config_file())?;
    if let ConfigLoad::Initialized(_) = &loaded {
        eprintln!(
            "[aurora-launcher] initialized default launcher configuration at {}",
            managed_paths.config_file().display()
        );
    }

    let registry = InstanceRegistry::load(&managed_paths.instance_registry_file())?;

    Ok(LauncherState::from_parts(loaded.into_config(), registry))
}

/// Typed artifact metadata accepted by the acquisition command.
///
/// The native layer re-validates everything: the URL must be HTTPS, the
/// digest must be a canonical hexadecimal SHA-256 value, and the optional
/// size must be positive.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquireArtifactRequest {
    url: String,
    sha256: String,
    size_bytes: Option<u64>,
}

/// The typed result of one acquisition: the verified cache object and how it
/// came to be there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquiredArtifact {
    path: String,
    sha256: String,
    bytes: u64,
    origin: AcquisitionOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AcquisitionOrigin {
    Downloaded,
    CacheHit,
}

/// Acquires one artifact into the launcher's verified cache.
///
/// This is the development-facing proof of the acquisition pipeline: it takes
/// typed artifact metadata (never a shell string or arbitrary path), runs the
/// full untrusted-staging → verify → promote flow natively, and returns the
/// verified object. It accepts no fixture shortcuts, so it can only be driven
/// with a real reachable source — deterministic pipeline verification lives
/// in the Rust test suite, which uses local loopback test servers.
#[tauri::command]
pub async fn acquire_artifact(
    app: AppHandle,
    request: AcquireArtifactRequest,
) -> Result<AcquiredArtifact, CommandError> {
    let managed_paths = managed_paths(&app)?;

    let source = ArtifactSource::https(&request.url, &request.sha256, request.size_bytes)?;

    let cache = ArtifactCache::new(managed_paths);
    let artifact = cache.acquire(&source).await?;

    eprintln!(
        "[aurora-launcher] artifact {} verified ({}) at {}",
        artifact.sha256,
        match artifact.origin {
            ArtifactOrigin::Downloaded => "downloaded",
            ArtifactOrigin::CacheHit => "cache hit",
        },
        artifact.path.display()
    );

    Ok(AcquiredArtifact {
        path: artifact.path.to_string_lossy().into_owned(),
        sha256: artifact.sha256.as_hex(),
        bytes: artifact.bytes,
        origin: match artifact.origin {
            ArtifactOrigin::Downloaded => AcquisitionOrigin::Downloaded,
            ArtifactOrigin::CacheHit => AcquisitionOrigin::CacheHit,
        },
    })
}

/// Typed request accepted by the Minecraft planning command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanMinecraftInstallRequest {
    version: String,
}

/// A concise summary of one resolved Minecraft installation plan.
///
/// The full normalized plan stays native; the proof UI only needs counts and
/// key requirements, never hundreds of library rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinecraftPlanSummary {
    minecraft_version: String,
    version_type: String,
    java_component: String,
    java_major_version: u32,
    client_sha1: String,
    client_size_bytes: u64,
    asset_index_id: String,
    library_count: usize,
    native_library_count: usize,
    main_class: String,
    game_argument_count: usize,
    jvm_argument_count: usize,
}

/// Resolves one exact Minecraft version into a normalized install plan for
/// the current platform and returns a concise summary.
///
/// This is the development-facing proof of the Phase 3 metadata-resolution
/// layer: it performs real discovery, SHA-1-verified version-document
/// fetching, parsing, and platform-aware planning in Rust, and installs
/// nothing — no client jar, library, native, asset, or runtime is downloaded.
#[tauri::command]
pub async fn plan_minecraft_install(
    request: PlanMinecraftInstallRequest,
) -> Result<MinecraftPlanSummary, CommandError> {
    let version = MinecraftVersionId::new(request.version.trim())?;
    let platform = crate::minecraft::rules::PlatformProfile::current()?;

    let plan = resolve_install_plan(
        &MetadataEndpoints::official(),
        &version,
        platform,
        &crate::downloads::DownloadOptions::default(),
    )
    .await?;

    eprintln!(
        "[aurora-launcher] planned Minecraft {} for {}-{}: {} libraries ({} native), Java {} ({}), asset index {}",
        plan.minecraft_version(),
        platform.os(),
        platform.arch(),
        plan.libraries().len(),
        plan.native_library_count(),
        plan.java().component(),
        plan.java().major_version(),
        plan.asset_index().id(),
    );

    Ok(MinecraftPlanSummary {
        minecraft_version: plan.minecraft_version().to_owned(),
        version_type: plan.version_type().to_string(),
        java_component: plan.java().component().to_owned(),
        java_major_version: plan.java().major_version(),
        client_sha1: plan.client().sha1().as_hex(),
        client_size_bytes: plan.client().size_bytes(),
        asset_index_id: plan.asset_index().id().to_owned(),
        library_count: plan.libraries().len(),
        native_library_count: plan.native_library_count(),
        main_class: plan.launch().main_class().to_owned(),
        game_argument_count: plan.launch().game_arguments().len(),
        jvm_argument_count: plan.launch().jvm_arguments().len(),
    })
}

/// Typed request accepted by the Fabric planning command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFabricInstallRequest {
    minecraft_version: String,
    loader_version: String,
}

/// A concise summary of one composed game installation plan.
///
/// As with the Minecraft summary, the full composed plan stays native; the
/// proof UI only needs the composed counts and key requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricPlanSummary {
    minecraft_version: String,
    loader_version: String,
    vanilla_library_count: usize,
    fabric_library_count: usize,
    final_library_count: usize,
    fabric_digested_library_count: usize,
    java_component: String,
    java_major_version: u32,
    java_raised_by_loader: bool,
    final_main_class: String,
}

/// Resolves one exact Minecraft + Fabric Loader combination into the composed
/// game install plan for the current platform and returns a concise summary.
///
/// This is the development-facing proof of the Phase 4 Fabric resolution and
/// composition layer: it performs real loader discovery, exact profile
/// resolution, Mojang planning, and composition in Rust, and installs
/// nothing — no Minecraft or Fabric artifact is downloaded.
#[tauri::command]
pub async fn plan_fabric_install(
    request: PlanFabricInstallRequest,
) -> Result<FabricPlanSummary, CommandError> {
    let version = MinecraftVersionId::new(request.minecraft_version.trim())?;
    let loader = LoaderVersionId::new(request.loader_version.trim())?;
    let platform = crate::minecraft::rules::PlatformProfile::current()?;

    let plan = resolve_game_plan(
        &MetadataEndpoints::official(),
        &crate::fabric::metadata::FabricMetaEndpoints::official(),
        &version,
        &loader,
        platform,
        &crate::downloads::DownloadOptions::default(),
    )
    .await?;

    Ok(FabricPlanSummary {
        minecraft_version: plan.minecraft().minecraft_version().to_owned(),
        loader_version: plan.loader().loader_version().to_owned(),
        vanilla_library_count: plan.vanilla_library_count(),
        fabric_library_count: plan.fabric_library_count(),
        final_library_count: plan.libraries().len(),
        fabric_digested_library_count: plan.loader().digested_library_count(),
        java_component: plan.java().component().to_owned(),
        java_major_version: plan.java().major_version(),
        java_raised_by_loader: plan.java().raised_by_loader(),
        final_main_class: plan.main_class().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_status_preserves_structured_platform_metadata() {
        let root = std::env::temp_dir().join("com.aurora.launcher");

        let status = ApplicationStatus::ready("0.1.0", "windows", "x86_64", root.clone());

        assert_eq!(status.launcher_version, "0.1.0");
        assert_eq!(status.platform.os, "windows");
        assert_eq!(status.platform.architecture, "x86_64");
        assert_eq!(status.managed_data_root, root.to_string_lossy());
        assert_eq!(status.backend_status, BackendStatus::Ready);
    }

    #[test]
    fn managed_path_errors_have_a_stable_machine_code() {
        let error = CommandError::managed_path("example failure");

        assert_eq!(error.code, "managed_path_unavailable");
        assert_eq!(error.message, "example failure");
    }

    #[test]
    fn config_errors_map_to_stable_machine_codes() {
        let malformed = CommandError::from(ConfigError::Malformed("broken".to_owned()));
        assert_eq!(malformed.code, "config_malformed");

        let unsupported = CommandError::from(ConfigError::UnsupportedSchema {
            found: 9,
            supported: 1,
        });
        assert_eq!(unsupported.code, "config_unsupported_schema");
        assert!(unsupported.message.contains("9"));

        let io = CommandError::from(ConfigError::Read(std::io::Error::other("disk")));
        assert_eq!(io.code, "storage_io_failure");
    }

    #[test]
    fn instance_registry_errors_map_to_stable_machine_codes() {
        let malformed = CommandError::from(InstanceRegistryError::Malformed("broken".to_owned()));
        assert_eq!(malformed.code, "instances_invalid");

        let unsupported = CommandError::from(InstanceRegistryError::UnsupportedSchema {
            found: 9,
            supported: 1,
        });
        assert_eq!(unsupported.code, "instances_unsupported_schema");

        let io = CommandError::from(InstanceRegistryError::Read(std::io::Error::other("disk")));
        assert_eq!(io.code, "storage_io_failure");
    }

    #[test]
    fn invalid_artifact_sources_map_to_a_stable_machine_code() {
        let insecure = CommandError::from(
            crate::downloads::ArtifactSource::https(
                "http://example.invalid/x",
                "0".repeat(64).as_str(),
                None,
            )
            .unwrap_err(),
        );
        assert_eq!(insecure.code, "artifact_source_invalid");
        assert!(insecure.message.contains("HTTPS"));
    }

    #[test]
    fn invalid_minecraft_versions_map_to_a_stable_machine_code() {
        let error = CommandError::from(
            MinecraftVersionId::new("../evil").expect_err("traversal ids are invalid"),
        );
        assert_eq!(error.code, "minecraft_version_invalid");
    }

    #[test]
    fn invalid_loader_versions_map_to_a_stable_machine_code() {
        let error = CommandError::from(
            LoaderVersionId::new("../evil").expect_err("traversal ids are invalid"),
        );
        assert_eq!(error.code, "fabric_loader_version_invalid");
    }

    #[test]
    fn fabric_resolution_errors_map_to_stable_machine_codes() {
        use crate::fabric::plan::FabricPlanError;

        let cases: Vec<(FabricResolutionError, &str)> = vec![
            (
                FabricResolutionError::Metadata(FabricMetadataError::Network(
                    DownloadError::HttpStatus { status: 503 },
                )),
                "fabric_metadata_network_failure",
            ),
            (
                FabricResolutionError::Metadata(FabricMetadataError::HttpStatus { status: 500 }),
                "fabric_metadata_network_failure",
            ),
            (
                FabricResolutionError::Metadata(FabricMetadataError::ResponseTooLarge {
                    limit_bytes: 8 * 1024 * 1024,
                }),
                "fabric_metadata_network_failure",
            ),
            (
                FabricResolutionError::Metadata(FabricMetadataError::Malformed {
                    reason: "broken".to_owned(),
                }),
                "fabric_metadata_invalid",
            ),
            (
                FabricResolutionError::Metadata(FabricMetadataError::Unsupported {
                    reason: "launcherMeta generation 3".to_owned(),
                }),
                "fabric_metadata_unsupported",
            ),
            (
                FabricResolutionError::LoaderNotFound {
                    requested: "0.99.0".to_owned(),
                },
                "fabric_loader_not_found",
            ),
            (
                FabricResolutionError::CombinationUnsupported {
                    game: "9.9.9".to_owned(),
                    loader: "0.19.5".to_owned(),
                },
                "fabric_combination_unsupported",
            ),
            (
                FabricResolutionError::Planning(FabricPlanError::LibraryInvalid {
                    name: "broken".to_owned(),
                    reason: "not a coordinate".to_owned(),
                }),
                "fabric_library_invalid",
            ),
            (
                FabricResolutionError::Planning(FabricPlanError::RepositoryInvalid {
                    url: "http://maven.example.invalid/".to_owned(),
                    reason: "must use HTTPS".to_owned(),
                }),
                "fabric_repository_invalid",
            ),
            (
                FabricResolutionError::Planning(FabricPlanError::ArtifactInvalid {
                    source: "org.ow2.asm:asm:9.10.1".to_owned(),
                    reason: "bad digest".to_owned(),
                }),
                "fabric_library_invalid",
            ),
        ];

        for (error, expected_code) in cases {
            let command_error = CommandError::from(error);
            assert_eq!(command_error.code, expected_code);
            assert!(
                !command_error.message.is_empty(),
                "every structured error carries a readable message"
            );
        }
    }

    #[test]
    fn game_resolution_errors_map_to_stable_machine_codes() {
        use crate::fabric::plan::CompositionError;

        let conflict = GameResolutionError::Composition(CompositionError::LibraryConflict {
            existing: "org.ow2.asm:asm:9.9.9".to_owned(),
            conflicting: "org.ow2.asm:asm:9.10.1".to_owned(),
        });
        assert_eq!(CommandError::from(conflict).code, "fabric_plan_conflict");

        let minecraft = GameResolutionError::Minecraft(MinecraftResolutionError::Metadata(
            MetadataError::VersionNotFound {
                requested: "9.9.9".to_owned(),
            },
        ));
        assert_eq!(
            CommandError::from(minecraft).code,
            "minecraft_version_not_found"
        );

        let fabric = GameResolutionError::Fabric(FabricResolutionError::LoaderNotFound {
            requested: "0.99.0".to_owned(),
        });
        assert_eq!(CommandError::from(fabric).code, "fabric_loader_not_found");
    }

    #[test]
    fn minecraft_resolution_errors_map_to_stable_machine_codes() {
        let cases: Vec<(MinecraftResolutionError, &str)> = vec![
            (
                MinecraftResolutionError::Metadata(MetadataError::VersionNotFound {
                    requested: "9.9.9".to_owned(),
                }),
                "minecraft_version_not_found",
            ),
            (
                MinecraftResolutionError::Metadata(MetadataError::ManifestInvalid {
                    reason: "broken".to_owned(),
                }),
                "minecraft_manifest_invalid",
            ),
            (
                MinecraftResolutionError::Metadata(MetadataError::DocumentInvalid {
                    reason: "broken".to_owned(),
                }),
                "minecraft_version_metadata_invalid",
            ),
            (
                MinecraftResolutionError::Metadata(MetadataError::Integrity {
                    context: "Minecraft version document '26.2'".to_owned(),
                    expected: "a".repeat(40),
                    actual: "b".repeat(40),
                }),
                "minecraft_metadata_integrity_failure",
            ),
            (
                MinecraftResolutionError::Metadata(MetadataError::Network(
                    DownloadError::HttpStatus { status: 503 },
                )),
                "minecraft_metadata_network_failure",
            ),
            (
                MinecraftResolutionError::Metadata(MetadataError::ResponseTooLarge {
                    limit_bytes: 8 * 1024 * 1024,
                }),
                "minecraft_metadata_network_failure",
            ),
            (
                MinecraftResolutionError::Metadata(MetadataError::Unsupported(
                    "historical".to_owned(),
                )),
                "minecraft_version_unsupported",
            ),
            (
                MinecraftResolutionError::Planning(PlanError::LibraryInvalid {
                    name: "broken".to_owned(),
                    reason: "no artifact".to_owned(),
                }),
                "minecraft_library_invalid",
            ),
            (
                MinecraftResolutionError::Planning(PlanError::ArtifactInvalid {
                    source: "client".to_owned(),
                    reason: "not HTTPS".to_owned(),
                }),
                "minecraft_artifact_invalid",
            ),
            (
                MinecraftResolutionError::Planning(PlanError::Unsupported {
                    reason: "feature-conditioned library".to_owned(),
                }),
                "minecraft_version_unsupported",
            ),
        ];

        for (error, expected_code) in cases {
            let command_error = CommandError::from(error);
            assert_eq!(command_error.code, expected_code);
            assert!(
                !command_error.message.is_empty(),
                "every structured error carries a readable message"
            );
        }
    }

    #[test]
    fn acquisition_errors_map_to_stable_machine_codes() {
        let cases: Vec<(AcquisitionError, &str)> = vec![
            (
                AcquisitionError::Download(DownloadError::Redirect(
                    crate::downloads::RedirectRefusal::LimitExceeded { limit: 8 },
                )),
                "download_redirect_failure",
            ),
            (
                AcquisitionError::Download(DownloadError::Redirect(
                    crate::downloads::RedirectRefusal::InsecureDowngrade {
                        to: "http://example.invalid/x".to_owned(),
                    },
                )),
                "download_redirect_failure",
            ),
            (
                AcquisitionError::Download(DownloadError::HttpStatus { status: 404 }),
                "download_http_failure",
            ),
            (
                AcquisitionError::Download(DownloadError::SizeMismatch {
                    expected: 1,
                    actual: 2,
                }),
                "artifact_size_mismatch",
            ),
            (
                AcquisitionError::Download(DownloadError::Sha256Mismatch {
                    expected: "e".repeat(64),
                    actual: "f".repeat(64),
                }),
                "artifact_hash_mismatch",
            ),
            (
                AcquisitionError::Download(DownloadError::StagingIo(std::io::Error::other("disk"))),
                "cache_io_failure",
            ),
            (
                AcquisitionError::StoreIo(std::io::Error::other("disk")),
                "cache_io_failure",
            ),
            (
                AcquisitionError::Promotion(crate::cache::PromotionError {
                    context: "replacing a corrupt cache object",
                    source: std::io::Error::other("locked"),
                }),
                "artifact_promotion_failure",
            ),
            (
                AcquisitionError::PromotionBlocked {
                    rename: std::io::Error::other("rename"),
                    validation: std::io::Error::other("validation"),
                },
                "artifact_promotion_failure",
            ),
        ];

        for (error, expected_code) in cases {
            let command_error = CommandError::from(error);
            assert_eq!(command_error.code, expected_code);
            assert!(
                !command_error.message.is_empty(),
                "every structured error carries a readable message"
            );
        }
    }

    /// Transport-level classifications are exercised on genuine reqwest
    /// errors produced by a dead loopback endpoint, keeping the mapping
    /// honest without public internet access. Whether the platform surfaces
    /// the dead endpoint as a connect error or lets the timeout win the race
    /// is platform-dependent; both must map to a transport category.
    #[tokio::test]
    async fn real_transport_errors_map_to_their_machine_codes() {
        // Bind and drop a listener to obtain a guaranteed-dead port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Build through the launcher's own client so the rustls crypto
        // provider is installed exactly like in production.
        let source = crate::downloads::ArtifactSource::loopback_http_for_testing(
            &format!("http://127.0.0.1:{port}/aurora.jar"),
            &"a".repeat(64),
            None,
        )
        .unwrap();
        let destination = std::env::temp_dir()
            .join("aurora-application-test")
            .join(std::process::id().to_string())
            .join("unreachable.part");

        let error = crate::downloads::download(
            &source,
            &destination,
            &crate::downloads::DownloadOptions::default(),
        )
        .await
        .expect_err("a dead endpoint must fail");

        let command_error = CommandError::from(AcquisitionError::Download(error));
        assert!(
            matches!(
                command_error.code.as_str(),
                "network_unavailable" | "download_timeout"
            ),
            "a transport failure must map to a transport category, got: {}",
            command_error.code
        );
    }

    #[test]
    fn launcher_state_flattens_persisted_records_into_summary_dtos() {
        let config = crate::config::LauncherConfig::from_json(
            r#"{ "schemaVersion": 1, "selectedInstanceId": "aurora-default" }"#,
        )
        .unwrap();
        let registry = InstanceRegistry::from_json(
            r#"{
                "schemaVersion": 1,
                "instances": [
                    {
                        "id": "aurora-default",
                        "displayName": "Aurora Default",
                        "release": { "channel": "stable", "auroraVersion": null }
                    }
                ]
            }"#,
        )
        .unwrap();

        let state = LauncherState::from_parts(config, registry);

        assert_eq!(state.config.schema_version, 1);
        assert_eq!(
            state.config.selected_instance_id.as_deref(),
            Some("aurora-default")
        );
        assert_eq!(state.instances.len(), 1);
        assert_eq!(state.instances[0].id, "aurora-default");
        assert_eq!(state.instances[0].display_name, "Aurora Default");
        assert_eq!(state.instances[0].channel, ReleaseChannel::Stable);
        assert_eq!(state.instances[0].aurora_version, None);
    }
}
