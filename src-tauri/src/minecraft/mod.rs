//! Minecraft metadata resolution and install planning.
//!
//! The pipeline owned by this module:
//!
//! ```text
//! official version manifest (HTTPS discovery, no prior digest)
//!         ↓ exact version lookup
//! version document (manifest-provided URL, verified against manifest SHA-1)
//!         ↓ parse + validate (external DTOs)
//! platform-aware rule evaluation + normalization
//!         ↓
//! MinecraftInstallPlan (Aurora-owned domain types)
//! ```
//!
//! The plan is data: it downloads, installs, extracts, and launches nothing.
//! Installers in later phases consume the normalized plan and never traverse
//! raw Mojang JSON.

pub mod metadata;
pub mod plan;
pub mod rules;

use std::fmt;

use crate::downloads::DownloadOptions;
use crate::minecraft::metadata::{
    MetadataEndpoints, MetadataError, MinecraftVersionId, fetch_manifest, fetch_version_document,
};
use crate::minecraft::plan::{MinecraftInstallPlan, PlanError, plan_version_document};
use crate::minecraft::rules::PlatformProfile;

/// Resolves one exact Minecraft version into a normalized install plan for
/// one platform.
///
/// This composes discovery, verified version-document fetching, parsing, and
/// planning. Metadata is fetched on demand and not persisted; no product
/// artifact is downloaded.
pub async fn resolve_install_plan(
    endpoints: &MetadataEndpoints,
    version: &MinecraftVersionId,
    platform: PlatformProfile,
    options: &DownloadOptions,
) -> Result<MinecraftInstallPlan, MinecraftResolutionError> {
    let manifest = fetch_manifest(endpoints, options).await?;
    let entry = manifest
        .find(version)
        .ok_or(MinecraftResolutionError::from(
            MetadataError::VersionNotFound {
                requested: version.to_string(),
            },
        ))?;

    let document = fetch_version_document(entry, options).await?;
    if document.id != entry.id {
        return Err(MinecraftResolutionError::Metadata(
            MetadataError::DocumentInvalid {
                reason: format!(
                    "the document fetched for '{}' identifies itself as '{}'",
                    entry.id, document.id
                ),
            },
        ));
    }

    Ok(plan_version_document(&document, platform)?)
}

/// A failure anywhere in the version → plan pipeline.
#[derive(Debug)]
pub enum MinecraftResolutionError {
    Metadata(MetadataError),
    Planning(PlanError),
}

impl fmt::Display for MinecraftResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Metadata(error) => write!(formatter, "{error}"),
            Self::Planning(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for MinecraftResolutionError {}

impl From<MetadataError> for MinecraftResolutionError {
    fn from(error: MetadataError) -> Self {
        Self::Metadata(error)
    }
}

impl From<PlanError> for MinecraftResolutionError {
    fn from(error: PlanError) -> Self {
        Self::Planning(error)
    }
}
