//! Microsoft → Xbox → Minecraft authentication for the launcher.
//!
//! The domain is organized like the other launcher domains:
//!
//! - `oauth`: pure public-client OAuth building blocks (PKCE, state,
//!   authorization request, callback parsing);
//! - `metadata`: the pinned external endpoints, wire DTOs, and the bounded
//!   exchange boundary for Microsoft, Xbox Live, XSTS, and Minecraft
//!   Services;
//! - `credentials`: the OS-backed credential store holding the only
//!   persisted secret (the Microsoft refresh credential);
//! - `accounts`: the versioned, non-secret account summaries and selection;
//! - `session`: the in-memory Minecraft session consumed by launch assembly;
//! - `callback`: the local-only loopback redirect receiver;
//! - `flow`: the orchestration — sign-in, session restoration, sign-out —
//!   and the process-local login transaction guard.
//!
//! Security posture: Aurora is a desktop public client. No client secret
//! exists, passwords are never handled, the system browser performs user
//! interaction, tokens never appear in logs/events/DTOs/ordinary JSON, and
//! the only persisted secret is the refresh credential in OS-backed storage.

pub mod accounts;
pub mod callback;
pub mod credentials;
pub mod flow;
pub mod metadata;
pub mod oauth;
pub mod session;

use std::fmt;

pub use accounts::{AccountRecord, AccountsDocument, AccountsError};
pub use credentials::{CredentialStore, CredentialStoreError, OsCredentialStore};
pub use metadata::AuthEndpoints;
pub use oauth::OAuthClientConfig;
pub use session::{MinecraftProfile, MinecraftSession};

use flow::BrowserOpenError;
use metadata::{
    AuthTransportError, EntitlementError, MinecraftExchangeError, TokenExchangeError,
    XboxExchangeError, XstsExchangeError,
};

/// Stable authentication failures, mapped one-to-one onto command error
/// codes at the application boundary.
///
/// No variant carries a token or a raw provider body: diagnostics name the
/// failing step and safe, bounded provider detail only.
#[derive(Debug)]
pub enum AuthError {
    /// No production Microsoft application registration is configured;
    /// live sign-in cannot start.
    ConfigurationMissing,
    /// Another login transaction is active in this process.
    LoginInProgress,
    LoginCancelled,
    LoginTimeout,
    /// OS entropy was unavailable for login material.
    EntropyFailure(oauth::OAuthEntropyError),
    /// The local loopback receiver could not bind or died.
    CallbackListener(String),
    /// The system browser could not be opened.
    BrowserOpen(BrowserOpenError),
    /// The redirect carried neither a code nor an error.
    CallbackInvalid,
    /// The provider redirected with an OAuth error.
    OAuthFailure {
        reason: String,
    },
    /// Microsoft token exchange failed (transport, rejection, or shape).
    TokenExchange(TokenExchangeError),
    Xbox(XboxExchangeError),
    Xsts(XstsExchangeError),
    /// The XSTS exchange was denied for a known, evidence-based account
    /// reason (or an unknown code, kept generic). Same stable code as
    /// [`AuthError::Xsts`]; this variant carries the user-understandable
    /// explanation the flow mapped from the numeric `XErr`.
    XstsDenied {
        xerr: i64,
        reason: String,
    },
    MinecraftServices(MinecraftExchangeError),
    /// The transport behind the entitlement/profile checks failed.
    MinecraftServicesTransport(AuthTransportError),
    EntitlementCheck(EntitlementError),
    /// Authenticated, but the account does not own Minecraft.
    EntitlementMissing,
    /// Authenticated and entitled, but no Minecraft profile exists yet.
    ProfileMissing,
    /// The profile response carried an invalid UUID or name.
    ProfileInvalid,
    /// Session restoration is impossible: the stored credential is gone,
    /// revoked, or no longer belongs to this account. The user must sign in
    /// again.
    ReauthenticationRequired {
        reason: String,
    },
    CredentialStore(CredentialStoreError),
    Accounts(AccountsError),
    AccountNotFound {
        account_id: String,
    },
    InvalidAccountId(accounts::InvalidAccount),
}

impl AuthError {
    /// The stable machine code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ConfigurationMissing => "auth_configuration_missing",
            Self::LoginInProgress => "auth_login_in_progress",
            Self::LoginCancelled => "auth_login_cancelled",
            Self::LoginTimeout => "auth_login_timeout",
            Self::EntropyFailure(_) => "auth_entropy_failure",
            Self::CallbackListener(_) => "auth_callback_listener_failure",
            Self::BrowserOpen(_) => "auth_browser_open_failure",
            Self::CallbackInvalid => "auth_callback_invalid",
            Self::OAuthFailure { .. } => "auth_oauth_failure",
            Self::TokenExchange(_) => "auth_token_exchange_failure",
            Self::Xbox(_) => "auth_xbox_failure",
            Self::Xsts(_) | Self::XstsDenied { .. } => "auth_xsts_failure",
            Self::MinecraftServices(_) | Self::MinecraftServicesTransport(_) => {
                "auth_minecraft_services_failure"
            }
            Self::EntitlementCheck(_) => "auth_minecraft_services_failure",
            Self::EntitlementMissing => "auth_entitlement_missing",
            Self::ProfileMissing => "auth_profile_missing",
            Self::ProfileInvalid => "auth_profile_invalid",
            Self::ReauthenticationRequired { .. } => "auth_reauthentication_required",
            Self::CredentialStore(_) => "auth_credential_store_failure",
            Self::Accounts(error) => match error {
                AccountsError::Malformed(_) => "accounts_invalid",
                AccountsError::UnsupportedSchema { .. } => "accounts_unsupported_schema",
                AccountsError::Read(_) => "storage_io_failure",
                AccountsError::Write(_) => "accounts_write_failure",
                AccountsError::Duplicate { .. } => "accounts_invalid",
                AccountsError::SelectedDangling { .. } => "accounts_selected_dangling",
            },
            Self::AccountNotFound { .. } => "auth_account_not_found",
            Self::InvalidAccountId(_) => "auth_account_invalid",
        }
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigurationMissing => write!(
                formatter,
                "Aurora has no Microsoft application registration configured, so Microsoft sign-in cannot start; a real client ID must be provided by the build configuration"
            ),
            Self::LoginInProgress => write!(
                formatter,
                "a Microsoft sign-in is already in progress; finish or cancel it before starting another"
            ),
            Self::LoginCancelled => write!(
                formatter,
                "the Microsoft sign-in was cancelled and its temporary state was discarded"
            ),
            Self::LoginTimeout => write!(
                formatter,
                "the Microsoft sign-in timed out waiting for the browser and its temporary state was discarded"
            ),
            Self::EntropyFailure(error) => write!(formatter, "{error}"),
            Self::CallbackListener(reason) => write!(
                formatter,
                "the local receiver for the Microsoft sign-in callback failed: {reason}"
            ),
            Self::BrowserOpen(error) => write!(
                formatter,
                "the system browser could not be opened for sign-in: {error}"
            ),
            Self::CallbackInvalid => write!(
                formatter,
                "Microsoft's sign-in redirect was malformed: it carried neither an authorization code nor an error"
            ),
            Self::OAuthFailure { reason } => {
                write!(formatter, "Microsoft reported the sign-in error '{reason}'")
            }
            Self::TokenExchange(error) => write!(formatter, "{error}"),
            Self::Xbox(error) => write!(formatter, "{error}"),
            Self::Xsts(error) => write!(formatter, "{error}"),
            Self::XstsDenied { reason, .. } => write!(formatter, "{reason}"),
            Self::MinecraftServices(error) => write!(formatter, "{error}"),
            Self::MinecraftServicesTransport(error) => write!(formatter, "{error}"),
            Self::EntitlementCheck(error) => write!(formatter, "{error}"),
            Self::EntitlementMissing => write!(
                formatter,
                "this Microsoft account is authenticated but does not own Minecraft: Java Edition, so it cannot be used"
            ),
            Self::ProfileMissing => write!(
                formatter,
                "this account owns Minecraft but has no Minecraft profile yet; create a profile in the official Minecraft launcher first"
            ),
            Self::ProfileInvalid => write!(
                formatter,
                "the Minecraft profile returned an invalid identity and cannot be used"
            ),
            Self::ReauthenticationRequired { reason } => {
                write!(formatter, "this account must sign in again: {reason}")
            }
            Self::CredentialStore(error) => write!(formatter, "{error}"),
            Self::Accounts(error) => write!(formatter, "{error}"),
            Self::AccountNotFound { account_id } => write!(
                formatter,
                "the account '{account_id}' is not known to this launcher"
            ),
            Self::InvalidAccountId(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<oauth::OAuthEntropyError> for AuthError {
    fn from(error: oauth::OAuthEntropyError) -> Self {
        Self::EntropyFailure(error)
    }
}

impl From<TokenExchangeError> for AuthError {
    fn from(error: TokenExchangeError) -> Self {
        Self::TokenExchange(error)
    }
}

impl From<XboxExchangeError> for AuthError {
    fn from(error: XboxExchangeError) -> Self {
        Self::Xbox(error)
    }
}

impl From<XstsExchangeError> for AuthError {
    fn from(error: XstsExchangeError) -> Self {
        Self::Xsts(error)
    }
}

impl From<MinecraftExchangeError> for AuthError {
    fn from(error: MinecraftExchangeError) -> Self {
        Self::MinecraftServices(error)
    }
}

impl From<EntitlementError> for AuthError {
    fn from(error: EntitlementError) -> Self {
        Self::EntitlementCheck(error)
    }
}

impl From<CredentialStoreError> for AuthError {
    fn from(error: CredentialStoreError) -> Self {
        Self::CredentialStore(error)
    }
}

impl From<AccountsError> for AuthError {
    fn from(error: AccountsError) -> Self {
        Self::Accounts(error)
    }
}

impl From<accounts::InvalidAccount> for AuthError {
    fn from(error: accounts::InvalidAccount) -> Self {
        Self::InvalidAccountId(error)
    }
}

impl From<metadata::ProfileError> for AuthError {
    fn from(error: metadata::ProfileError) -> Self {
        match error {
            metadata::ProfileError::Malformed => Self::ProfileInvalid,
            metadata::ProfileError::Transport(transport) => {
                Self::MinecraftServicesTransport(transport)
            }
        }
    }
}

impl From<callback::CallbackReceiverError> for AuthError {
    fn from(error: callback::CallbackReceiverError) -> Self {
        match error {
            callback::CallbackReceiverError::ProviderError { error, description } => {
                Self::OAuthFailure {
                    reason: match description {
                        Some(detail) => format!("{error}: {detail}"),
                        None => error,
                    },
                }
            }
            callback::CallbackReceiverError::Invalid => Self::CallbackInvalid,
            callback::CallbackReceiverError::Listener(reason) => Self::CallbackListener(reason),
        }
    }
}

/// Maps an XSTS denial's numeric code to an evidence-based, stable,
/// user-understandable explanation. Unknown codes deliberately stay generic
/// — an account condition is never guessed from an unrecognized code.
pub(crate) fn xsts_denial_reason(xerr: i64) -> String {
    match xerr {
        2148916227 => "the account is banned from Xbox".to_owned(),
        2148916233 => "this Microsoft account has no Xbox profile; one is required for Minecraft sign-in and can be created on minecraft.net".to_owned(),
        2148916235 => "Xbox Live is not available in the account's country or region".to_owned(),
        2148916236 | 2148916237 => "the account requires adult verification (South Korea) before it can sign in to Xbox".to_owned(),
        2148916238 => "the account belongs to a child who must be added to a Microsoft family group by an adult before signing in to Xbox".to_owned(),
        other => format!("Xbox authorization was denied with code {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xsts_denial_reasons_are_evidence_based_and_unknown_codes_stay_generic() {
        assert!(xsts_denial_reason(2148916233).contains("no Xbox profile"));
        assert!(xsts_denial_reason(2148916238).contains("family group"));
        assert!(xsts_denial_reason(2148916236).contains("adult verification"));

        let unknown = xsts_denial_reason(123456789);
        assert_eq!(unknown, "Xbox authorization was denied with code 123456789");
        assert!(!unknown.contains("child"));
        assert!(!unknown.contains("banned"));
    }

    #[test]
    fn provider_redirect_errors_map_to_oauth_failures() {
        let error = AuthError::from(callback::CallbackReceiverError::ProviderError {
            error: "access_denied".to_owned(),
            description: Some("the user declined".to_owned()),
        });

        assert_eq!(error.code(), "auth_oauth_failure");
        assert!(error.to_string().contains("access_denied"));
    }
}
