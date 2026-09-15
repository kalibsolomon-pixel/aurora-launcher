//! Normalized, platform-explicit Java runtime installation plans.

use std::fmt;
use std::path::PathBuf;

use crate::integrity::Sha1Digest;
use crate::paths::ManagedPaths;
use crate::runtime::metadata::{RuntimeFileDocument, RuntimeFileKind, RuntimeSelection};

const MAX_COMPONENT_LENGTH: usize = 64;
const MAX_RUNTIME_PATH_LENGTH: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeOperatingSystem {
    Windows,
    Linux,
    MacOs,
}

impl RuntimeOperatingSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "macos",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeArchitecture {
    X86_64,
    Aarch64,
    X86,
}

impl RuntimeArchitecture {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::X86 => "x86",
        }
    }
}

/// A normalized target. OS and CPU architecture are never inferred from a
/// runtime component name and unsupported pairs never fall back to x86_64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimePlatform {
    os: RuntimeOperatingSystem,
    architecture: RuntimeArchitecture,
}

impl RuntimePlatform {
    pub const fn new(os: RuntimeOperatingSystem, architecture: RuntimeArchitecture) -> Self {
        Self { os, architecture }
    }

    pub fn current() -> Result<Self, RuntimePlanError> {
        let os = match std::env::consts::OS {
            "windows" => RuntimeOperatingSystem::Windows,
            "linux" => RuntimeOperatingSystem::Linux,
            "macos" => RuntimeOperatingSystem::MacOs,
            other => {
                return Err(RuntimePlanError::UnsupportedHost(format!(
                    "operating system '{other}'"
                )));
            }
        };
        let architecture = match std::env::consts::ARCH {
            "x86_64" => RuntimeArchitecture::X86_64,
            "aarch64" => RuntimeArchitecture::Aarch64,
            "x86" | "i686" => RuntimeArchitecture::X86,
            other => {
                return Err(RuntimePlanError::UnsupportedHost(format!(
                    "CPU architecture '{other}'"
                )));
            }
        };
        let platform = Self::new(os, architecture);
        platform.mojang_key()?;
        Ok(platform)
    }

    pub fn os(self) -> RuntimeOperatingSystem {
        self.os
    }
    pub fn architecture(self) -> RuntimeArchitecture {
        self.architecture
    }

    pub fn mojang_key(self) -> Result<&'static str, RuntimePlanError> {
        match (self.os, self.architecture) {
            (RuntimeOperatingSystem::Windows, RuntimeArchitecture::X86_64) => Ok("windows-x64"),
            (RuntimeOperatingSystem::Windows, RuntimeArchitecture::Aarch64) => Ok("windows-arm64"),
            (RuntimeOperatingSystem::Windows, RuntimeArchitecture::X86) => Ok("windows-x86"),
            (RuntimeOperatingSystem::Linux, RuntimeArchitecture::X86_64) => Ok("linux"),
            (RuntimeOperatingSystem::Linux, RuntimeArchitecture::X86) => Ok("linux-i386"),
            (RuntimeOperatingSystem::MacOs, RuntimeArchitecture::X86_64) => Ok("mac-os"),
            (RuntimeOperatingSystem::MacOs, RuntimeArchitecture::Aarch64) => Ok("mac-os-arm64"),
            (os, architecture) => Err(RuntimePlanError::UnsupportedPlatform {
                os: os.as_str().to_owned(),
                architecture: architecture.as_str().to_owned(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeArtifact {
    url: String,
    sha1: Sha1Digest,
    size_bytes: u64,
}

impl RuntimeArtifact {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEntry {
    Directory {
        path: String,
    },
    File {
        path: String,
        executable: bool,
        artifact: RuntimeArtifact,
    },
    Link {
        path: String,
        target: String,
    },
}

impl RuntimeEntry {
    pub fn path(&self) -> &str {
        match self {
            Self::Directory { path } | Self::File { path, .. } | Self::Link { path, .. } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRuntimePlan {
    component: String,
    required_major_version: u32,
    platform: RuntimePlatform,
    platform_key: String,
    runtime_version: String,
    released: String,
    manifest_sha1: Sha1Digest,
    entries: Vec<RuntimeEntry>,
    launch_executable: String,
    diagnostic_executable: String,
}

impl JavaRuntimePlan {
    pub(crate) fn from_metadata(
        component: &str,
        required_major_version: u32,
        platform: RuntimePlatform,
        selection: RuntimeSelection,
        document: RuntimeFileDocument,
    ) -> Result<Self, RuntimePlanError> {
        validate_component(component)?;
        if required_major_version == 0 {
            return Err(RuntimePlanError::InvalidRequirement(
                "the required Java major version must be at least 1".to_owned(),
            ));
        }
        let published_major = parse_java_major(&selection.version_name).ok_or_else(|| {
            RuntimePlanError::Metadata(format!(
                "runtime version '{}' does not begin with a Java major version",
                selection.version_name
            ))
        })?;
        if published_major != required_major_version {
            return Err(RuntimePlanError::MajorVersionMismatch {
                required: required_major_version,
                published: published_major,
            });
        }

        let platform_key = platform.mojang_key()?.to_owned();
        let mut entries = Vec::with_capacity(document.files.len());
        for (path, entry) in document.files {
            validate_relative_path(&path)?;
            let normalized = match entry {
                RuntimeFileKind::Directory => RuntimeEntry::Directory { path },
                RuntimeFileKind::File {
                    executable,
                    downloads,
                } => {
                    let raw = downloads.raw;
                    if raw.size == 0 {
                        return Err(RuntimePlanError::Metadata(format!(
                            "runtime file '{path}' has zero size"
                        )));
                    }
                    let sha1 = Sha1Digest::parse(&raw.sha1).map_err(|error| {
                        RuntimePlanError::Metadata(format!(
                            "runtime file '{path}' has an invalid SHA-1: {error}"
                        ))
                    })?;
                    validate_artifact_url(&raw.url).map_err(|reason| {
                        RuntimePlanError::Metadata(format!(
                            "runtime file '{path}' has an unusable URL: {reason}"
                        ))
                    })?;
                    RuntimeEntry::File {
                        path,
                        executable,
                        artifact: RuntimeArtifact {
                            url: raw.url,
                            sha1,
                            size_bytes: raw.size,
                        },
                    }
                }
                RuntimeFileKind::Link { target } => {
                    validate_link_target(&path, &target)?;
                    RuntimeEntry::Link { path, target }
                }
            };
            entries.push(normalized);
        }
        if entries.is_empty() {
            return Err(RuntimePlanError::Metadata(
                "the runtime manifest has no entries".to_owned(),
            ));
        }
        for entry in &entries {
            if matches!(entry, RuntimeEntry::File { .. } | RuntimeEntry::Link { .. }) {
                let prefix = format!("{}/", entry.path());
                if entries
                    .iter()
                    .any(|candidate| candidate.path().starts_with(&prefix))
                {
                    return Err(RuntimePlanError::Metadata(format!(
                        "runtime non-directory path '{}' is an ancestor of another entry",
                        entry.path()
                    )));
                }
            }
        }

        let (launch_executable, diagnostic_executable) = match platform.os {
            RuntimeOperatingSystem::Windows => ("bin/javaw.exe", "bin/java.exe"),
            RuntimeOperatingSystem::Linux => ("bin/java", "bin/java"),
            RuntimeOperatingSystem::MacOs => (
                "jre.bundle/Contents/Home/bin/java",
                "jre.bundle/Contents/Home/bin/java",
            ),
        };
        for required in [launch_executable, diagnostic_executable] {
            let present = entries.iter().any(|entry| matches!(entry, RuntimeEntry::File { path, executable: true, .. } if path == required));
            if !present {
                return Err(RuntimePlanError::MissingExecutable(required.to_owned()));
            }
        }

        Ok(Self {
            component: component.to_owned(),
            required_major_version,
            platform,
            platform_key,
            runtime_version: selection.version_name,
            released: selection.released,
            manifest_sha1: selection.manifest_sha1,
            entries,
            launch_executable: launch_executable.to_owned(),
            diagnostic_executable: diagnostic_executable.to_owned(),
        })
    }

    pub fn component(&self) -> &str {
        &self.component
    }
    pub fn required_major_version(&self) -> u32 {
        self.required_major_version
    }
    pub fn platform(&self) -> RuntimePlatform {
        self.platform
    }
    pub fn platform_key(&self) -> &str {
        &self.platform_key
    }
    pub fn runtime_version(&self) -> &str {
        &self.runtime_version
    }
    pub fn released(&self) -> &str {
        &self.released
    }
    pub fn manifest_sha1(&self) -> &Sha1Digest {
        &self.manifest_sha1
    }
    pub fn entries(&self) -> &[RuntimeEntry] {
        &self.entries
    }
    pub fn launch_executable(&self) -> &str {
        &self.launch_executable
    }
    pub fn diagnostic_executable(&self) -> &str {
        &self.diagnostic_executable
    }
    pub fn file_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry, RuntimeEntry::File { .. }))
            .count()
    }
    pub fn total_download_bytes(&self) -> u64 {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                RuntimeEntry::File { artifact, .. } => Some(artifact.size_bytes),
                _ => None,
            })
            .sum()
    }

    pub fn identity(&self) -> String {
        format!("{}-{}", self.platform_key, self.manifest_sha1.as_hex())
    }
    pub fn installation_dir(&self, managed: &ManagedPaths) -> PathBuf {
        managed
            .runtimes_dir()
            .join(&self.component)
            .join(self.identity())
    }
    pub fn launch_executable_path(&self, managed: &ManagedPaths) -> PathBuf {
        join_relative(&self.installation_dir(managed), &self.launch_executable)
    }
    pub fn diagnostic_executable_path(&self, managed: &ManagedPaths) -> PathBuf {
        join_relative(&self.installation_dir(managed), &self.diagnostic_executable)
    }
}

pub(crate) fn join_relative(root: &std::path::Path, relative: &str) -> PathBuf {
    let mut result = root.to_path_buf();
    for segment in relative.split('/') {
        result.push(segment);
    }
    result
}

pub(crate) fn validate_component(component: &str) -> Result<(), RuntimePlanError> {
    let valid = !component.is_empty()
        && component.len() <= MAX_COMPONENT_LENGTH
        && !component.starts_with('.')
        && component
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(RuntimePlanError::InvalidRequirement(format!(
            "runtime component '{component}' is not a safe identifier"
        )))
    }
}

pub(crate) fn validate_relative_path(path: &str) -> Result<(), RuntimePlanError> {
    if path.is_empty()
        || path.len() > MAX_RUNTIME_PATH_LENGTH
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
    {
        return Err(RuntimePlanError::UnsafePath(path.to_owned()));
    }
    if path.split('/').any(|segment| {
        segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    }) {
        return Err(RuntimePlanError::UnsafePath(path.to_owned()));
    }
    Ok(())
}

pub(crate) fn validate_link_target(link_path: &str, target: &str) -> Result<(), RuntimePlanError> {
    if target.is_empty()
        || target.len() > MAX_RUNTIME_PATH_LENGTH
        || target.starts_with('/')
        || target.contains('\\')
        || target.contains(':')
    {
        return Err(RuntimePlanError::UnsafeLink {
            path: link_path.to_owned(),
            target: target.to_owned(),
        });
    }
    let mut depth = link_path.split('/').count().saturating_sub(1);
    for segment in target.split('/') {
        match segment {
            "" | "." => {
                return Err(RuntimePlanError::UnsafeLink {
                    path: link_path.to_owned(),
                    target: target.to_owned(),
                });
            }
            ".." if depth == 0 => {
                return Err(RuntimePlanError::UnsafeLink {
                    path: link_path.to_owned(),
                    target: target.to_owned(),
                });
            }
            ".." => depth -= 1,
            value
                if value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) =>
            {
                depth += 1
            }
            _ => {
                return Err(RuntimePlanError::UnsafeLink {
                    path: link_path.to_owned(),
                    target: target.to_owned(),
                });
            }
        }
    }
    Ok(())
}

fn validate_artifact_url(value: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|_| "the URL is invalid".to_owned())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("embedded credentials are forbidden".to_owned());
    }
    if url.scheme() == "https"
        || (url.scheme() == "http" && crate::downloads::is_loopback_host(&url))
    {
        Ok(())
    } else {
        Err("the URL must use HTTPS (loopback HTTP is test-only)".to_owned())
    }
}

pub(crate) fn parse_java_major(version: &str) -> Option<u32> {
    let token = version.trim().split(['.', 'u', '-', '+']).next()?;
    let first = token.parse::<u32>().ok()?;
    if first == 1 {
        version
            .trim()
            .strip_prefix("1.")?
            .split(['.', '_', '-', '+'])
            .next()?
            .parse()
            .ok()
    } else {
        Some(first)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimePlanError {
    UnsupportedHost(String),
    UnsupportedPlatform { os: String, architecture: String },
    InvalidRequirement(String),
    Metadata(String),
    MajorVersionMismatch { required: u32, published: u32 },
    UnsafePath(String),
    UnsafeLink { path: String, target: String },
    MissingExecutable(String),
}

impl fmt::Display for RuntimePlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedHost(value) => write!(
                f,
                "the current host is unsupported for managed Java runtimes: {value}"
            ),
            Self::UnsupportedPlatform { os, architecture } => write!(
                f,
                "Mojang does not define a managed Java runtime mapping for {os}/{architecture}"
            ),
            Self::InvalidRequirement(reason) | Self::Metadata(reason) => f.write_str(reason),
            Self::MajorVersionMismatch {
                required,
                published,
            } => write!(
                f,
                "the selected Mojang runtime is Java {published}, but the game requires Java {required}"
            ),
            Self::UnsafePath(path) => write!(
                f,
                "runtime manifest path '{path}' is not a safe relative path"
            ),
            Self::UnsafeLink { path, target } => {
                write!(f, "runtime link '{path}' has unsafe target '{target}'")
            }
            Self::MissingExecutable(path) => write!(
                f,
                "runtime manifest does not contain required executable '{path}'"
            ),
        }
    }
}

impl std::error::Error for RuntimePlanError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_platform_mapping_never_falls_back() {
        assert_eq!(
            RuntimePlatform::new(RuntimeOperatingSystem::Windows, RuntimeArchitecture::X86_64)
                .mojang_key()
                .unwrap(),
            "windows-x64"
        );
        assert_eq!(
            RuntimePlatform::new(
                RuntimeOperatingSystem::Windows,
                RuntimeArchitecture::Aarch64
            )
            .mojang_key()
            .unwrap(),
            "windows-arm64"
        );
        assert_eq!(
            RuntimePlatform::new(RuntimeOperatingSystem::MacOs, RuntimeArchitecture::Aarch64)
                .mojang_key()
                .unwrap(),
            "mac-os-arm64"
        );
        assert!(
            RuntimePlatform::new(RuntimeOperatingSystem::Linux, RuntimeArchitecture::Aarch64)
                .mojang_key()
                .is_err()
        );
        assert!(
            RuntimePlatform::new(RuntimeOperatingSystem::MacOs, RuntimeArchitecture::X86)
                .mojang_key()
                .is_err()
        );
    }

    #[test]
    fn java_major_parsing_handles_modern_and_legacy_names() {
        assert_eq!(parse_java_major("25.0.1"), Some(25));
        assert_eq!(parse_java_major("1.8.0_51"), Some(8));
        assert_eq!(parse_java_major("17-ea"), Some(17));
        assert_eq!(parse_java_major("nonsense"), None);
    }

    #[test]
    fn paths_and_links_fail_closed() {
        for path in ["../java", "/bin/java", "bin\\java", "C:/java", "bin//java"] {
            assert!(validate_relative_path(path).is_err(), "{path}");
        }
        assert!(validate_relative_path("bin/java").is_ok());
        assert!(validate_link_target("legal/compiler/LICENSE", "../base/LICENSE").is_ok());
        assert!(validate_link_target("top-link", "../outside").is_err());
        assert!(validate_component("java-runtime-epsilon").is_ok());
        assert!(validate_component("../epsilon").is_err());
    }
}
