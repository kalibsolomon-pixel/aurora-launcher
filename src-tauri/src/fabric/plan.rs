//! The normalized Fabric plan and the composed game install plan.
//!
//! This module converts a validated loader profile document into Aurora's own
//! Fabric representation (`FabricPlan`), then composes that plan with a
//! vanilla `MinecraftInstallPlan` into the complete software requirement set
//! for an Aurora/Fabric installation (`GameInstallPlan`).
//!
//! Both outputs are *plans*: no library is downloaded, nothing is installed or
//! extracted, and no process is launched. A future installer consumes the
//! composed plan and never re-parses Fabric Meta JSON.
//!
//! ## Library composition policy (deliberate)
//!
//! The official composition order — verified against fabric-meta's own
//! profile builder and the live `/profile/json` output — is: the loader's
//! `common` libraries in document order, then the intermediary artifact (only
//! for Minecraft versions that ship obfuscated names, indicated by a
//! non-placeholder intermediary), then the Fabric Loader artifact itself,
//! then the `client`-side library group. Aurora appends that ordered Fabric
//! set after the Mojang libraries of the vanilla plan, producing the
//! deterministic classpath order launchers use.
//!
//! Collisions are never silently resolved:
//!
//! - an *exact* duplicate coordinate (same group, artifact, version, and
//!   classifier) collapses to one requirement — the earlier entry wins, so a
//!   library Mojang already provides is not downloaded twice;
//! - the same group, artifact, and classifier at *different versions* is a
//!   hard composition conflict — Aurora picks no winner;
//! - different classifiers of one group and artifact legitimately coexist
//!   (that is how Mojang publishes platform natives).

use std::fmt;

use url::Url;

use crate::fabric::maven::{MavenCoordinate, MavenRepository};
use crate::fabric::metadata::{
    LoaderProfileDocument, LoaderVersionId, MetaLibraryDocument, NOOP_INTERMEDIARY_VERSION,
};
use crate::integrity::ArtifactDigest;
use crate::minecraft::metadata::MinecraftVersionId;
use crate::minecraft::plan::{LibraryCoordinate, MinecraftInstallPlan, PlannedLibrary};

/// One Fabric-provided artifact requirement.
///
/// `sha256` and `size_bytes` are the values official Fabric metadata
/// publishes; they are optional because the loader and intermediary
/// artifacts Fabric Meta adds to a profile carry no digests at all. A `None`
/// digest is an honest trust limitation, never a fabricated value: acquiring
/// such an artifact is a deliberate decision for the installation phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabricArtifact {
    url: Url,
    sha256: Option<ArtifactDigest>,
    size_bytes: Option<u64>,
}

impl FabricArtifact {
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The official expected SHA-256 digest, when Fabric publishes one.
    pub fn sha256(&self) -> Option<&ArtifactDigest> {
        self.sha256.as_ref()
    }

    /// The official expected byte size, when Fabric publishes one.
    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }

    /// Whether a pre-known official digest exists for this artifact.
    pub fn has_official_digest(&self) -> bool {
        self.sha256.is_some()
    }
}

/// Where a Fabric library comes from within the official profile composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FabricLibraryRole {
    /// A library from the loader's `launcherMeta.libraries.common` group.
    Common,
    /// The intermediary mappings artifact (obfuscated Minecraft versions).
    Intermediary,
    /// The Fabric Loader artifact itself.
    Loader,
    /// A library from the `launcherMeta.libraries.client` group.
    ClientSide,
}

/// One Fabric library the installer must acquire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabricLibrary {
    coordinate: MavenCoordinate,
    repository: MavenRepository,
    role: FabricLibraryRole,
    artifact: FabricArtifact,
}

impl FabricLibrary {
    pub fn coordinate(&self) -> &MavenCoordinate {
        &self.coordinate
    }

    pub fn repository(&self) -> &MavenRepository {
        &self.repository
    }

    pub fn role(&self) -> FabricLibraryRole {
        self.role
    }

    pub fn artifact(&self) -> &FabricArtifact {
        &self.artifact
    }

    /// The repository-relative Maven layout path of the artifact.
    pub fn path(&self) -> String {
        self.coordinate.repository_path()
    }
}

/// The normalized, Fabric-owned half of an installation plan.
///
/// The plan carries everything a future installer needs for the Fabric layer
/// — exact versions, the Fabric client entry point, the composed library set
/// with repositories, digests where officially published, and the loader's
/// declared Java floor — without retaining any Fabric Meta JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabricPlan {
    minecraft_version: String,
    loader_version: String,
    main_class: String,
    libraries: Vec<FabricLibrary>,
    min_java_major_version: u32,
}

impl FabricPlan {
    pub fn minecraft_version(&self) -> &str {
        &self.minecraft_version
    }

    pub fn loader_version(&self) -> &str {
        &self.loader_version
    }

    /// The Fabric client launch entry point (for example KnotClient).
    pub fn main_class(&self) -> &str {
        &self.main_class
    }

    /// The Fabric libraries in official composition order.
    pub fn libraries(&self) -> &[FabricLibrary] {
        &self.libraries
    }

    /// The minimum Java major version the loader metadata declares.
    pub fn min_java_major_version(&self) -> u32 {
        self.min_java_major_version
    }

    /// How many Fabric libraries carry an official pre-known digest.
    pub fn digested_library_count(&self) -> usize {
        self.libraries
            .iter()
            .filter(|library| library.artifact().has_official_digest())
            .count()
    }
}

/// A failure while normalizing a validated profile into a Fabric plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FabricPlanError {
    /// A library entry is not usable as a structured, downloadable library.
    LibraryInvalid { name: String, reason: String },
    /// A repository base URL is not usable.
    RepositoryInvalid { url: String, reason: String },
    /// A digest or size expectation is not usable.
    ArtifactInvalid { source: String, reason: String },
}

impl fmt::Display for FabricPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibraryInvalid { name, reason } => {
                write!(
                    formatter,
                    "Fabric library '{name}' cannot be planned: {reason}"
                )
            }
            Self::RepositoryInvalid { url, reason } => {
                write!(
                    formatter,
                    "Fabric repository '{url}' cannot be used: {reason}"
                )
            }
            Self::ArtifactInvalid { source, reason } => write!(
                formatter,
                "Fabric artifact expectation from '{source}' is invalid: {reason}"
            ),
        }
    }
}

impl std::error::Error for FabricPlanError {}

impl From<crate::fabric::maven::InvalidMavenCoordinate> for FabricPlanError {
    fn from(error: crate::fabric::maven::InvalidMavenCoordinate) -> Self {
        Self::LibraryInvalid {
            name: error.coordinate,
            reason: error.reason,
        }
    }
}

impl From<crate::fabric::maven::InvalidMavenRepository> for FabricPlanError {
    fn from(error: crate::fabric::maven::InvalidMavenRepository) -> Self {
        Self::RepositoryInvalid {
            url: error.url,
            reason: error.reason,
        }
    }
}

/// Normalizes a validated loader profile document into a Fabric plan.
///
/// Determinism: libraries follow the official composition order exactly
/// (common, then the intermediary when it is not the no-op placeholder, then
/// the loader, then the client group); that order never depends on the host.
pub fn plan_loader_profile(
    document: &LoaderProfileDocument,
    game: &MinecraftVersionId,
    loader: &LoaderVersionId,
) -> Result<FabricPlan, FabricPlanError> {
    let loader_repository = MavenRepository::parse(MavenRepository::OFFICIAL_FABRIC_URL)?;

    let mut libraries: Vec<FabricLibrary> =
        Vec::with_capacity(document.launcher_meta.libraries.common.len() + 3);

    for library in &document.launcher_meta.libraries.common {
        libraries.push(plan_meta_library(library, FabricLibraryRole::Common)?);
    }

    if document.intermediary.version != NOOP_INTERMEDIARY_VERSION {
        let coordinate = MavenCoordinate::parse(&document.intermediary.maven)?;
        libraries.push(FabricLibrary {
            artifact: FabricArtifact {
                url: loader_repository.artifact_url(&coordinate),
                sha256: None,
                size_bytes: None,
            },
            coordinate,
            repository: loader_repository.clone(),
            role: FabricLibraryRole::Intermediary,
        });
    }

    let loader_coordinate = MavenCoordinate::parse(&document.loader.maven)?;
    libraries.push(FabricLibrary {
        artifact: FabricArtifact {
            url: loader_repository.artifact_url(&loader_coordinate),
            sha256: None,
            size_bytes: None,
        },
        coordinate: loader_coordinate,
        repository: loader_repository,
        role: FabricLibraryRole::Loader,
    });

    for library in &document.launcher_meta.libraries.client {
        libraries.push(plan_meta_library(library, FabricLibraryRole::ClientSide)?);
    }

    Ok(FabricPlan {
        minecraft_version: game.to_string(),
        loader_version: loader.to_string(),
        main_class: document.launcher_meta.main_class.client().to_owned(),
        libraries,
        min_java_major_version: document.launcher_meta.min_java_version,
    })
}

/// Normalizes one `launcherMeta` library entry (a digested, repository-
/// addressed library).
fn plan_meta_library(
    library: &MetaLibraryDocument,
    role: FabricLibraryRole,
) -> Result<FabricLibrary, FabricPlanError> {
    let coordinate = MavenCoordinate::parse(&library.name)?;
    let repository = MavenRepository::parse(&library.url)?;

    let sha256 = library
        .sha256
        .as_deref()
        .map(|hex| {
            ArtifactDigest::parse(hex).map_err(|error| FabricPlanError::ArtifactInvalid {
                source: library.name.clone(),
                reason: error.to_string(),
            })
        })
        .transpose()?;

    if let Some(size) = library.size {
        if size == 0 {
            return Err(FabricPlanError::ArtifactInvalid {
                source: library.name.clone(),
                reason: "the declared size must be greater than zero".to_owned(),
            });
        }
    }

    Ok(FabricLibrary {
        artifact: FabricArtifact {
            url: repository.artifact_url(&coordinate),
            sha256,
            size_bytes: library.size,
        },
        coordinate,
        repository,
        role,
    })
}

/// The effective Java requirement of a composed installation.
///
/// Minecraft's runtime component stays the runtime family a future installer
/// obtains; the effective minimum major version is the stricter of Mojang's
/// requirement and the loader's declared floor. `raised_by_loader` records
/// the (so far unobserved) case where Fabric demands more than Mojang.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameJavaRequirement {
    component: String,
    major_version: u32,
    loader_min_major_version: u32,
    raised_by_loader: bool,
}

impl GameJavaRequirement {
    pub fn component(&self) -> &str {
        &self.component
    }

    pub fn major_version(&self) -> u32 {
        self.major_version
    }

    pub fn loader_min_major_version(&self) -> u32 {
        self.loader_min_major_version
    }

    pub fn raised_by_loader(&self) -> bool {
        self.raised_by_loader
    }
}

/// One entry of the composed library set, with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameLibrary {
    Minecraft(PlannedLibrary),
    Fabric(FabricLibrary),
}

/// Which metadata source contributed a composed library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibraryProvenance {
    Mojang,
    Fabric,
}

impl GameLibrary {
    pub fn provenance(&self) -> LibraryProvenance {
        match self {
            Self::Minecraft(_) => LibraryProvenance::Mojang,
            Self::Fabric(_) => LibraryProvenance::Fabric,
        }
    }

    /// The canonical Maven coordinate string of this library.
    pub fn coordinate_string(&self) -> String {
        match self {
            Self::Minecraft(library) => library.coordinate().as_maven_string(),
            Self::Fabric(library) => library.coordinate().as_maven_string(),
        }
    }

    /// The repository-relative Maven layout path of this library.
    pub fn repository_path(&self) -> String {
        match self {
            Self::Minecraft(library) => library.path().to_owned(),
            Self::Fabric(library) => library.path(),
        }
    }
}

/// The complete, deterministic plan for one Aurora/Fabric installation.
///
/// Composition is explicit containment: the vanilla plan and the Fabric plan
/// remain independently meaningful parts (provenance is preserved for
/// diagnostics and repair), while the derived views — the ordered composed
/// library set, the effective Java requirement, and the final entry point —
/// are what a future installer consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInstallPlan {
    minecraft: MinecraftInstallPlan,
    loader: FabricPlan,
    libraries: Vec<GameLibrary>,
    java: GameJavaRequirement,
    main_class: String,
}

impl GameInstallPlan {
    /// The vanilla Minecraft half, unchanged by composition.
    pub fn minecraft(&self) -> &MinecraftInstallPlan {
        &self.minecraft
    }

    /// The Fabric half, unchanged by composition.
    pub fn loader(&self) -> &FabricPlan {
        &self.loader
    }

    /// The composed library set in deterministic classpath order: Mojang
    /// libraries in official document order, then the Fabric libraries in
    /// official composition order, with exact duplicates collapsed.
    pub fn libraries(&self) -> &[GameLibrary] {
        &self.libraries
    }

    /// The effective Java requirement.
    pub fn java(&self) -> &GameJavaRequirement {
        &self.java
    }

    /// The final launch entry point: the Fabric client main class.
    pub fn main_class(&self) -> &str {
        &self.main_class
    }

    pub fn vanilla_library_count(&self) -> usize {
        self.libraries
            .iter()
            .filter(|library| library.provenance() == LibraryProvenance::Mojang)
            .count()
    }

    pub fn fabric_library_count(&self) -> usize {
        self.libraries
            .iter()
            .filter(|library| library.provenance() == LibraryProvenance::Fabric)
            .count()
    }
}

/// A failure while composing the vanilla and Fabric plans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionError {
    /// The two plans describe different Minecraft versions.
    VersionMismatch {
        minecraft_version: String,
        fabric_version: String,
    },
    /// Two requirements claim the same library identity at different
    /// versions; Aurora resolves no winner.
    LibraryConflict {
        existing: String,
        conflicting: String,
    },
}

impl fmt::Display for CompositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionMismatch {
                minecraft_version,
                fabric_version,
            } => write!(
                formatter,
                "the Minecraft plan describes '{minecraft_version}' but the Fabric plan describes '{fabric_version}'"
            ),
            Self::LibraryConflict {
                existing,
                conflicting,
            } => write!(
                formatter,
                "library identity conflict: '{conflicting}' conflicts with the already-planned '{existing}'"
            ),
        }
    }
}

impl std::error::Error for CompositionError {}

/// The identity two library requirements must share to be the *same*
/// artifact: group, artifact, and classifier (version decides duplicate
/// versus conflict). Different classifiers of one group and artifact are
/// genuinely different artifacts — that is how Mojang publishes natives.
type LibraryIdentity<'a> = (&'a str, &'a str, Option<&'a str>);

fn mojang_identity(coordinate: &LibraryCoordinate) -> LibraryIdentity<'_> {
    (
        coordinate.group(),
        coordinate.artifact(),
        coordinate.classifier(),
    )
}

fn fabric_identity(coordinate: &MavenCoordinate) -> LibraryIdentity<'_> {
    (
        coordinate.group(),
        coordinate.artifact(),
        coordinate.classifier(),
    )
}

/// Composes a vanilla Minecraft plan and a Fabric plan into the complete
/// game install plan.
///
/// The inputs are not mutated; composition validates version agreement and
/// library-identity collisions, then derives the ordered library set, the
/// effective Java requirement, and the final main class.
pub fn compose_game_plan(
    minecraft: MinecraftInstallPlan,
    fabric: FabricPlan,
) -> Result<GameInstallPlan, CompositionError> {
    if minecraft.minecraft_version() != fabric.minecraft_version() {
        return Err(CompositionError::VersionMismatch {
            minecraft_version: minecraft.minecraft_version().to_owned(),
            fabric_version: fabric.minecraft_version().to_owned(),
        });
    }

    let mut libraries: Vec<GameLibrary> =
        Vec::with_capacity(minecraft.libraries().len() + fabric.libraries().len());

    // Exact duplicates collapse to the earlier entry (Mojang first); a same
    // identity at a different version is a hard conflict.
    let mut push = |library: GameLibrary,
                    identity: LibraryIdentity<'_>,
                    version: &str|
     -> Result<(), CompositionError> {
        for planned in &libraries {
            let (existing_identity, existing_version) = match planned {
                GameLibrary::Minecraft(entry) => (
                    mojang_identity(entry.coordinate()),
                    entry.coordinate().version(),
                ),
                GameLibrary::Fabric(entry) => (
                    fabric_identity(entry.coordinate()),
                    entry.coordinate().version(),
                ),
            };
            if existing_identity == identity {
                if existing_version == version {
                    return Ok(()); // exact duplicate: one requirement suffices
                }
                return Err(CompositionError::LibraryConflict {
                    existing: planned.coordinate_string(),
                    conflicting: library.coordinate_string(),
                });
            }
        }
        libraries.push(library);
        Ok(())
    };

    for entry in minecraft.libraries() {
        let identity = mojang_identity(entry.coordinate());
        let version = entry.coordinate().version().to_owned();
        push(GameLibrary::Minecraft(entry.clone()), identity, &version)?;
    }
    for entry in fabric.libraries() {
        let identity = fabric_identity(entry.coordinate());
        let version = entry.coordinate().version().to_owned();
        push(GameLibrary::Fabric(entry.clone()), identity, &version)?;
    }

    let minecraft_java = minecraft.java();
    let loader_min = fabric.min_java_major_version();
    let raised_by_loader = loader_min > minecraft_java.major_version();
    let java = GameJavaRequirement {
        component: minecraft_java.component().to_owned(),
        major_version: minecraft_java.major_version().max(loader_min),
        loader_min_major_version: loader_min,
        raised_by_loader,
    };

    let main_class = fabric.main_class().to_owned();

    Ok(GameInstallPlan {
        minecraft,
        loader: fabric,
        libraries,
        java,
        main_class,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::metadata::LoaderProfileDocument;
    use crate::minecraft::metadata::VersionDocument;
    use crate::minecraft::plan::plan_version_document;
    use crate::minecraft::rules::{OperatingSystem, PlatformProfile as Profile};

    const WINDOWS_X64: Profile = Profile::new(
        OperatingSystem::Windows,
        crate::minecraft::rules::Architecture::X86_64,
    );

    /// A loader profile representative of live Fabric Meta data (abridged):
    /// two digested common libraries plus loader, with either a real or the
    /// no-op intermediary.
    fn profile_document(intermediary_version: &str) -> LoaderProfileDocument {
        let intermediary_maven = if intermediary_version == NOOP_INTERMEDIARY_VERSION {
            "net.fabricmc:intermediary:0.0.0".to_owned()
        } else {
            format!("net.fabricmc:intermediary:{intermediary_version}")
        };
        let json = format!(
            r#"{{
                "loader": {{ "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true }},
                "intermediary": {{ "maven": "{intermediary_maven}", "version": "{intermediary_version}", "stable": true }},
                "launcherMeta": {{
                    "version": 2,
                    "min_java_version": 8,
                    "libraries": {{
                        "client": [],
                        "common": [
                            {{
                                "name": "org.ow2.asm:asm:9.10.1",
                                "url": "https://maven.fabricmc.net/",
                                "sha256": "ed825d10ab1399c8c0cb669e688cf0c8c82629b4c8399b58352b68e92ca10fcb",
                                "size": 126151
                            }},
                            {{
                                "name": "net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7",
                                "url": "https://maven.fabricmc.net/",
                                "sha256": "1f0ae44db7295f8626f33b1dc0ad7f043d8954a8d6847247875fcc5dfcecc934",
                                "size": 1539080
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
            }}"#
        );
        LoaderProfileDocument::from_json(&json).unwrap()
    }

    fn fabric_plan(intermediary_version: &str) -> FabricPlan {
        plan_loader_profile(
            &profile_document(intermediary_version),
            &MinecraftVersionId::new("1.21.11").unwrap(),
            &LoaderVersionId::new("0.19.5").unwrap(),
        )
        .unwrap()
    }

    /// A minimal modern Mojang version document whose library list is
    /// parameterized, so composition tests can construct duplicates and
    /// conflicts deliberately.
    fn minecraft_plan(libraries: &[&str]) -> MinecraftInstallPlan {
        assert!(
            !libraries.is_empty(),
            "official documents always declare libraries"
        );
        let library_entries: Vec<String> = libraries
            .iter()
            .map(|name| {
                let mut segments = name.split(':');
                let group = segments.next().unwrap();
                let artifact = segments.next().unwrap();
                let version = segments.next().unwrap();
                let classifier = segments.next();
                let group_path = group.replace('.', "/");
                let file = match classifier {
                    Some(classifier) => format!("{artifact}-{version}-{classifier}.jar"),
                    None => format!("{artifact}-{version}.jar"),
                };
                let path = format!("{group_path}/{artifact}/{version}/{file}");
                format!(
                    r#"{{
                        "downloads": {{ "artifact": {{
                            "path": "{path}",
                            "sha1": "1832adbc4eee60faa097bb1409be305a0abbf3d2",
                            "size": 1000,
                            "url": "https://libraries.minecraft.net/{path}"
                        }} }},
                        "name": "{name}"
                    }}"#
                )
            })
            .collect();

        let json = format!(
            r#"{{
                "id": "1.21.11",
                "type": "release",
                "mainClass": "net.minecraft.client.main.Main",
                "javaVersion": {{ "component": "java-runtime-delta", "majorVersion": 21 }},
                "assetIndex": {{
                    "id": "31",
                    "sha1": "1832adbc4eee60faa097bb1409be305a0abbf3d2",
                    "size": 400253,
                    "totalSize": 470877946,
                    "url": "https://piston-meta.mojang.com/v1/packages/1832adbc4eee60faa097bb1409be305a0abbf3d2/31.json"
                }},
                "downloads": {{
                    "client": {{
                        "sha1": "1832adbc4eee60faa097bb1409be305a0abbf3d2",
                        "size": 26000000,
                        "url": "https://piston-data.mojang.com/v1/objects/1832adbc4eee60faa097bb1409be305a0abbf3d2/client.jar"
                    }}
                }},
                "libraries": [{}],
                "arguments": {{
                    "game": ["--username", "${{auth_player_name}}"],
                    "jvm": ["-Djava.library.path=${{natives_directory}}"]
                }}
            }}"#,
            library_entries.join(",")
        );

        let document = VersionDocument::from_json(&json).unwrap();
        plan_version_document(&document, WINDOWS_X64).unwrap()
    }

    #[test]
    fn a_fabric_plan_records_versions_entry_point_and_ordered_libraries() {
        let plan = fabric_plan("1.21.11");

        assert_eq!(plan.minecraft_version(), "1.21.11");
        assert_eq!(plan.loader_version(), "0.19.5");
        assert_eq!(
            plan.main_class(),
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_eq!(plan.min_java_major_version(), 8);

        // Official composition order: common, intermediary, loader, client.
        let coordinates: Vec<String> = plan
            .libraries()
            .iter()
            .map(|library| library.coordinate().as_maven_string())
            .collect();
        assert_eq!(
            coordinates,
            vec![
                "org.ow2.asm:asm:9.10.1",
                "net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7",
                "net.fabricmc:intermediary:1.21.11",
                "net.fabricmc:fabric-loader:0.19.5",
            ]
        );
        assert_eq!(
            plan.libraries()[0].role(),
            FabricLibraryRole::Common,
            "common libraries lead"
        );
        assert_eq!(plan.libraries()[2].role(), FabricLibraryRole::Intermediary);
        assert_eq!(plan.libraries()[3].role(), FabricLibraryRole::Loader);
    }

    #[test]
    fn the_noop_intermediary_placeholder_contributes_no_library() {
        let plan = fabric_plan(NOOP_INTERMEDIARY_VERSION);

        let coordinates: Vec<String> = plan
            .libraries()
            .iter()
            .map(|library| library.coordinate().as_maven_string())
            .collect();
        assert_eq!(
            coordinates,
            vec![
                "org.ow2.asm:asm:9.10.1",
                "net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7",
                "net.fabricmc:fabric-loader:0.19.5",
            ],
            "unobfuscated Minecraft versions need no intermediary"
        );
    }

    #[test]
    fn fabric_libraries_carry_derived_urls_and_honest_digests() {
        let plan = fabric_plan("1.21.11");

        // Digested common library: derived deterministic URL plus the
        // official SHA-256 and size.
        let asm = &plan.libraries()[0];
        assert_eq!(
            asm.artifact().url().as_str(),
            "https://maven.fabricmc.net/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar"
        );
        assert_eq!(
            asm.artifact().sha256().unwrap().as_hex(),
            "ed825d10ab1399c8c0cb669e688cf0c8c82629b4c8399b58352b68e92ca10fcb"
        );
        assert_eq!(asm.artifact().size_bytes(), Some(126151));
        assert!(asm.artifact().has_official_digest());

        // The loader artifact has no published digest and says so honestly.
        let loader = &plan.libraries()[3];
        assert_eq!(
            loader.artifact().url().as_str(),
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.19.5/fabric-loader-0.19.5.jar"
        );
        assert_eq!(loader.artifact().sha256(), None);
        assert_eq!(loader.artifact().size_bytes(), None);
        assert!(!loader.artifact().has_official_digest());

        assert_eq!(
            plan.digested_library_count(),
            2,
            "only the digested common libraries count as pre-verified"
        );
    }

    #[test]
    fn malformed_libraries_repositories_and_digests_fail_normalization() {
        let mut document = profile_document("1.21.11");

        document.launcher_meta.libraries.common[0].name = "not a coordinate".to_owned();
        assert!(matches!(
            plan_loader_profile(
                &document,
                &MinecraftVersionId::new("1.21.11").unwrap(),
                &LoaderVersionId::new("0.19.5").unwrap()
            ),
            Err(FabricPlanError::LibraryInvalid { .. })
        ));

        let mut document = profile_document("1.21.11");
        document.launcher_meta.libraries.common[0].url = "http://maven.example.invalid/".to_owned();
        assert!(matches!(
            plan_loader_profile(
                &document,
                &MinecraftVersionId::new("1.21.11").unwrap(),
                &LoaderVersionId::new("0.19.5").unwrap()
            ),
            Err(FabricPlanError::RepositoryInvalid { .. })
        ));

        let mut document = profile_document("1.21.11");
        document.launcher_meta.libraries.common[0].sha256 = Some("deadbeef".to_owned());
        assert!(matches!(
            plan_loader_profile(
                &document,
                &MinecraftVersionId::new("1.21.11").unwrap(),
                &LoaderVersionId::new("0.19.5").unwrap()
            ),
            Err(FabricPlanError::ArtifactInvalid { .. })
        ));

        let mut document = profile_document("1.21.11");
        document.launcher_meta.libraries.common[0].size = Some(0);
        assert!(matches!(
            plan_loader_profile(
                &document,
                &MinecraftVersionId::new("1.21.11").unwrap(),
                &LoaderVersionId::new("0.19.5").unwrap()
            ),
            Err(FabricPlanError::ArtifactInvalid { .. })
        ));
    }

    #[test]
    fn composition_preserves_both_plans_and_selects_the_fabric_entry_point() {
        let minecraft = minecraft_plan(&["com.mojang:brigadier:1.0.18"]);
        let fabric = fabric_plan("1.21.11");
        let vanilla_clone = minecraft.clone();

        let game = compose_game_plan(minecraft, fabric).unwrap();

        // The vanilla plan remains independently meaningful and unchanged.
        assert_eq!(game.minecraft(), &vanilla_clone);
        assert_eq!(
            game.minecraft().launch().main_class(),
            "net.minecraft.client.main.Main"
        );
        assert_eq!(
            game.minecraft().client().sha1().as_hex(),
            "1832adbc4eee60faa097bb1409be305a0abbf3d2",
            "Mojang SHA-1 expectations stay intact"
        );
        assert_eq!(game.minecraft().asset_index().id(), "31");

        // The composed plan exposes the Fabric entry point.
        assert_eq!(
            game.main_class(),
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );

        // Java: Mojang's requirement preserved; the loader floor recorded.
        assert_eq!(game.java().component(), "java-runtime-delta");
        assert_eq!(game.java().major_version(), 21);
        assert_eq!(game.java().loader_min_major_version(), 8);
        assert!(!game.java().raised_by_loader());

        // Assets and client requirements are preserved through the parts.
        assert_eq!(game.minecraft().asset_index().total_size(), 470877946);
        assert_eq!(game.minecraft().client().size_bytes(), 26000000);

        // Ordering: Mojang libraries first, then Fabric, deterministic.
        let coordinates: Vec<String> = game
            .libraries()
            .iter()
            .map(|library| library.coordinate_string())
            .collect();
        assert_eq!(
            coordinates,
            vec![
                "com.mojang:brigadier:1.0.18",
                "org.ow2.asm:asm:9.10.1",
                "net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7",
                "net.fabricmc:intermediary:1.21.11",
                "net.fabricmc:fabric-loader:0.19.5",
            ]
        );
        assert_eq!(game.vanilla_library_count(), 1);
        assert_eq!(game.fabric_library_count(), 4);
    }

    #[test]
    fn composition_is_deterministic() {
        let first = compose_game_plan(
            minecraft_plan(&["com.mojang:brigadier:1.0.18"]),
            fabric_plan("1.21.11"),
        )
        .unwrap();
        let second = compose_game_plan(
            minecraft_plan(&["com.mojang:brigadier:1.0.18"]),
            fabric_plan("1.21.11"),
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn exact_duplicate_requirements_collapse_to_one() {
        // Mojang providing exactly what Fabric also lists: one requirement.
        let game = compose_game_plan(
            minecraft_plan(&["org.ow2.asm:asm:9.10.1"]),
            fabric_plan("1.21.11"),
        )
        .unwrap();

        let duplicates = game
            .libraries()
            .iter()
            .filter(|library| library.coordinate_string() == "org.ow2.asm:asm:9.10.1")
            .count();
        assert_eq!(duplicates, 1);
        assert_eq!(
            game.libraries()[0].provenance(),
            LibraryProvenance::Mojang,
            "the earlier (Mojang) entry wins an exact duplicate"
        );
        assert_eq!(game.vanilla_library_count(), 1);
        assert_eq!(game.fabric_library_count(), 3);
    }

    #[test]
    fn same_identity_at_different_versions_is_a_deliberate_conflict() {
        let error = compose_game_plan(
            minecraft_plan(&["org.ow2.asm:asm:9.9.9"]),
            fabric_plan("1.21.11"),
        )
        .expect_err("version conflicts must never be silently resolved");

        assert!(matches!(
            error,
            CompositionError::LibraryConflict {
                ref existing,
                ref conflicting,
            } if existing == "org.ow2.asm:asm:9.9.9" && conflicting == "org.ow2.asm:asm:9.10.1"
        ));
    }

    #[test]
    fn different_classifiers_of_one_library_coexist() {
        // Native-classifier coexistence is legitimate Mojang practice.
        let game = compose_game_plan(
            minecraft_plan(&[
                "org.lwjgl:lwjgl:3.4.1",
                "org.lwjgl:lwjgl:3.4.1:natives-windows",
            ]),
            fabric_plan("1.21.11"),
        )
        .unwrap();

        assert_eq!(game.vanilla_library_count(), 2);
        assert_eq!(game.libraries().len(), 6);
    }

    #[test]
    fn version_disagreement_between_plans_is_rejected() {
        let mut fabric = fabric_plan("1.21.11");
        fabric = FabricPlan {
            minecraft_version: "26.2".to_owned(),
            ..fabric
        };

        assert!(matches!(
            compose_game_plan(minecraft_plan(&["com.mojang:brigadier:1.0.18"]), fabric),
            Err(CompositionError::VersionMismatch { .. })
        ));
    }

    #[test]
    fn a_loader_floor_above_mojangs_requirement_is_normalized_deliberately() {
        let fabric = fabric_plan("1.21.11");
        let fabric = FabricPlan {
            min_java_major_version: 25,
            ..fabric
        };

        let game =
            compose_game_plan(minecraft_plan(&["com.mojang:brigadier:1.0.18"]), fabric).unwrap();
        assert_eq!(game.java().major_version(), 25);
        assert!(game.java().raised_by_loader());
        assert_eq!(game.java().component(), "java-runtime-delta");
    }

    #[test]
    fn trust_representation_stays_honest() {
        let game = compose_game_plan(
            minecraft_plan(&["com.mojang:brigadier:1.0.18"]),
            fabric_plan("1.21.11"),
        )
        .unwrap();

        // Mojang entries keep their official SHA-1 expectations...
        let mojang_entry = game
            .libraries()
            .iter()
            .find(|library| library.provenance() == LibraryProvenance::Mojang)
            .unwrap();
        assert_eq!(
            match mojang_entry {
                GameLibrary::Minecraft(entry) => entry.artifact().sha1().as_hex(),
                GameLibrary::Fabric(_) => unreachable!(),
            },
            "1832adbc4eee60faa097bb1409be305a0abbf3d2"
        );

        // ...while undigested Fabric artifacts are represented as such
        // rather than borrowing a fabricated digest.
        let loader_entry = game
            .libraries()
            .iter()
            .find(|library| library.coordinate_string() == "net.fabricmc:fabric-loader:0.19.5")
            .unwrap();
        match loader_entry {
            GameLibrary::Fabric(entry) => {
                assert!(entry.artifact().sha256().is_none());
                assert!(!entry.artifact().has_official_digest());
            }
            GameLibrary::Minecraft(_) => unreachable!(),
        }
    }
}
