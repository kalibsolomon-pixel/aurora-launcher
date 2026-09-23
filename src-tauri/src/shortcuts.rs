//! Windows shell-shortcut integration for Aurora's desktop identity.
//!
//! Aurora exposes user-controlled Windows shortcut integration with one
//! ownership rule: the launcher only ever creates or removes a `.lnk` it can
//! positively identify as its own — the deterministic, well-named shortcut
//! slot whose target is the currently running Aurora executable. Anything
//! else that occupies the slot is a reported conflict and is never
//! overwritten or deleted.
//!
//! Slots and owners:
//!
//! - **Desktop** — `%Desktop%\Aurora Launcher.lnk`, owned by the launcher.
//!   The Settings page may create and remove it. Creation and removal are
//!   offered only for installed production builds: a development or build-
//!   output executable must never become a permanent user shortcut.
//! - **Start menu** — owned by the installers, never by the launcher. Both
//!   Aurora installers create Start-menu shortcuts and remove them on
//!   uninstall (WiX lays out `Programs\Aurora Launcher\Aurora Launcher.lnk`;
//!   NSIS writes `Programs\Aurora Launcher.lnk` by default). The launcher
//!   only *reports* presence for these slots and never duplicates or
//!   competes with the installer.
//!
//! Shortcut status is always queried from the filesystem, never persisted:
//! a user may delete a shortcut outside Aurora, and Settings must reflect
//! that reality when opened or refreshed.
//!
//! Shortcuts are real `.lnk` shell links written through the Windows shell's
//! COM API (`IShellLinkW` + `IPersistFile`) — no shell-command strings, no
//! script interpolation. A shortcut carries the target executable, its
//! working directory, and a description; the icon deliberately stays unset
//! so the shell resolves the target's own embedded application icon
//! (Aurora's external icon resource), matching the WiX desktop shortcut's
//! property set. Windows-known-folder APIs resolve the Desktop and Programs
//! directories, so redirected and localized profiles work; no path is
//! derived from hard-coded English folder names.
//!
//! Taskbar pinning is intentionally absent: Windows deliberately restricts
//! programmatic pinning and it stays a user choice.

use std::fmt;
use std::path::{Path, PathBuf};

/// The fixed display name of the Aurora shortcut file and its description.
///
/// This is the compatibility contract with the installers (both name their
/// shortcuts after the product name) and with the Settings page copy.
pub const PRODUCT_NAME: &str = "Aurora Launcher";

/// The `.lnk` file name of every Aurora-managed shortcut slot.
pub fn shortcut_file_name() -> String {
    format!("{PRODUCT_NAME}.lnk")
}

/// The ownership classification of one shortcut slot on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutPresence {
    /// No file occupies the slot.
    Absent,
    /// The slot holds a shortcut targeting this Aurora executable: owned.
    Present,
    /// Something occupies the slot that Aurora cannot prove ownership of —
    /// a user file or a shortcut pointing elsewhere. Never touched.
    Conflict,
}

/// What the filesystem and the shell say about one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotEvidence {
    /// No file exists at the slot path.
    Missing,
    /// The slot is a readable shell shortcut whose target is this path.
    Target(PathBuf),
    /// A file exists but is not a readable shell shortcut.
    Unreadable,
}

/// Pure ownership classification from gathered evidence.
///
/// A shortcut Aurora created always records the absolute path of the exact
/// executable that created it, so a target mismatch means the file belongs
/// to something else — another install, another copy, or a user's own link.
pub fn classify_shortcut(evidence: &SlotEvidence, expected_target: &Path) -> ShortcutPresence {
    match evidence {
        SlotEvidence::Missing => ShortcutPresence::Absent,
        SlotEvidence::Target(target) => {
            if paths_equivalent(target, expected_target) {
                ShortcutPresence::Present
            } else {
                ShortcutPresence::Conflict
            }
        }
        SlotEvidence::Unreadable => ShortcutPresence::Conflict,
    }
}

/// Case-insensitive, separator-normalized path equivalence (Windows path
/// semantics; the shortcut domain exists for Windows).
///
/// Component-wise comparison avoids `canonicalize`'s `\\?\` prefixes and
/// keeps working when one side is the literal path stored inside a `.lnk`
/// and the other is the running executable's reported path. Windows 8.3
/// short-path aliases are not expanded; they do not occur for the install
/// locations Aurora's own installers use.
pub fn paths_equivalent(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| -> Vec<String> {
        path.components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_string_lossy()
                    .to_lowercase()
                    .replace('/', "\\")
            })
            .collect()
    };
    normalize(left) == normalize(right)
}

/// The Aurora-owned desktop shortcut slot inside a resolved Desktop folder.
pub fn desktop_slot(desktop_dir: &Path) -> PathBuf {
    desktop_dir.join(shortcut_file_name())
}

/// The Start-menu slots Aurora's installers own, inside a resolved Programs
/// folder: the NSIS default (no menu category) and the WiX per-product
/// folder layout. Order is deterministic.
pub fn start_menu_slots(programs_dir: &Path) -> Vec<PathBuf> {
    vec![
        programs_dir.join(shortcut_file_name()),
        programs_dir.join(PRODUCT_NAME).join(shortcut_file_name()),
    ]
}

/// Whether this build may create or remove user-visible shortcuts.
///
/// Installed production builds only: a release-profile executable that does
/// not live inside a Cargo build-output directory. Development/debug builds
/// and executables run straight from `target/…` must never become permanent
/// user shortcuts; their Settings state reports that an installed production
/// build is required.
pub fn shortcut_manageable(current_exe: &Path, release_build: bool) -> bool {
    if !release_build {
        return false;
    }
    !current_exe
        .ancestors()
        .any(|ancestor| ancestor.file_name().is_some_and(|name| name == "target"))
}

/// Failures of the shortcut boundary, mapped to stable command codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutError {
    /// This platform has no shortcut integration implemented.
    UnsupportedPlatform,
    /// Shortcut management requires an installed production build.
    ManagementUnavailable,
    /// The slot is occupied by something Aurora does not own.
    Conflict,
    /// Filesystem operation on the slot failed.
    Io(String),
    /// The Windows shell rejected the operation.
    Shell(String),
}

impl ShortcutError {
    /// The stable command error code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "shortcut_unsupported_platform",
            Self::ManagementUnavailable => "shortcut_management_unavailable",
            Self::Conflict => "shortcut_conflict",
            Self::Io(_) => "shortcut_io_failure",
            Self::Shell(_) => "shortcut_shell_failure",
        }
    }
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(
                formatter,
                "Windows shortcut integration is not available on this platform"
            ),
            Self::ManagementUnavailable => write!(
                formatter,
                "shortcut management requires an installed production build of Aurora"
            ),
            Self::Conflict => write!(
                formatter,
                "an item named \"{PRODUCT_NAME}\" already exists there and is not an Aurora shortcut, so Aurora left it untouched"
            ),
            Self::Io(reason) => {
                write!(formatter, "a shortcut file operation failed: {reason}")
            }
            Self::Shell(reason) => {
                write!(
                    formatter,
                    "the Windows shell rejected the shortcut operation: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for ShortcutError {}

/// One resolved desktop-integration world: the known folders to inspect and
/// the executable that ownership is proven against.
///
/// The fields are injected so deterministic tests can exercise the exact
/// create/inspect/remove flows against scratch directories instead of the
/// developer's real Desktop. `common_programs_dir` covers the machine-wide
/// Programs folder the per-user WiX install writes its Start-menu shortcut
/// into (the NSIS per-user install writes the user folder).
#[derive(Debug, Clone)]
pub struct ShortcutContext {
    pub desktop_dir: PathBuf,
    pub programs_dir: PathBuf,
    /// The machine-wide Programs folder, when it resolves. MSI installs are
    /// per-machine by default; without this folder their Start-menu shortcut
    /// would be invisible to the read-only reporting.
    pub common_programs_dir: Option<PathBuf>,
    pub target_exe: PathBuf,
}

impl ShortcutContext {
    /// Evidence for one slot, gathered through the platform shell reader.
    fn evidence(&self, slot: &Path) -> SlotEvidence {
        if !slot.exists() {
            return SlotEvidence::Missing;
        }
        match windows_impl::read_shortcut_target(slot) {
            Some(target) => SlotEvidence::Target(target),
            None => SlotEvidence::Unreadable,
        }
    }

    /// The presence of the Aurora-owned desktop shortcut.
    pub fn desktop_presence(&self) -> ShortcutPresence {
        classify_shortcut(
            &self.evidence(&desktop_slot(&self.desktop_dir)),
            &self.target_exe,
        )
    }

    /// The presence of the installer-owned Start-menu shortcut (reported,
    /// never managed): present when any installer slot — in the user's or
    /// the machine's Programs folder — targets this executable, conflict
    /// when a slot exists that Aurora cannot attribute.
    pub fn start_menu_presence(&self) -> ShortcutPresence {
        let mut saw_conflict = false;
        let mut slots = start_menu_slots(&self.programs_dir);
        if let Some(common) = &self.common_programs_dir {
            slots.extend(start_menu_slots(common));
        }
        for slot in slots {
            match classify_shortcut(&self.evidence(&slot), &self.target_exe) {
                ShortcutPresence::Present => return ShortcutPresence::Present,
                ShortcutPresence::Conflict => saw_conflict = true,
                ShortcutPresence::Absent => {}
            }
        }
        if saw_conflict {
            ShortcutPresence::Conflict
        } else {
            ShortcutPresence::Absent
        }
    }

    /// Creates (or refreshes) the Aurora-owned desktop shortcut.
    ///
    /// Refuses a slot occupied by anything Aurora cannot prove it owns.
    pub fn create_desktop_shortcut(&self) -> Result<ShortcutPresence, ShortcutError> {
        let slot = desktop_slot(&self.desktop_dir);
        match self.desktop_presence() {
            ShortcutPresence::Conflict => Err(ShortcutError::Conflict),
            ShortcutPresence::Present | ShortcutPresence::Absent => {
                let working_dir = self
                    .target_exe
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."));
                windows_impl::write_shortcut(&slot, &self.target_exe, &working_dir, PRODUCT_NAME)?;
                Ok(ShortcutPresence::Present)
            }
        }
    }

    /// Removes the Aurora-owned desktop shortcut.
    ///
    /// Never deletes a slot that is not provably Aurora's; an absent slot is
    /// an idempotent success.
    pub fn remove_desktop_shortcut(&self) -> Result<ShortcutPresence, ShortcutError> {
        let slot = desktop_slot(&self.desktop_dir);
        match self.desktop_presence() {
            ShortcutPresence::Absent => Ok(ShortcutPresence::Absent),
            ShortcutPresence::Conflict => Err(ShortcutError::Conflict),
            ShortcutPresence::Present => {
                std::fs::remove_file(&slot)
                    .map_err(|error| ShortcutError::Io(error.to_string()))?;
                Ok(ShortcutPresence::Absent)
            }
        }
    }
}

/// Resolves the user's Desktop known folder through the Windows API.
pub fn desktop_known_folder() -> Result<PathBuf, ShortcutError> {
    windows_impl::desktop_known_folder()
}

/// Resolves the user's Start-menu Programs known folder through the Windows
/// API.
pub fn programs_known_folder() -> Result<PathBuf, ShortcutError> {
    windows_impl::programs_known_folder()
}

/// Resolves the machine-wide Start-menu Programs known folder (where the
/// per-machine MSI writes its shortcut). Read-only resolution needs no
/// elevation; failure simply omits the folder from reporting.
pub fn common_programs_known_folder() -> Result<PathBuf, ShortcutError> {
    windows_impl::common_programs_known_folder()
}

/// The Windows shell implementation, isolated so the unsafe COM surface
/// stays in one audited place.
#[cfg(windows)]
mod windows_impl {
    use std::path::{Path, PathBuf};

    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::{
        FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW,
        SHGetKnownFolderPath, ShellLink,
    };
    use windows::core::{GUID, Interface};

    use super::ShortcutError;

    /// Balances `CoInitializeEx` per the COM rules: every success (including
    /// `S_FALSE`, "already initialized") owes exactly one `CoUninitialize`;
    /// `RPC_E_CHANGED_MODE` owes nothing because COM is already up in
    /// another mode on this thread.
    struct ComApartment {
        balanced: bool,
    }

    impl ComApartment {
        fn enter() -> Self {
            // SAFETY: initializing COM on this thread is a thread-local,
            // reference-counted operation with no external preconditions.
            let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            Self {
                balanced: result.is_ok(),
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            if self.balanced {
                // SAFETY: paired with the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// HRESULT messages are developer diagnostics; keep them concise.
    fn sanitize(message: &str) -> String {
        message.chars().take(200).collect()
    }

    fn wide(value: &Path) -> windows::core::HSTRING {
        windows::core::HSTRING::from(value.as_os_str())
    }

    fn wide_str(value: &str) -> windows::core::HSTRING {
        windows::core::HSTRING::from(value)
    }

    pub(super) fn desktop_known_folder() -> Result<PathBuf, ShortcutError> {
        known_folder(&FOLDERID_Desktop)
    }

    pub(super) fn programs_known_folder() -> Result<PathBuf, ShortcutError> {
        known_folder(&FOLDERID_Programs)
    }

    pub(super) fn common_programs_known_folder() -> Result<PathBuf, ShortcutError> {
        known_folder(&FOLDERID_CommonPrograms)
    }

    fn known_folder(folder_id: &GUID) -> Result<PathBuf, ShortcutError> {
        let _apartment = ComApartment::enter();

        // SAFETY: a valid KNOWNFOLDERID with default flags and no token
        // handle; on success the returned string is copied out and then
        // freed with the allocator that produced it.
        let wide_path = unsafe { SHGetKnownFolderPath(folder_id, Default::default(), None) }
            .map_err(|error| {
                ShortcutError::Shell(format!(
                    "resolving a known folder failed: {}",
                    sanitize(&error.to_string())
                ))
            })?;
        // SAFETY: the PWSTR is a valid NUL-terminated UTF-16 string owned by
        // the shell allocation freed below; reading it is a pure operation.
        let decoded = unsafe { wide_path.to_string() };
        let path = PathBuf::from(decoded.map_err(|_| {
            ShortcutError::Shell("a known-folder path was not valid UTF-16".to_string())
        })?);
        // SAFETY: the PWSTR was allocated by the shell with the COM task
        // allocator and is freed exactly once.
        unsafe { CoTaskMemFree(Some(wide_path.as_ptr().cast())) };
        Ok(path)
    }

    /// Reads the target path recorded inside a `.lnk`. `None` means the
    /// file is not a readable shell shortcut.
    pub(super) fn read_shortcut_target(slot: &Path) -> Option<PathBuf> {
        let _apartment = ComApartment::enter();

        // SAFETY: CoCreateInstance with the documented ShellLink class and
        // an in-process server context yields a correctly ref-counted
        // IShellLinkW or an error.
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        // SAFETY: the file name is a valid HSTRING and the mode flags are
        // the defaults (read access, no shared-mode denial).
        unsafe { persist.Load(&wide(slot), Default::default()) }.ok()?;
        let mut buffer = [0u16; 1024];
        let mut find_data = WIN32_FIND_DATAW::default();
        // SLGP_RAWPATH (0x4): the path exactly as the shortcut records it,
        // without indirect expansion. The buffer is a writable 1024-unit
        // UTF-16 array and find_data is zeroed, as the API expects.
        unsafe { link.GetPath(&mut buffer, &mut find_data, 0x4) }.ok()?;
        let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
        if length == 0 {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
    }

    /// Writes a shell shortcut: target, working directory, description.
    ///
    /// The icon stays unset so the shell shows the target executable's own
    /// embedded application icon (Aurora's external icon resource),
    /// matching the WiX desktop shortcut's property set.
    pub(super) fn write_shortcut(
        slot: &Path,
        target: &Path,
        working_dir: &Path,
        description: &str,
    ) -> Result<(), ShortcutError> {
        let _apartment = ComApartment::enter();

        // SAFETY: as in read_shortcut_target.
        let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
            .map_err(|error| {
                ShortcutError::Shell(format!(
                    "creating the shell link object failed: {}",
                    sanitize(&error.to_string())
                ))
            })?;

        // SAFETY: each setter receives a valid HSTRING; the object is a
        // live in-process shell link owned by `link`.
        unsafe {
            link.SetPath(&wide(target))
                .map_err(|error| ShortcutError::Shell(sanitize(&error.to_string())))?;
            link.SetWorkingDirectory(&wide(working_dir))
                .map_err(|error| ShortcutError::Shell(sanitize(&error.to_string())))?;
            link.SetDescription(&wide_str(description))
                .map_err(|error| ShortcutError::Shell(sanitize(&error.to_string())))?;
        }

        let persist: IPersistFile = link
            .cast()
            .map_err(|error| ShortcutError::Shell(sanitize(&error.to_string())))?;
        // SAFETY: the destination path is a valid HSTRING; `true` asks the
        // shell to remember the link as the persistent copy.
        unsafe { persist.Save(&wide(slot), true) }
            .map_err(|error| ShortcutError::Shell(sanitize(&error.to_string())))?;
        Ok(())
    }
}

/// Non-Windows platforms: no shortcut integration is implemented, and the
/// boundary fails deliberately instead of pretending support.
#[cfg(not(windows))]
mod windows_impl {
    use std::path::{Path, PathBuf};

    use super::ShortcutError;

    pub(super) fn desktop_known_folder() -> Result<PathBuf, ShortcutError> {
        Err(ShortcutError::UnsupportedPlatform)
    }

    pub(super) fn programs_known_folder() -> Result<PathBuf, ShortcutError> {
        Err(ShortcutError::UnsupportedPlatform)
    }

    pub(super) fn common_programs_known_folder() -> Result<PathBuf, ShortcutError> {
        Err(ShortcutError::UnsupportedPlatform)
    }

    pub(super) fn read_shortcut_target(_slot: &Path) -> Option<PathBuf> {
        None
    }

    pub(super) fn write_shortcut(
        _slot: &Path,
        _target: &Path,
        _working_dir: &Path,
        _description: &str,
    ) -> Result<(), ShortcutError> {
        Err(ShortcutError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_slot_is_deterministic_and_named_after_the_product() {
        let slot = desktop_slot(Path::new(r"C:\Users\user\Desktop"));
        assert_eq!(
            slot,
            Path::new(r"C:\Users\user\Desktop\Aurora Launcher.lnk")
        );
    }

    #[test]
    fn start_menu_slots_cover_both_installer_layouts_in_order() {
        let slots = start_menu_slots(Path::new(r"C:\Users\user\Programs"));
        assert_eq!(slots.len(), 2);
        assert_eq!(
            slots[0],
            Path::new(r"C:\Users\user\Programs\Aurora Launcher.lnk")
        );
        assert_eq!(
            slots[1],
            Path::new(r"C:\Users\user\Programs\Aurora Launcher\Aurora Launcher.lnk")
        );
    }

    #[test]
    fn missing_evidence_classifies_absent() {
        assert_eq!(
            classify_shortcut(&SlotEvidence::Missing, Path::new(r"C:\aurora.exe")),
            ShortcutPresence::Absent
        );
    }

    #[test]
    fn unreadable_evidence_classifies_conflict() {
        assert_eq!(
            classify_shortcut(&SlotEvidence::Unreadable, Path::new(r"C:\aurora.exe")),
            ShortcutPresence::Conflict
        );
    }

    #[test]
    fn matching_target_classifies_present_case_insensitively() {
        assert_eq!(
            classify_shortcut(
                &SlotEvidence::Target(PathBuf::from(r"C:\PROGRAM FILES\Aurora Launcher\app.EXE")),
                Path::new(r"c:\Program Files\aurora launcher\App.exe")
            ),
            ShortcutPresence::Present
        );
    }

    #[test]
    fn mismatched_target_classifies_conflict() {
        assert_eq!(
            classify_shortcut(
                &SlotEvidence::Target(PathBuf::from(r"C:\somewhere\other.exe")),
                Path::new(r"C:\Program Files\Aurora Launcher\app.exe")
            ),
            ShortcutPresence::Conflict
        );
    }

    #[test]
    fn path_equivalence_normalizes_separators_and_case() {
        assert!(paths_equivalent(
            Path::new(r"C:\dir\Aurora.exe"),
            Path::new("c:/dir/aurora.exe")
        ));
        assert!(!paths_equivalent(
            Path::new(r"C:\dir\Aurora.exe"),
            Path::new(r"C:\dir\Aurora2.exe")
        ));
    }

    #[test]
    fn debug_and_target_build_outputs_are_never_manageable() {
        let debug_exe = Path::new(r"C:\dev\aurora\target\debug\aurora.exe");
        let release_exe = Path::new(r"C:\dev\aurora\target\release\aurora.exe");
        assert!(!shortcut_manageable(debug_exe, false));
        assert!(!shortcut_manageable(release_exe, true));
        assert!(!shortcut_manageable(Path::new(r"C:\anywhere\a.exe"), false));
    }

    #[test]
    fn installed_release_builds_are_manageable() {
        assert!(shortcut_manageable(
            Path::new(r"C:\Program Files\Aurora Launcher\aurora-launcher.exe"),
            true
        ));
        assert!(shortcut_manageable(
            Path::new(r"C:\Users\u\AppData\Local\Aurora Launcher\aurora-launcher.exe"),
            true
        ));
    }

    #[test]
    fn error_codes_are_stable_and_messages_are_readable() {
        let cases = [
            (
                ShortcutError::UnsupportedPlatform,
                "shortcut_unsupported_platform",
            ),
            (
                ShortcutError::ManagementUnavailable,
                "shortcut_management_unavailable",
            ),
            (ShortcutError::Conflict, "shortcut_conflict"),
            (ShortcutError::Io("x".into()), "shortcut_io_failure"),
            (ShortcutError::Shell("x".into()), "shortcut_shell_failure"),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
            assert!(!error.to_string().is_empty());
        }
    }

    /// A scratch-directory exercise of the full real-Windows shortcut flow:
    /// create through the shell COM API, detect as owned, refuse to touch a
    /// conflicting foreign shortcut or user file, remove only the owned
    /// shortcut, and report installer-owned Start-menu slots read-only.
    ///
    /// Runs entirely inside a process-named temporary directory beneath the
    /// system temp root; nothing on the real Desktop is read or written, and
    /// cleanup removes exactly that scratch root.
    #[cfg(windows)]
    #[test]
    fn windows_shortcut_lifecycle_in_scratch_directory() {
        let scratch =
            std::env::temp_dir().join(format!("aurora-shortcut-test-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("create scratch root");

        let outcome = (|| -> Result<(), Box<dyn std::error::Error>> {
            let desktop = scratch.join("desktop");
            let programs = scratch.join("programs");
            std::fs::create_dir_all(&desktop)?;
            std::fs::create_dir_all(&programs)?;

            // A stand-in "installed production executable" as the target.
            let target = scratch.join("install").join("aurora-launcher.exe");
            std::fs::create_dir_all(target.parent().unwrap())?;
            std::fs::write(&target, b"test exe")?;

            let context = ShortcutContext {
                desktop_dir: desktop.clone(),
                programs_dir: programs,
                common_programs_dir: None,
                target_exe: target.clone(),
            };

            // Nothing exists yet.
            assert_eq!(context.desktop_presence(), ShortcutPresence::Absent);
            assert_eq!(context.start_menu_presence(), ShortcutPresence::Absent);

            // Create through the real shell API, then observe it.
            assert_eq!(
                context.create_desktop_shortcut()?,
                ShortcutPresence::Present
            );
            assert_eq!(context.desktop_presence(), ShortcutPresence::Present);

            // Creation is idempotent and refreshes the existing owned link.
            assert_eq!(
                context.create_desktop_shortcut()?,
                ShortcutPresence::Present
            );

            // A foreign .lnk targeting another executable is a conflict:
            // neither removal nor re-creation may touch it. Rewriting the
            // slot through the same shell API keeps it a real .lnk.
            let foreign_target = scratch.join("other").join("app.exe");
            std::fs::create_dir_all(foreign_target.parent().unwrap())?;
            std::fs::write(&foreign_target, b"foreign")?;
            let slot = desktop_slot(&desktop);
            windows_impl::write_shortcut(
                &slot,
                &foreign_target,
                foreign_target.parent().unwrap(),
                "foreign",
            )?;
            assert_eq!(context.desktop_presence(), ShortcutPresence::Conflict);
            assert_eq!(
                context.remove_desktop_shortcut(),
                Err(ShortcutError::Conflict)
            );
            assert_eq!(
                context.create_desktop_shortcut(),
                Err(ShortcutError::Conflict)
            );
            assert!(slot.exists(), "conflicting shortcut must survive");

            // A non-shortcut file at the slot is equally a conflict.
            std::fs::remove_file(&slot)?;
            std::fs::write(&slot, b"not a lnk")?;
            assert_eq!(context.desktop_presence(), ShortcutPresence::Conflict);
            assert_eq!(
                context.remove_desktop_shortcut(),
                Err(ShortcutError::Conflict)
            );
            assert!(slot.exists(), "user file must survive");

            // Recreate the owned shortcut and remove it.
            std::fs::remove_file(&slot)?;
            assert_eq!(
                context.create_desktop_shortcut()?,
                ShortcutPresence::Present
            );
            assert_eq!(context.remove_desktop_shortcut()?, ShortcutPresence::Absent);
            assert!(!slot.exists());

            // Idempotent removal of an absent slot.
            assert_eq!(context.remove_desktop_shortcut()?, ShortcutPresence::Absent);

            // Installer-owned Start-menu slots report presence read-only.
            let msi_layout = start_menu_slots(&context.programs_dir)[1].clone();
            std::fs::create_dir_all(msi_layout.parent().unwrap())?;
            windows_impl::write_shortcut(
                &msi_layout,
                &target,
                target.parent().unwrap(),
                PRODUCT_NAME,
            )?;
            assert_eq!(context.start_menu_presence(), ShortcutPresence::Present);

            // A Start-menu shortcut targeting something else is not ours and
            // never reports Present.
            windows_impl::write_shortcut(
                &msi_layout,
                &foreign_target,
                foreign_target.parent().unwrap(),
                PRODUCT_NAME,
            )?;
            assert_eq!(context.start_menu_presence(), ShortcutPresence::Conflict);

            // The machine-wide Programs folder (per-machine MSI installs)
            // is reported too: a shortcut there targeting this executable
            // reads Present, independent of the user Programs folder.
            let common_programs = scratch.join("common-programs");
            std::fs::create_dir_all(common_programs.join(PRODUCT_NAME))?;
            let common_context = ShortcutContext {
                desktop_dir: desktop,
                programs_dir: context.programs_dir.clone(),
                common_programs_dir: Some(common_programs.clone()),
                target_exe: target.clone(),
            };
            let msi_machine_wide = start_menu_slots(&common_programs)[1].clone();
            windows_impl::write_shortcut(
                &msi_machine_wide,
                &target,
                target.parent().unwrap(),
                PRODUCT_NAME,
            )?;
            assert_eq!(
                common_context.start_menu_presence(),
                ShortcutPresence::Present
            );
            Ok(())
        })();

        // Cleanup: exactly the scratch root this test created.
        let _ = std::fs::remove_dir_all(&scratch);
        outcome.expect("windows shortcut lifecycle");
    }

    /// Non-Windows builds: the shell implementation fails deliberately and
    /// never pretends shortcut support exists.
    #[cfg(not(windows))]
    #[test]
    fn unsupported_platforms_fail_deliberately() {
        assert_eq!(
            windows_impl::write_shortcut(
                Path::new("/tmp/a.lnk"),
                Path::new("/tmp/a"),
                Path::new("/tmp"),
                "x"
            ),
            Err(ShortcutError::UnsupportedPlatform)
        );
        assert_eq!(
            windows_impl::desktop_known_folder(),
            Err(ShortcutError::UnsupportedPlatform)
        );
        assert_eq!(
            windows_impl::programs_known_folder(),
            Err(ShortcutError::UnsupportedPlatform)
        );
    }
}
