//! Secure credential storage for authentication secrets.
//!
//! Tokens never live in ordinary JSON files: the only persisted secret is the
//! Microsoft refresh credential (plus the client ID it belongs to), and it is
//! stored through an OS-backed credential facility. On Windows that is the
//! real Windows Credential Manager (generic credentials). Other platforms
//! deliberately fail until a phase owns their backed store — the interface is
//! portable, the fallback is never a plaintext token file.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A string whose value must never reach logs, debug output, error
/// messages, events, or ordinary persisted JSON.
///
/// `Debug` and `Display` are redacted by construction. Rust cannot guarantee
/// erasure of every copy of the underlying bytes; this type's promise is
/// narrower and honest — it prevents accidental disclosure through the
/// common formatting paths and keeps secret lifetimes explicit.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Accesses the secret value. Call sites are limited to the
    /// authentication transport and the in-memory session.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[redacted secret]")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[redacted secret]")
    }
}

/// The minimum credential set required to restore a session: the Microsoft
/// refresh token and the client configuration it was issued for.
///
/// Everything downstream (Microsoft access token, Xbox token, XSTS token,
/// Minecraft access token) is memory-only and regenerated from this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedCredential {
    client_id: String,
    refresh_token: SecretString,
}

impl PersistedCredential {
    pub fn new(client_id: impl Into<String>, refresh_token: SecretString) -> Self {
        Self {
            client_id: client_id.into(),
            refresh_token,
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn refresh_token(&self) -> &SecretString {
        &self.refresh_token
    }

    /// Serializes the credential for the OS credential store.
    ///
    /// The JSON representation exists only inside this boundary and inside
    /// the OS-backed store entry; it is never written to launcher state
    /// files and never logged.
    fn encode(&self) -> String {
        let wire = StoredCredentialWire {
            client_id: self.client_id.clone(),
            refresh_token: self.refresh_token.expose().to_owned(),
        };
        serde_json::to_string(&wire).expect("credential serialization cannot fail")
    }

    /// Parses a credential previously written by [`Self::encode`].
    fn decode(stored: &str) -> Result<Self, CredentialStoreError> {
        let wire: StoredCredentialWire = serde_json::from_str(stored).map_err(|_| {
            CredentialStoreError::MalformedEntry(
                "the stored credential is not valid credential data".to_owned(),
            )
        })?;

        if wire.client_id.is_empty() || wire.refresh_token.is_empty() {
            return Err(CredentialStoreError::MalformedEntry(
                "the stored credential is missing required fields".to_owned(),
            ));
        }

        Ok(Self {
            client_id: wire.client_id,
            refresh_token: SecretString::new(wire.refresh_token),
        })
    }
}

/// Private wire shape for the credential-store payload.
#[derive(Serialize, Deserialize)]
struct StoredCredentialWire {
    client_id: String,
    refresh_token: String,
}

/// Failures of the credential-store boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialStoreError {
    /// This platform has no backed credential store implemented yet; Aurora
    /// refuses to fall back to plaintext token files.
    UnsupportedPlatform,
    /// The OS facility rejected the operation.
    Backend(String),
    /// An entry exists but cannot be parsed as an Aurora credential.
    MalformedEntry(String),
}

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(
                formatter,
                "this platform has no supported secure credential store yet; authentication persistence is unavailable until it is implemented"
            ),
            Self::Backend(reason) => write!(
                formatter,
                "the secure credential store rejected the operation: {reason}"
            ),
            Self::MalformedEntry(reason) => write!(
                formatter,
                "a stored credential is damaged and cannot be used: {reason}"
            ),
        }
    }
}

impl std::error::Error for CredentialStoreError {}

/// The narrow OS-backed credential-storage boundary: save, load, delete one
/// account's refresh credential, addressed by the account identifier.
///
/// This is deliberately not a general secrets manager.
pub trait CredentialStore: Send + Sync {
    fn save(
        &self,
        account_id: &str,
        credential: &PersistedCredential,
    ) -> Result<(), CredentialStoreError>;

    /// Loads the stored credential; `Ok(None)` means no credential exists.
    fn load(&self, account_id: &str) -> Result<Option<PersistedCredential>, CredentialStoreError>;

    /// Deletes the stored credential. Returns whether an entry existed.
    fn delete(&self, account_id: &str) -> Result<bool, CredentialStoreError>;
}

/// The launcher's OS-backed credential store.
///
/// Windows: the Windows Credential Manager via its documented generic
/// credential API (`CredWriteW`/`CredReadW`/`CredDeleteW`). Every other
/// platform fails deliberately with [`CredentialStoreError::UnsupportedPlatform`].
#[derive(Debug, Clone, Copy, Default)]
pub struct OsCredentialStore;

/// The credential-manager target name for one account. Fixed derivation from
/// the validated account identifier only.
fn entry_target(account_id: &str) -> String {
    format!("com.aurora.launcher/account/{account_id}")
}

impl CredentialStore for OsCredentialStore {
    fn save(
        &self,
        account_id: &str,
        credential: &PersistedCredential,
    ) -> Result<(), CredentialStoreError> {
        windows_impl::save(&entry_target(account_id), credential.encode().as_bytes())
    }

    fn load(&self, account_id: &str) -> Result<Option<PersistedCredential>, CredentialStoreError> {
        let stored = windows_impl::load(&entry_target(account_id))?;
        match stored {
            None => Ok(None),
            Some(bytes) => {
                let text = String::from_utf8(bytes).map_err(|_| {
                    CredentialStoreError::MalformedEntry(
                        "the stored credential is not valid UTF-8".to_owned(),
                    )
                })?;
                Ok(Some(PersistedCredential::decode(&text)?))
            }
        }
    }

    fn delete(&self, account_id: &str) -> Result<bool, CredentialStoreError> {
        windows_impl::delete(&entry_target(account_id))
    }
}

/// The Windows Credential Manager implementation, isolated so the unsafe FFI
/// surface stays in one audited place.
///
/// `windows-sys` models `PWSTR`/`PCWSTR` as raw pointer aliases, so the
/// credential fields below take plain pointers.
#[cfg(windows)]
mod windows_impl {
    use super::CredentialStoreError;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error(operation: &str) -> CredentialStoreError {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        CredentialStoreError::Backend(format!("{operation} failed with OS error {code}"))
    }

    pub fn save(target: &str, blob: &[u8]) -> Result<(), CredentialStoreError> {
        let mut target_wide = wide(target);
        let mut user_name = wide("Aurora Launcher");
        // The credential blob must stay mutable memory for the duration of
        // the call; CredWriteW copies it synchronously.
        let blob = blob.to_vec();

        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target_wide.as_mut_ptr(),
            Comment: std::ptr::null_mut(),
            LastWritten: FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: user_name.as_mut_ptr(),
        };

        // SAFETY: `credential` describes valid UTF-16 target/user buffers and
        // a blob pointer of the declared size, all owned above and alive for
        // the duration of the synchronous call.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(last_error("saving the credential"));
        }

        Ok(())
    }

    pub fn load(target: &str) -> Result<Option<Vec<u8>>, CredentialStoreError> {
        let target_wide = wide(target);
        let mut read: *mut CREDENTIALW = std::ptr::null_mut();

        // SAFETY: `target_wide` is a valid NUL-terminated UTF-16 string and
        // `read` receives a credential the caller frees with CredFree.
        let result = unsafe { CredReadW(target_wide.as_ptr(), CRED_TYPE_GENERIC, 0, &mut read) };

        if result == 0 {
            let error = std::io::Error::last_os_error();
            // ERROR_NOT_FOUND (1168) means no entry exists — not a failure.
            if error.raw_os_error() == Some(1168) {
                return Ok(None);
            }
            return Err(CredentialStoreError::Backend(format!(
                "reading the credential failed with OS error {}",
                error.raw_os_error().unwrap_or(0)
            )));
        }

        // SAFETY: on success `read` points at one credential allocated by the
        // credential manager; copy out the blob, then free the allocation.
        let credential = unsafe { &*read };
        let blob = unsafe {
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
            .to_vec()
        };

        // SAFETY: `read` was allocated by CredReadW and is freed exactly once.
        unsafe { CredFree(read.cast()) };

        Ok(Some(blob))
    }

    pub fn delete(target: &str) -> Result<bool, CredentialStoreError> {
        let target_wide = wide(target);

        // SAFETY: `target_wide` is a valid NUL-terminated UTF-16 string for
        // the duration of the synchronous call.
        if unsafe { CredDeleteW(target_wide.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(1168) {
                return Ok(false);
            }
            return Err(CredentialStoreError::Backend(format!(
                "deleting the credential failed with OS error {}",
                error.raw_os_error().unwrap_or(0)
            )));
        }

        Ok(true)
    }
}

/// Non-Windows platforms: no backed store is implemented yet.
#[cfg(not(windows))]
mod windows_impl {
    use super::CredentialStoreError;

    pub fn save(_target: &str, _blob: &[u8]) -> Result<(), CredentialStoreError> {
        Err(CredentialStoreError::UnsupportedPlatform)
    }

    pub fn load(_target: &str) -> Result<Option<Vec<u8>>, CredentialStoreError> {
        Err(CredentialStoreError::UnsupportedPlatform)
    }

    pub fn delete(_target: &str) -> Result<bool, CredentialStoreError> {
        Err(CredentialStoreError::UnsupportedPlatform)
    }
}

/// In-memory credential store for deterministic tests (and nothing else).
#[cfg(test)]
#[derive(Debug, Default)]
pub struct MemoryCredentialStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// When set, every operation fails — used to exercise store failures.
    fail: bool,
}

#[cfg(test)]
impl MemoryCredentialStore {
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }
}

#[cfg(test)]
impl CredentialStore for MemoryCredentialStore {
    fn save(
        &self,
        account_id: &str,
        credential: &PersistedCredential,
    ) -> Result<(), CredentialStoreError> {
        if self.fail {
            return Err(CredentialStoreError::Backend(
                "injected test failure".to_owned(),
            ));
        }
        self.entries
            .lock()
            .expect("test credential store is not poisoned")
            .insert(account_id.to_owned(), credential.encode());
        Ok(())
    }

    fn load(&self, account_id: &str) -> Result<Option<PersistedCredential>, CredentialStoreError> {
        if self.fail {
            return Err(CredentialStoreError::Backend(
                "injected test failure".to_owned(),
            ));
        }
        let stored = self
            .entries
            .lock()
            .expect("test credential store is not poisoned")
            .get(account_id)
            .cloned();
        match stored {
            None => Ok(None),
            Some(text) => Ok(Some(PersistedCredential::decode(&text)?)),
        }
    }

    fn delete(&self, account_id: &str) -> Result<bool, CredentialStoreError> {
        if self.fail {
            return Err(CredentialStoreError::Backend(
                "injected test failure".to_owned(),
            ));
        }
        Ok(self
            .entries
            .lock()
            .expect("test credential store is not poisoned")
            .remove(account_id)
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_credential() -> PersistedCredential {
        PersistedCredential::new(
            "aurora-test-client-id",
            SecretString::new("FIXTURE-REFRESH-TOKEN-0123456789"),
        )
    }

    #[test]
    fn secret_strings_redact_debug_and_display() {
        let secret = SecretString::new("FIXTURE-SECRET-VALUE");

        assert!(!format!("{secret:?}").contains("FIXTURE-SECRET-VALUE"));
        assert!(!format!("{secret}").contains("FIXTURE-SECRET-VALUE"));
        assert!(!format!("{secret:#?}").contains("FIXTURE-SECRET-VALUE"));
        assert_eq!(secret.expose(), "FIXTURE-SECRET-VALUE");
    }

    #[test]
    fn credential_debug_output_redacts_the_refresh_token() {
        let credential = sample_credential();
        let rendered = format!("{credential:?}");

        assert!(rendered.contains("aurora-test-client-id"));
        assert!(!rendered.contains("FIXTURE-REFRESH-TOKEN"));
    }

    #[test]
    fn memory_store_round_trips_credentials() {
        let store = MemoryCredentialStore::default();

        assert_eq!(store.load("account").unwrap(), None);

        store.save("account", &sample_credential()).unwrap();
        let loaded = store.load("account").unwrap().unwrap();
        assert_eq!(loaded.client_id(), "aurora-test-client-id");
        assert_eq!(
            loaded.refresh_token().expose(),
            "FIXTURE-REFRESH-TOKEN-0123456789"
        );

        assert_eq!(store.delete("account").unwrap(), true);
        assert_eq!(store.load("account").unwrap(), None);
        assert_eq!(store.delete("account").unwrap(), false);
    }

    #[test]
    fn memory_store_failure_is_reported() {
        let store = MemoryCredentialStore::failing();

        assert!(matches!(
            store.save("account", &sample_credential()),
            Err(CredentialStoreError::Backend(_))
        ));
        assert!(matches!(
            store.load("account"),
            Err(CredentialStoreError::Backend(_))
        ));
        assert!(matches!(
            store.delete("account"),
            Err(CredentialStoreError::Backend(_))
        ));
    }

    #[test]
    fn malformed_entries_fail_rather_than_panic() {
        let store = MemoryCredentialStore::default();
        store
            .entries
            .lock()
            .unwrap()
            .insert("account".to_owned(), "{ not credential json".to_owned());

        assert!(matches!(
            store.load("account"),
            Err(CredentialStoreError::MalformedEntry(_))
        ));
    }

    #[test]
    fn entry_targets_derive_from_account_ids_only() {
        assert_eq!(
            entry_target("986dec87b7ec47ff89ff033fdb95c4b5"),
            "com.aurora.launcher/account/986dec87b7ec47ff89ff033fdb95c4b5"
        );
    }

    /// Proves the real Windows Credential Manager backend round-trips a
    /// credential end to end, using a test-only account namespace and
    /// cleaning up after itself.
    #[cfg(windows)]
    #[test]
    fn windows_credential_manager_round_trips() {
        let target_account = format!("test-{}", std::process::id());
        let store = OsCredentialStore;

        let outcome = (|| -> Result<(), CredentialStoreError> {
            store.delete(&target_account).ok();

            store.save(&target_account, &sample_credential())?;
            let loaded = store.load(&target_account)?.expect("entry must exist");
            assert_eq!(loaded.client_id(), "aurora-test-client-id");
            assert_eq!(
                loaded.refresh_token().expose(),
                "FIXTURE-REFRESH-TOKEN-0123456789"
            );

            assert!(store.delete(&target_account)?);
            assert_eq!(store.load(&target_account)?, None);
            Ok(())
        })();

        // Best-effort cleanup even when assertions fail.
        let _ = store.delete(&target_account);
        outcome.expect("Windows credential manager round trip");
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platforms_fail_deliberately() {
        let store = OsCredentialStore;

        assert_eq!(
            store.save("account", &sample_credential()),
            Err(CredentialStoreError::UnsupportedPlatform)
        );
        assert_eq!(
            store.load("account"),
            Err(CredentialStoreError::UnsupportedPlatform)
        );
        assert_eq!(
            store.delete("account"),
            Err(CredentialStoreError::UnsupportedPlatform)
        );
    }
}
