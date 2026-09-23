use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

use crate::aurora::AuroraInstallError;
use crate::cache::{AcquisitionError, ArtifactCache, ArtifactOrigin};
use crate::config::{ConfigError, ConfigLoad};
use crate::distribution::ReleaseChannel;
use crate::downloads::{ArtifactSource, DownloadError, InvalidArtifactSource};
use crate::fabric::metadata::{FabricMetadataError, InvalidLoaderVersion, LoaderVersionId};
use crate::fabric::{FabricResolutionError, GameResolutionError, resolve_game_plan};
use crate::install::{
    InstallContext, InstallError, InstallFaults, InstalledGameValidation, ValidationOutcome,
    ValidationStatus, install_game as execute_install_game,
    validate_installed_game as run_installed_game_validation,
};
use crate::instances::lifecycle::{
    CreateInstanceRequest as LifecycleCreateRequest, InstanceError, InstanceFaults,
};
use crate::instances::{
    InstanceRecord, InstanceRegistry, InstanceRegistryError, InvalidInstanceId,
};
use crate::minecraft::metadata::{
    InvalidMinecraftVersion, MetadataEndpoints, MetadataError, MinecraftVersionId,
};
use crate::minecraft::plan::PlanError;
use crate::minecraft::rules::UnsupportedPlatform;
use crate::minecraft::{MinecraftResolutionError, resolve_install_plan};
use crate::paths::ManagedPaths;
use crate::runtime::install::{RuntimeInstallError, RuntimeValidation};
use crate::runtime::metadata::RuntimeMetadataError;

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
    fn from_parts(
        config: crate::config::LauncherConfig,
        registry: InstanceRegistry,
    ) -> Result<Self, CommandError> {
        // Referential integrity: a stored selection must name a registered
        // instance. A dangling one is a deliberate, documented error â€” the
        // launcher never silently selects a random instance.
        if let Some(selected) = config.selected_instance_id() {
            if registry.find(selected).is_none() {
                return Err(CommandError::new(
                    "config_selected_instance_dangling",
                    format!(
                        "the configured selected instance '{}' does not exist in the instance registry; select an existing instance to repair the selection",
                        selected
                    ),
                ));
            }
        }

        Ok(Self {
            config: LauncherConfigSummary {
                schema_version: config.schema_version(),
                selected_instance_id: config.selected_instance_id().map(|id| id.to_string()),
            },
            instances: registry
                .instances()
                .iter()
                .map(InstanceSummary::from_record)
                .collect(),
        })
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
    /// The stored lifecycle state (`installing`/`ready`). Whether a ready
    /// instance is still internally consistent is answered by the
    /// `validate_instance` command, which performs the deep (hashing)
    /// validation on demand rather than on every state load.
    state: String,
    channel: ReleaseChannel,
    aurora_version: String,
    minecraft_version: String,
    fabric_loader_version: String,
    /// The desired configuration, verbatim. The UI edits a draft and sends
    /// the whole proposed configuration to `update_instance_configuration`.
    configuration: InstanceConfigurationDto,
}

impl InstanceSummary {
    fn from_record(record: &InstanceRecord) -> Self {
        Self {
            id: record.id().to_string(),
            display_name: record.display_name().to_owned(),
            state: record.state().as_str().to_owned(),
            channel: record.release().channel(),
            aurora_version: record.release().aurora_version().to_owned(),
            minecraft_version: record.release().minecraft_version().to_owned(),
            fabric_loader_version: record.release().fabric_loader_version().to_owned(),
            configuration: InstanceConfigurationDto::from_configuration(record.configuration()),
        }
    }
}

/// The desired instance configuration as it crosses the command boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceConfigurationDto {
    pub minecraft_version: String,
    pub loader: InstanceLoaderDto,
    pub memory_mib: u32,
    pub additional_jvm_arguments: String,
    pub window: Option<InstanceWindowDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceLoaderDto {
    pub kind: String,
    pub policy: InstanceLoaderPolicyDto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceWindowDto {
    pub width: u16,
    pub height: u16,
}

impl InstanceConfigurationDto {
    fn from_configuration(
        configuration: &crate::instances::settings::InstanceConfiguration,
    ) -> Self {
        Self {
            minecraft_version: configuration.minecraft_version().to_owned(),
            loader: InstanceLoaderDto {
                kind: configuration.loader().kind().as_str().to_owned(),
                policy: match configuration.loader().policy() {
                    crate::instances::settings::LoaderPolicy::Automatic => {
                        InstanceLoaderPolicyDto::Automatic
                    }
                    crate::instances::settings::LoaderPolicy::Pinned { version } => {
                        InstanceLoaderPolicyDto::Pinned {
                            version: version.clone(),
                        }
                    }
                },
            },
            memory_mib: configuration.memory_mib(),
            additional_jvm_arguments: configuration.additional_jvm_arguments().to_owned(),
            window: configuration.window().map(|window| InstanceWindowDto {
                width: window.width(),
                height: window.height(),
            }),
        }
    }

    fn into_configuration(
        self,
    ) -> Result<crate::instances::settings::InstanceConfiguration, CommandError> {
        let kind = match self.loader.kind.as_str() {
            "fabric" => crate::instances::settings::LoaderKind::Fabric,
            other => {
                return Err(CommandError::new(
                    "instance_configuration_invalid",
                    format!("mod loader '{other}' is not supported"),
                ));
            }
        };
        let policy = (&self.loader.policy).try_into()?;
        Ok(
            crate::instances::settings::InstanceConfiguration::from_parts(
                self.minecraft_version.trim(),
                crate::instances::settings::LoaderConfiguration::from_parts(kind, policy),
                self.memory_mib,
                self.additional_jvm_arguments,
                self.window.map(|window| {
                    crate::instances::settings::WindowConfiguration::new(
                        window.width,
                        window.height,
                    )
                }),
            ),
        )
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

impl From<InvalidInstanceId> for CommandError {
    fn from(error: InvalidInstanceId) -> Self {
        Self::new("instance_id_invalid", error.to_string())
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
            InstanceRegistryError::Write(_) => "instance_registry_write_failure",
        };
        Self::new(code, error.to_string())
    }
}

impl From<crate::auth::AuthError> for CommandError {
    fn from(error: crate::auth::AuthError) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

impl From<crate::auth::AccountsError> for CommandError {
    fn from(error: crate::auth::AccountsError) -> Self {
        Self::from(crate::auth::AuthError::Accounts(error))
    }
}

impl From<crate::auth::accounts::InvalidAccount> for CommandError {
    fn from(error: crate::auth::accounts::InvalidAccount) -> Self {
        Self::from(crate::auth::AuthError::InvalidAccountId(error))
    }
}

impl From<crate::shortcuts::ShortcutError> for CommandError {
    fn from(error: crate::shortcuts::ShortcutError) -> Self {
        Self::new(error.code(), error.to_string())
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
                DownloadError::Sha1Mismatch { .. } => "artifact_hash_mismatch",
                DownloadError::ObservedDigestDrift { .. } => "fabric_artifact_unverified",
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

    LauncherState::from_parts(loaded.into_config(), registry)
}

/// Typed request for updating the launcher-wide appearance preferences.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAppearanceRequest {
    theme: String,
    accent: crate::appearance::AccentSelection,
}

/// The accent-family CSS token values for the current accent, as applied by
/// the frontend to the `--color-accent*` custom properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccentPaletteDto {
    accent: String,
    accent_strong: String,
    accent_hover: String,
    accent_pressed: String,
    accent_contrast: String,
    accent_soft: String,
    accent_outline: String,
}

impl From<&crate::appearance::AccentPalette> for AccentPaletteDto {
    fn from(palette: &crate::appearance::AccentPalette) -> Self {
        Self {
            accent: palette.base.as_hex(),
            accent_strong: palette.strong.as_hex(),
            accent_hover: palette.hover.as_hex(),
            accent_pressed: palette.pressed.as_hex(),
            accent_contrast: palette.on_accent.as_hex(),
            accent_soft: palette.soft_css(),
            accent_outline: palette.outline_css(),
        }
    }
}

/// One selectable built-in theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeOptionDto {
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

/// One curated accent preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccentOptionDto {
    id: &'static str,
    label: &'static str,
    /// The preset's base color, for the swatch preview.
    hex: String,
}

/// The launcher-wide appearance state plus the catalogs the Settings page
/// renders. Every value is validated Rust-owned state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceDto {
    theme: &'static str,
    accent: crate::appearance::AccentSelection,
    palette: AccentPaletteDto,
    themes: Vec<ThemeOptionDto>,
    accents: Vec<AccentOptionDto>,
}

impl AppearanceDto {
    fn of(preferences: &crate::appearance::AppearancePreferences) -> Result<Self, CommandError> {
        let palette = preferences
            .accent
            .palette()
            .map_err(|error| CommandError::new("appearance_invalid", error.to_string()))?;

        Ok(Self {
            theme: preferences.theme_id().as_str(),
            accent: preferences.accent.clone(),
            palette: AccentPaletteDto::from(&palette),
            themes: crate::appearance::ThemeId::all()
                .into_iter()
                .map(|theme| ThemeOptionDto {
                    id: theme.as_str(),
                    label: theme.label(),
                    description: theme.description(),
                })
                .collect(),
            accents: crate::appearance::accent_presets()
                .iter()
                .map(|preset| AccentOptionDto {
                    id: preset.id,
                    label: preset.label,
                    hex: preset.palette.base.as_hex(),
                })
                .collect(),
        })
    }
}

/// Loads the persisted launcher-wide appearance preferences.
///
/// These are cosmetic launcher preferences: unknown stored values were
/// already normalized to safe defaults at load, and this command never
/// blocks the rest of the launcher.
#[tauri::command]
pub fn get_appearance(app: AppHandle) -> Result<AppearanceDto, CommandError> {
    let managed_paths = managed_paths(&app)?;

    let config = match crate::config::load(&managed_paths.config_file())? {
        Some(config) => config,
        None => crate::config::LauncherConfig::default(),
    };

    Ok(AppearanceDto::of(config.appearance())?)
}

/// Validates and persists new launcher-wide appearance preferences.
///
/// The whole proposed appearance is validated first — an unknown theme or an
/// unusable accent color is rejected with an explanation and nothing is
/// written — then saved atomically with the rest of the launcher
/// configuration.
#[tauri::command]
pub fn set_appearance(
    app: AppHandle,
    request: SetAppearanceRequest,
) -> Result<AppearanceDto, CommandError> {
    let managed_paths = managed_paths(&app)?;

    let theme = crate::appearance::ThemeId::parse(&request.theme).ok_or_else(|| {
        CommandError::new(
            "appearance_invalid",
            format!(
                "'{}' is not a known theme; choose one of the built-in themes",
                request.theme.trim()
            ),
        )
    })?;
    request
        .accent
        .palette()
        .map_err(|error| CommandError::new("appearance_invalid", error.to_string()))?;

    let preferences = crate::appearance::AppearancePreferences {
        theme: theme.as_str().to_owned(),
        accent: request.accent,
    };

    let mut config = match crate::config::load(&managed_paths.config_file())? {
        Some(config) => config,
        None => crate::config::LauncherConfig::default(),
    };
    config.set_appearance(preferences);
    crate::config::save(&managed_paths.config_file(), &config)?;

    Ok(AppearanceDto::of(config.appearance())?)
}

/// The status of one Windows shortcut slot, as Settings renders it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutStatusDto {
    /// `present`, `absent`, `conflict`, or `unknown` (the OS state could not
    /// be queried right now).
    state: &'static str,
    /// Who owns the slot: `aurora` (launcher-managed) or `installer`
    /// (created and removed by the Aurora installers, reported read-only).
    managed_by: &'static str,
}

impl ShortcutStatusDto {
    fn for_presence(
        presence: crate::shortcuts::ShortcutPresence,
        managed_by: &'static str,
    ) -> Self {
        let state = match presence {
            crate::shortcuts::ShortcutPresence::Present => "present",
            crate::shortcuts::ShortcutPresence::Absent => "absent",
            crate::shortcuts::ShortcutPresence::Conflict => "conflict",
        };
        Self { state, managed_by }
    }

    fn unknown(managed_by: &'static str) -> Self {
        Self {
            state: "unknown",
            managed_by,
        }
    }
}

/// The Windows desktop-integration state for the Settings page: live,
/// filesystem-derived shortcut status (never persisted), whether this
/// platform supports shortcut integration at all, and whether this build
/// may manage shortcuts (installed production builds only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopIntegrationDto {
    supported: bool,
    manageable: bool,
    desktop_shortcut: ShortcutStatusDto,
    start_menu_shortcut: ShortcutStatusDto,
}

impl DesktopIntegrationDto {
    /// Queries the real OS state: known folders, current executable, and the
    /// shortcut slots inside them. Failure to resolve any input degrades to
    /// an honest `unknown` status rather than a guess.
    fn assess() -> Self {
        let (desktop_dir, programs_dir) = (
            crate::shortcuts::desktop_known_folder(),
            crate::shortcuts::programs_known_folder(),
        );
        let current_exe = std::env::current_exe().ok();
        let manageable = current_exe
            .as_ref()
            .is_some_and(|exe| crate::shortcuts::shortcut_manageable(exe, !cfg!(debug_assertions)));

        match (desktop_dir, programs_dir, current_exe) {
            (Ok(desktop), Ok(programs), Some(exe)) => {
                let context = crate::shortcuts::ShortcutContext {
                    desktop_dir: desktop,
                    programs_dir: programs,
                    // Best-effort: the machine-wide Programs folder widens
                    // Start-menu reporting to per-machine MSI installs; a
                    // resolution failure only narrows reporting.
                    common_programs_dir: crate::shortcuts::common_programs_known_folder().ok(),
                    target_exe: exe,
                };
                Self {
                    supported: true,
                    manageable,
                    desktop_shortcut: ShortcutStatusDto::for_presence(
                        context.desktop_presence(),
                        "aurora",
                    ),
                    start_menu_shortcut: ShortcutStatusDto::for_presence(
                        context.start_menu_presence(),
                        "installer",
                    ),
                }
            }
            _ => Self {
                supported: true,
                manageable,
                desktop_shortcut: ShortcutStatusDto::unknown("aurora"),
                start_menu_shortcut: ShortcutStatusDto::unknown("installer"),
            },
        }
    }

    /// The non-Windows shape: shortcut integration does not exist on this
    /// platform and nothing is claimed about any slot.
    #[cfg(not(windows))]
    fn unsupported() -> Self {
        Self {
            supported: false,
            manageable: false,
            desktop_shortcut: ShortcutStatusDto::unknown("aurora"),
            start_menu_shortcut: ShortcutStatusDto::unknown("installer"),
        }
    }
}

/// Reports the live Windows shortcut state for the Settings page.
///
/// Status is queried from the filesystem on every call — a user may delete
/// a shortcut outside Aurora, and nothing is persisted in its place.
#[tauri::command]
pub fn get_desktop_integration() -> Result<DesktopIntegrationDto, CommandError> {
    #[cfg(windows)]
    {
        Ok(DesktopIntegrationDto::assess())
    }
    #[cfg(not(windows))]
    {
        Ok(DesktopIntegrationDto::unsupported())
    }
}

/// Creates (or refreshes) the Aurora-owned desktop shortcut.
///
/// Refuses development builds and shortcuts slots Aurora cannot prove it
/// owns; conflicts surface as `shortcut_conflict` and the existing file is
/// never touched.
#[tauri::command]
pub fn create_desktop_shortcut() -> Result<DesktopIntegrationDto, CommandError> {
    #[cfg(windows)]
    {
        let context = shortcut_context_for_management()?;
        context
            .create_desktop_shortcut()
            .map_err(CommandError::from)?;
        Ok(DesktopIntegrationDto::assess())
    }
    #[cfg(not(windows))]
    {
        let _ = ShortcutStatusDto::unknown("aurora");
        Err(CommandError::from(
            crate::shortcuts::ShortcutError::UnsupportedPlatform,
        ))
    }
}

/// Removes the Aurora-owned desktop shortcut.
///
/// Only a shortcut proven to target this executable is removed; anything
/// else at the slot is a reported conflict and survives untouched.
#[tauri::command]
pub fn remove_desktop_shortcut() -> Result<DesktopIntegrationDto, CommandError> {
    #[cfg(windows)]
    {
        let context = shortcut_context_for_management()?;
        context
            .remove_desktop_shortcut()
            .map_err(CommandError::from)?;
        Ok(DesktopIntegrationDto::assess())
    }
    #[cfg(not(windows))]
    {
        let _ = ShortcutStatusDto::unknown("aurora");
        Err(CommandError::from(
            crate::shortcuts::ShortcutError::UnsupportedPlatform,
        ))
    }
}

/// Resolves the live shortcut context for a management action, applying the
/// installed-production-build gate first.
#[cfg(windows)]
fn shortcut_context_for_management() -> Result<crate::shortcuts::ShortcutContext, CommandError> {
    let current_exe = std::env::current_exe().map_err(|error| {
        CommandError::from(crate::shortcuts::ShortcutError::Io(error.to_string()))
    })?;
    if !crate::shortcuts::shortcut_manageable(&current_exe, !cfg!(debug_assertions)) {
        return Err(CommandError::from(
            crate::shortcuts::ShortcutError::ManagementUnavailable,
        ));
    }
    let desktop_dir = crate::shortcuts::desktop_known_folder().map_err(CommandError::from)?;
    let programs_dir = crate::shortcuts::programs_known_folder().map_err(CommandError::from)?;
    Ok(crate::shortcuts::ShortcutContext {
        desktop_dir,
        programs_dir,
        common_programs_dir: crate::shortcuts::common_programs_known_folder().ok(),
        target_exe: current_exe,
    })
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
/// full untrusted-staging â†’ verify â†’ promote flow natively, and returns the
/// verified object. It accepts no fixture shortcuts, so it can only be driven
/// with a real reachable source â€” deterministic pipeline verification lives
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
/// nothing â€” no client jar, library, native, asset, or runtime is downloaded.
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
/// nothing â€” no Minecraft or Fabric artifact is downloaded.
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

impl From<InstallError> for CommandError {
    fn from(error: InstallError) -> Self {
        match error {
            // Acquisition failures keep their established transport,
            // integrity, and cache codes.
            InstallError::Acquisition(acquisition) => Self::from(acquisition),
            other => {
                let code = match &other {
                    InstallError::AlreadyInProgress { .. } => "installation_already_in_progress",
                    InstallError::TargetConflict { .. } => "installation_target_conflict",
                    InstallError::State(_) => "installation_state_invalid",
                    InstallError::AssetIndexInvalid { .. } => "minecraft_asset_index_invalid",
                    InstallError::AssetInvalid { .. } => "minecraft_asset_invalid",
                    InstallError::InvalidSource(_) => "artifact_source_invalid",
                    InstallError::Materialization { .. } => "artifact_materialization_failure",
                    InstallError::Native(native) => match native {
                        crate::install::natives::NativeExtractionError::ArchiveInvalid {
                            ..
                        } => "native_archive_invalid",
                        crate::install::natives::NativeExtractionError::EntryConflict {
                            ..
                        }
                        | crate::install::natives::NativeExtractionError::Io { .. } => {
                            "native_extraction_failure"
                        }
                    },
                    InstallError::Validation { .. } => "installation_validation_failure",
                    InstallError::Commit { .. } => "installation_commit_failure",
                    InstallError::Storage(_) => "storage_io_failure",
                    InstallError::Acquisition(_) => unreachable!("handled by value above"),
                };
                Self::new(code, other.to_string())
            }
        }
    }
}

/// Typed request accepted by the installation command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallGameRequest {
    instance_id: String,
    minecraft_version: String,
    loader_version: String,
}

/// One native progress event payload; Rust owns the state, the frontend only
/// displays it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgressEvent {
    phase: &'static str,
    completed_items: u32,
    total_items: u32,
    current_item: Option<String>,
}

/// A concise summary of one committed installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledGameSummary {
    minecraft_version: String,
    loader_version: String,
    installation_id: String,
    file_count: usize,
    total_bytes: u64,
    verified_sha1_files: usize,
    verified_sha256_files: usize,
    transport_observed_files: usize,
    natives_directory: String,
    game_directory: String,
}

/// Resolves one exact Minecraft + Fabric Loader combination and installs the
/// complete, isolated game into launcher-managed instance storage.
///
/// This is the development-facing proof of the Phase 5 installation
/// executor: it performs the full resolve â†’ acquire â†’ stage â†’ validate â†’
/// commit flow natively and reports progress through `install-progress`
/// events. It installs a game, never launches one, installs no Java runtime,
/// and never touches the user's `.minecraft`.
#[tauri::command]
pub async fn install_game(
    app: AppHandle,
    request: InstallGameRequest,
) -> Result<InstalledGameSummary, CommandError> {
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let version = MinecraftVersionId::new(request.minecraft_version.trim())?;
    let loader = LoaderVersionId::new(request.loader_version.trim())?;
    let platform = crate::minecraft::rules::PlatformProfile::current()?;

    let managed_paths = managed_paths(&app)?;
    let options = crate::downloads::DownloadOptions::default();

    let plan = resolve_game_plan(
        &MetadataEndpoints::official(),
        &crate::fabric::metadata::FabricMetaEndpoints::official(),
        &version,
        &loader,
        platform,
        &options,
    )
    .await?;

    let installed = execute_install_game(
        &managed_paths,
        &instance,
        &plan,
        &InstallContext::official(),
        &mut |progress| {
            let _ = app.emit(
                "install-progress",
                InstallProgressEvent {
                    phase: progress.phase.as_str(),
                    completed_items: progress.completed_items,
                    total_items: progress.total_items,
                    current_item: progress.current_item.clone(),
                },
            );
        },
        InstallFaults::default(),
    )
    .await?;

    eprintln!(
        "[aurora-launcher] installed Minecraft {} + Fabric Loader {} into {} ({} files, {} bytes)",
        installed.manifest().minecraft_version(),
        installed.manifest().fabric_loader_version(),
        installed.game_directory().display(),
        installed.manifest().files().len(),
        installed.total_byte_count(),
    );

    let manifest = installed.manifest();
    let mut verified_sha1_files = 0usize;
    let mut verified_sha256_files = 0usize;
    let mut transport_observed_files = 0usize;
    for file in manifest.files() {
        match file.trust() {
            crate::integrity::ArtifactTrust::ExpectedDigestVerified { algorithm, .. } => {
                match algorithm {
                    crate::integrity::DigestAlgorithm::Sha1 => verified_sha1_files += 1,
                    crate::integrity::DigestAlgorithm::Sha256 => verified_sha256_files += 1,
                }
            }
            crate::integrity::ArtifactTrust::SecureTransportObserved { .. } => {
                transport_observed_files += 1;
            }
        }
    }

    Ok(InstalledGameSummary {
        minecraft_version: manifest.minecraft_version().to_owned(),
        loader_version: manifest.fabric_loader_version().to_owned(),
        installation_id: manifest.installation_id().to_owned(),
        file_count: manifest.files().len(),
        total_bytes: installed.total_byte_count(),
        verified_sha1_files,
        verified_sha256_files,
        transport_observed_files,
        natives_directory: manifest.natives().directory().to_owned(),
        game_directory: installed.game_directory().to_string_lossy().into_owned(),
    })
}

/// Typed request accepted by the installed-game validation command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateInstalledGameRequest {
    instance_id: String,
}

/// The validation outcome of one installed game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledGameValidationDto {
    status: String,
    minecraft_version: Option<String>,
    loader_version: Option<String>,
    installation_id: Option<String>,
    checked_files: usize,
    verified_bytes: u64,
    problems: Vec<ValidationProblemDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationProblemDto {
    path: String,
    reason: String,
}

/// Validates one instance's installed game against its installed-state
/// record â€” a read-only, download-free check that reports damage precisely.
#[tauri::command]
pub fn validate_installed_game(
    app: AppHandle,
    request: ValidateInstalledGameRequest,
) -> Result<InstalledGameValidationDto, CommandError> {
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let managed_paths = managed_paths(&app)?;

    match run_installed_game_validation(&managed_paths, &instance)? {
        ValidationOutcome::NotInstalled => Ok(InstalledGameValidationDto {
            status: "notInstalled".to_owned(),
            minecraft_version: None,
            loader_version: None,
            installation_id: None,
            checked_files: 0,
            verified_bytes: 0,
            problems: Vec::new(),
        }),
        ValidationOutcome::Installed(validation) => Ok(validation_dto(validation)),
    }
}

fn validation_dto(validation: InstalledGameValidation) -> InstalledGameValidationDto {
    let status = match validation.status {
        ValidationStatus::Valid => "valid",
        ValidationStatus::Damaged => "damaged",
    };
    InstalledGameValidationDto {
        status: status.to_owned(),
        minecraft_version: Some(validation.minecraft_version),
        loader_version: Some(validation.fabric_loader_version),
        installation_id: Some(validation.installation_id),
        checked_files: validation.checked_files,
        verified_bytes: validation.verified_bytes,
        problems: validation
            .problems
            .into_iter()
            .map(|problem| ValidationProblemDto {
                path: problem.path,
                reason: problem.reason,
            })
            .collect(),
    }
}

impl From<InstanceError> for CommandError {
    fn from(error: InstanceError) -> Self {
        match error {
            InstanceError::Registry(registry) => Self::from(registry),
            InstanceError::Config(config) => Self::from(config),
            InstanceError::GameInstall(install) => Self::from(install),
            InstanceError::Aurora(aurora) => Self::from(aurora),
            InstanceError::GameResolution(resolution) => Self::from(resolution),
            InstanceError::RuntimeMetadata(runtime) => Self::from(runtime),
            InstanceError::RuntimeInstall(runtime) => Self::from(runtime),
            other => {
                let code = match &other {
                    InstanceError::NotFound { .. } => "instance_not_found",
                    InstanceError::NotReady { .. } => "instance_not_ready",
                    InstanceError::NameInvalid(_) => "instance_name_invalid",
                    InstanceError::ReleaseInvalid(_) => "instance_release_invalid",
                    InstanceError::AuroraReleaseNotFound { .. } => "aurora_release_not_found",
                    InstanceError::ConfigurationInvalid(_) => "instance_configuration_invalid",
                    InstanceError::ConfigurationStale { .. } => "instance_not_ready",
                    InstanceError::ReleaseUnavailable { .. } => "instance_release_invalid",
                    InstanceError::LoaderResolution { .. } => "fabric_loader_not_found",
                    InstanceError::Platform(_) => "minecraft_platform_unsupported",
                    InstanceError::GameInstallState(_) => "installation_state_invalid",
                    InstanceError::ValidationFailed { .. } => "instance_consistency_failure",
                    InstanceError::Registry(_)
                    | InstanceError::Config(_)
                    | InstanceError::GameInstall(_)
                    | InstanceError::Aurora(_)
                    | InstanceError::GameResolution(_)
                    | InstanceError::FabricMetadata(_)
                    | InstanceError::RuntimeMetadata(_)
                    | InstanceError::RuntimeInstall(_) => {
                        unreachable!("handled by value above")
                    }
                };
                Self::new(code, other.to_string())
            }
        }
    }
}

impl From<RuntimeMetadataError> for CommandError {
    fn from(error: RuntimeMetadataError) -> Self {
        let code = match &error {
            RuntimeMetadataError::IndexNetwork(_) | RuntimeMetadataError::IndexTooLarge { .. } => {
                "runtime_metadata_network_failure"
            }
            RuntimeMetadataError::PlatformUnavailable { .. } => "runtime_platform_unsupported",
            RuntimeMetadataError::ComponentUnavailable { .. }
            | RuntimeMetadataError::AmbiguousComponent { .. } => "runtime_component_unavailable",
            RuntimeMetadataError::ManifestAcquisition(_) => "runtime_manifest_acquisition_failure",
            RuntimeMetadataError::ManifestRead(_) => "storage_io_failure",
            RuntimeMetadataError::IndexInvalid(_)
            | RuntimeMetadataError::ManifestInvalid(_)
            | RuntimeMetadataError::Plan(_) => "runtime_metadata_invalid",
        };
        let message = match &error {
            RuntimeMetadataError::IndexNetwork(_) => {
                "Official Mojang Java runtime metadata could not be reached.".to_owned()
            }
            RuntimeMetadataError::ManifestAcquisition(_) => {
                "The official Java runtime manifest could not be acquired and verified.".to_owned()
            }
            _ => error.to_string(),
        };
        eprintln!("[aurora-launcher] runtime metadata error ({code}): {error}");
        Self::new(code, message)
    }
}

impl From<RuntimeInstallError> for CommandError {
    fn from(error: RuntimeInstallError) -> Self {
        let code = match &error {
            RuntimeInstallError::AlreadyInProgress { .. } => {
                "runtime_installation_already_in_progress"
            }
            RuntimeInstallError::Plan(_) => "runtime_plan_invalid",
            RuntimeInstallError::Acquisition(_) => "runtime_artifact_acquisition_failure",
            RuntimeInstallError::State(_) => "runtime_state_invalid",
            RuntimeInstallError::TargetConflict { .. } => "runtime_target_conflict",
            RuntimeInstallError::Materialization { .. } => "runtime_materialization_failure",
            RuntimeInstallError::Validation(_) => "runtime_validation_failure",
            RuntimeInstallError::Commit(_) => "runtime_commit_failure",
            RuntimeInstallError::Storage(_) => "storage_io_failure",
        };
        let message = match &error {
            RuntimeInstallError::Acquisition(_) => {
                "A managed Java runtime file could not be acquired and verified.".to_owned()
            }
            _ => error.to_string(),
        };
        eprintln!("[aurora-launcher] runtime install error ({code}): {error}");
        Self::new(code, message)
    }
}

impl From<AuroraInstallError> for CommandError {
    fn from(error: AuroraInstallError) -> Self {
        let code = match &error {
            AuroraInstallError::ReleaseInvalid(_) => "aurora_artifact_invalid",
            AuroraInstallError::ArtifactInvalid(_) => "aurora_artifact_invalid",
            AuroraInstallError::Acquisition(_) => "aurora_artifact_invalid",
            AuroraInstallError::Materialization { .. } => "aurora_materialization_failure",
            AuroraInstallError::StateRead(_) | AuroraInstallError::State(_) => {
                "aurora_installation_invalid"
            }
            AuroraInstallError::StateWrite(_) => "aurora_installation_invalid",
        };
        Self::new(code, error.to_string())
    }
}

/// One Aurora release offered for instance creation.
///
/// `source` is always explicit: today the only operational source is the
/// checked-in development fixture, and the UI must not imply production
/// release discovery exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraReleaseSummary {
    source: &'static str,
    aurora_version: String,
    channel: ReleaseChannel,
    minecraft_version: String,
    fabric_loader_version: String,
    java_major_version: u32,
}

/// Lists the Aurora releases available for instance creation.
///
/// Resolves from the operational development release source (a checked-in
/// fixture) â€” no production Aurora release endpoint exists yet.
#[tauri::command]
pub fn list_aurora_releases() -> Result<Vec<AuroraReleaseSummary>, CommandError> {
    let manifest = crate::distribution::development_manifest()
        .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;

    Ok(manifest
        .releases()
        .iter()
        .map(|release| AuroraReleaseSummary {
            source: "development-fixture",
            aurora_version: release.aurora_version().to_owned(),
            channel: release.channel(),
            minecraft_version: release.minecraft_version().to_owned(),
            fabric_loader_version: release.fabric_loader_version().to_owned(),
            java_major_version: release.java().major_version(),
        })
        .collect())
}

/// Typed request for instance creation.
///
/// Creation asks for the essentials: a display name and the desired
/// configuration's Minecraft version and loader policy; memory, JVM
/// arguments, and window default safely.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInstanceRequest {
    display_name: String,
    minecraft_version: String,
    loader_policy: InstanceLoaderPolicyDto,
}

/// The loader policy as it crosses the command boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum InstanceLoaderPolicyDto {
    Automatic,
    Pinned { version: String },
}

impl TryFrom<&InstanceLoaderPolicyDto> for crate::instances::settings::LoaderPolicy {
    type Error = CommandError;

    fn try_from(value: &InstanceLoaderPolicyDto) -> Result<Self, CommandError> {
        match value {
            InstanceLoaderPolicyDto::Automatic => Ok(Self::Automatic),
            InstanceLoaderPolicyDto::Pinned { version } => Ok(Self::Pinned {
                version: version.trim().to_owned(),
            }),
        }
    }
}

/// Creates a complete persistent Aurora instance from a desired
/// configuration: resolve the release and loader version, install the game
/// through the Phase 5 executor, install Aurora, validate everything, and
/// mark the instance ready. Progress is reported through
/// `instance-progress` events.
#[tauri::command]
pub async fn create_instance(
    app: AppHandle,
    request: CreateInstanceRequest,
) -> Result<InstanceSummary, CommandError> {
    let managed_paths = managed_paths(&app)?;
    let endpoints = crate::instances::lifecycle::InstanceEndpoints::development()
        .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;

    let configuration = crate::instances::settings::InstanceConfiguration::from_parts(
        request.minecraft_version.trim(),
        crate::instances::settings::LoaderConfiguration::from_parts(
            crate::instances::settings::LoaderKind::Fabric,
            (&request.loader_policy).try_into()?,
        ),
        crate::instances::settings::DEFAULT_MEMORY_MIB,
        String::new(),
        None,
    );

    let record = crate::instances::lifecycle::create_instance(
        &managed_paths,
        &managed_paths.instance_registry_file(),
        &managed_paths.config_file(),
        &endpoints,
        LifecycleCreateRequest::new(request.display_name.trim(), configuration),
        &mut |progress| {
            let _ = app.emit(
                "instance-progress",
                InstanceProgressEvent {
                    phase: progress.phase.as_str(),
                    game: progress.game.map(|game| InstallProgressEvent {
                        phase: game.phase.as_str(),
                        completed_items: game.completed_items,
                        total_items: game.total_items,
                        current_item: game.current_item,
                    }),
                },
            );
        },
        InstanceFaults::default(),
    )
    .await?;

    eprintln!(
        "[aurora-launcher] created instance '{}' ({} {} for Minecraft {} + Fabric Loader {})",
        record.id(),
        record.release().channel(),
        record.release().aurora_version(),
        record.release().minecraft_version(),
        record.release().fabric_loader_version(),
    );

    Ok(InstanceSummary::from_record(&record))
}

/// One lifecycle progress event payload; the game installer's item progress
/// is embedded verbatim during the game-installation phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceProgressEvent {
    phase: &'static str,
    game: Option<InstallProgressEvent>,
}

/// Typed request for retrying an unfinished instance installation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryInstanceRequest {
    instance_id: String,
}

/// Retries the installation of an instance whose creation did not finish.
#[tauri::command]
pub async fn retry_instance_install(
    app: AppHandle,
    request: RetryInstanceRequest,
) -> Result<InstanceSummary, CommandError> {
    let managed_paths = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let endpoints = crate::instances::lifecycle::InstanceEndpoints::development()
        .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;

    let record = crate::instances::lifecycle::retry_instance_install(
        &managed_paths,
        &managed_paths.instance_registry_file(),
        &managed_paths.config_file(),
        &endpoints,
        &instance,
        &mut |progress| {
            let _ = app.emit(
                "instance-progress",
                InstanceProgressEvent {
                    phase: progress.phase.as_str(),
                    game: progress.game.map(|game| InstallProgressEvent {
                        phase: game.phase.as_str(),
                        completed_items: game.completed_items,
                        total_items: game.total_items,
                        current_item: game.current_item,
                    }),
                },
            );
        },
        InstanceFaults::default(),
    )
    .await?;

    Ok(InstanceSummary::from_record(&record))
}

/// Typed request for renaming an instance.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameInstanceRequest {
    instance_id: String,
    new_display_name: String,
}

/// Renames an instance's display name. Metadata only â€” identifiers and
/// filesystem paths are untouched.
#[tauri::command]
pub fn rename_instance(
    app: AppHandle,
    request: RenameInstanceRequest,
) -> Result<InstanceSummary, CommandError> {
    let managed_paths = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;

    let record = crate::instances::lifecycle::rename_instance(
        &managed_paths.instance_registry_file(),
        &instance,
        request.new_display_name.trim(),
    )?;

    Ok(InstanceSummary::from_record(&record))
}

/// Typed request for atomically persisting a new desired configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstanceConfigurationRequest {
    instance_id: String,
    configuration: InstanceConfigurationDto,
}

/// Atomically persists a new desired configuration for one ready instance.
///
/// The persisted change is metadata only; installed content, identifiers,
/// and paths are untouched. Readiness reflects the change immediately — deep
/// validation reports a stale instance until its new configuration is
/// installed through `install_instance_configuration`.
#[tauri::command]
pub fn update_instance_configuration(
    app: AppHandle,
    request: UpdateInstanceConfigurationRequest,
) -> Result<InstanceSummary, CommandError> {
    let managed_paths = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let endpoints = crate::instances::lifecycle::InstanceEndpoints::development()
        .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;
    let configuration = request.configuration.into_configuration()?;

    let record = crate::instances::lifecycle::update_instance_configuration(
        &managed_paths.instance_registry_file(),
        &endpoints,
        &instance,
        configuration,
    )
    .map_err(|error| match error {
        crate::instances::lifecycle::InstanceError::ConfigurationInvalid(reason) => {
            CommandError::new("instance_configuration_invalid", reason.to_string())
        }
        crate::instances::lifecycle::InstanceError::ReleaseUnavailable { .. } => {
            CommandError::new("instance_release_invalid", error.to_string())
        }
        other => CommandError::from(other),
    })?;

    Ok(InstanceSummary::from_record(&record))
}

/// Resolves an instance's desired configuration into a new release pin and
/// (re)installs the content to match it — the deliberate path for
/// install-affecting configuration changes. Progress is reported through
/// `instance-progress` events.
#[tauri::command]
pub async fn install_instance_configuration(
    app: AppHandle,
    request: RetryInstanceRequest,
) -> Result<InstanceSummary, CommandError> {
    let managed_paths = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let endpoints = crate::instances::lifecycle::InstanceEndpoints::development()
        .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;

    let record = crate::instances::lifecycle::install_instance_configuration(
        &managed_paths,
        &managed_paths.instance_registry_file(),
        &managed_paths.config_file(),
        &endpoints,
        &instance,
        &mut |progress| {
            let _ = app.emit(
                "instance-progress",
                InstanceProgressEvent {
                    phase: progress.phase.as_str(),
                    game: progress.game.map(|game| InstallProgressEvent {
                        phase: game.phase.as_str(),
                        completed_items: game.completed_items,
                        total_items: game.total_items,
                        current_item: game.current_item,
                    }),
                },
            );
        },
        InstanceFaults::default(),
    )
    .await
    .map_err(|error| match error {
        crate::instances::lifecycle::InstanceError::ConfigurationInvalid(reason) => {
            CommandError::new("instance_configuration_invalid", reason.to_string())
        }
        crate::instances::lifecycle::InstanceError::ReleaseUnavailable { .. } => {
            CommandError::new("instance_release_invalid", error.to_string())
        }
        crate::instances::lifecycle::InstanceError::LoaderResolution { .. } => {
            CommandError::new("fabric_loader_not_found", error.to_string())
        }
        other => CommandError::from(other),
    })?;

    Ok(InstanceSummary::from_record(&record))
}

/// One Minecraft version offered for instance configuration, from the
/// official Mojang manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinecraftVersionDto {
    id: String,
    version_type: String,
}

/// One Fabric Loader version available for a Minecraft version, from the
/// official Fabric Meta per-game listing (newest first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricLoaderVersionDto {
    version: String,
    stable: bool,
}

/// Lists the Minecraft versions available for instance configuration.
///
/// Modern releases and snapshots are served from the pinned official Mojang
/// manifest, newest first. Historical types are absent because the launcher
/// deliberately does not support them. `include_snapshots` is an explicit
/// filter: snapshots never mix into the ordinary release list.
#[tauri::command]
pub async fn list_minecraft_versions(
    include_snapshots: bool,
) -> Result<Vec<MinecraftVersionDto>, CommandError> {
    let manifest = crate::minecraft::metadata::fetch_manifest(
        &MetadataEndpoints::official(),
        &crate::downloads::DownloadOptions::default(),
    )
    .await
    .map_err(|error| CommandError::from(MinecraftResolutionError::Metadata(error)))?;

    Ok(manifest
        .versions
        .iter()
        .filter(|entry| {
            matches!(
                (&entry.kind, include_snapshots),
                (crate::minecraft::metadata::VersionType::Release, _)
                    | (crate::minecraft::metadata::VersionType::Snapshot, true)
            )
        })
        .map(|entry| MinecraftVersionDto {
            id: entry.id.clone(),
            version_type: entry.kind.as_mojang_str().to_owned(),
        })
        .collect())
}

/// Lists the Fabric Loader versions available for one exact Minecraft
/// version, newest first, with the official `stable` markers. An
/// unsupported Minecraft version is Fabric Meta's own answer and surfaces as
/// `fabric_combination_unsupported`.
#[tauri::command]
pub async fn list_fabric_loader_versions(
    minecraft_version: String,
) -> Result<Vec<FabricLoaderVersionDto>, CommandError> {
    let version = MinecraftVersionId::new(minecraft_version.trim())?;
    let entries = crate::fabric::metadata::fetch_game_loader_versions(
        &crate::fabric::metadata::FabricMetaEndpoints::official(),
        &version,
        &crate::downloads::DownloadOptions::default(),
    )
    .await
    .map_err(FabricMetadataError::from)?;

    Ok(entries
        .into_iter()
        .map(|entry| FabricLoaderVersionDto {
            version: entry.version,
            stable: entry.stable,
        })
        .collect())
}

impl From<FabricMetadataError> for CommandError {
    fn from(error: FabricMetadataError) -> Self {
        // Mirrors the FabricResolutionError mapping for the same conditions.
        match &error {
            FabricMetadataError::HttpStatus { status: 400 } => {
                Self::new("fabric_combination_unsupported", error.to_string())
            }
            _ => crate::instances::lifecycle::InstanceError::FabricMetadata(error).into(),
        }
    }
}

/// Typed request for selecting an instance.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectInstanceRequest {
    instance_id: String,
}

/// Selects an instance. The selection always refers to a registered
/// instance; a dangling selection is never written.
#[tauri::command]
pub fn select_instance(app: AppHandle, request: SelectInstanceRequest) -> Result<(), CommandError> {
    let managed_paths = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;

    crate::instances::lifecycle::select_instance(
        &managed_paths.instance_registry_file(),
        &managed_paths.config_file(),
        &instance,
    )?;

    Ok(())
}

/// The complete validation outcome of one instance, as reported by the
/// read-only deep validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceValidationDto {
    instance_id: String,
    display_name: String,
    status: String,
    problems: Vec<InstanceProblemDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceProblemDto {
    component: String,
    reason: String,
}

/// Validates one instance completely â€” a read-only, download-free check
/// covering the game installation, the Aurora installation and artifact
/// bytes, and cross-component version consistency.
#[tauri::command]
pub fn validate_instance(
    app: AppHandle,
    request: ValidateInstalledGameRequest,
) -> Result<InstanceValidationDto, CommandError> {
    let managed_paths = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let registry = InstanceRegistry::load(&managed_paths.instance_registry_file())?;

    let validation =
        crate::instances::lifecycle::validate_instance(&managed_paths, &registry, &instance)?;

    Ok(InstanceValidationDto {
        instance_id: validation.instance_id,
        display_name: validation.display_name,
        status: validation.status.as_str().to_owned(),
        problems: validation
            .problems
            .into_iter()
            .map(|problem| InstanceProblemDto {
                component: problem.component.to_owned(),
                reason: problem.reason,
            })
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatusDto {
    instance_id: String,
    content_status: &'static str,
    status: String,
    component: String,
    required_major_version: u32,
    runtime_version: Option<String>,
    runtime_root: String,
    launch_executable: Option<String>,
    checked_files: u32,
    verified_bytes: u64,
    reported_major_version: Option<u32>,
    diagnostic_summary: Option<String>,
    problems: Vec<String>,
    reused: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProgressEvent {
    phase: &'static str,
    completed_items: u32,
    total_items: u32,
    current_item: Option<String>,
}

fn runtime_status_dto(instance_id: &str, validation: RuntimeValidation) -> RuntimeStatusDto {
    RuntimeStatusDto {
        instance_id: instance_id.to_owned(),
        content_status: "ready",
        status: validation.status.as_str().to_owned(),
        component: validation.component,
        required_major_version: validation.required_major_version,
        runtime_version: validation.runtime_version,
        runtime_root: validation.root.to_string_lossy().into_owned(),
        launch_executable: validation
            .launch_executable
            .map(|path| path.to_string_lossy().into_owned()),
        checked_files: validation.checked_files,
        verified_bytes: validation.verified_bytes,
        reported_major_version: validation
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.reported_major_version),
        diagnostic_summary: validation.diagnostic.map(|diagnostic| diagnostic.summary),
        problems: validation.problems,
        reused: None,
    }
}

/// Resolves and deeply validates the exact managed runtime for one
/// content-ready instance. Validation is read-only; runtime absence is
/// reported independently and never damages the instance's content status.
#[tauri::command]
pub async fn get_instance_runtime_status(
    app: AppHandle,
    request: ValidateInstalledGameRequest,
) -> Result<RuntimeStatusDto, CommandError> {
    let managed = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let endpoints = crate::instances::lifecycle::InstanceEndpoints::development()
        .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;
    let runtime_endpoints = crate::runtime::metadata::RuntimeMetadataEndpoints::official();
    let validation = crate::instances::lifecycle::validate_instance_runtime(
        &managed,
        &managed.instance_registry_file(),
        &endpoints,
        &runtime_endpoints,
        &instance,
        true,
    )
    .await?;
    Ok(runtime_status_dto(instance.as_str(), validation))
}

/// Acquires, installs, validates, and executes the exact official runtime for
/// one content-ready instance. Shared exact runtimes are reused.
#[tauri::command]
pub async fn ensure_instance_runtime(
    app: AppHandle,
    request: ValidateInstalledGameRequest,
) -> Result<RuntimeStatusDto, CommandError> {
    let managed = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let endpoints = crate::instances::lifecycle::InstanceEndpoints::development()
        .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;
    let runtime_endpoints = crate::runtime::metadata::RuntimeMetadataEndpoints::official();
    let installed = crate::instances::lifecycle::ensure_instance_runtime(
        &managed,
        &managed.instance_registry_file(),
        &endpoints,
        &runtime_endpoints,
        &instance,
        &mut |progress| {
            let _ = app.emit(
                "runtime-progress",
                RuntimeProgressEvent {
                    phase: progress.phase.as_str(),
                    completed_items: progress.completed_items,
                    total_items: progress.total_items,
                    current_item: progress.current_item,
                },
            );
        },
    )
    .await?;
    let state = installed.state();
    let checked_files = state
        .entries()
        .iter()
        .filter(|entry| entry.kind() == crate::runtime::state::RuntimeStateEntryKind::File)
        .count() as u32;
    let verified_bytes = state
        .entries()
        .iter()
        .filter_map(|entry| entry.size_bytes())
        .sum();
    Ok(RuntimeStatusDto {
        instance_id: instance.to_string(),
        content_status: "ready",
        status: "ready".to_owned(),
        component: state.component().to_owned(),
        required_major_version: state.required_major_version(),
        runtime_version: Some(state.runtime_version().to_owned()),
        runtime_root: installed.root().to_string_lossy().into_owned(),
        launch_executable: Some(installed.launch_executable().to_string_lossy().into_owned()),
        checked_files,
        verified_bytes,
        reported_major_version: Some(state.required_major_version()),
        diagnostic_summary: Some(format!(
            "managed Java {} executed successfully",
            state.runtime_version()
        )),
        problems: Vec::new(),
        reused: Some(installed.reused()),
    })
}

// ---------------------------------------------------------------------------
// Accounts (Phase 8)
// ---------------------------------------------------------------------------

/// One non-secret account summary for the UI. Status is derived honestly:
/// `signedIn` means a stored credential exists; whether it still refreshes
/// is answered by `refresh_account_session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummaryDto {
    account_id: String,
    minecraft_name: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsStateDto {
    accounts: Vec<AccountSummaryDto>,
    selected_account_id: Option<String>,
}

/// Coarse sign-in progress; never carries data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProgressEvent {
    phase: &'static str,
}

/// The outcome of an on-demand session restoration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSessionDto {
    account_id: String,
    minecraft_name: String,
    status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectAccountRequest {
    account_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountIdRequest {
    account_id: String,
}

/// Lists the launcher's accounts with derived status and the selection.
#[tauri::command]
pub fn get_accounts(app: AppHandle) -> Result<AccountsStateDto, CommandError> {
    let managed = managed_paths(&app)?;
    let store = crate::auth::OsCredentialStore;
    let document = crate::auth::accounts::load(&managed.accounts_file())?;

    let accounts = document
        .accounts()
        .iter()
        .map(|record| AccountSummaryDto {
            account_id: record.account_id().to_owned(),
            minecraft_name: record.minecraft_name().to_owned(),
            status: crate::auth::flow::AccountStatus::of(&store, record.account_id())
                .as_str()
                .to_owned(),
        })
        .collect();

    Ok(AccountsStateDto {
        accounts,
        selected_account_id: document.selected_account_id().map(str::to_owned),
    })
}

/// Starts Microsoft sign-in in the system browser and completes the whole
/// authentication chain. The command stays in flight (bounded by the login
/// timeout) while the user signs in; `cancel_microsoft_login` aborts it.
///
/// Requires Aurora's real Microsoft application registration (the committed
/// public client ID, optionally overridden through the
/// `AURORA_MICROSOFT_CLIENT_ID` build-environment variable); without it the
/// command fails deliberately instead of pretending.
#[tauri::command]
pub async fn begin_microsoft_login(app: AppHandle) -> Result<AccountSummaryDto, CommandError> {
    let managed = managed_paths(&app)?;
    let registration = crate::auth::flow::production_registration()
        .ok_or_else(|| CommandError::from(crate::auth::AuthError::ConfigurationMissing))?;

    let endpoints = crate::auth::AuthEndpoints::official();
    let store = crate::auth::OsCredentialStore;
    let context = crate::auth::flow::AuthContext {
        endpoints: &endpoints,
        oauth: registration,
        credentials: &store,
        accounts_path: &managed.accounts_file(),
        options: crate::auth::flow::AuthFlowOptions::default(),
        browser: &|url: &str| {
            // The official Tauri opener: the user's default system browser,
            // exactly the public-client model — no embedded webview.
            tauri_plugin_opener::open_url(url, None::<&str>)
                .map_err(|error| crate::auth::flow::BrowserOpenError::new(error.to_string()))
        },
    };

    let record = crate::auth::flow::sign_in(&context, &mut |phase| {
        let _ = app.emit(
            "auth-progress",
            AuthProgressEvent {
                phase: phase.as_str(),
            },
        );
    })
    .await?;

    Ok(AccountSummaryDto {
        account_id: record.account_id().to_owned(),
        minecraft_name: record.minecraft_name().to_owned(),
        status: "signedIn".to_owned(),
    })
}

/// Cancels the active sign-in, if one exists. Idempotent.
#[tauri::command]
pub fn cancel_microsoft_login() -> Result<(), CommandError> {
    crate::auth::flow::cancel_active_login();
    Ok(())
}

/// Selects an account. The selection always refers to a known account; a
/// dangling selection is never written.
#[tauri::command]
pub fn select_account(app: AppHandle, request: SelectAccountRequest) -> Result<(), CommandError> {
    let managed = managed_paths(&app)?;
    let account_id = request.account_id.trim().to_owned();
    crate::auth::accounts::AccountId::validate(&account_id)?;

    let mut document = crate::auth::accounts::load(&managed.accounts_file())?;
    if document.find(&account_id).is_none() {
        return Err(CommandError::from(
            crate::auth::AuthError::AccountNotFound { account_id },
        ));
    }
    document
        .set_selected_account_id(Some(&account_id))
        .map_err(CommandError::from)?;
    crate::auth::accounts::save(&managed.accounts_file(), &document)?;

    Ok(())
}

/// Removes one account from Aurora: deletes the persisted credential,
/// the non-secret record, and the cached session. Local removal only — the
/// Microsoft account session itself is not revoked.
#[tauri::command]
pub fn remove_account(app: AppHandle, request: AccountIdRequest) -> Result<(), CommandError> {
    let managed = managed_paths(&app)?;
    let account_id = request.account_id.trim().to_owned();
    crate::auth::accounts::AccountId::validate(&account_id)?;

    let endpoints = crate::auth::AuthEndpoints::official();
    let store = crate::auth::OsCredentialStore;
    let context = crate::auth::flow::AuthContext {
        endpoints: &endpoints,
        oauth: crate::auth::OAuthClientConfig::new("unused-by-sign-out"),
        credentials: &store,
        accounts_path: &managed.accounts_file(),
        options: crate::auth::flow::AuthFlowOptions::default(),
        browser: &|_url: &str| Ok(()),
    };

    crate::auth::flow::sign_out(&context, &account_id)?;

    Ok(())
}

/// Restores or refreshes one account's session on demand and reports the
/// non-secret outcome. The Minecraft token itself stays in Rust.
#[tauri::command]
pub async fn refresh_account_session(
    app: AppHandle,
    request: AccountIdRequest,
) -> Result<AccountSessionDto, CommandError> {
    let managed = managed_paths(&app)?;
    let account_id = request.account_id.trim().to_owned();
    crate::auth::accounts::AccountId::validate(&account_id)?;

    let registration = crate::auth::flow::production_registration()
        .ok_or_else(|| CommandError::from(crate::auth::AuthError::ConfigurationMissing))?;

    let endpoints = crate::auth::AuthEndpoints::official();
    let store = crate::auth::OsCredentialStore;
    let context = crate::auth::flow::AuthContext {
        endpoints: &endpoints,
        oauth: registration,
        credentials: &store,
        accounts_path: &managed.accounts_file(),
        options: crate::auth::flow::AuthFlowOptions::default(),
        browser: &|_url: &str| Ok(()),
    };

    let session = crate::auth::flow::ensure_session(&context, &account_id, &mut |phase| {
        let _ = app.emit(
            "auth-progress",
            AuthProgressEvent {
                phase: phase.as_str(),
            },
        );
    })
    .await?;

    Ok(AccountSessionDto {
        account_id: session.account_id().to_owned(),
        minecraft_name: session.profile().name().to_owned(),
        status: "ready".to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Launch (Phase 9)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchReadinessRequest {
    instance_id: String,
    account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayRequest {
    instance_id: String,
    account_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayBlockerDto {
    code: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayReadinessDto {
    ready: bool,
    instance_id: String,
    account_id: Option<String>,
    account_name: Option<String>,
    instance_status: String,
    runtime_status: String,
    account_status: String,
    process_status: String,
    blockers: Vec<PlayBlockerDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProcessDto {
    instance_id: String,
    status: String,
    process_id: Option<u32>,
    started_at_unix_seconds: Option<u64>,
    exit_code: Option<i32>,
    message: Option<String>,
}

impl From<crate::launch::process::ProcessSnapshot> for LaunchProcessDto {
    fn from(snapshot: crate::launch::process::ProcessSnapshot) -> Self {
        Self {
            instance_id: snapshot.instance_id,
            status: snapshot.status.as_str().to_owned(),
            process_id: snapshot.process_id,
            started_at_unix_seconds: snapshot.started_at_unix_seconds,
            exit_code: snapshot.exit_code,
            message: snapshot.message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProgressEvent {
    phase: &'static str,
}

impl From<crate::launch::resolve::LaunchResolveError> for CommandError {
    fn from(error: crate::launch::resolve::LaunchResolveError) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

impl From<crate::launch::process::LaunchProcessError> for CommandError {
    fn from(error: crate::launch::process::LaunchProcessError) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

fn emit_launch_phase(app: &AppHandle, phase: &'static str) {
    let _ = app.emit("launch-progress", LaunchProgressEvent { phase });
}

fn launch_plan_error(error: InstanceError) -> CommandError {
    match error {
        InstanceError::NotFound { .. } | InstanceError::NotReady { .. } => {
            CommandError::new("launch_instance_not_ready", error.to_string())
        }
        InstanceError::GameInstallState(_)
        | InstanceError::ValidationFailed { .. }
        | InstanceError::GameInstall(_)
        | InstanceError::Aurora(_) => {
            CommandError::new("launch_instance_damaged", error.to_string())
        }
        InstanceError::RuntimeMetadata(_) | InstanceError::RuntimeInstall(_) => {
            CommandError::new("launch_runtime_not_ready", error.to_string())
        }
        InstanceError::ReleaseInvalid(_)
        | InstanceError::AuroraReleaseNotFound { .. }
        | InstanceError::GameResolution(_)
        | InstanceError::Platform(_) => CommandError::new(
            "launch_metadata_invalid",
            "The exact pinned launch metadata could not be resolved.",
        ),
        other => CommandError::from(other),
    }
}

fn launch_account_status(
    managed: &ManagedPaths,
    account_id: Option<&str>,
) -> Result<(crate::launch::state::LaunchAccountStatus, Option<String>), CommandError> {
    use crate::auth::credentials::CredentialStore as _;
    use crate::launch::state::LaunchAccountStatus;

    let Some(account_id) = account_id.filter(|value| !value.trim().is_empty()) else {
        return Ok((LaunchAccountStatus::Missing, None));
    };
    crate::auth::accounts::AccountId::validate(account_id)?;
    let accounts = crate::auth::accounts::load(&managed.accounts_file())?;
    let Some(record) = accounts.find(account_id) else {
        return Ok((LaunchAccountStatus::Missing, None));
    };
    let name = Some(record.minecraft_name().to_owned());
    if crate::auth::session::SessionCache::usable(account_id).is_some() {
        return Ok((LaunchAccountStatus::Ready, name));
    }
    let store = crate::auth::OsCredentialStore;
    if store
        .load(account_id)
        .map_err(|error| CommandError::from(crate::auth::AuthError::CredentialStore(error)))?
        .is_none()
    {
        return Ok((LaunchAccountStatus::ReauthenticationRequired, name));
    }
    if crate::auth::flow::production_registration().is_none() {
        return Ok((LaunchAccountStatus::ConfigurationMissing, name));
    }
    Ok((LaunchAccountStatus::Ready, name))
}

async fn calculate_play_readiness(
    managed: &ManagedPaths,
    instance_id: &crate::instances::InstanceId,
    account_id: Option<&str>,
) -> Result<PlayReadinessDto, CommandError> {
    use crate::instances::InstanceState;
    use crate::launch::state::{LaunchInstanceStatus, LaunchRuntimeStatus, PlayReadiness};
    use crate::runtime::install::RuntimeValidationStatus;

    let process = crate::launch::process::snapshot(instance_id.as_str());
    let registry = InstanceRegistry::load(&managed.instance_registry_file())?;
    let (instance, runtime_plan) = match registry.find(instance_id) {
        None => (LaunchInstanceStatus::Missing, None),
        Some(record) if record.state() == InstanceState::Installing => {
            (LaunchInstanceStatus::Installing, None)
        }
        Some(record)
            if !record.configuration().matches_release_pin(
                record.release().minecraft_version(),
                record.release().fabric_loader_version(),
            ) =>
        {
            // The desired configuration no longer matches the installed
            // content. Stale is distinct from damaged: nothing is wrong with
            // the installed bytes, they are simply no longer what the user
            // wants. Installing the new configuration restores launchability.
            (LaunchInstanceStatus::Stale, None)
        }
        Some(_) => {
            // This one lifecycle call performs deep content validation and,
            // only after it passes, one exact metadata resolution pass.
            // Readiness does not hash the instance twice.
            let endpoints = crate::instances::lifecycle::InstanceEndpoints::development()
                .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;
            let runtime_endpoints = crate::runtime::metadata::RuntimeMetadataEndpoints::official();
            match crate::instances::lifecycle::resolve_instance_launch_plans(
                managed,
                &managed.instance_registry_file(),
                &endpoints,
                &runtime_endpoints,
                instance_id,
            )
            .await
            {
                Ok((_, runtime_plan)) => (LaunchInstanceStatus::Ready, Some(runtime_plan)),
                Err(
                    InstanceError::NotReady { .. }
                    | InstanceError::GameInstallState(_)
                    | InstanceError::ValidationFailed { .. }
                    | InstanceError::GameInstall(_)
                    | InstanceError::Aurora(_),
                ) => (LaunchInstanceStatus::Damaged, None),
                Err(_) => (LaunchInstanceStatus::Ready, None),
            }
        }
    };

    let runtime = if let Some(plan) = runtime_plan {
        match crate::runtime::install::validate_runtime(
            managed,
            &plan,
            false,
            crate::runtime::install::DEFAULT_JAVA_DIAGNOSTIC_TIMEOUT,
        )
        .await?
        .status
        {
            RuntimeValidationStatus::Ready => LaunchRuntimeStatus::Ready,
            RuntimeValidationStatus::Missing => LaunchRuntimeStatus::Missing,
            RuntimeValidationStatus::Damaged => LaunchRuntimeStatus::Damaged,
        }
    } else {
        LaunchRuntimeStatus::Unresolved
    };

    let (account, account_name) = launch_account_status(managed, account_id)?;
    let readiness = PlayReadiness::evaluate(instance, runtime, account, process.status);
    Ok(PlayReadinessDto {
        ready: readiness.ready(),
        instance_id: instance_id.to_string(),
        account_id: account_id.map(str::to_owned),
        account_name,
        instance_status: readiness.instance.as_str().to_owned(),
        runtime_status: readiness.runtime.as_str().to_owned(),
        account_status: readiness.account.as_str().to_owned(),
        process_status: readiness.process.as_str().to_owned(),
        blockers: readiness
            .blockers
            .into_iter()
            .map(|blocker| PlayBlockerDto {
                code: blocker.code.to_owned(),
                message: blocker.message,
            })
            .collect(),
    })
}

/// Returns the complete Rust-owned, non-secret Play decision for the
/// selected instance/account pair.
#[tauri::command]
pub async fn get_play_readiness(
    app: AppHandle,
    request: LaunchReadinessRequest,
) -> Result<PlayReadinessDto, CommandError> {
    let managed = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    calculate_play_readiness(&managed, &instance, request.account_id.as_deref()).await
}

/// Returns process-local state only. It never reconstructs process truth by
/// name and never claims a process survived a launcher restart.
#[tauri::command]
pub fn get_launch_state(
    request: ValidateInstalledGameRequest,
) -> Result<LaunchProcessDto, CommandError> {
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    Ok(crate::launch::process::snapshot(instance.as_str()).into())
}

/// Checks all preconditions, obtains a Rust-only session, assembles the exact
/// argument vector, and starts supervised Minecraft without a shell.
#[tauri::command]
pub async fn play_instance(
    app: AppHandle,
    request: PlayRequest,
) -> Result<LaunchProcessDto, CommandError> {
    use crate::runtime::install::RuntimeValidationStatus;
    use std::sync::Arc;

    let managed = managed_paths(&app)?;
    let instance = crate::instances::InstanceId::new(request.instance_id.trim())?;
    let account_id = request.account_id.trim().to_owned();
    crate::auth::accounts::AccountId::validate(&account_id)?;

    emit_launch_phase(&app, "checkingPreconditions");
    if crate::launch::process::snapshot(instance.as_str())
        .status
        .blocks_launch()
    {
        return Err(CommandError::new(
            "launch_already_running",
            "This instance is already starting or running.",
        ));
    }

    emit_launch_phase(&app, "resolvingLaunch");
    let endpoints = crate::instances::lifecycle::InstanceEndpoints::development()
        .map_err(|error| CommandError::new("aurora_manifest_invalid", error.to_string()))?;
    let runtime_endpoints = crate::runtime::metadata::RuntimeMetadataEndpoints::official();
    let (game_plan, runtime_plan) = crate::instances::lifecycle::resolve_instance_launch_plans(
        &managed,
        &managed.instance_registry_file(),
        &endpoints,
        &runtime_endpoints,
        &instance,
    )
    .await
    .map_err(launch_plan_error)?;

    // The desired configuration shapes the process: memory, additional JVM
    // arguments, and the windowed resolution. Stale configuration never
    // reaches this point — resolve_instance_launch_plans performs the deep
    // validation that reports staleness as NotReady.
    let registry = InstanceRegistry::load(&managed.instance_registry_file())?;
    let record = registry
        .find(&instance)
        .ok_or_else(|| {
            CommandError::new("launch_instance_not_ready", "The instance does not exist.")
        })?
        .clone();
    let configuration = record.configuration();
    configuration
        .validate()
        .map_err(|error| CommandError::new("launch_metadata_invalid", error.to_string()))?;
    let additional_jvm_arguments =
        crate::instances::settings::parse_jvm_arguments(configuration.additional_jvm_arguments())
            .map_err(|error| CommandError::new("launch_jvm_arguments_invalid", error.to_string()))?;
    let launch_options = crate::launch::resolve::LaunchOptions::new(
        configuration.memory_mib(),
        additional_jvm_arguments,
        configuration
            .window()
            .map(|window| (window.width(), window.height())),
    );
    let features = launch_options.feature_profile();

    let runtime = crate::runtime::install::validate_runtime(
        &managed,
        &runtime_plan,
        true,
        crate::runtime::install::DEFAULT_JAVA_DIAGNOSTIC_TIMEOUT,
    )
    .await?;
    if runtime.status != RuntimeValidationStatus::Ready {
        return Err(CommandError::new(
            "launch_runtime_not_ready",
            format!(
                "The exact managed Java runtime is {}.",
                runtime.status.as_str()
            ),
        ));
    }
    let java_executable = runtime.launch_executable.ok_or_else(|| {
        CommandError::new(
            "launch_runtime_not_ready",
            "The validated runtime has no launch executable.",
        )
    })?;

    emit_launch_phase(&app, "restoringSession");
    let session = if let Some(session) = crate::auth::session::SessionCache::usable(&account_id) {
        session
    } else {
        let registration = crate::auth::flow::production_registration().ok_or_else(|| {
            CommandError::new(
                "launch_authentication_required",
                "Aurora's Microsoft application registration is not configured.",
            )
        })?;
        let auth_endpoints = crate::auth::AuthEndpoints::official();
        let store = crate::auth::OsCredentialStore;
        let context = crate::auth::flow::AuthContext {
            endpoints: &auth_endpoints,
            oauth: registration,
            credentials: &store,
            accounts_path: &managed.accounts_file(),
            options: crate::auth::flow::AuthFlowOptions::default(),
            browser: &|_url: &str| Ok(()),
        };
        crate::auth::flow::ensure_session(&context, &account_id, &mut |phase| {
            let _ = app.emit(
                "auth-progress",
                AuthProgressEvent {
                    phase: phase.as_str(),
                },
            );
        })
        .await
        .map_err(|error| match error {
            crate::auth::AuthError::ConfigurationMissing
            | crate::auth::AuthError::ReauthenticationRequired { .. }
            | crate::auth::AuthError::AccountNotFound { .. } => {
                CommandError::new("launch_authentication_required", error.to_string())
            }
            other => CommandError::new("launch_session_unavailable", other.to_string()),
        })?
    };

    emit_launch_phase(&app, "assemblingArguments");
    let game_root = managed.instance_paths(&instance).game().to_path_buf();
    let installed = crate::install::state::load_installed_state(&game_root)
        .map_err(|error| CommandError::new("launch_instance_damaged", error.to_string()))?
        .ok_or_else(|| {
            CommandError::new(
                "launch_instance_not_ready",
                "The instance has no complete installed-game state.",
            )
        })?;
    let launch_plan = crate::launch::resolve::LaunchPlan::from_game_plan(
        &game_plan,
        crate::minecraft::rules::PlatformProfile::current()?,
    );
    let spec = crate::launch::resolve::resolve_launch_spec(
        &managed,
        &instance,
        &launch_plan,
        &installed,
        &java_executable,
        &session,
        &features,
        &launch_options,
    )?;

    emit_launch_phase(&app, "startingProcess");
    let listener_app = app.clone();
    let snapshot = crate::launch::process::spawn_supervised(
        spec,
        managed.instance_paths(&instance).logs(),
        Arc::new(move |snapshot| {
            let _ = listener_app.emit("launch-state", LaunchProcessDto::from(snapshot));
        }),
    )
    .await?;
    Ok(snapshot.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_failures_map_to_stable_machine_codes() {
        let resolve_cases = [
            (
                crate::launch::resolve::LaunchResolveError::Metadata("fixture".to_owned()),
                "launch_metadata_invalid",
            ),
            (
                crate::launch::resolve::LaunchResolveError::Placeholder("fixture".to_owned()),
                "launch_placeholder_unresolved",
            ),
            (
                crate::launch::resolve::LaunchResolveError::Classpath("fixture".to_owned()),
                "launch_classpath_invalid",
            ),
            (
                crate::launch::resolve::LaunchResolveError::Natives("fixture".to_owned()),
                "launch_native_path_invalid",
            ),
            (
                crate::launch::resolve::LaunchResolveError::Logging("fixture".to_owned()),
                "launch_logging_invalid",
            ),
        ];
        for (error, code) in resolve_cases {
            assert_eq!(CommandError::from(error).code, code);
        }
        assert_eq!(
            CommandError::from(crate::launch::process::LaunchProcessError::AlreadyRunning {
                instance_id: "fixture".to_owned(),
            })
            .code,
            "launch_already_running"
        );
        assert_eq!(
            launch_plan_error(InstanceError::NotFound {
                instance_id: "fixture".to_owned(),
            })
            .code,
            "launch_instance_not_ready"
        );
    }

    #[test]
    fn production_launch_dtos_have_no_secret_or_argument_fields() {
        let readiness = PlayReadinessDto {
            ready: false,
            instance_id: "fixture".to_owned(),
            account_id: None,
            account_name: None,
            instance_status: "ready".to_owned(),
            runtime_status: "ready".to_owned(),
            account_status: "missing".to_owned(),
            process_status: "stopped".to_owned(),
            blockers: vec![PlayBlockerDto {
                code: "launch_authentication_required".to_owned(),
                message: "Select an account.".to_owned(),
            }],
        };
        let json = serde_json::to_string(&readiness).unwrap();
        assert!(!json.contains("accessToken"));
        assert!(!json.contains("arguments"));
        assert!(!json.contains("javaExecutable"));
        assert!(!json.contains("classpath"));
    }

    #[test]
    fn auth_errors_map_to_stable_machine_codes() {
        use crate::auth::metadata::{
            AuthTransportError, EntitlementError, MinecraftExchangeError, TokenExchangeError,
            XboxExchangeError, XstsExchangeError,
        };

        let cases: Vec<(crate::auth::AuthError, &str)> = vec![
            (
                crate::auth::AuthError::ConfigurationMissing,
                "auth_configuration_missing",
            ),
            (
                crate::auth::AuthError::LoginInProgress,
                "auth_login_in_progress",
            ),
            (
                crate::auth::AuthError::LoginCancelled,
                "auth_login_cancelled",
            ),
            (crate::auth::AuthError::LoginTimeout, "auth_login_timeout"),
            (
                crate::auth::AuthError::CallbackInvalid,
                "auth_callback_invalid",
            ),
            (
                crate::auth::AuthError::OAuthFailure {
                    reason: "access_denied".to_owned(),
                },
                "auth_oauth_failure",
            ),
            (
                crate::auth::AuthError::TokenExchange(TokenExchangeError::Malformed),
                "auth_token_exchange_failure",
            ),
            (
                crate::auth::AuthError::Xbox(XboxExchangeError::Malformed),
                "auth_xbox_failure",
            ),
            (
                crate::auth::AuthError::Xsts(XstsExchangeError::Malformed),
                "auth_xsts_failure",
            ),
            (
                crate::auth::AuthError::XstsDenied {
                    xerr: 2148916238,
                    reason: "family group".to_owned(),
                },
                "auth_xsts_failure",
            ),
            (
                crate::auth::AuthError::MinecraftServices(MinecraftExchangeError::Malformed),
                "auth_minecraft_services_failure",
            ),
            (
                crate::auth::AuthError::EntitlementCheck(EntitlementError::Malformed),
                "auth_minecraft_services_failure",
            ),
            (
                crate::auth::AuthError::MinecraftServicesTransport(
                    AuthTransportError::HttpStatus { status: 503 },
                ),
                "auth_minecraft_services_failure",
            ),
            (
                crate::auth::AuthError::EntitlementMissing,
                "auth_entitlement_missing",
            ),
            (
                crate::auth::AuthError::ProfileMissing,
                "auth_profile_missing",
            ),
            (
                crate::auth::AuthError::ProfileInvalid,
                "auth_profile_invalid",
            ),
            (
                crate::auth::AuthError::ReauthenticationRequired {
                    reason: "revoked".to_owned(),
                },
                "auth_reauthentication_required",
            ),
            (
                crate::auth::AuthError::CredentialStore(
                    crate::auth::CredentialStoreError::UnsupportedPlatform,
                ),
                "auth_credential_store_failure",
            ),
            (
                crate::auth::AuthError::Accounts(crate::auth::AccountsError::Malformed(
                    "broken".to_owned(),
                )),
                "accounts_invalid",
            ),
            (
                crate::auth::AuthError::Accounts(crate::auth::AccountsError::UnsupportedSchema {
                    found: 9,
                    supported: 1,
                }),
                "accounts_unsupported_schema",
            ),
            (
                crate::auth::AuthError::Accounts(crate::auth::AccountsError::SelectedDangling {
                    account_id: "x".repeat(32),
                }),
                "accounts_selected_dangling",
            ),
            (
                crate::auth::AuthError::AccountNotFound {
                    account_id: "y".repeat(32),
                },
                "auth_account_not_found",
            ),
            (
                crate::auth::AuthError::BrowserOpen(crate::auth::flow::BrowserOpenError::new(
                    "no browser",
                )),
                "auth_browser_open_failure",
            ),
            (
                crate::auth::AuthError::CallbackListener("bind failed".to_owned()),
                "auth_callback_listener_failure",
            ),
            (
                crate::auth::AuthError::EntropyFailure(crate::auth::oauth::OAuthEntropyError),
                "auth_entropy_failure",
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
    fn invalid_instance_ids_map_to_a_stable_machine_code() {
        let error = CommandError::from(crate::instances::InstanceId::new("../evil").unwrap_err());
        assert_eq!(error.code, "instance_id_invalid");
    }

    #[test]
    fn installation_errors_map_to_stable_machine_codes() {
        use crate::install::natives::NativeExtractionError;

        let cases: Vec<(InstallError, &str)> = vec![
            (
                InstallError::AlreadyInProgress {
                    instance_id: "aurora-default".to_owned(),
                },
                "installation_already_in_progress",
            ),
            (
                InstallError::TargetConflict {
                    path: "C:/managed/instances/x/game".to_owned(),
                    reason: "partial prior tree".to_owned(),
                },
                "installation_target_conflict",
            ),
            (
                InstallError::State(crate::install::state::InstalledStateError::Malformed {
                    reason: "broken".to_owned(),
                }),
                "installation_state_invalid",
            ),
            (
                InstallError::AssetIndexInvalid {
                    reason: "not JSON".to_owned(),
                },
                "minecraft_asset_index_invalid",
            ),
            (
                InstallError::AssetInvalid {
                    name: "icons/icon.png".to_owned(),
                    reason: "bad hash".to_owned(),
                },
                "minecraft_asset_invalid",
            ),
            (
                InstallError::InvalidSource("bad metadata".to_owned()),
                "artifact_source_invalid",
            ),
            (
                InstallError::Materialization {
                    path: "libraries/x.jar".to_owned(),
                    source: std::io::Error::other("disk"),
                },
                "artifact_materialization_failure",
            ),
            (
                InstallError::Native(NativeExtractionError::ArchiveInvalid {
                    archive: "org.lwjgl:lwjgl:3.4.1:natives-windows".to_owned(),
                    reason: "traversal entry".to_owned(),
                }),
                "native_archive_invalid",
            ),
            (
                InstallError::Native(NativeExtractionError::EntryConflict {
                    archive: "a".to_owned(),
                    entry: "lwjgl.dll".to_owned(),
                    reason: "differing duplicates".to_owned(),
                }),
                "native_extraction_failure",
            ),
            (
                InstallError::Validation {
                    path: "versions/26.2/client.jar".to_owned(),
                    reason: "digest drift".to_owned(),
                },
                "installation_validation_failure",
            ),
            (
                InstallError::Commit {
                    context: "promoting the staged installation".to_owned(),
                    source: std::io::Error::other("locked"),
                },
                "installation_commit_failure",
            ),
            (
                InstallError::Storage(std::io::Error::other("disk")),
                "storage_io_failure",
            ),
            (
                InstallError::Acquisition(AcquisitionError::StoreIo(std::io::Error::other("disk"))),
                "cache_io_failure",
            ),
            (
                InstallError::Acquisition(AcquisitionError::Download(
                    DownloadError::Sha1Mismatch {
                        expected: "a".repeat(40),
                        actual: "b".repeat(40),
                    },
                )),
                "artifact_hash_mismatch",
            ),
            (
                InstallError::Acquisition(AcquisitionError::Download(
                    DownloadError::ObservedDigestDrift {
                        url: "https://maven.fabricmc.net/x.jar".to_owned(),
                        recorded: "a".repeat(64),
                        received: "b".repeat(64),
                    },
                )),
                "fabric_artifact_unverified",
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
    fn launcher_state_flattens_persisted_records_into_summary_dtos() {
        let config = crate::config::LauncherConfig::from_json(
            r#"{ "schemaVersion": 1, "selectedInstanceId": "aurora-default" }"#,
        )
        .unwrap();
        let registry = InstanceRegistry::from_json(
            r#"{
                "schemaVersion": 2,
                "instances": [
                    {
                        "id": "aurora-default",
                        "displayName": "Aurora Default",
                        "state": "ready",
                        "release": {
                            "channel": "stable",
                            "auroraVersion": "0.3.0",
                            "minecraftVersion": "26.2",
                            "fabricLoaderVersion": "0.19.5"
                        }
                    }
                ]
            }"#,
        )
        .unwrap();

        let state = LauncherState::from_parts(config, registry).unwrap();

        // The schema-1 input migrated deterministically to schema 2 with the
        // selection preserved and default appearance.
        assert_eq!(state.config.schema_version, 2);
        assert_eq!(
            state.config.selected_instance_id.as_deref(),
            Some("aurora-default")
        );
        assert_eq!(state.instances.len(), 1);
        assert_eq!(state.instances[0].id, "aurora-default");
        assert_eq!(state.instances[0].display_name, "Aurora Default");
        assert_eq!(state.instances[0].state, "ready");
        assert_eq!(state.instances[0].channel, ReleaseChannel::Stable);
        assert_eq!(state.instances[0].aurora_version, "0.3.0");
        assert_eq!(state.instances[0].minecraft_version, "26.2");
        assert_eq!(state.instances[0].fabric_loader_version, "0.19.5");
    }

    #[test]
    fn a_dangling_selected_instance_is_a_deliberate_state_error() {
        let config = crate::config::LauncherConfig::from_json(
            r#"{ "schemaVersion": 1, "selectedInstanceId": "ghost" }"#,
        )
        .unwrap();
        let registry = InstanceRegistry::empty();

        let error = LauncherState::from_parts(config, registry).unwrap_err();

        assert_eq!(error.code, "config_selected_instance_dangling");
    }

    #[test]
    fn no_selection_or_a_valid_selection_loads_normally() {
        let unselected = crate::config::LauncherConfig::default();
        assert!(LauncherState::from_parts(unselected, InstanceRegistry::empty()).is_ok());
    }

    #[test]
    fn appearance_dtos_expose_the_catalog_and_the_derived_palette() {
        let preferences = crate::appearance::AppearancePreferences::new();
        let dto = AppearanceDto::of(&preferences).unwrap();

        assert_eq!(dto.theme, "aurora-dark");
        assert_eq!(
            dto.accent,
            crate::appearance::AccentSelection::Preset {
                id: "violet".to_owned()
            }
        );
        assert_eq!(dto.themes.len(), 3);
        assert_eq!(dto.themes[0].id, "aurora-dark");
        assert_eq!(dto.accents.len(), 7);
        assert_eq!(dto.accents[0].id, "violet");
        assert_eq!(dto.palette.accent, "#8b80ff");
        assert_eq!(dto.palette.accent_strong, "#6f5df2");
        assert_eq!(dto.palette.accent_contrast, "#ffffff");
        assert_eq!(dto.palette.accent_soft, "rgba(139, 128, 255, 0.14)");

        // The DTO never carries secrets or machine paths.
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("accessToken"));
        assert!(!json.contains("C:\\"));
    }

    #[test]
    fn invalid_appearance_requests_map_to_a_stable_machine_code() {
        // An unknown theme id is already normalized by the time the DTO is
        // built (config load normalizes); only an unusable accent fails here.
        let bad_theme = crate::appearance::AppearancePreferences {
            theme: "neon".to_owned(),
            accent: crate::appearance::AccentSelection::default(),
        };
        let dto = AppearanceDto::of(&bad_theme).unwrap();
        assert_eq!(dto.theme, "aurora-dark");

        let bad_accent = crate::appearance::AppearancePreferences {
            theme: "oled".to_owned(),
            accent: crate::appearance::AccentSelection::Custom {
                hex: "#000000".to_owned(),
            },
        };
        let error = AppearanceDto::of(&bad_accent).unwrap_err();
        assert_eq!(error.code, "appearance_invalid");
        assert!(error.message.contains("too dark"));
    }

    #[test]
    fn shortcut_errors_map_to_stable_machine_codes() {
        let cases = [
            (
                crate::shortcuts::ShortcutError::UnsupportedPlatform,
                "shortcut_unsupported_platform",
            ),
            (
                crate::shortcuts::ShortcutError::ManagementUnavailable,
                "shortcut_management_unavailable",
            ),
            (
                crate::shortcuts::ShortcutError::Conflict,
                "shortcut_conflict",
            ),
            (
                crate::shortcuts::ShortcutError::Io("io".into()),
                "shortcut_io_failure",
            ),
            (
                crate::shortcuts::ShortcutError::Shell("shell".into()),
                "shortcut_shell_failure",
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
    fn desktop_integration_dtos_serialize_the_live_status_vocabulary() {
        let present =
            ShortcutStatusDto::for_presence(crate::shortcuts::ShortcutPresence::Present, "aurora");
        let json = serde_json::to_string(&present).unwrap();
        assert_eq!(json, r#"{"state":"present","managedBy":"aurora"}"#);

        let conflict = ShortcutStatusDto::for_presence(
            crate::shortcuts::ShortcutPresence::Conflict,
            "installer",
        );
        assert_eq!(
            serde_json::to_string(&conflict).unwrap(),
            r#"{"state":"conflict","managedBy":"installer"}"#
        );

        let unknown = ShortcutStatusDto::unknown("aurora");
        assert_eq!(
            serde_json::to_string(&unknown).unwrap(),
            r#"{"state":"unknown","managedBy":"aurora"}"#
        );

        // The non-Windows shape stays honest: nothing is claimed present.
        #[cfg(not(windows))]
        {
            let dto = DesktopIntegrationDto::unsupported();
            assert!(!dto.supported);
            assert!(!dto.manageable);
            assert_eq!(dto.desktop_shortcut.state, "unknown");
            assert_eq!(dto.start_menu_shortcut.managed_by, "installer");
            let json = serde_json::to_string(&dto).unwrap();
            assert!(!json.contains("present"));
        }
    }
}
