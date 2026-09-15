//! Versioned, non-secret account summaries and their persistence.
//!
//! The accounts document records only identity Aurora needs to display and
//! address accounts: the account identifier, the Minecraft name, and the
//! selection. Tokens never appear here. Persistence follows the launcher's
//! document conventions exactly: camelCase pretty JSON, atomic
//! sibling-plus-rename writes, schema-versioned parsing, and malformed or
//! unsupported files that are never silently overwritten.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The only accounts-document schema version this launcher understands.
pub const ACCOUNTS_SCHEMA_VERSION: u32 = 1;

/// The account identifier shape: the Minecraft profile UUID in its
/// canonical undashed lowercase-hex form.
///
/// The Minecraft UUID is deliberately the account key (see the architecture
/// document): it is stable for the account's lifetime, non-secret, and makes
/// re-signing into a known account an update rather than a duplicate.
/// Display names, emails, and tokens are never identifiers.
pub struct AccountId;

impl AccountId {
    /// Validates an account identifier (32 lowercase hex characters).
    pub fn validate(value: &str) -> Result<(), InvalidAccount> {
        let valid_length = value.len() == 32;
        let valid_charset = value
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        if !valid_length || !valid_charset {
            return Err(InvalidAccount {
                value: value.to_owned(),
            });
        }
        Ok(())
    }
}

/// An identifier that is not a validated account id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAccount {
    value: String,
}

impl fmt::Display for InvalidAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "'{}' is not a valid account identifier (32 lowercase hexadecimal characters)",
            self.value
        )
    }
}

impl std::error::Error for InvalidAccount {}

/// Validates a Minecraft profile name: 3–16 characters of `A-Za-z0-9_`.
pub fn validate_minecraft_name(name: &str) -> Result<(), InvalidAccount> {
    let valid_length = (3..=16).contains(&name.chars().count());
    let valid_charset = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid_length || !valid_charset {
        return Err(InvalidAccount {
            value: name.to_owned(),
        });
    }
    Ok(())
}

/// One persisted, non-secret account summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    account_id: String,
    minecraft_name: String,
}

impl AccountRecord {
    pub fn new(account_id: impl Into<String>, minecraft_name: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            minecraft_name: minecraft_name.into(),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn minecraft_name(&self) -> &str {
        &self.minecraft_name
    }

    pub(crate) fn set_minecraft_name(&mut self, name: impl Into<String>) {
        self.minecraft_name = name.into();
    }
}

/// The versioned accounts document stored at
/// `<managed-data-root>/launcher/accounts.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountsDocument {
    schema_version: u32,
    selected_account_id: Option<String>,
    accounts: Vec<AccountRecord>,
}

impl Default for AccountsDocument {
    fn default() -> Self {
        Self {
            schema_version: ACCOUNTS_SCHEMA_VERSION,
            selected_account_id: None,
            accounts: Vec::new(),
        }
    }
}

impl AccountsDocument {
    /// The accounts schema version this launcher writes and enforces.
    pub const SCHEMA_VERSION: u32 = ACCOUNTS_SCHEMA_VERSION;

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn selected_account_id(&self) -> Option<&str> {
        self.selected_account_id.as_deref()
    }

    pub fn accounts(&self) -> &[AccountRecord] {
        &self.accounts
    }

    /// Mutable access for the lifecycle operations that upsert or remove
    /// records under the accounts lock. Invariants are re-proven on save.
    pub(crate) fn accounts_mut(&mut self) -> &mut Vec<AccountRecord> {
        &mut self.accounts
    }

    pub fn find(&self, account_id: &str) -> Option<&AccountRecord> {
        self.accounts
            .iter()
            .find(|record| record.account_id == account_id)
    }

    /// Sets the selection. Referential integrity against the account list is
    /// enforced here: a dangling selection is never representable.
    pub fn set_selected_account_id(
        &mut self,
        account_id: Option<&str>,
    ) -> Result<(), AccountsError> {
        if let Some(id) = account_id {
            if self.find(id).is_none() {
                return Err(AccountsError::SelectedDangling {
                    account_id: id.to_owned(),
                });
            }
        }
        self.selected_account_id = account_id.map(str::to_owned);
        Ok(())
    }

    /// Parses and validates an accounts document from JSON text.
    pub fn from_json(json: &str) -> Result<Self, AccountsError> {
        let document: Self = serde_json::from_str(json)
            .map_err(|error| AccountsError::Malformed(error.to_string()))?;

        if document.schema_version != Self::SCHEMA_VERSION {
            return Err(AccountsError::UnsupportedSchema {
                found: document.schema_version,
                supported: Self::SCHEMA_VERSION,
            });
        }

        let mut seen = std::collections::HashSet::new();
        for record in &document.accounts {
            AccountId::validate(&record.account_id).map_err(|_| {
                AccountsError::Malformed(format!(
                    "account identifier '{}' is malformed",
                    record.account_id
                ))
            })?;
            validate_minecraft_name(&record.minecraft_name).map_err(|_| {
                AccountsError::Malformed(format!(
                    "Minecraft name for account '{}' is malformed",
                    record.account_id
                ))
            })?;
            if !seen.insert(record.account_id.clone()) {
                return Err(AccountsError::Duplicate {
                    account_id: record.account_id.clone(),
                });
            }
        }

        if let Some(selected) = document.selected_account_id.as_deref() {
            if document.find(selected).is_none() {
                return Err(AccountsError::SelectedDangling {
                    account_id: selected.to_owned(),
                });
            }
        }

        Ok(document)
    }

    /// Serializes as pretty, human-inspectable JSON (containing no secrets
    /// by construction: the type has no secret-bearing fields).
    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self)
            .expect("accounts document serialization cannot fail");
        json.push('\n');
        json
    }
}

/// Loads the accounts document. A missing file is an empty document, exactly
/// like the instance registry; malformed files are errors and are never
/// overwritten.
pub fn load(path: &Path) -> Result<AccountsDocument, AccountsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AccountsDocument::default());
        }
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Err(AccountsError::Malformed(
                "the accounts document is not valid UTF-8 and may be corrupted".to_owned(),
            ));
        }
        Err(error) => return Err(AccountsError::Read(error)),
    };

    AccountsDocument::from_json(&text)
}

/// Persists the accounts document atomically: write to a sibling temporary
/// file, then rename over the target. Duplicate identifiers and dangling
/// selections are rejected before the filesystem is touched.
pub fn save(path: &Path, document: &AccountsDocument) -> Result<(), AccountsError> {
    // Re-validate the exact invariants `from_json` enforces so a damaged
    // document can never be written by this code path either.
    AccountsDocument::from_json(&document.to_json())?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(AccountsError::Write)?;
    }

    let temporary_path = temporary_sibling(path);
    std::fs::write(&temporary_path, document.to_json()).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        AccountsError::Write(error)
    })?;

    match std::fs::rename(&temporary_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_path);
            Err(AccountsError::Write(error))
        }
    }
}

fn temporary_sibling(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    PathBuf::from(temporary)
}

#[derive(Debug)]
pub enum AccountsError {
    Read(std::io::Error),
    Write(std::io::Error),
    Malformed(String),
    UnsupportedSchema { found: u32, supported: u32 },
    Duplicate { account_id: String },
    SelectedDangling { account_id: String },
}

impl fmt::Display for AccountsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(
                formatter,
                "the accounts document could not be read: {error}"
            ),
            Self::Write(error) => {
                write!(
                    formatter,
                    "the accounts document could not be written: {error}"
                )
            }
            Self::Malformed(detail) => write!(
                formatter,
                "the accounts document is malformed and must be corrected manually: {detail}"
            ),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "the accounts document uses schema version {found}, which is not supported; this launcher supports version {supported}"
            ),
            Self::Duplicate { account_id } => write!(
                formatter,
                "the accounts document would contain a duplicate account '{account_id}'"
            ),
            Self::SelectedDangling { account_id } => write!(
                formatter,
                "the selected account '{account_id}' does not exist in the accounts document; sign in again or select an existing account to repair the selection"
            ),
        }
    }
}

impl std::error::Error for AccountsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) | Self::Write(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_account_id() -> &'static str {
        "986dec87b7ec47ff89ff033fdb95c4b5"
    }

    fn test_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir()
            .join(name)
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn account_identifiers_accept_only_undashed_lowercase_hex() {
        assert!(AccountId::validate(valid_account_id()).is_ok());

        for bad in [
            "",
            "986DEC87B7EC47FF89FF033FDB95C4B5",
            "986dec87b7ec47ff89ff033fdb95c4b",
            "986dec87-b7ec-47ff-89ff-033fdb95c4b5",
            "../escape",
            "986dec87b7ec47ff89ff033fdb95c4b\u{7f}5",
        ] {
            assert!(AccountId::validate(bad).is_err(), "accepted: {bad:?}");
        }
    }

    #[test]
    fn minecraft_names_are_validated() {
        for good in ["Steve", "how_auth_works", "Player123", "abc"] {
            assert!(validate_minecraft_name(good).is_ok(), "rejected: {good}");
        }
        for bad in ["ab", "a".repeat(17).as_str(), "two words", "dash-name", ""] {
            assert!(validate_minecraft_name(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn json_round_trip_preserves_accounts_and_selection() {
        let document = AccountsDocument {
            schema_version: ACCOUNTS_SCHEMA_VERSION,
            selected_account_id: Some(valid_account_id().to_owned()),
            accounts: vec![AccountRecord::new(valid_account_id(), "HowDoesAuthWork")],
        };

        let parsed = AccountsDocument::from_json(&document.to_json()).unwrap();

        assert_eq!(parsed, document);
    }

    #[test]
    fn serializes_to_inspectable_camel_case_json_without_secrets() {
        let document = AccountsDocument {
            schema_version: ACCOUNTS_SCHEMA_VERSION,
            selected_account_id: None,
            accounts: vec![AccountRecord::new(valid_account_id(), "HowDoesAuthWork")],
        };

        let json = document.to_json();

        assert!(json.contains("\"schemaVersion\": 1"));
        assert!(json.contains("\"selectedAccountId\": null"));
        assert!(json.contains("\"accountId\": \"986dec87b7ec47ff89ff033fdb95c4b5\""));
        assert!(json.contains("\"minecraftName\": \"HowDoesAuthWork\""));
        // The document type cannot carry tokens; prove the serialized shape
        // contains no token-like field names at all.
        assert!(!json.contains("token"));
        assert!(!json.contains("refresh"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn malformed_and_unsupported_documents_fail_deliberately() {
        assert!(matches!(
            AccountsDocument::from_json("{ not json"),
            Err(AccountsError::Malformed(_))
        ));
        assert!(matches!(
            AccountsDocument::from_json("{}"),
            Err(AccountsError::Malformed(_))
        ));
        assert!(matches!(
            AccountsDocument::from_json(
                &AccountsDocument {
                    schema_version: 2,
                    selected_account_id: None,
                    accounts: Vec::new(),
                }
                .to_json()
            ),
            Err(AccountsError::UnsupportedSchema {
                found: 2,
                supported: 1
            })
        ));
    }

    #[test]
    fn duplicate_accounts_and_dangling_selections_are_rejected() {
        let duplicate = AccountsDocument {
            schema_version: ACCOUNTS_SCHEMA_VERSION,
            selected_account_id: None,
            accounts: vec![
                AccountRecord::new(valid_account_id(), "Name1"),
                AccountRecord::new(valid_account_id(), "Name2"),
            ],
        }
        .to_json();
        assert!(matches!(
            AccountsDocument::from_json(&duplicate),
            Err(AccountsError::Duplicate { .. })
        ));

        let dangling = r#"{
            "schemaVersion": 1,
            "selectedAccountId": "ffffffffffffffffffffffffffffffff",
            "accounts": []
        }"#;
        assert!(matches!(
            AccountsDocument::from_json(dangling),
            Err(AccountsError::SelectedDangling { .. })
        ));
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_document() {
        let directory = test_directory("aurora-accounts-test-missing");
        let path = directory.join("accounts.json");

        assert_eq!(load(&path).unwrap(), AccountsDocument::default());
        assert!(!path.exists());
    }

    #[test]
    fn malformed_files_are_never_overwritten() {
        let directory = test_directory("aurora-accounts-test-malformed");
        let path = directory.join("accounts.json");
        let damaged = "{ this is not json";
        std::fs::write(&path, damaged).unwrap();

        assert!(matches!(load(&path), Err(AccountsError::Malformed(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), damaged);
    }

    #[test]
    fn save_is_atomic_and_rejects_invalid_documents() {
        let directory = test_directory("aurora-accounts-test-save");
        let path = directory.join("launcher").join("accounts.json");

        let document = AccountsDocument {
            schema_version: ACCOUNTS_SCHEMA_VERSION,
            selected_account_id: None,
            accounts: vec![AccountRecord::new(valid_account_id(), "HowDoesAuthWork")],
        };
        save(&path, &document).unwrap();
        assert_eq!(load(&path).unwrap(), document);

        // A duplicate can never be written.
        let invalid = AccountsDocument {
            schema_version: ACCOUNTS_SCHEMA_VERSION,
            selected_account_id: None,
            accounts: vec![
                AccountRecord::new(valid_account_id(), "A"),
                AccountRecord::new(valid_account_id(), "B"),
            ],
        };
        assert!(save(&path, &invalid).is_err());
        assert_eq!(load(&path).unwrap(), document);

        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["accounts.json".to_owned()]);
    }
}
