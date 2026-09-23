//! Deterministic launch assembly from normalized, already-validated state.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::auth::MinecraftSession;
use crate::auth::credentials::SecretString;
use crate::fabric::plan::GameInstallPlan;
use crate::install::state::{InstalledFileRole, InstalledGameManifest};
use crate::instances::InstanceId;
use crate::minecraft::plan::LaunchArgument;
use crate::minecraft::rules::{FeatureFlag, FeatureProfile, PlatformProfile};
use crate::paths::ManagedPaths;

const LAUNCHER_NAME: &str = "aurora-launcher";
const MAX_ARGUMENT_LENGTH: usize = 32 * 1024;

/// Launch-only options derived from the instance's desired configuration.
///
/// These never authorize launch by themselves (deep validation does) and
/// never change what is installed; they shape the process: the launcher-owned
/// heap argument, the parsed custom JVM arguments, and the windowed
/// resolution mapped onto Minecraft's own `has_custom_resolution` feature.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchOptions {
    /// Memory in whole MiB; the authoritative `-Xmx` Aurora generates.
    memory_mib: Option<u32>,
    /// Additional JVM arguments, already parsed and validated at the
    /// configuration trust boundary and re-validated here.
    additional_jvm_arguments: Vec<String>,
    /// Custom windowed resolution; enables Mojang's
    /// `has_custom_resolution` feature arguments.
    window: Option<(u16, u16)>,
}

impl LaunchOptions {
    pub fn new(
        memory_mib: u32,
        additional_jvm_arguments: Vec<String>,
        window: Option<(u16, u16)>,
    ) -> Self {
        Self {
            memory_mib: Some(memory_mib),
            additional_jvm_arguments,
            window,
        }
    }

    /// The feature profile these options imply: a custom window enables
    /// Minecraft's `has_custom_resolution` feature, which adds the official
    /// `--width`/`--height` game arguments from the version document.
    pub fn feature_profile(&self) -> FeatureProfile {
        let mut profile = FeatureProfile::none();
        if self.window.is_some() {
            profile.enable(FeatureFlag::HasCustomResolution);
        }
        profile
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum ResolvedArgument {
    Plain(String),
    Sensitive(SecretString),
}

impl ResolvedArgument {
    pub fn expose(&self) -> &str {
        match self {
            Self::Plain(value) => value,
            Self::Sensitive(value) => value.expose(),
        }
    }

    pub fn is_sensitive(&self) -> bool {
        matches!(self, Self::Sensitive(_))
    }
}

impl fmt::Debug for ResolvedArgument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(value) => value.fmt(formatter),
            Self::Sensitive(_) => formatter.write_str("\"[redacted launch argument]\""),
        }
    }
}

/// Complete native process input. It is deliberately not serializable and
/// its custom Debug implementation cannot reveal secret-bearing arguments.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    instance_id: String,
    java_executable: PathBuf,
    working_directory: PathBuf,
    main_class: String,
    jvm_arguments: Vec<ResolvedArgument>,
    game_arguments: Vec<ResolvedArgument>,
    classpath: Vec<PathBuf>,
    natives_directory: PathBuf,
    assets_root: PathBuf,
    asset_index: String,
    logging_config: Option<PathBuf>,
    extra_redactions: Vec<SecretString>,
}

impl LaunchSpec {
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
    pub fn java_executable(&self) -> &Path {
        &self.java_executable
    }
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }
    pub fn main_class(&self) -> &str {
        &self.main_class
    }
    pub fn jvm_arguments(&self) -> &[ResolvedArgument] {
        &self.jvm_arguments
    }
    pub fn game_arguments(&self) -> &[ResolvedArgument] {
        &self.game_arguments
    }
    pub fn classpath(&self) -> &[PathBuf] {
        &self.classpath
    }
    pub fn natives_directory(&self) -> &Path {
        &self.natives_directory
    }
    pub fn assets_root(&self) -> &Path {
        &self.assets_root
    }
    pub fn asset_index(&self) -> &str {
        &self.asset_index
    }
    pub fn logging_config(&self) -> Option<&Path> {
        self.logging_config.as_deref()
    }

    pub(crate) fn command_arguments(&self) -> Vec<ResolvedArgument> {
        let mut arguments =
            Vec::with_capacity(self.jvm_arguments.len() + self.game_arguments.len() + 1);
        arguments.extend(self.jvm_arguments.clone());
        arguments.push(ResolvedArgument::Plain(self.main_class.clone()));
        arguments.extend(self.game_arguments.clone());
        arguments
    }

    pub(crate) fn sensitive_values(&self) -> Vec<String> {
        let mut values: Vec<String> = self
            .command_arguments()
            .into_iter()
            .filter_map(|argument| match argument {
                ResolvedArgument::Sensitive(value) => Some(value.expose().to_owned()),
                ResolvedArgument::Plain(_) => None,
            })
            .collect();
        values.extend(
            self.extra_redactions
                .iter()
                .map(|value| value.expose().to_owned()),
        );
        values
    }

    #[cfg(test)]
    pub(crate) fn fake_process(
        instance_id: &str,
        executable: PathBuf,
        test_name: &str,
        working_directory: PathBuf,
    ) -> Self {
        Self {
            instance_id: instance_id.to_owned(),
            java_executable: executable,
            working_directory,
            main_class: test_name.to_owned(),
            jvm_arguments: vec![ResolvedArgument::Plain("--exact".to_owned())],
            game_arguments: vec![
                ResolvedArgument::Plain("--ignored".to_owned()),
                ResolvedArgument::Plain("--nocapture".to_owned()),
            ],
            classpath: Vec::new(),
            natives_directory: PathBuf::new(),
            assets_root: PathBuf::new(),
            asset_index: String::new(),
            logging_config: None,
            extra_redactions: vec![SecretString::new("FIXTURE-PROCESS-TOKEN")],
        }
    }
}

impl fmt::Debug for LaunchSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchSpec")
            .field("instance_id", &self.instance_id)
            .field("java_executable", &self.java_executable)
            .field("working_directory", &self.working_directory)
            .field("main_class", &self.main_class)
            .field("jvm_arguments", &self.jvm_arguments)
            .field("game_arguments", &self.game_arguments)
            .field("classpath_entries", &self.classpath.len())
            .field("natives_directory", &self.natives_directory)
            .field("assets_root", &self.assets_root)
            .field("asset_index", &self.asset_index)
            .field("logging_config", &self.logging_config)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    minecraft_version: String,
    fabric_loader_version: String,
    version_type: String,
    main_class: String,
    libraries: Vec<String>,
    game_arguments: Vec<LaunchArgument>,
    jvm_arguments: Vec<LaunchArgument>,
    asset_index: String,
    logging: Option<LaunchLogging>,
    platform: PlatformProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchLogging {
    file_name: String,
    argument: String,
}

impl LaunchPlan {
    pub fn from_game_plan(plan: &GameInstallPlan, platform: PlatformProfile) -> Self {
        let minecraft = plan.minecraft();
        Self {
            minecraft_version: minecraft.minecraft_version().to_owned(),
            fabric_loader_version: plan.loader().loader_version().to_owned(),
            version_type: minecraft.version_type().as_mojang_str().to_owned(),
            main_class: plan.main_class().to_owned(),
            libraries: plan
                .libraries()
                .iter()
                .filter(|library| library.is_classpath_entry())
                .map(|library| library.repository_path())
                .collect(),
            game_arguments: minecraft.launch().game_arguments().to_vec(),
            jvm_arguments: minecraft.launch().jvm_arguments().to_vec(),
            asset_index: minecraft.asset_index().id().to_owned(),
            logging: minecraft.logging().map(|logging| LaunchLogging {
                file_name: logging.file_name().to_owned(),
                argument: logging.argument().to_owned(),
            }),
            platform,
        }
    }
}

/// Builds the exact process specification after the caller has completed the
/// deep instance/runtime/session preconditions. This function performs no
/// network access and never scans libraries or user mods.
///
/// `options` carries the instance's launch-only configuration. The
/// launcher-owned heap argument is emitted first among JVM arguments, then
/// Mojang's planned JVM arguments, then the logging argument, then the
/// user's additional arguments last — one deterministic order, no
/// duplicates possible (heap/classpath conflicts were already rejected at
/// the configuration boundary and are re-checked here).
pub fn resolve_launch_spec(
    managed: &ManagedPaths,
    instance_id: &InstanceId,
    plan: &LaunchPlan,
    installed: &InstalledGameManifest,
    java_executable: &Path,
    session: &MinecraftSession,
    features: &FeatureProfile,
    options: &LaunchOptions,
) -> Result<LaunchSpec, LaunchResolveError> {
    let paths = managed.instance_paths(instance_id);
    let instance_root = paths.root().to_path_buf();
    let game_root = paths.game().to_path_buf();

    if installed.minecraft_version() != plan.minecraft_version {
        return Err(LaunchResolveError::Metadata(format!(
            "installed Minecraft '{}' does not match launch plan '{}'",
            installed.minecraft_version(),
            plan.minecraft_version
        )));
    }
    if installed.fabric_loader_version() != plan.fabric_loader_version {
        return Err(LaunchResolveError::Metadata(format!(
            "installed Fabric Loader '{}' does not match launch plan '{}'",
            installed.fabric_loader_version(),
            plan.fabric_loader_version
        )));
    }
    if session.account_id() != session.profile().uuid()
        || crate::auth::accounts::AccountId::validate(session.profile().uuid()).is_err()
        || crate::auth::accounts::validate_minecraft_name(session.profile().name()).is_err()
        || session.minecraft_access_token().expose().is_empty()
        || !session.usable()
    {
        return Err(LaunchResolveError::Metadata(
            "the authenticated Minecraft session identity is invalid or expired".to_owned(),
        ));
    }
    prove_real_directory(&instance_root, "instance game directory")?;
    prove_real_directory(&game_root, "managed game installation")?;

    if !java_executable.is_absolute()
        || !java_executable.starts_with(managed.runtimes_dir())
        || !java_executable.is_file()
    {
        return Err(LaunchResolveError::Runtime(
            "the validated managed Java executable is missing or outside managed runtimes"
                .to_owned(),
        ));
    }
    prove_canonical_containment_as(
        &managed.runtimes_dir(),
        java_executable,
        "managed Java executable",
        LaunchResolveError::Runtime,
    )?;

    let libraries_root = game_root.join("libraries");
    prove_real_directory(&libraries_root, "managed library directory")?;
    let mut classpath = Vec::with_capacity(plan.libraries.len() + 1);
    for relative in &plan.libraries {
        let installed_path = format!("libraries/{relative}");
        require_manifest_role(
            installed,
            &installed_path,
            &[InstalledFileRole::Library],
            LaunchResolveError::Classpath,
        )?;
        let absolute = game_root.join(&installed_path);
        if !absolute.is_file() {
            return Err(LaunchResolveError::Classpath(format!(
                "planned library '{installed_path}' is missing"
            )));
        }
        prove_canonical_containment_as(
            &game_root,
            &absolute,
            "classpath library",
            LaunchResolveError::Classpath,
        )?;
        classpath.push(absolute);
    }

    let client_relative = format!("versions/{}/client.jar", plan.minecraft_version);
    require_manifest_role(
        installed,
        &client_relative,
        &[InstalledFileRole::Client],
        LaunchResolveError::Classpath,
    )?;
    let client = game_root.join(&client_relative);
    if !client.is_file() {
        return Err(LaunchResolveError::Classpath(
            "the installed Minecraft client is missing".to_owned(),
        ));
    }
    prove_canonical_containment_as(
        &game_root,
        &client,
        "Minecraft client",
        LaunchResolveError::Classpath,
    )?;
    classpath.push(client);

    let natives_directory = game_root.join(installed.natives().directory());
    prove_real_directory(&natives_directory, "installed native directory")
        .map_err(|error| LaunchResolveError::Natives(error.to_string()))?;
    if std::fs::read_dir(&natives_directory)
        .map_err(|error| LaunchResolveError::Natives(error.to_string()))?
        .next()
        .is_none()
    {
        return Err(LaunchResolveError::Natives(
            "the installed native directory is empty".to_owned(),
        ));
    }
    prove_canonical_containment(&game_root, &natives_directory, "native directory")?;

    let assets_root = game_root.join("assets");
    prove_real_directory(&assets_root, "installed assets directory")
        .map_err(|error| LaunchResolveError::Assets(error.to_string()))?;
    let asset_index_relative = format!("assets/indexes/{}.json", plan.asset_index);
    require_manifest_role(
        installed,
        &asset_index_relative,
        &[InstalledFileRole::AssetIndex],
        LaunchResolveError::Assets,
    )?;
    if !game_root.join(&asset_index_relative).is_file() {
        return Err(LaunchResolveError::Assets(
            "the installed asset index is missing".to_owned(),
        ));
    }
    prove_canonical_containment_as(
        &game_root,
        &game_root.join(&asset_index_relative),
        "asset index",
        LaunchResolveError::Assets,
    )?;

    let logging_config = plan
        .logging
        .as_ref()
        .map(|logging| {
            let relative = format!("versions/{}/{}", plan.minecraft_version, logging.file_name);
            require_manifest_role(
                installed,
                &relative,
                &[InstalledFileRole::LoggingConfig],
                LaunchResolveError::Logging,
            )?;
            let path = game_root.join(&relative);
            if !path.is_file() {
                return Err(LaunchResolveError::Logging(
                    "the installed logging configuration is missing".to_owned(),
                ));
            }
            prove_canonical_containment_as(
                &game_root,
                &path,
                "logging configuration",
                LaunchResolveError::Logging,
            )?;
            Ok(path)
        })
        .transpose()?;

    let separator = match plan.platform.os() {
        crate::minecraft::rules::OperatingSystem::Windows => ";",
        crate::minecraft::rules::OperatingSystem::Linux
        | crate::minecraft::rules::OperatingSystem::MacOs => ":",
    };
    let classpath_value = classpath
        .iter()
        .map(|path| path_text(path, "classpath"))
        .collect::<Result<Vec<_>, _>>()?
        .join(separator);

    let values = PlaceholderValues {
        player_name: session.profile().name(),
        uuid: session.profile().uuid(),
        token: session.minecraft_access_token(),
        version_name: &plan.minecraft_version,
        version_type: &plan.version_type,
        game_directory: path_text(&instance_root, "game directory")?,
        assets_root: path_text(&assets_root, "assets root")?,
        asset_index: &plan.asset_index,
        natives_directory: path_text(&natives_directory, "native directory")?,
        library_directory: path_text(&libraries_root, "library directory")?,
        classpath: &classpath_value,
        classpath_separator: separator,
        logging_path: logging_config
            .as_deref()
            .map(|path| path_text(path, "logging configuration"))
            .transpose()?,
    };

    let mut jvm_arguments =
        Vec::with_capacity(1 + plan.jvm_arguments.len() + options.additional_jvm_arguments.len());
    // The launcher-owned heap argument comes first: Aurora owns the heap,
    // and the configuration boundary already rejected user heap flags, so
    // no duplicate `-Xmx` can exist in the vector.
    if let Some(memory_mib) = options.memory_mib {
        jvm_arguments.push(ResolvedArgument::Plain(
            crate::instances::settings::heap_argument(memory_mib),
        ));
    }
    jvm_arguments.extend(resolve_groups(&plan.jvm_arguments, features, &values)?);
    if let Some(logging) = &plan.logging {
        jvm_arguments.push(substitute_argument(&logging.argument, &values)?);
    }
    // The user's additional arguments come last, re-validated here so this
    // boundary stays honest even if a future caller bypasses configuration
    // validation.
    {
        let rejoin = options
            .additional_jvm_arguments
            .iter()
            .map(|argument| {
                if argument.contains(' ') || argument.contains('\t') || argument.contains('"') {
                    // Re-quote so the round trip through the parser is exact.
                    format!(
                        "\"{}\"",
                        argument.replace('\\', "\\\\").replace('"', "\\\"")
                    )
                } else {
                    argument.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        crate::instances::settings::parse_jvm_arguments(&rejoin)
            .map_err(|reason| LaunchResolveError::JvmArguments(reason.to_string()))?;
    }
    for argument in &options.additional_jvm_arguments {
        if argument.len() > MAX_ARGUMENT_LENGTH || argument.contains('\0') {
            return Err(LaunchResolveError::JvmArguments(
                "an additional JVM argument is oversized or contains a NUL byte".to_owned(),
            ));
        }
        jvm_arguments.push(ResolvedArgument::Plain(argument.clone()));
    }

    let game_arguments = resolve_groups(&plan.game_arguments, features, &values)?;

    Ok(LaunchSpec {
        instance_id: instance_id.to_string(),
        java_executable: java_executable.to_path_buf(),
        working_directory: instance_root,
        main_class: plan.main_class.clone(),
        jvm_arguments,
        game_arguments,
        classpath,
        natives_directory,
        assets_root,
        asset_index: plan.asset_index.clone(),
        logging_config,
        extra_redactions: Vec::new(),
    })
}

struct PlaceholderValues<'a> {
    player_name: &'a str,
    uuid: &'a str,
    token: &'a SecretString,
    version_name: &'a str,
    version_type: &'a str,
    game_directory: &'a str,
    assets_root: &'a str,
    asset_index: &'a str,
    natives_directory: &'a str,
    library_directory: &'a str,
    classpath: &'a str,
    classpath_separator: &'a str,
    logging_path: Option<&'a str>,
}

fn resolve_groups(
    groups: &[LaunchArgument],
    features: &FeatureProfile,
    values: &PlaceholderValues<'_>,
) -> Result<Vec<ResolvedArgument>, LaunchResolveError> {
    let mut result = Vec::new();
    for group in groups {
        if group.features.iter().all(|flag| features.is_enabled(*flag)) {
            for value in &group.values {
                result.push(substitute_argument(value, values)?);
            }
        }
    }
    Ok(result)
}

fn substitute_argument(
    template: &str,
    values: &PlaceholderValues<'_>,
) -> Result<ResolvedArgument, LaunchResolveError> {
    if template.len() > MAX_ARGUMENT_LENGTH || template.contains('\0') {
        return Err(LaunchResolveError::Placeholder(
            "a launch argument is empty, oversized, or contains a NUL byte".to_owned(),
        ));
    }

    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    let mut sensitive = false;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(LaunchResolveError::Placeholder(format!(
                "launch argument contains an unterminated placeholder: {template}"
            )));
        };
        let name = &after[..end];
        let replacement = match name {
            "auth_player_name" => values.player_name,
            "version_name" => values.version_name,
            "game_directory" => values.game_directory,
            "assets_root" => values.assets_root,
            "assets_index_name" => values.asset_index,
            "auth_uuid" => values.uuid,
            "auth_access_token" => {
                sensitive = true;
                values.token.expose()
            }
            // Current official metadata requires these arguments, but the
            // Minecraft Services session does not expose either optional
            // telemetry identifier. Empty is the honest launcher value.
            "clientid" | "auth_xuid" => "",
            "user_type" => "msa",
            "user_properties" => "{}",
            "version_type" => values.version_type,
            "natives_directory" => values.natives_directory,
            "launcher_name" => LAUNCHER_NAME,
            "launcher_version" => env!("CARGO_PKG_VERSION"),
            "classpath" => values.classpath,
            "classpath_separator" => values.classpath_separator,
            "library_directory" => values.library_directory,
            "path" => values.logging_path.ok_or_else(|| {
                LaunchResolveError::Logging(
                    "the logging argument requested a path without a logging file".to_owned(),
                )
            })?,
            other => {
                return Err(LaunchResolveError::Placeholder(format!(
                    "unsupported launch placeholder '${{{other}}}'"
                )));
            }
        };
        output.push_str(replacement);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    if output.contains("${") {
        return Err(LaunchResolveError::Placeholder(
            "a launch argument still contains an unresolved placeholder".to_owned(),
        ));
    }
    if output.len() > MAX_ARGUMENT_LENGTH {
        return Err(LaunchResolveError::Placeholder(
            "a resolved launch argument exceeds the supported size".to_owned(),
        ));
    }
    Ok(if sensitive {
        ResolvedArgument::Sensitive(SecretString::new(output))
    } else {
        ResolvedArgument::Plain(output)
    })
}

fn require_manifest_role(
    installed: &InstalledGameManifest,
    path: &str,
    roles: &[InstalledFileRole],
    error: fn(String) -> LaunchResolveError,
) -> Result<(), LaunchResolveError> {
    if installed
        .files()
        .iter()
        .any(|file| file.path() == path && roles.contains(&file.role()))
    {
        Ok(())
    } else {
        Err(error(format!(
            "installed state does not record required launch file '{path}'"
        )))
    }
}

fn prove_real_directory(path: &Path, label: &str) -> Result<(), LaunchResolveError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| LaunchResolveError::Metadata(format!("{label}: {error}")))?;
    if !metadata.file_type().is_dir() {
        return Err(LaunchResolveError::Metadata(format!(
            "{label} is not a real directory"
        )));
    }
    Ok(())
}

fn prove_canonical_containment(
    parent: &Path,
    child: &Path,
    label: &str,
) -> Result<(), LaunchResolveError> {
    let parent = std::fs::canonicalize(parent)
        .map_err(|error| LaunchResolveError::Natives(format!("{label}: {error}")))?;
    let child = std::fs::canonicalize(child)
        .map_err(|error| LaunchResolveError::Natives(format!("{label}: {error}")))?;
    if !child.starts_with(&parent) {
        return Err(LaunchResolveError::Natives(format!(
            "{label} escapes the validated game installation"
        )));
    }
    Ok(())
}

fn prove_canonical_containment_as(
    parent: &Path,
    child: &Path,
    label: &str,
    error: fn(String) -> LaunchResolveError,
) -> Result<(), LaunchResolveError> {
    let parent = std::fs::canonicalize(parent).map_err(|cause| error(cause.to_string()))?;
    let child = std::fs::canonicalize(child).map_err(|cause| error(cause.to_string()))?;
    if !child.starts_with(&parent) {
        return Err(error(format!("{label} escapes managed storage")));
    }
    Ok(())
}

fn path_text<'a>(path: &'a Path, label: &str) -> Result<&'a str, LaunchResolveError> {
    path.to_str().ok_or_else(|| {
        LaunchResolveError::Metadata(format!(
            "{label} cannot be represented as a Java argument on this platform"
        ))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchResolveError {
    Metadata(String),
    Runtime(String),
    Placeholder(String),
    Classpath(String),
    Natives(String),
    Logging(String),
    Assets(String),
    JvmArguments(String),
}

impl LaunchResolveError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Metadata(_) => "launch_metadata_invalid",
            Self::Runtime(_) => "launch_runtime_not_ready",
            Self::Placeholder(_) => "launch_placeholder_unresolved",
            Self::Classpath(_) => "launch_classpath_invalid",
            Self::Natives(_) => "launch_native_path_invalid",
            Self::Logging(_) => "launch_logging_invalid",
            Self::Assets(_) => "launch_assets_invalid",
            Self::JvmArguments(_) => "launch_jvm_arguments_invalid",
        }
    }
}

impl fmt::Display for LaunchResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Metadata(reason) => write!(formatter, "launch metadata is invalid: {reason}"),
            Self::Runtime(reason) => write!(formatter, "managed Java is not ready: {reason}"),
            Self::Placeholder(reason) => {
                write!(formatter, "launch argument resolution failed: {reason}")
            }
            Self::Classpath(reason) => write!(formatter, "launch classpath is invalid: {reason}"),
            Self::Natives(reason) => write!(formatter, "launch native path is invalid: {reason}"),
            Self::Logging(reason) => write!(
                formatter,
                "launch logging configuration is invalid: {reason}"
            ),
            Self::Assets(reason) => write!(formatter, "launch assets are invalid: {reason}"),
            Self::JvmArguments(reason) => {
                write!(formatter, "additional JVM arguments are invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for LaunchResolveError {}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use std::time::Duration;

    use crate::auth::MinecraftProfile;
    use crate::auth::credentials::SecretString;
    use crate::install::state::{InstalledFile, NativesRecord};
    use crate::integrity::{ArtifactTrust, DigestAlgorithm};
    use crate::minecraft::rules::{Architecture, FeatureFlag, OperatingSystem};

    const TOKEN: &str = "FIXTURE-LAUNCH-TOKEN-NEVER-LOG";

    fn session() -> MinecraftSession {
        MinecraftSession::new(
            "986dec87b7ec47ff89ff033fdb95c4b5",
            MinecraftProfile::new("986dec87b7ec47ff89ff033fdb95c4b5", "LaunchTester"),
            SecretString::new(TOKEN),
            Duration::from_secs(3_600),
        )
    }

    fn plan() -> LaunchPlan {
        LaunchPlan {
            minecraft_version: "26.2".to_owned(),
            fabric_loader_version: "0.19.5".to_owned(),
            version_type: "release".to_owned(),
            main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".to_owned(),
            libraries: vec!["a/first.jar".to_owned(), "b/fabric.jar".to_owned()],
            game_arguments: vec![
                LaunchArgument {
                    features: Vec::new(),
                    values: vec![
                        "--username".to_owned(),
                        "${auth_player_name}".to_owned(),
                        "--uuid".to_owned(),
                        "${auth_uuid}".to_owned(),
                        "--accessToken".to_owned(),
                        "${auth_access_token}".to_owned(),
                        "--gameDir".to_owned(),
                        "${game_directory}".to_owned(),
                        "--assetsDir".to_owned(),
                        "${assets_root}".to_owned(),
                        "--assetIndex".to_owned(),
                        "${assets_index_name}".to_owned(),
                        "--clientId".to_owned(),
                        "${clientid}".to_owned(),
                        "--xuid".to_owned(),
                        "${auth_xuid}".to_owned(),
                        "--versionType".to_owned(),
                        "${version_type}".to_owned(),
                        "--userType".to_owned(),
                        "${user_type}".to_owned(),
                        "--userProperties".to_owned(),
                        "${user_properties}".to_owned(),
                    ],
                },
                LaunchArgument {
                    features: vec![FeatureFlag::IsDemoUser],
                    values: vec!["--demo".to_owned()],
                },
            ],
            jvm_arguments: vec![LaunchArgument {
                features: Vec::new(),
                values: vec![
                    "-Djava.library.path=${natives_directory}/java".to_owned(),
                    "-Dlauncher=${launcher_name}/${launcher_version}".to_owned(),
                    "-Dlibs=${library_directory}".to_owned(),
                    "-Dsep=${classpath_separator}".to_owned(),
                    "-cp".to_owned(),
                    "${classpath}".to_owned(),
                ],
            }],
            asset_index: "32".to_owned(),
            logging: Some(LaunchLogging {
                file_name: "client.xml".to_owned(),
                argument: "-Dlog4j.configurationFile=${path}".to_owned(),
            }),
            platform: PlatformProfile::new(OperatingSystem::Windows, Architecture::X86_64),
        }
    }

    fn trust() -> ArtifactTrust {
        ArtifactTrust::ExpectedDigestVerified {
            algorithm: DigestAlgorithm::Sha1,
            digest: "0000000000000000000000000000000000000000".to_owned(),
        }
    }

    struct Fixture {
        root: PathBuf,
        managed: ManagedPaths,
        id: InstanceId,
        manifest: InstalledGameManifest,
        java: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(1);
            let root = std::env::temp_dir().join(format!(
                "aurora-launch-resolve-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let managed = ManagedPaths::from_app_local_data_dir(root.clone()).unwrap();
            let id = InstanceId::new("launch-fixture").unwrap();
            let instance = managed.instance_paths(&id);
            let game = instance.game();
            let files = [
                ("libraries/a/first.jar", InstalledFileRole::Library),
                ("libraries/b/fabric.jar", InstalledFileRole::Library),
                ("versions/26.2/client.jar", InstalledFileRole::Client),
                ("versions/26.2/client.xml", InstalledFileRole::LoggingConfig),
                ("assets/indexes/32.json", InstalledFileRole::AssetIndex),
            ];
            for (relative, _) in files {
                let path = game.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, b"x").unwrap();
            }
            std::fs::create_dir_all(game.join("natives/26.2/windows/x64/org/lwjgl")).unwrap();
            std::fs::write(
                game.join("natives/26.2/windows/x64/org/lwjgl/lwjgl.dll"),
                b"x",
            )
            .unwrap();
            let java = managed.runtimes_dir().join("epsilon/runtime/bin/javaw.exe");
            std::fs::create_dir_all(java.parent().unwrap()).unwrap();
            std::fs::write(&java, b"fake").unwrap();
            let manifest = InstalledGameManifest::new(
                "26.2",
                "0.19.5",
                "install-1",
                1,
                files
                    .into_iter()
                    .map(|(path, role)| InstalledFile::new(role, path.to_owned(), trust(), 1))
                    .collect(),
                NativesRecord::new("natives/26.2".to_owned()),
            );
            Self {
                root,
                managed,
                id,
                manifest,
                java,
            }
        }

        fn resolve(&self, features: &FeatureProfile) -> Result<LaunchSpec, LaunchResolveError> {
            self.resolve_with_options(features, LaunchOptions::default())
        }

        fn resolve_with_options(
            &self,
            features: &FeatureProfile,
            options: LaunchOptions,
        ) -> Result<LaunchSpec, LaunchResolveError> {
            resolve_launch_spec(
                &self.managed,
                &self.id,
                &plan(),
                &self.manifest,
                &self.java,
                &session(),
                features,
                &options,
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn launch_spec_is_deterministic_ordered_and_uses_fabric_entry_point() {
        let fixture = Fixture::new();
        let first = fixture.resolve(&FeatureProfile::none()).unwrap();
        let second = fixture.resolve(&FeatureProfile::none()).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.main_class(),
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_ne!(first.main_class(), "net.minecraft.client.main.Main");
        assert_eq!(first.classpath()[0].file_name().unwrap(), "first.jar");
        assert_eq!(first.classpath()[1].file_name().unwrap(), "fabric.jar");
        assert_eq!(first.classpath()[2].file_name().unwrap(), "client.jar");
        assert!(first.natives_directory().ends_with("natives/26.2"));
        assert!(!first.natives_directory().ends_with("windows/x64"));
        let user_mod = fixture
            .managed
            .instance_paths(&fixture.id)
            .mods()
            .join("user.jar");
        std::fs::create_dir_all(user_mod.parent().unwrap()).unwrap();
        std::fs::write(&user_mod, b"user mod").unwrap();
        assert!(!first.classpath().contains(&user_mod));
        assert!(
            first
                .jvm_arguments()
                .iter()
                .any(|a| a.expose().contains(";"))
        );
        assert!(
            !first
                .game_arguments()
                .iter()
                .any(|a| a.expose() == "--demo")
        );
        assert!(
            !first
                .command_arguments()
                .iter()
                .any(|a| a.expose().contains("${"))
        );
    }

    #[test]
    fn feature_conditioned_arguments_are_resolved_at_launch() {
        let fixture = Fixture::new();
        let enabled = fixture
            .resolve(&FeatureProfile::enabling(FeatureFlag::IsDemoUser))
            .unwrap();
        assert!(
            enabled
                .game_arguments()
                .iter()
                .any(|a| a.expose() == "--demo")
        );
    }

    #[test]
    fn structured_arguments_preserve_spaces_unicode_and_quotes() {
        let values = PlaceholderValues {
            player_name: "Player",
            uuid: "uuid",
            token: &SecretString::new(TOKEN),
            version_name: "26.2",
            version_type: "release",
            game_directory: "C:\\Aurora World\\雪 \"quoted\"",
            assets_root: "assets",
            asset_index: "32",
            natives_directory: "natives",
            library_directory: "libraries",
            classpath: "a;b",
            classpath_separator: ";",
            logging_path: Some("log.xml"),
        };
        let resolved = substitute_argument("--path=${game_directory}", &values).unwrap();
        assert_eq!(resolved.expose(), "--path=C:\\Aurora World\\雪 \"quoted\"");
    }

    #[test]
    fn unknown_or_unterminated_placeholders_fail_closed() {
        let token = SecretString::new(TOKEN);
        let values = PlaceholderValues {
            player_name: "P",
            uuid: "u",
            token: &token,
            version_name: "v",
            version_type: "release",
            game_directory: "g",
            assets_root: "a",
            asset_index: "i",
            natives_directory: "n",
            library_directory: "l",
            classpath: "c",
            classpath_separator: ";",
            logging_path: Some("p"),
        };
        assert!(matches!(
            substitute_argument("${unknown}", &values),
            Err(LaunchResolveError::Placeholder(_))
        ));
        assert!(matches!(
            substitute_argument("${broken", &values),
            Err(LaunchResolveError::Placeholder(_))
        ));
    }

    #[test]
    fn token_is_sensitive_and_redacted_from_debug() {
        let fixture = Fixture::new();
        let spec = fixture.resolve(&FeatureProfile::none()).unwrap();
        assert!(
            spec.game_arguments()
                .iter()
                .any(ResolvedArgument::is_sensitive)
        );
        let debug = format!("{spec:?}");
        assert!(!debug.contains(TOKEN));
        assert!(debug.contains("[redacted launch argument]"));
    }

    #[test]
    fn missing_layout_components_block_launch() {
        let fixture = Fixture::new();
        std::fs::remove_file(
            fixture
                .managed
                .instance_paths(&fixture.id)
                .game()
                .join("versions/26.2/client.xml"),
        )
        .unwrap();
        assert!(matches!(
            fixture.resolve(&FeatureProfile::none()),
            Err(LaunchResolveError::Logging(_))
        ));

        let fixture = Fixture::new();
        std::fs::remove_file(
            fixture
                .managed
                .instance_paths(&fixture.id)
                .game()
                .join("assets/indexes/32.json"),
        )
        .unwrap();
        assert!(matches!(
            fixture.resolve(&FeatureProfile::none()),
            Err(LaunchResolveError::Assets(_))
        ));

        let fixture = Fixture::new();
        std::fs::remove_dir_all(
            fixture
                .managed
                .instance_paths(&fixture.id)
                .game()
                .join("natives/26.2"),
        )
        .unwrap();
        assert!(matches!(
            fixture.resolve(&FeatureProfile::none()),
            Err(LaunchResolveError::Natives(_))
        ));
    }

    #[test]
    fn native_directory_must_remain_canonically_inside_the_game_tree() {
        let fixture = Fixture::new();
        let escaped = fixture
            .managed
            .instance_paths(&fixture.id)
            .root()
            .join("escaped-natives");
        std::fs::create_dir_all(&escaped).unwrap();
        std::fs::write(escaped.join("native.dll"), b"x").unwrap();
        let manifest = InstalledGameManifest::new(
            fixture.manifest.minecraft_version(),
            fixture.manifest.fabric_loader_version(),
            fixture.manifest.installation_id(),
            fixture.manifest.installed_at_unix_seconds(),
            fixture.manifest.files().to_vec(),
            NativesRecord::new("../escaped-natives".to_owned()),
        );
        assert!(matches!(
            resolve_launch_spec(
                &fixture.managed,
                &fixture.id,
                &plan(),
                &manifest,
                &fixture.java,
                &session(),
                &FeatureProfile::none(),
                &LaunchOptions::default(),
            ),
            Err(LaunchResolveError::Natives(_))
        ));
    }

    #[test]
    fn memory_and_custom_jvm_arguments_reach_the_launch_spec_exactly_once() {
        let fixture = Fixture::new();
        let options = LaunchOptions::new(
            4096,
            vec![
                "-Dexample=value".to_owned(),
                "-Dlabel=hello world".to_owned(),
                "-XX:+UseG1GC".to_owned(),
            ],
            None,
        );
        let spec = fixture
            .resolve_with_options(&options.feature_profile(), options)
            .unwrap();

        let jvm = spec.jvm_arguments();
        // The launcher-owned heap argument is the very first JVM argument…
        assert_eq!(jvm[0].expose(), "-Xmx4096m");
        // …appears exactly once…
        assert_eq!(
            jvm.iter()
                .filter(|argument| argument.expose().starts_with("-Xmx"))
                .count(),
            1
        );
        // …with no conflicting -Xms anywhere.
        assert!(
            jvm.iter()
                .all(|argument| !argument.expose().starts_with("-Xms"))
        );
        // The custom arguments are present verbatim, after the planned ones.
        assert_eq!(
            jvm.iter()
                .filter(|argument| argument.expose().starts_with("-Dexample="))
                .count(),
            1
        );
        assert!(jvm.iter().any(|a| a.expose() == "-Dlabel=hello world"));
        assert!(jvm.iter().any(|a| a.expose() == "-XX:+UseG1GC"));
        let custom_position = jvm
            .iter()
            .position(|a| a.expose() == "-XX:+UseG1GC")
            .unwrap();
        let logging_position = jvm
            .iter()
            .position(|a| a.expose().starts_with("-Dlog4j.configurationFile="))
            .unwrap();
        let classpath_position = jvm.iter().position(|a| a.expose() == "-cp").unwrap();
        assert!(custom_position > logging_position);
        assert!(logging_position > classpath_position);
    }

    #[test]
    fn conflicting_custom_jvm_arguments_fail_at_the_launch_boundary() {
        let fixture = Fixture::new();
        let options = LaunchOptions::new(2048, vec!["-Xmx12G".to_owned()], None);
        let error = fixture
            .resolve_with_options(&FeatureProfile::none(), options)
            .unwrap_err();
        assert!(matches!(error, LaunchResolveError::JvmArguments(_)));
        assert!(error.to_string().contains("Aurora owns"));
    }

    #[test]
    fn a_custom_window_enables_minecrafts_own_resolution_arguments() {
        let fixture = Fixture::new();
        let options = LaunchOptions::new(2048, Vec::new(), Some((1280, 720)));
        let profile = options.feature_profile();
        assert!(profile.is_enabled(FeatureFlag::HasCustomResolution));

        // With the feature enabled, the version document's official
        // `--width`/`--height` argument group participates; the launch plan
        // fixture carries no such group, so the spec simply resolves without
        // it — the substitution mechanics are what must hold.
        let spec = fixture.resolve_with_options(&profile, options).unwrap();
        assert!(
            spec.game_arguments()
                .iter()
                .all(|argument| !argument.expose().contains("${"))
        );

        let plain = fixture.resolve_with_options(&FeatureProfile::none(), LaunchOptions::default());
        assert!(plain.is_ok());
    }

    #[test]
    fn memory_and_custom_jvm_arguments_reach_the_launch_spec_exactly_once_and_deterministically() {
        let fixture = Fixture::new();
        let options = LaunchOptions::new(2048, vec!["-Dexample=value".to_owned()], None);
        let first = fixture
            .resolve_with_options(&FeatureProfile::none(), options.clone())
            .unwrap();
        let second = fixture
            .resolve_with_options(&FeatureProfile::none(), options)
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn custom_jvm_arguments_are_redaction_safe() {
        let fixture = Fixture::new();
        let options = LaunchOptions::new(2048, vec!["-Dsecret=value".to_owned()], None);
        let spec = fixture
            .resolve_with_options(&FeatureProfile::none(), options)
            .unwrap();
        // Custom arguments are plain, non-sensitive arguments: they contain
        // no session material, and the sensitive marker stays exclusively on
        // the token argument.
        assert!(
            spec.jvm_arguments()
                .iter()
                .all(|argument| !argument.is_sensitive())
        );
        assert!(
            spec.game_arguments()
                .iter()
                .any(ResolvedArgument::is_sensitive)
        );
    }

    /// Live metadata drift check. Only Mojang/Fabric metadata is fetched;
    /// synthetic installed files and a synthetic session exercise argument
    /// assembly without downloading or spawning Minecraft.
    #[tokio::test]
    #[ignore = "fetches live official Mojang and Fabric metadata"]
    async fn live_current_launch_plans_assemble_without_spawning_minecraft() {
        use crate::downloads::DownloadOptions;
        use crate::fabric::metadata::{FabricMetaEndpoints, LoaderVersionId};
        use crate::minecraft::metadata::{MetadataEndpoints, MinecraftVersionId};

        static NEXT_LIVE_ID: AtomicU64 = AtomicU64::new(1);
        let platform = PlatformProfile::current().expect("the host platform must be supported");
        for version in ["26.2", "1.21.11"] {
            let plan = crate::fabric::resolve_game_plan(
                &MetadataEndpoints::official(),
                &FabricMetaEndpoints::official(),
                &MinecraftVersionId::new(version).unwrap(),
                &LoaderVersionId::new("0.19.5").unwrap(),
                platform,
                &DownloadOptions::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("{version} launch metadata must resolve: {error}"));
            let launch_plan = LaunchPlan::from_game_plan(&plan, platform);
            let root = std::env::temp_dir().join(format!(
                "aurora-launch-live-{}-{}",
                std::process::id(),
                NEXT_LIVE_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let managed = ManagedPaths::from_app_local_data_dir(root.clone()).unwrap();
            let instance_id =
                InstanceId::new(format!("live-{}", version.replace('.', "-"))).unwrap();
            let game = managed.instance_paths(&instance_id).game().to_path_buf();
            let mut files = Vec::new();
            let fixture_trust = || ArtifactTrust::ExpectedDigestVerified {
                algorithm: DigestAlgorithm::Sha1,
                digest: "0000000000000000000000000000000000000000".to_owned(),
            };
            for library in plan
                .libraries()
                .iter()
                .filter(|library| library.is_classpath_entry())
            {
                let relative = format!("libraries/{}", library.repository_path());
                let path = game.join(&relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, b"synthetic library").unwrap();
                files.push(InstalledFile::new(
                    InstalledFileRole::Library,
                    relative,
                    fixture_trust(),
                    17,
                ));
            }
            let client_relative = format!("versions/{version}/client.jar");
            let client = game.join(&client_relative);
            std::fs::create_dir_all(client.parent().unwrap()).unwrap();
            std::fs::write(&client, b"synthetic client").unwrap();
            files.push(InstalledFile::new(
                InstalledFileRole::Client,
                client_relative,
                fixture_trust(),
                16,
            ));
            let index_relative = format!(
                "assets/indexes/{}.json",
                plan.minecraft().asset_index().id()
            );
            let index = game.join(&index_relative);
            std::fs::create_dir_all(index.parent().unwrap()).unwrap();
            std::fs::write(&index, b"{}").unwrap();
            files.push(InstalledFile::new(
                InstalledFileRole::AssetIndex,
                index_relative,
                fixture_trust(),
                2,
            ));
            if let Some(logging) = plan.minecraft().logging() {
                let relative = format!("versions/{version}/{}", logging.file_name());
                let path = game.join(&relative);
                std::fs::write(&path, b"synthetic logging").unwrap();
                files.push(InstalledFile::new(
                    InstalledFileRole::LoggingConfig,
                    relative,
                    fixture_trust(),
                    17,
                ));
            }
            let natives_relative = format!("natives/{version}");
            let native = game.join(&natives_relative).join("synthetic-native");
            std::fs::create_dir_all(native.parent().unwrap()).unwrap();
            std::fs::write(native, b"native").unwrap();
            let installed = InstalledGameManifest::new(
                version,
                "0.19.5",
                "live-launch-plan",
                1,
                files,
                NativesRecord::new(natives_relative),
            );
            let java = managed.runtimes_dir().join("live/runtime/bin/java.exe");
            std::fs::create_dir_all(java.parent().unwrap()).unwrap();
            std::fs::write(&java, b"synthetic java").unwrap();

            let spec = resolve_launch_spec(
                &managed,
                &instance_id,
                &launch_plan,
                &installed,
                &java,
                &session(),
                &FeatureProfile::none(),
                &LaunchOptions::default(),
            )
            .unwrap_or_else(|error| panic!("{version} launch must assemble: {error}"));
            let all_arguments = spec.command_arguments();
            assert_eq!(
                spec.main_class(),
                "net.fabricmc.loader.impl.launch.knot.KnotClient"
            );
            assert_eq!(
                spec.classpath().len(),
                plan.libraries()
                    .iter()
                    .filter(|library| library.is_classpath_entry())
                    .count()
                    + 1
            );
            assert!(
                all_arguments
                    .iter()
                    .all(|argument| !argument.expose().contains("${"))
            );
            assert!(spec.logging_config().is_some());
            assert!(spec.natives_directory().ends_with(version));
            eprintln!(
                "[live launch] Minecraft {version} + Fabric 0.19.5: {} classpath entries, {} JVM args, {} game args, Java {}, assets {}, logging {}, native root {}; main {}",
                spec.classpath().len(),
                spec.jvm_arguments().len(),
                spec.game_arguments().len(),
                plan.java().major_version(),
                spec.asset_index(),
                spec.logging_config().is_some(),
                spec.natives_directory().display(),
                spec.main_class(),
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
