use std::fmt;
use std::path::{Path, PathBuf};

use crate::instances::InstanceId;

/// The platform-resolved Aurora managed-data root and the deterministic
/// locations derived from it.
///
/// Derivation is pure: resolving these locations never touches the
/// filesystem, and every location remains inside the managed root. Directory
/// creation happens only where behavior needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPaths {
    data_root: PathBuf,
}

impl ManagedPaths {
    pub fn from_app_local_data_dir(data_root: PathBuf) -> Result<Self, ManagedPathError> {
        if !data_root.is_absolute() {
            return Err(ManagedPathError::RelativeDataRoot(data_root));
        }

        Ok(Self { data_root })
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// Versioned, non-secret launcher configuration and state.
    pub fn launcher_dir(&self) -> PathBuf {
        self.data_root.join("launcher")
    }

    pub fn config_file(&self) -> PathBuf {
        self.launcher_dir().join("config.json")
    }

    pub fn instance_registry_file(&self) -> PathBuf {
        self.launcher_dir().join("instances.json")
    }

    /// Non-secret account summaries and the account selection. The matching
    /// secret lives in the OS-backed credential store, never in this file.
    pub fn accounts_file(&self) -> PathBuf {
        self.launcher_dir().join("accounts.json")
    }

    /// Re-downloadable metadata and temporary artifacts.
    pub fn cache_dir(&self) -> PathBuf {
        self.data_root.join("cache")
    }

    /// Verified manifests and installation metadata.
    pub fn metadata_dir(&self) -> PathBuf {
        self.data_root.join("metadata")
    }

    /// Launcher-managed Java runtimes.
    pub fn runtimes_dir(&self) -> PathBuf {
        self.data_root.join("runtimes")
    }

    /// The parent of all instance directories.
    pub fn instances_dir(&self) -> PathBuf {
        self.data_root.join("instances")
    }

    /// Derives the filesystem locations for one instance.
    ///
    /// Accepts only a validated [`InstanceId`]; display names and raw strings
    /// cannot reach this function, so every derived path stays inside
    /// [`ManagedPaths::instances_dir`].
    pub fn instance_paths(&self, id: &InstanceId) -> InstancePaths {
        let root = self.instances_dir().join(id.as_str());

        InstancePaths {
            root: root.clone(),
            game: root.join("game"),
            mods: root.join("mods"),
            resourcepacks: root.join("resourcepacks"),
            shaderpacks: root.join("shaderpacks"),
            config: root.join("config"),
            logs: root.join("logs"),
        }
    }
}

/// The deterministic directory layout of a single instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePaths {
    root: PathBuf,
    game: PathBuf,
    mods: PathBuf,
    resourcepacks: PathBuf,
    shaderpacks: PathBuf,
    config: PathBuf,
    logs: PathBuf,
}

impl InstancePaths {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Isolated game directory and managed libraries/assets.
    pub fn game(&self) -> &Path {
        &self.game
    }

    /// Aurora plus optional instance-scoped mods.
    pub fn mods(&self) -> &Path {
        &self.mods
    }

    pub fn resourcepacks(&self) -> &Path {
        &self.resourcepacks
    }

    pub fn shaderpacks(&self) -> &Path {
        &self.shaderpacks
    }

    /// Minecraft, Fabric, Aurora, and mod configuration.
    pub fn config(&self) -> &Path {
        &self.config
    }

    /// Instance-local launch and game logs.
    pub fn logs(&self) -> &Path {
        &self.logs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedPathError {
    RelativeDataRoot(PathBuf),
}

impl fmt::Display for ManagedPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RelativeDataRoot(path) => write!(
                formatter,
                "application-local data root must be absolute, but resolved to {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ManagedPathError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute_test_path(parts: &[&str]) -> PathBuf {
        let mut path = std::env::temp_dir();
        for part in parts {
            path.push(part);
        }
        path
    }

    fn test_managed_paths() -> (PathBuf, ManagedPaths) {
        let root = absolute_test_path(&["AuroraLauncherPathTest", "com.aurora.launcher"]);
        let managed = ManagedPaths::from_app_local_data_dir(root.clone()).unwrap();
        (root, managed)
    }

    #[test]
    fn preserves_the_platform_resolved_application_root() {
        let resolved = absolute_test_path(&["AuroraLauncherPathTest", "com.aurora.launcher"]);

        let managed = ManagedPaths::from_app_local_data_dir(resolved.clone()).unwrap();

        assert_eq!(managed.data_root(), resolved);
    }

    #[test]
    fn rejects_relative_data_roots() {
        let result = ManagedPaths::from_app_local_data_dir(PathBuf::from("Aurora/instances"));

        assert!(matches!(
            result,
            Err(ManagedPathError::RelativeDataRoot(path))
                if path == PathBuf::from("Aurora/instances")
        ));
    }

    #[test]
    fn managed_root_is_not_the_standard_minecraft_directory() {
        let home = absolute_test_path(&["AuroraLauncherIsolationTest"]);
        let minecraft = home.join(".minecraft");
        let app_local = home
            .join("AppData")
            .join("Local")
            .join("com.aurora.launcher");

        let managed = ManagedPaths::from_app_local_data_dir(app_local).unwrap();

        assert_ne!(managed.data_root(), minecraft);
        assert!(!managed.data_root().starts_with(&minecraft));
    }

    #[test]
    fn domain_directories_resolve_deterministically_beneath_the_managed_root() {
        let (root, managed) = test_managed_paths();

        let expected = [
            ("launcher", managed.launcher_dir()),
            ("cache", managed.cache_dir()),
            ("metadata", managed.metadata_dir()),
            ("runtimes", managed.runtimes_dir()),
            ("instances", managed.instances_dir()),
        ];

        for (name, directory) in expected {
            assert_eq!(directory, root.join(name));
            assert!(
                directory.starts_with(&root),
                "{} must stay inside the managed root",
                directory.display()
            );
        }

        assert_eq!(
            managed.config_file(),
            managed.launcher_dir().join("config.json")
        );
        assert_eq!(
            managed.instance_registry_file(),
            managed.launcher_dir().join("instances.json")
        );
        assert_eq!(
            managed.accounts_file(),
            managed.launcher_dir().join("accounts.json")
        );
    }

    #[test]
    fn instance_paths_derive_deterministically_from_the_identifier() {
        let (_, managed) = test_managed_paths();
        let id = InstanceId::new("aurora-default").unwrap();

        let first = managed.instance_paths(&id);
        let second = managed.instance_paths(&id);

        assert_eq!(first, second);
        assert_eq!(first.root(), managed.instances_dir().join("aurora-default"));
        assert_eq!(first.game(), first.root().join("game"));
        assert_eq!(first.mods(), first.root().join("mods"));
        assert_eq!(first.config(), first.root().join("config"));
        assert_eq!(first.logs(), first.root().join("logs"));
    }

    #[test]
    fn instance_paths_remain_inside_managed_storage() {
        let (root, managed) = test_managed_paths();
        let id = InstanceId::new("scoped").unwrap();

        let instance = managed.instance_paths(&id);

        assert!(instance.root().starts_with(&root));
        for path in [
            instance.game(),
            instance.mods(),
            instance.config(),
            instance.logs(),
        ] {
            assert!(
                path.starts_with(instance.root()),
                "{} must stay inside the instance root",
                path.display()
            );
        }
    }

    #[test]
    fn distinct_identifiers_derive_distinct_roots() {
        let (_, managed) = test_managed_paths();

        let alpha = managed.instance_paths(&InstanceId::new("alpha").unwrap());
        let beta = managed.instance_paths(&InstanceId::new("beta").unwrap());

        assert_ne!(alpha.root(), beta.root());
    }

    #[test]
    fn display_names_cannot_influence_instance_paths() {
        let (_, managed) = test_managed_paths();

        // The same identifier always maps to the same root regardless of any
        // display name, because derivation accepts only validated IDs.
        let id = InstanceId::new("shared-id").unwrap();
        let derivation = managed.instance_paths(&id);

        assert_eq!(derivation.root(), managed.instances_dir().join("shared-id"));
        assert!(InstanceId::new("My Fancy Instance").is_err());
    }
}
