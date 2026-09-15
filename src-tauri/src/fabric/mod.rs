//! Fabric metadata resolution and install-plan composition.
//!
//! The pipeline owned by this module:
//!
//! ```text
//! exact Minecraft version + exact Fabric Loader version
//!         ↓ loader version list (HTTPS discovery, no prior digest)
//! exact loader lookup (no "latest", no substitution)
//!         ↓ loader profile document (HTTPS, official Fabric Meta)
//! parse + validate (external DTOs)
//!         ↓ normalization
//! FabricPlan (Aurora-owned domain types)
//!         ↓ composition with MinecraftInstallPlan
//! GameInstallPlan
//! ```
//!
//! Everything here is planning. No Minecraft or Fabric artifact is
//! downloaded, nothing is installed, extracted, or launched, and no instance
//! is created. Installers in later phases consume the composed plan and
//! never parse Fabric Meta JSON themselves.

pub mod maven;
pub mod metadata;
pub mod plan;

use std::fmt;

use crate::downloads::DownloadOptions;
use crate::fabric::metadata::{
    FabricMetaEndpoints, FabricMetadataError, LoaderVersionEntry, LoaderVersionId,
    fetch_loader_profile, fetch_loader_versions,
};
use crate::fabric::plan::{FabricPlan, FabricPlanError, GameInstallPlan, compose_game_plan};
use crate::minecraft::MinecraftResolutionError;
use crate::minecraft::metadata::{MetadataEndpoints, MinecraftVersionId};
use crate::minecraft::plan::MinecraftInstallPlan;
use crate::minecraft::rules::PlatformProfile;

/// Resolves one exact Minecraft + Fabric Loader combination into a
/// normalized Fabric plan.
///
/// Resolution is exact and two-step, mirroring how official Fabric metadata
/// is authoritative: first the requested Loader version must exist in the
/// official loader list (an unknown version is reported as not found, never
/// substituted with a newer or "latest" loader), then the combination must
/// be supported (official Fabric Meta answers HTTP 400 for a game version it
/// has no profile for). Metadata is fetched on demand and not persisted.
pub async fn resolve_fabric_plan(
    endpoints: &FabricMetaEndpoints,
    game: &MinecraftVersionId,
    loader: &LoaderVersionId,
    options: &DownloadOptions,
) -> Result<FabricPlan, FabricResolutionError> {
    let versions = fetch_loader_versions(endpoints, options).await?;
    if LoaderVersionEntry::find(&versions, loader).is_none() {
        return Err(FabricResolutionError::LoaderNotFound {
            requested: loader.to_string(),
        });
    }

    let document = match fetch_loader_profile(endpoints, game, loader, options).await {
        Ok(document) => document,
        // Official Fabric Meta answers HTTP 400 with a plain-text reason for
        // an unsupported Minecraft/Loader combination; 404 is treated the
        // same way defensively. Anything else is transport trouble.
        Err(FabricMetadataError::HttpStatus { status: 400 | 404 }) => {
            return Err(FabricResolutionError::CombinationUnsupported {
                game: game.to_string(),
                loader: loader.to_string(),
            });
        }
        Err(error) => return Err(error.into()),
    };

    if document.loader.version != loader.as_str() {
        return Err(FabricResolutionError::Metadata(
            FabricMetadataError::Malformed {
                reason: format!(
                    "the profile fetched for loader '{}' identifies itself as loader '{}'",
                    loader, document.loader.version
                ),
            },
        ));
    }

    Ok(plan::plan_loader_profile(&document, game, loader)?)
}

/// A failure anywhere in the combination → Fabric plan pipeline.
#[derive(Debug)]
pub enum FabricResolutionError {
    Metadata(FabricMetadataError),
    /// The requested Loader version does not exist in official metadata.
    LoaderNotFound {
        requested: String,
    },
    /// Official Fabric metadata does not support the Minecraft/Loader
    /// combination.
    CombinationUnsupported {
        game: String,
        loader: String,
    },
    Planning(FabricPlanError),
}

impl fmt::Display for FabricResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Metadata(error) => write!(formatter, "{error}"),
            Self::LoaderNotFound { requested } => write!(
                formatter,
                "Fabric Loader version '{requested}' does not exist in official Fabric metadata"
            ),
            Self::CombinationUnsupported { game, loader } => write!(
                formatter,
                "official Fabric metadata does not support Minecraft '{game}' with Fabric Loader '{loader}'"
            ),
            Self::Planning(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for FabricResolutionError {}

impl From<FabricMetadataError> for FabricResolutionError {
    fn from(error: FabricMetadataError) -> Self {
        Self::Metadata(error)
    }
}

impl From<FabricPlanError> for FabricResolutionError {
    fn from(error: FabricPlanError) -> Self {
        Self::Planning(error)
    }
}

/// Resolves one exact Minecraft + Fabric Loader combination into the
/// complete, composed game install plan for one platform.
///
/// This composes the Phase 3 vanilla resolution (`MinecraftInstallPlan`) with
/// the Fabric resolution (`FabricPlan`) and validates the result
/// (`GameInstallPlan`). It plans only: no product artifact of either source
/// is downloaded.
pub async fn resolve_game_plan(
    minecraft_endpoints: &MetadataEndpoints,
    fabric_endpoints: &FabricMetaEndpoints,
    game: &MinecraftVersionId,
    loader: &LoaderVersionId,
    platform: PlatformProfile,
    options: &DownloadOptions,
) -> Result<GameInstallPlan, GameResolutionError> {
    let minecraft: MinecraftInstallPlan =
        crate::minecraft::resolve_install_plan(minecraft_endpoints, game, platform, options)
            .await?;
    let fabric = resolve_fabric_plan(fabric_endpoints, game, loader, options).await?;
    let plan = compose_game_plan(minecraft, fabric)?;

    eprintln!(
        "[aurora-launcher] planned Minecraft {} + Fabric Loader {}: {} vanilla + {} Fabric = {} libraries, Java {}, main class {}",
        plan.minecraft().minecraft_version(),
        plan.loader().loader_version(),
        plan.vanilla_library_count(),
        plan.fabric_library_count(),
        plan.libraries().len(),
        plan.java().major_version(),
        plan.main_class(),
    );

    Ok(plan)
}

/// A failure anywhere in the combination → game plan pipeline.
#[derive(Debug)]
pub enum GameResolutionError {
    Minecraft(MinecraftResolutionError),
    Fabric(FabricResolutionError),
    Composition(plan::CompositionError),
}

impl fmt::Display for GameResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Minecraft(error) => write!(formatter, "{error}"),
            Self::Fabric(error) => write!(formatter, "{error}"),
            Self::Composition(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for GameResolutionError {}

impl From<MinecraftResolutionError> for GameResolutionError {
    fn from(error: MinecraftResolutionError) -> Self {
        Self::Minecraft(error)
    }
}

impl From<FabricResolutionError> for GameResolutionError {
    fn from(error: FabricResolutionError) -> Self {
        Self::Fabric(error)
    }
}

impl From<plan::CompositionError> for GameResolutionError {
    fn from(error: plan::CompositionError) -> Self {
        Self::Composition(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::metadata::NOOP_INTERMEDIARY_VERSION;
    use crate::test_support::{TestResponse, TestServer};
    use std::sync::Arc;
    use std::time::Duration;

    fn quick_options() -> DownloadOptions {
        DownloadOptions {
            connect_timeout: Duration::from_secs(5),
            idle_read_timeout: Duration::from_secs(5),
            max_redirects: crate::downloads::MAX_REDIRECTS,
        }
    }

    const LOADER_LIST: &str = r#"[
        { "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true },
        { "separator": ".", "build": 4, "maven": "net.fabricmc:fabric-loader:0.19.4", "version": "0.19.4", "stable": false }
    ]"#;

    fn profile_body(intermediary_version: &str) -> String {
        format!(
            r#"{{
                "loader": {{ "separator": ".", "build": 5, "maven": "net.fabricmc:fabric-loader:0.19.5", "version": "0.19.5", "stable": true }},
                "intermediary": {{ "maven": "net.fabricmc:intermediary:{intermediary_version}", "version": "{intermediary_version}", "stable": true }},
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
        )
    }

    fn fabric_test_server() -> TestServer {
        TestServer::spawn(Arc::new(|request| match request.path.as_str() {
            "/v2/versions/loader" => TestResponse::ok(LOADER_LIST.as_bytes()),
            path if path.starts_with("/v2/versions/loader/1.21.11/0.19.5") => {
                TestResponse::ok(profile_body("1.21.11").as_bytes())
            }
            path if path.starts_with("/v2/versions/loader/26.2/0.19.5") => {
                TestResponse::ok(profile_body(NOOP_INTERMEDIARY_VERSION).as_bytes())
            }
            _ => TestResponse::status(400),
        }))
    }

    fn endpoints(server: &TestServer) -> FabricMetaEndpoints {
        FabricMetaEndpoints::loopback_for_testing(&format!("{}/v2/", server.base_url()))
    }

    fn game(id: &str) -> MinecraftVersionId {
        MinecraftVersionId::new(id).unwrap()
    }

    fn loader(id: &str) -> LoaderVersionId {
        LoaderVersionId::new(id).unwrap()
    }

    #[tokio::test]
    async fn an_exact_supported_combination_resolves_into_a_fabric_plan() {
        let server = fabric_test_server();

        let plan = resolve_fabric_plan(
            &endpoints(&server),
            &game("1.21.11"),
            &loader("0.19.5"),
            &quick_options(),
        )
        .await
        .unwrap();

        assert_eq!(plan.minecraft_version(), "1.21.11");
        assert_eq!(plan.loader_version(), "0.19.5");
        assert_eq!(
            plan.main_class(),
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );
        assert_eq!(plan.libraries().len(), 3); // asm + intermediary + loader

        // The same loader against an unobfuscated version drops the
        // intermediary artifact from the plan.
        let unobfuscated = resolve_fabric_plan(
            &endpoints(&server),
            &game("26.2"),
            &loader("0.19.5"),
            &quick_options(),
        )
        .await
        .unwrap();
        assert_eq!(unobfuscated.libraries().len(), 2);
    }

    #[tokio::test]
    async fn an_unknown_loader_version_is_reported_not_substituted() {
        let server = fabric_test_server();

        let error = resolve_fabric_plan(
            &endpoints(&server),
            &game("1.21.11"),
            &loader("0.99.0"),
            &quick_options(),
        )
        .await
        .expect_err("unknown loaders must not resolve");

        assert!(
            matches!(error, FabricResolutionError::LoaderNotFound { ref requested } if requested == "0.99.0"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn an_unsupported_combination_is_rejected_deliberately() {
        let server = fabric_test_server();

        let error = resolve_fabric_plan(
            &endpoints(&server),
            &game("9.9.9"),
            &loader("0.19.5"),
            &quick_options(),
        )
        .await
        .expect_err("unsupported combinations must not resolve");

        assert!(
            matches!(
                error,
                FabricResolutionError::CombinationUnsupported { ref game, ref loader }
                    if game == "9.9.9" && loader == "0.19.5"
            ),
            "unexpected error: {error}"
        );
    }

    /// Controlled live verification against the real official Fabric Meta
    /// and Mojang metadata chains, composed end to end.
    ///
    /// Ignored by default so the offline suite never depends on public
    /// services; run explicitly with `cargo test -- --ignored --nocapture`
    /// when verifying drift. Only metadata documents are fetched: no client
    /// jar, library, native, asset, runtime, or Fabric artifact is
    /// downloaded.
    #[tokio::test]
    #[ignore = "fetches live official Mojang and Fabric metadata"]
    async fn live_official_metadata_composes_two_modern_game_plans() {
        let minecraft_endpoints = MetadataEndpoints::official();
        let fabric_endpoints = FabricMetaEndpoints::official();
        let options = DownloadOptions::default();
        let platform = PlatformProfile::current().expect("the host platform must be plannable");

        for (game_version, loader_version) in [
            ("26.2", "0.19.5"),    // current Aurora-era target; unobfuscated
            ("1.21.11", "0.19.5"), // prior modern release; obfuscated
        ] {
            let game = MinecraftVersionId::new(game_version).unwrap();
            let loader = LoaderVersionId::new(loader_version).unwrap();

            let plan = resolve_game_plan(
                &minecraft_endpoints,
                &fabric_endpoints,
                &game,
                &loader,
                platform,
                &options,
            )
            .await
            .unwrap_or_else(|error| {
                panic!("{game_version} + {loader_version} must resolve: {error}")
            });

            let fabric = plan.loader();
            assert_eq!(fabric.minecraft_version(), game_version);
            assert_eq!(fabric.loader_version(), loader_version);
            assert_eq!(
                plan.main_class(),
                "net.fabricmc.loader.impl.launch.knot.KnotClient"
            );

            let intermediary_count = fabric
                .libraries()
                .iter()
                .filter(|library| library.role() == plan::FabricLibraryRole::Intermediary)
                .count();

            eprintln!(
                "[live] Minecraft {game_version} + Fabric Loader {loader_version} on {}-{}: vanilla {} libraries + Fabric {} libraries = {} composed; Fabric libraries with official digests: {}/{}; intermediary artifacts: {intermediary_count}; Java {} (Mojang) vs loader floor {}; main class {}",
                platform.os(),
                platform.arch(),
                plan.vanilla_library_count(),
                plan.fabric_library_count(),
                plan.libraries().len(),
                fabric.digested_library_count(),
                fabric.libraries().len(),
                plan.minecraft().java().major_version(),
                fabric.min_java_major_version(),
                plan.main_class(),
            );

            // Semantic assertions, not snapshots.
            assert!(!plan.libraries().is_empty());
            assert!(
                plan.fabric_library_count() >= 2,
                "at least common libraries plus the loader"
            );
            assert!(
                fabric.digested_library_count() < fabric.libraries().len(),
                "the loader artifact itself is honestly digest-less"
            );
            assert_eq!(
                plan.java().major_version(),
                plan.minecraft().java().major_version(),
                "Mojang's Java requirement governs while its floor is lower"
            );
            assert!(!plan.java().raised_by_loader());
            assert!(plan.libraries().iter().all(|library| library.provenance()
                == plan::LibraryProvenance::Mojang
                || library.provenance() == plan::LibraryProvenance::Fabric));
            assert!(plan.minecraft().client().size_bytes() > 0);
            assert!(!plan.minecraft().asset_index().id().is_empty());
        }
    }
}
