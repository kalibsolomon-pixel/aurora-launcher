use std::fmt;
use std::path::{Path, PathBuf};

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
}
