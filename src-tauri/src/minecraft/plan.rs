//! The normalized Minecraft installation plan.
//!
//! This module converts a validated version document plus a target platform
//! into Aurora's own domain representation. The output is a *plan*: it names
//! every artifact a future installer must acquire (with each artifact's
//! official URL, SHA-1, and size), the Java runtime component consumed by the
//! managed-runtime domain, and launch metadata with placeholders deliberately
//! unresolved.
//!
//! The plan performs no filesystem mutation, downloads nothing, and never
//! reparses raw Mojang JSON — installers consume only these types.

use std::fmt;

use crate::integrity::Sha1Digest;
use crate::minecraft::metadata::{ArgumentDocument, LibraryDocument, VersionDocument, VersionType};
use crate::minecraft::rules::{FeatureFlag, PlanDecision, PlatformProfile, plan_decision};

/// The Java runtime a Minecraft version requires.
///
/// The component names Mojang's runtime registry entry (for example
/// `java-runtime-epsilon`); discovery, download, and exact executable
/// selection belong to the separate managed-runtime domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRequirement {
    component: String,
    major_version: u32,
}

impl JavaRequirement {
    pub fn component(&self) -> &str {
        &self.component
    }

    pub fn major_version(&self) -> u32 {
        self.major_version
    }
}

/// A downloadable object as officially described: HTTPS URL, official SHA-1
/// digest, and exact byte size.
///
/// The digest is Mojang's, represented as SHA-1 rather than disguised as the
/// SHA-256 Aurora uses for its own artifacts. Verification against these
/// values happens when a future phase acquires the artifact; the plan only
/// records the expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRequirement {
    url: String,
    sha1: Sha1Digest,
    size_bytes: u64,
}

impl ArtifactRequirement {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn sha1(&self) -> &Sha1Digest {
        &self.sha1
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

/// The asset-index requirement: which index a version uses and the officially
/// described document that lists every asset object.
///
/// The index itself is not fetched or enumerated during planning; acquiring
/// the index document and its asset objects belongs to installation
/// execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetIndexRequirement {
    id: String,
    artifact: ArtifactRequirement,
    total_size: u64,
}

impl AssetIndexRequirement {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn artifact(&self) -> &ArtifactRequirement {
        &self.artifact
    }

    /// The total byte size of all asset objects, as declared by official
    /// metadata (planning information only).
    pub fn total_size(&self) -> u64 {
        self.total_size
    }
}

/// The official client logging configuration a modern version requires at
/// launch.
///
/// Current documents publish a log4j2 XML configuration file with an official
/// SHA-1; installing it is part of a complete installation. The official
/// launch-time JVM argument template is preserved for later `${path}`
/// substitution rather than reconstructed or hardcoded by the launcher.
/// A version without a logging block plans without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggingRequirement {
    /// The official file id (for example `client-1.21.2.xml`); a validated
    /// single-segment name used as the installed file name.
    file_name: String,
    /// The single JVM argument template published alongside the file. The
    /// `${path}` value remains unresolved until launch assembly.
    argument: String,
    artifact: ArtifactRequirement,
}

impl LoggingRequirement {
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    pub fn argument(&self) -> &str {
        &self.argument
    }

    pub fn artifact(&self) -> &ArtifactRequirement {
        &self.artifact
    }
}

/// A parsed Maven coordinate identifying one library.
///
/// Coordinates are structured identity: group, artifact, version, and an
/// optional classifier. Nothing downstream needs to parse file names to know
/// what a library is or where it belongs on the classpath.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryCoordinate {
    group: String,
    artifact: String,
    version: String,
    classifier: Option<String>,
}

impl LibraryCoordinate {
    pub fn group(&self) -> &str {
        &self.group
    }

    pub fn artifact(&self) -> &str {
        &self.artifact
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn classifier(&self) -> Option<&str> {
        self.classifier.as_deref()
    }

    /// The canonical Maven coordinate string (`group:artifact:version[:classifier]`).
    pub fn as_maven_string(&self) -> String {
        match &self.classifier {
            Some(classifier) => {
                format!(
                    "{}:{}:{}:{classifier}",
                    self.group, self.artifact, self.version
                )
            }
            None => format!("{}:{}:{}", self.group, self.artifact, self.version),
        }
    }

    /// The repository-relative path this coordinate occupies, using the
    /// Maven layout Mojang's metadata follows
    /// (`<artifact>-<version>[-<classifier>].jar`).
    fn repository_path(&self) -> String {
        let file = match &self.classifier {
            Some(classifier) => {
                format!("{}-{}-{classifier}.jar", self.artifact, self.version)
            }
            None => format!("{}-{}.jar", self.artifact, self.version),
        };
        format!(
            "{}/{}/{}/{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            file
        )
    }
}

/// Whether a planned library is an ordinary classpath library or a
/// platform-native artifact.
///
/// Modern metadata expresses natives as separate libraries whose coordinates
/// carry a `natives-…` classifier and whose rules restrict them to one
/// platform; the plan surfaces that distinction without re-deriving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    PlatformLibrary,
    NativeArtifact,
}

/// One library the installer must acquire for the target platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedLibrary {
    coordinate: LibraryCoordinate,
    /// The repository-relative path (forward slashes, `.jar`) the artifact
    /// occupies in the managed libraries layout.
    path: String,
    kind: LibraryKind,
    artifact: ArtifactRequirement,
}

impl PlannedLibrary {
    pub fn coordinate(&self) -> &LibraryCoordinate {
        &self.coordinate
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn kind(&self) -> LibraryKind {
        self.kind
    }

    pub fn artifact(&self) -> &ArtifactRequirement {
        &self.artifact
    }
}

/// One launch argument group with its feature conditions.
///
/// An empty feature list means the argument applies unconditionally on the
/// planned platform. Values are verbatim metadata strings: unresolved
/// `${placeholder}` tokens stay unresolved; substituting real values is a
/// launch resolution concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArgument {
    pub features: Vec<FeatureFlag>,
    pub values: Vec<String>,
}

/// The launch metadata consumed by launch-plan construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchMetadata {
    main_class: String,
    game_arguments: Vec<LaunchArgument>,
    jvm_arguments: Vec<LaunchArgument>,
}

impl LaunchMetadata {
    pub fn main_class(&self) -> &str {
        &self.main_class
    }

    /// The ordered game arguments for the planned platform; feature-
    /// conditioned arguments are present with their conditions attached.
    pub fn game_arguments(&self) -> &[LaunchArgument] {
        &self.game_arguments
    }

    /// The ordered JVM arguments for the planned platform.
    pub fn jvm_arguments(&self) -> &[LaunchArgument] {
        &self.jvm_arguments
    }
}

/// The complete, deterministic installation plan for one Minecraft version on
/// one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftInstallPlan {
    minecraft_version: String,
    version_type: VersionType,
    java: JavaRequirement,
    client: ArtifactRequirement,
    asset_index: AssetIndexRequirement,
    logging: Option<LoggingRequirement>,
    libraries: Vec<PlannedLibrary>,
    launch: LaunchMetadata,
}

impl MinecraftInstallPlan {
    pub fn minecraft_version(&self) -> &str {
        &self.minecraft_version
    }

    pub fn version_type(&self) -> VersionType {
        self.version_type
    }

    pub fn java(&self) -> &JavaRequirement {
        &self.java
    }

    pub fn client(&self) -> &ArtifactRequirement {
        &self.client
    }

    pub fn asset_index(&self) -> &AssetIndexRequirement {
        &self.asset_index
    }

    /// The official logging configuration requirement, when the version
    /// publishes one.
    pub fn logging(&self) -> Option<&LoggingRequirement> {
        self.logging.as_ref()
    }

    /// The applicable libraries in official metadata order — the deterministic
    /// classpath order for a future installer.
    pub fn libraries(&self) -> &[PlannedLibrary] {
        &self.libraries
    }

    pub fn launch(&self) -> &LaunchMetadata {
        &self.launch
    }

    /// How many planned libraries are platform-native artifacts.
    pub fn native_library_count(&self) -> usize {
        self.libraries
            .iter()
            .filter(|library| library.kind() == LibraryKind::NativeArtifact)
            .count()
    }
}

/// A failure while normalizing validated metadata into a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// A library entry is not usable as a structured, downloadable library.
    LibraryInvalid { name: String, reason: String },
    /// An artifact requirement is not usable (bad digest, URL, size, or path).
    ArtifactInvalid { source: String, reason: String },
    /// The document uses library/argument semantics planning does not support.
    Unsupported { reason: String },
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibraryInvalid { name, reason } => {
                write!(formatter, "library '{name}' cannot be planned: {reason}")
            }
            Self::ArtifactInvalid { source, reason } => {
                write!(
                    formatter,
                    "artifact requirement from '{source}' is invalid: {reason}"
                )
            }
            Self::Unsupported { reason } => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// Normalizes a validated version document into an install plan for one
/// platform.
///
/// Determinism: library and argument order follows the document's order
/// exactly; platform filtering never reorders. Placeholders and feature
/// conditions remain unresolved.
pub fn plan_version_document(
    document: &VersionDocument,
    platform: PlatformProfile,
) -> Result<MinecraftInstallPlan, PlanError> {
    let client = artifact_requirement(
        &format!("{} client", document.id),
        &document.downloads.client.sha1,
        document.downloads.client.size,
        &document.downloads.client.url,
        None,
        None,
    )?;
    let asset_index = AssetIndexRequirement {
        id: document.asset_index.id.clone(),
        artifact: artifact_requirement(
            &format!("{} asset index", document.id),
            &document.asset_index.sha1,
            document.asset_index.size,
            &document.asset_index.url,
            None,
            None,
        )?,
        total_size: document.asset_index.total_size,
    };

    let logging = document
        .logging
        .as_ref()
        .and_then(|logging| logging.client.as_ref())
        .map(|client| {
            let file_name = validate_logging_file_name(&client.file.id)?;
            Ok(LoggingRequirement {
                file_name,
                argument: validate_logging_argument(&client.argument)?,
                artifact: artifact_requirement(
                    &format!("{} logging configuration", document.id),
                    &client.file.sha1,
                    client.file.size,
                    &client.file.url,
                    None,
                    None,
                )?,
            })
        })
        .transpose()?;

    let mut libraries = Vec::with_capacity(document.libraries.len());
    for library in &document.libraries {
        match plan_decision(library.rules.as_deref(), platform) {
            PlanDecision::Excluded => continue,
            PlanDecision::IncludedIfFeatures(features) => {
                return Err(PlanError::Unsupported {
                    reason: format!(
                        "library '{}' is conditioned on launch features ({}); libraries must be platform-selected, not feature-selected",
                        library.name,
                        features
                            .iter()
                            .map(FeatureFlag::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
            PlanDecision::Included => libraries.push(plan_library(library)?),
        }
    }

    let launch = LaunchMetadata {
        main_class: document.main_class.clone(),
        game_arguments: plan_arguments(&document.arguments.game, platform)?,
        jvm_arguments: plan_arguments(&document.arguments.jvm, platform)?,
    };

    Ok(MinecraftInstallPlan {
        minecraft_version: document.id.clone(),
        version_type: document.kind,
        java: JavaRequirement {
            component: document.java_version.component.clone(),
            major_version: document.java_version.major_version,
        },
        client,
        asset_index,
        logging,
        libraries,
        launch,
    })
}

fn validate_logging_argument(argument: &str) -> Result<String, PlanError> {
    if argument.is_empty()
        || argument.len() > 1_024
        || argument.contains('\0')
        || argument.matches("${path}").count() != 1
    {
        return Err(PlanError::ArtifactInvalid {
            source: "logging configuration".to_owned(),
            reason: "the logging argument must be one bounded argument containing exactly one '${path}' placeholder"
                .to_owned(),
        });
    }
    Ok(argument.to_owned())
}

/// Validates an official logging-configuration file id as a single safe
/// installed file name: one segment over the conservative charset, not a
/// traversal shape, ending in `.xml` as every current official id does.
fn validate_logging_file_name(file_name: &str) -> Result<String, PlanError> {
    let invalid = |reason: String| PlanError::ArtifactInvalid {
        source: "logging configuration".to_owned(),
        reason,
    };

    if file_name.split('/').count() != 1 || file_name.split('\\').count() > 1 {
        return Err(invalid(
            "the logging configuration file id must be a single path segment".to_owned(),
        ));
    }
    if !file_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(invalid(
            "the logging configuration file id may only contain letters, digits, '.', '_', and '-'"
                .to_owned(),
        ));
    }
    if file_name.starts_with('.') || file_name == ".." || file_name.ends_with(".tmp") {
        return Err(invalid(
            "the logging configuration file id is not a safe file name".to_owned(),
        ));
    }
    if !file_name.ends_with(".xml") {
        return Err(invalid(
            "the logging configuration file id must end with '.xml'".to_owned(),
        ));
    }

    Ok(file_name.to_owned())
}

/// Builds one typed artifact requirement from an official artifact block.
///
/// When both paths are provided (libraries), the declared repository path
/// must equal the path derived from the library's Maven coordinate, so
/// structured identity and install layout can never drift apart silently.
/// Client and asset-index artifacts carry no repository path.
fn artifact_requirement(
    source: &str,
    sha1_text: &str,
    size: u64,
    url: &str,
    declared_path: Option<&str>,
    expected_path: Option<&str>,
) -> Result<ArtifactRequirement, PlanError> {
    let invalid = |reason: String| PlanError::ArtifactInvalid {
        source: source.to_owned(),
        reason,
    };

    let sha1 = Sha1Digest::parse(sha1_text).map_err(|error| invalid(error.to_string()))?;

    if size == 0 {
        return Err(invalid(
            "the declared size must be greater than zero".to_owned(),
        ));
    }

    if !is_secure_artifact_url(url) {
        return Err(invalid(format!(
            "the artifact URL must use HTTPS ('{url}' does not)"
        )));
    }

    if let (Some(declared), Some(expected)) = (declared_path, expected_path) {
        if declared != expected {
            return Err(invalid(format!(
                "the declared path '{declared}' does not match the Maven coordinate layout '{expected}'"
            )));
        }
    }

    Ok(ArtifactRequirement {
        url: url.to_owned(),
        sha1,
        size_bytes: size,
    })
}

/// Whether an official artifact URL satisfies the launcher's transport
/// policy: production HTTPS, with cleartext HTTP accepted only for explicit
/// loopback hosts — the same test-transport policy every other transport
/// boundary (metadata URLs, repositories) applies. Official production
/// metadata is always HTTPS; the loopback allowance exists so deterministic
/// offline tests can plan against local servers.
fn is_secure_artifact_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.cannot_be_a_base() {
        return false;
    }
    if parsed.scheme() == "https" {
        return true;
    }
    parsed.scheme() == "http" && crate::downloads::is_loopback_host(&parsed)
}

/// Plans one applicable library.
fn plan_library(library: &LibraryDocument) -> Result<PlannedLibrary, PlanError> {
    let coordinate = LibraryCoordinate::parse(&library.name)?;

    let artifact =
        library
            .downloads
            .artifact
            .as_ref()
            .ok_or_else(|| PlanError::LibraryInvalid {
                name: library.name.clone(),
                reason: "the library has no downloadable artifact".to_owned(),
            })?;
    let path = artifact
        .path
        .as_deref()
        .ok_or_else(|| PlanError::LibraryInvalid {
            name: library.name.clone(),
            reason: "the library artifact has no repository path".to_owned(),
        })?;
    validate_repository_path(path, &library.name)?;

    let requirement = artifact_requirement(
        &library.name,
        &artifact.sha1,
        artifact.size,
        &artifact.url,
        Some(path),
        Some(&coordinate.repository_path()),
    )?;

    let kind = match &coordinate.classifier {
        Some(classifier) if classifier.starts_with("natives-") => LibraryKind::NativeArtifact,
        _ => LibraryKind::PlatformLibrary,
    };

    Ok(PlannedLibrary {
        coordinate,
        path: path.to_owned(),
        kind,
        artifact: requirement,
    })
}

/// Flattens official argument entries into planned arguments for one platform.
fn plan_arguments(
    arguments: &[ArgumentDocument],
    platform: PlatformProfile,
) -> Result<Vec<LaunchArgument>, PlanError> {
    let mut planned = Vec::with_capacity(arguments.len());
    for entry in arguments {
        match entry {
            ArgumentDocument::Plain(value) => planned.push(LaunchArgument {
                features: Vec::new(),
                values: vec![value.clone()],
            }),
            ArgumentDocument::Conditional { rules, value } => {
                match plan_decision(Some(rules), platform) {
                    PlanDecision::Excluded => continue,
                    PlanDecision::Included => planned.push(LaunchArgument {
                        features: Vec::new(),
                        values: value.values(),
                    }),
                    PlanDecision::IncludedIfFeatures(features) => planned.push(LaunchArgument {
                        features,
                        values: value.values(),
                    }),
                }
            }
        }
    }
    Ok(planned)
}

impl LibraryCoordinate {
    /// Parses a Maven coordinate from official metadata.
    ///
    /// Only the shapes Mojang publishes are accepted: three or four
    /// non-empty segments over a conservative charset. This is deliberate
    /// validation for identity, not a general Maven client.
    pub fn parse(coordinate: &str) -> Result<Self, PlanError> {
        let invalid = |reason: String| PlanError::LibraryInvalid {
            name: coordinate.to_owned(),
            reason,
        };

        let segments: Vec<&str> = coordinate.split(':').collect();
        if segments.len() < 3 || segments.len() > 4 {
            return Err(invalid(format!(
                "a Maven coordinate needs 3 or 4 segments, found {}",
                segments.len()
            )));
        }

        for segment in &segments {
            if segment.is_empty() {
                return Err(invalid("coordinate segments must not be empty".to_owned()));
            }
            let valid = segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
            if !valid {
                return Err(invalid(
                    "coordinate segments may only contain letters, digits, '.', '_', and '-'"
                        .to_owned(),
                ));
            }
        }

        Ok(Self {
            group: segments[0].to_owned(),
            artifact: segments[1].to_owned(),
            version: segments[2].to_owned(),
            classifier: segments.get(3).map(|segment| (*segment).to_owned()),
        })
    }
}

/// Validates that a library repository path is a safe relative Maven-layout
/// path (forward slashes, no traversal, no absolute or Windows-specific
/// components) before any future installer places a file under it.
fn validate_repository_path(path: &str, name: &str) -> Result<(), PlanError> {
    let invalid = |reason: String| PlanError::LibraryInvalid {
        name: name.to_owned(),
        reason,
    };

    if path.starts_with('/') || path.starts_with('\\') || path.contains('\\') {
        return Err(invalid(
            "the repository path must be relative with forward slashes".to_owned(),
        ));
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(invalid(
            "the repository path must not traverse or repeat segments".to_owned(),
        ));
    }
    if path.split('/').any(|segment| {
        segment.ends_with(':')
            || segment
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    }) {
        return Err(invalid(
            "the repository path may only contain letters, digits, '.', '_', and '-' per segment"
                .to_owned(),
        ));
    }
    if !path.ends_with(".jar") {
        return Err(invalid(
            "the repository path must point at a .jar file".to_owned(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::metadata::VersionDocument;
    use crate::minecraft::rules::{Architecture, OperatingSystem, PlatformProfile as Profile};

    const WINDOWS_X64: Profile = Profile::new(OperatingSystem::Windows, Architecture::X86_64);
    const LINUX_X64: Profile = Profile::new(OperatingSystem::Linux, Architecture::X86_64);
    const MACOS_ARM64: Profile = Profile::new(OperatingSystem::MacOs, Architecture::Arm64);

    /// A complete representative modern version document (shape-faithful,
    /// values abridged) exercising every planning branch.
    fn fixture_document() -> String {
        r#"{
            "id": "26.2",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "javaVersion": { "component": "java-runtime-epsilon", "majorVersion": 25 },
            "assetIndex": {
                "id": "32",
                "sha1": "958c0c70425f7802b1c21ce25b84f3ffd66f778c",
                "size": 586366,
                "totalSize": 480492719,
                "url": "https://piston-meta.mojang.com/v1/packages/958c0c70425f7802b1c21ce25b84f3ffd66f778c/32.json"
            },
            "downloads": {
                "client": {
                    "sha1": "e6e7b5c2f8e0f8e7e6a1b2c3d4e5f60718293a4b",
                    "size": 33816576,
                    "url": "https://piston-data.mojang.com/v1/objects/e6e7b5c2f8e0f8e7e6a1b2c3d4e5f60718293a4b/client.jar"
                }
            },
            "libraries": [
                {
                    "downloads": { "artifact": {
                        "path": "at/yawk/lz4/lz4-java/1.10.1/lz4-java-1.10.1.jar",
                        "sha1": "f541d7f910fe3d76f38f799c507c48cc81b12ecb",
                        "size": 910232,
                        "url": "https://libraries.minecraft.net/at/yawk/lz4/lz4-java/1.10.1/lz4-java-1.10.1.jar"
                    } },
                    "name": "at.yawk.lz4:lz4-java:1.10.1"
                },
                {
                    "downloads": { "artifact": {
                        "path": "ca/weblite/java-objc-bridge/1.1/java-objc-bridge-1.1.jar",
                        "sha1": "1227f9e0666314f9de41477e3ec277e542ed7f7b",
                        "size": 1330045,
                        "url": "https://libraries.minecraft.net/ca/weblite/java-objc-bridge/1.1/java-objc-bridge-1.1.jar"
                    } },
                    "name": "ca.weblite:java-objc-bridge:1.1",
                    "rules": [ { "action": "allow", "os": { "name": "osx" } } ]
                },
                {
                    "downloads": { "artifact": {
                        "path": "com/mojang/jtracy/1.0.37/jtracy-1.0.37-natives-linux.jar",
                        "sha1": "e1b4395227af41195da9e2ead13c48ee0b7d31cd",
                        "size": 193951,
                        "url": "https://libraries.minecraft.net/com/mojang/jtracy/1.0.37/jtracy-1.0.37-natives-linux.jar"
                    } },
                    "name": "com.mojang:jtracy:1.0.37:natives-linux",
                    "rules": [ { "action": "allow", "os": { "name": "linux" } } ]
                },
                {
                    "downloads": { "artifact": {
                        "path": "org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar",
                        "sha1": "6ff6d710fd4ffd470f2799653fab57e27fed0c9d",
                        "size": 46763,
                        "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar"
                    } },
                    "name": "org.lwjgl:lwjgl:3.4.1:natives-windows",
                    "rules": [ { "action": "allow", "os": { "name": "windows" } } ]
                }
            ],
            "arguments": {
                "game": [
                    "--username", "${auth_player_name}",
                    "--version", "${version_name}",
                    { "rules": [ { "action": "allow", "features": { "is_demo_user": true } } ], "value": "--demo" },
                    { "rules": [ { "action": "allow", "features": { "has_custom_resolution": true } } ], "value": ["--width", "${resolution_width}", "--height", "${resolution_height}"] }
                ],
                "jvm": [
                    "-Djava.library.path=${natives_directory}",
                    { "rules": [ { "action": "allow", "os": { "name": "osx" } } ], "value": "-XstartOnFirstThread" },
                    { "rules": [ { "action": "allow", "os": { "name": "windows" } } ], "value": "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump" },
                    { "rules": [ { "action": "allow", "os": { "arch": "x86" } } ], "value": ["-Xss1M"] }
                ]
            }
        }"#
        .to_owned()
    }

    fn parsed_fixture() -> VersionDocument {
        VersionDocument::from_json(&fixture_document()).unwrap()
    }

    #[test]
    fn the_full_plan_contains_exactly_the_expected_semantic_requirements() {
        let plan = plan_version_document(&parsed_fixture(), WINDOWS_X64).unwrap();

        assert_eq!(plan.minecraft_version(), "26.2");
        assert_eq!(plan.version_type(), VersionType::Release);

        // Java requirement is recorded, not resolved.
        assert_eq!(plan.java().component(), "java-runtime-epsilon");
        assert_eq!(plan.java().major_version(), 25);

        // Client artifact requirement, official SHA-1 and size.
        assert_eq!(
            plan.client().url(),
            "https://piston-data.mojang.com/v1/objects/e6e7b5c2f8e0f8e7e6a1b2c3d4e5f60718293a4b/client.jar"
        );
        assert_eq!(
            plan.client().sha1().as_hex(),
            "e6e7b5c2f8e0f8e7e6a1b2c3d4e5f60718293a4b"
        );
        assert_eq!(plan.client().size_bytes(), 33816576);

        // Asset index requirement, including declared total size.
        assert_eq!(plan.asset_index().id(), "32");
        assert_eq!(plan.asset_index().total_size(), 480492719);
        assert!(plan.asset_index().artifact().url().starts_with("https://"));

        // Platform filtering: lz4 everywhere, macOS bridge excluded, the
        // windows native included, the linux native excluded. Order preserved.
        let names: Vec<String> = plan
            .libraries()
            .iter()
            .map(|library| library.coordinate().as_maven_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "at.yawk.lz4:lz4-java:1.10.1",
                "org.lwjgl:lwjgl:3.4.1:natives-windows",
            ]
        );
        assert_eq!(plan.native_library_count(), 1);
        assert_eq!(plan.libraries()[1].kind(), LibraryKind::NativeArtifact);
        assert_eq!(plan.libraries()[0].kind(), LibraryKind::PlatformLibrary);

        // Structured identity is preserved for classpath construction.
        assert_eq!(
            plan.libraries()[0].path(),
            "at/yawk/lz4/lz4-java/1.10.1/lz4-java-1.10.1.jar"
        );
        assert_eq!(plan.libraries()[0].coordinate().group(), "at.yawk.lz4");
        assert_eq!(plan.libraries()[0].coordinate().classifier(), None);

        // Launch metadata: main class, unconditional args in order,
        // platform-conditioned args resolved, feature args pending,
        // placeholders verbatim.
        assert_eq!(plan.launch().main_class(), "net.minecraft.client.main.Main");

        let game: Vec<&[String]> = plan
            .launch()
            .game_arguments()
            .iter()
            .filter(|argument| argument.features.is_empty())
            .map(|argument| argument.values.as_slice())
            .collect();
        assert_eq!(
            game,
            vec![
                vec!["--username".to_owned()],
                vec!["${auth_player_name}".to_owned()],
                vec!["--version".to_owned()],
                vec!["${version_name}".to_owned()],
            ],
            "unconditional game arguments resolve in document order"
        );
        let pending: Vec<_> = plan
            .launch()
            .game_arguments()
            .iter()
            .filter(|argument| !argument.features.is_empty())
            .collect();
        assert_eq!(
            pending.len(),
            2,
            "--demo and --width/--height remain pending"
        );
        assert_eq!(pending[0].features, vec![FeatureFlag::IsDemoUser]);
        assert_eq!(pending[0].values, vec!["--demo".to_owned()]);
        assert_eq!(pending[1].features, vec![FeatureFlag::HasCustomResolution]);
        assert_eq!(
            pending[1].values,
            vec![
                "--width".to_owned(),
                "${resolution_width}".to_owned(),
                "--height".to_owned(),
                "${resolution_height}".to_owned(),
            ]
        );

        let jvm: Vec<String> = plan
            .launch()
            .jvm_arguments()
            .iter()
            .flat_map(|argument| argument.values.iter().cloned())
            .collect();
        assert_eq!(
            jvm,
            vec![
                "-Djava.library.path=${natives_directory}".to_owned(),
                "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump".to_owned(),
            ],
            "-XstartOnFirstThread (osx) and -Xss1M (x86) must not apply on windows-x64"
        );
    }

    #[test]
    fn each_platform_plans_its_own_libraries_and_arguments() {
        let linux = plan_version_document(&parsed_fixture(), LINUX_X64).unwrap();
        let macos = plan_version_document(&parsed_fixture(), MACOS_ARM64).unwrap();

        let linux_names: Vec<String> = linux
            .libraries()
            .iter()
            .map(|library| library.coordinate().as_maven_string())
            .collect();
        assert_eq!(
            linux_names,
            vec![
                "at.yawk.lz4:lz4-java:1.10.1",
                "com.mojang:jtracy:1.0.37:natives-linux"
            ]
        );
        assert_eq!(linux.native_library_count(), 1);

        let macos_names: Vec<String> = macos
            .libraries()
            .iter()
            .map(|library| library.coordinate().as_maven_string())
            .collect();
        assert_eq!(
            macos_names,
            vec![
                "at.yawk.lz4:lz4-java:1.10.1",
                "ca.weblite:java-objc-bridge:1.1"
            ]
        );
        assert_eq!(macos.native_library_count(), 0);

        assert!(
            macos
                .launch()
                .jvm_arguments()
                .iter()
                .any(|argument| argument.values == vec!["-XstartOnFirstThread".to_owned()])
        );
        assert!(!linux.launch().jvm_arguments().iter().any(|argument| {
            argument
                .values
                .iter()
                .any(|value| value == "-XstartOnFirstThread")
        }));
    }

    #[test]
    fn planning_is_deterministic_for_one_document_and_platform() {
        let document = parsed_fixture();
        let first = plan_version_document(&document, WINDOWS_X64).unwrap();
        let second = plan_version_document(&document, WINDOWS_X64).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn malformed_coordinates_and_paths_are_rejected_deliberately() {
        assert_eq!(
            LibraryCoordinate::parse("only-two:segments"),
            Err(PlanError::LibraryInvalid {
                name: "only-two:segments".to_owned(),
                reason: "a Maven coordinate needs 3 or 4 segments, found 2".to_owned(),
            })
        );
        assert!(LibraryCoordinate::parse("g:a:1:classifier:extra").is_err());
        assert!(LibraryCoordinate::parse("g:a:").is_err());
        assert!(LibraryCoordinate::parse("g:a:1.0:bad classifier").is_err());
        assert!(LibraryCoordinate::parse("g:a:1.0").is_ok());

        let coordinate = LibraryCoordinate::parse("org.lwjgl:lwjgl:3.4.1:natives-windows").unwrap();
        assert_eq!(
            coordinate.repository_path(),
            "org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar",
            "the classifier follows the version, matching official layout"
        );
    }

    #[test]
    fn inconsistent_or_unsafe_artifact_metadata_is_rejected() {
        fn mutated_document(mutate: impl FnOnce(&mut serde_json::Value)) -> VersionDocument {
            let mut value: serde_json::Value = serde_json::from_str(&fixture_document()).unwrap();
            mutate(&mut value);
            VersionDocument::from_json(&value.to_string()).unwrap()
        }
        fn first_artifact_mutate(field: &str, value: serde_json::Value) -> VersionDocument {
            mutated_document(|document| {
                document["libraries"][0]["downloads"]["artifact"][field] = value;
            })
        }

        // Path disagrees with the Maven coordinate.
        let mismatched = mutated_document(|document| {
            document["libraries"][0]["downloads"]["artifact"]["path"] =
                "somewhere/else/entirely.jar".into();
        });
        assert!(matches!(
            plan_version_document(&mismatched, WINDOWS_X64),
            Err(PlanError::ArtifactInvalid { .. })
        ));

        // Non-HTTPS artifact URL.
        let insecure = first_artifact_mutate(
            "url",
            "http://libraries.minecraft.net/at/yawk/lz4/lz4-java/1.10.1/lz4-java-1.10.1.jar".into(),
        );
        assert!(matches!(
            plan_version_document(&insecure, WINDOWS_X64),
            Err(PlanError::ArtifactInvalid { .. })
        ));

        // Zero size.
        let zero_size = first_artifact_mutate("size", 0.into());
        assert!(matches!(
            plan_version_document(&zero_size, WINDOWS_X64),
            Err(PlanError::ArtifactInvalid { .. })
        ));

        // Traversal in the repository path.
        let traversal = first_artifact_mutate("path", "../../evil.jar".into());
        assert!(matches!(
            plan_version_document(&traversal, WINDOWS_X64),
            Err(PlanError::LibraryInvalid { .. })
        ));

        // Malformed digest.
        let bad_digest = first_artifact_mutate("sha1", "deadbeef".into());
        assert!(matches!(
            plan_version_document(&bad_digest, WINDOWS_X64),
            Err(PlanError::ArtifactInvalid { .. })
        ));
    }

    #[test]
    fn a_library_without_a_downloadable_artifact_is_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(&fixture_document()).unwrap();
        value["libraries"][0]["downloads"] = serde_json::json!({});
        let document = VersionDocument::from_json(&value.to_string()).unwrap();

        assert!(matches!(
            plan_version_document(&document, WINDOWS_X64),
            Err(PlanError::LibraryInvalid { .. })
        ));
    }

    #[test]
    fn feature_conditioned_libraries_are_refused_rather_than_guessed() {
        let mut value: serde_json::Value = serde_json::from_str(&fixture_document()).unwrap();
        value["libraries"][0]["rules"] = serde_json::json!([
            { "action": "allow", "features": { "is_demo_user": true } }
        ]);
        let document = VersionDocument::from_json(&value.to_string()).unwrap();

        assert!(matches!(
            plan_version_document(&document, WINDOWS_X64),
            Err(PlanError::Unsupported { .. })
        ));
    }

    #[test]
    fn a_version_with_a_logging_block_plans_the_configuration_requirement() {
        let mut value: serde_json::Value = serde_json::from_str(&fixture_document()).unwrap();
        value["logging"] = serde_json::json!({
            "client": {
                "argument": "-Dlog4j.configurationFile=${path}",
                "file": {
                    "id": "client-1.21.2.xml",
                    "sha1": "39384bd14c0606d812afec88d8aff595b2587dd9",
                    "size": 1073,
                    "url": "https://piston-data.mojang.com/v1/objects/39384bd14c0606d812afec88d8aff595b2587dd9/client-1.21.2.xml"
                },
                "type": "log4j2-xml"
            }
        });
        let document = VersionDocument::from_json(&value.to_string()).unwrap();

        let plan = plan_version_document(&document, WINDOWS_X64).unwrap();
        let logging = plan.logging().expect("the requirement must be planned");

        assert_eq!(logging.file_name(), "client-1.21.2.xml");
        assert_eq!(logging.argument(), "-Dlog4j.configurationFile=${path}");
        assert_eq!(
            logging.artifact().sha1().as_hex(),
            "39384bd14c0606d812afec88d8aff595b2587dd9"
        );
        assert_eq!(logging.artifact().size_bytes(), 1073);
        assert_eq!(
            plan_version_document(&parsed_fixture(), WINDOWS_X64)
                .unwrap()
                .logging(),
            None,
            "the base fixture carries no logging block and plans without one"
        );
    }

    #[test]
    fn logging_file_ids_must_be_safe_single_segment_xml_names() {
        assert_eq!(
            validate_logging_file_name("client-1.21.2.xml").unwrap(),
            "client-1.21.2.xml"
        );
        for broken in [
            "nested/client.xml",
            "..",
            ".hidden.xml",
            "weird name.xml",
            "client.json",
            "client.xml/../escape.xml",
            "",
        ] {
            assert!(
                validate_logging_file_name(broken).is_err(),
                "{broken:?} must be rejected as a logging file name"
            );
        }
    }

    #[test]
    fn logging_argument_must_contain_exactly_one_path_placeholder() {
        assert_eq!(
            validate_logging_argument("-Dlog4j.configurationFile=${path}").unwrap(),
            "-Dlog4j.configurationFile=${path}"
        );
        for broken in [
            "-Dlog4j.configurationFile=client.xml",
            "${path}${path}",
            "-Dlog=${other}",
            "-Dlog=${path}\0",
        ] {
            assert!(
                validate_logging_argument(broken).is_err(),
                "{broken:?} must be rejected as a logging argument"
            );
        }
    }

    #[test]
    fn loopback_http_artifact_urls_are_the_only_cleartext_urls_accepted() {
        // The planning boundary accepts the documented test-transport policy:
        // HTTPS production URLs, plus cleartext loopback for offline tests.
        assert!(is_secure_artifact_url(
            "https://piston-data.mojang.com/v1/objects/abc/client.jar"
        ));
        assert!(is_secure_artifact_url("http://127.0.0.1:9123/client.jar"));
        assert!(is_secure_artifact_url("http://localhost:9123/client.jar"));
        assert!(!is_secure_artifact_url(
            "http://piston-data.mojang.com/v1/objects/abc/client.jar"
        ));
        assert!(!is_secure_artifact_url("ftp://example.invalid/client.jar"));
        assert!(!is_secure_artifact_url("not a url"));
    }
}
