//! Mojang-managed Java runtime resolution, installation, and validation.
//!
//! The boundary is deliberately narrow: official Mojang metadata is
//! normalized into [`plan::JavaRuntimePlan`], installation consumes only that
//! plan, and a versioned state document is the sole completion marker for a
//! shared runtime. Instance content never owns or embeds a Java installation.

pub mod install;
pub mod metadata;
pub mod plan;
pub mod state;

pub use install::{
    InstallRuntimeProgress, InstalledRuntime, RuntimeInstallError, RuntimeInstallPhase,
    RuntimeValidation, RuntimeValidationStatus, ensure_runtime, validate_runtime,
};
pub use metadata::{RuntimeMetadataEndpoints, RuntimeMetadataError, resolve_runtime_plan};
pub use plan::{
    JavaRuntimePlan, RuntimeArchitecture, RuntimeOperatingSystem, RuntimePlanError, RuntimePlatform,
};
