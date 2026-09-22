//! Authentication orchestration: sign-in, session restoration, sign-out.
//!
//! One function per user-visible operation, each consuming the injectable
//! [`AuthContext`] (endpoints, OAuth registration, credential store, accounts
//! document path, and the system-browser opener). Deterministic tests inject
//! a loopback endpoint set, an in-memory credential store, and a synthetic
//! "browser" that drives the real loopback receiver.

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;

use super::AuthError;
use super::accounts::{self, AccountRecord};
use super::callback::CallbackReceiver;
use super::credentials::{CredentialStore, PersistedCredential, SecretString};
use super::metadata::{
    self, AuthEndpoints, MinecraftProfileDocument, TokenGrant, XstsExchangeError,
};
use super::oauth::{OAuthClientConfig, OAuthState, PkcePair, build_authorization_url};
use super::session::{MinecraftProfile, MinecraftSession, SessionCache};

/// How long one sign-in may wait for the browser before failing with a
/// timeout (and discarding all transient login state).
const DEFAULT_LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

/// Coarse progress phases surfaced to the UI. Never carry data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthPhase {
    WaitingForMicrosoft,
    ExchangingMicrosoftToken,
    AuthenticatingWithXbox,
    AuthorizingXsts,
    AuthenticatingWithMinecraft,
    CheckingEntitlement,
    FetchingProfile,
    SavingAccount,
    RestoringSession,
}

impl AuthPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WaitingForMicrosoft => "waitingForMicrosoft",
            Self::ExchangingMicrosoftToken => "exchangingMicrosoftToken",
            Self::AuthenticatingWithXbox => "authenticatingWithXbox",
            Self::AuthorizingXsts => "authorizingXsts",
            Self::AuthenticatingWithMinecraft => "authenticatingMinecraft",
            Self::CheckingEntitlement => "checkingEntitlement",
            Self::FetchingProfile => "fetchingProfile",
            Self::SavingAccount => "savingAccount",
            Self::RestoringSession => "restoringSession",
        }
    }
}

/// Tunable bounds of one authentication flow.
#[derive(Debug, Clone)]
pub struct AuthFlowOptions {
    pub login_timeout: Duration,
}

impl Default for AuthFlowOptions {
    fn default() -> Self {
        Self {
            login_timeout: DEFAULT_LOGIN_TIMEOUT,
        }
    }
}

/// The system browser could not be opened.
#[derive(Debug, Clone)]
pub struct BrowserOpenError {
    detail: String,
}

impl BrowserOpenError {
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for BrowserOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.detail)
    }
}

impl std::error::Error for BrowserOpenError {}

/// Everything one authentication operation needs, injected by the caller.
pub struct AuthContext<'a> {
    pub endpoints: &'a AuthEndpoints,
    pub oauth: OAuthClientConfig,
    pub credentials: &'a dyn CredentialStore,
    pub accounts_path: &'a Path,
    pub options: AuthFlowOptions,
    /// Opens the authorization URL in the system browser. The real opener
    /// uses the platform default browser; tests substitute a scripted one.
    pub browser: &'a (dyn Fn(&str) -> Result<(), BrowserOpenError> + Send + Sync),
}

/// Aurora's production Microsoft application registration, if the build was
/// given one.
///
/// A client ID is public configuration supplied by the release process via
/// the `AURORA_MICROSOFT_CLIENT_ID` build-environment variable. No
/// registration exists yet, so production sign-in fails deliberately with
/// `auth_configuration_missing` rather than borrowing or inventing an ID.
/// The registration must also be approved for Minecraft Services (Microsoft
/// requires new applications to request access) before live sign-in can
/// succeed end to end.
pub fn production_registration() -> Option<OAuthClientConfig> {
    option_env!("AURORA_MICROSOFT_CLIENT_ID").map(OAuthClientConfig::new)
}

// ---------------------------------------------------------------------------
// Process-local login transaction
// ---------------------------------------------------------------------------

fn login_slot() -> MutexGuard<'static, Option<Arc<Notify>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<Notify>>>> = OnceLock::new();
    let slot = SLOT.get_or_init(|| Mutex::new(None));
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The process-local login transaction guard: at most one active sign-in,
/// cancellable from outside. Cross-process coordination stays deferred.
struct LoginTransaction {
    cancel: Arc<Notify>,
}

impl LoginTransaction {
    fn acquire() -> Result<Self, AuthError> {
        let mut slot = login_slot();
        if slot.is_some() {
            return Err(AuthError::LoginInProgress);
        }
        let cancel = Arc::new(Notify::new());
        *slot = Some(Arc::clone(&cancel));
        Ok(Self { cancel })
    }

    async fn cancelled(&self) {
        self.cancel.notified().await
    }
}

impl Drop for LoginTransaction {
    fn drop(&mut self) {
        let mut slot = login_slot();
        if slot
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &self.cancel))
        {
            *slot = None;
        }
    }
}

/// Cancels the active login transaction, if one exists. Returns whether a
/// login was active. The cancelled transaction's receiver is closed and its
/// PKCE/state are discarded; a late browser callback cannot resurrect them.
pub fn cancel_active_login() -> bool {
    let slot = login_slot();
    match slot.as_ref() {
        Some(notify) => {
            notify.notify_one();
            true
        }
        None => false,
    }
}

/// Serializes accounts-document read-modify-write windows (matching the
/// instance registry policy); long network work runs outside it.
fn accounts_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------

/// Runs the complete sign-in: browser authorization on the system browser,
/// loopback callback, token exchange, the Xbox → XSTS → Minecraft chain,
/// entitlement and profile validation, then persistence of the non-secret
/// account record and the refresh credential.
pub async fn sign_in(
    context: &AuthContext<'_>,
    progress: &mut (dyn FnMut(AuthPhase) + Send),
) -> Result<AccountRecord, AuthError> {
    let transaction = LoginTransaction::acquire()?;

    let pkce = PkcePair::generate()?;
    let state = OAuthState::generate()?;
    let receiver = CallbackReceiver::bind()
        .await
        .map_err(|error| AuthError::CallbackListener(error.to_string()))?;
    let redirect_uri = receiver.redirect_uri();

    let authorize_url = build_authorization_url(
        context.endpoints.authorize(),
        &context.oauth,
        &redirect_uri,
        &state,
        &pkce,
    );
    (context.browser)(authorize_url.as_str()).map_err(AuthError::BrowserOpen)?;

    progress(AuthPhase::WaitingForMicrosoft);
    let code = tokio::select! {
        result = receiver.wait_for_callback(state.as_str()) => match result {
            Ok(code) => code,
            Err(error) => return Err(error.into()),
        },
        _ = transaction.cancelled() => return Err(AuthError::LoginCancelled),
        _ = tokio::time::sleep(context.options.login_timeout) => {
            return Err(AuthError::LoginTimeout);
        }
    };

    progress(AuthPhase::ExchangingMicrosoftToken);
    let tokens = metadata::request_microsoft_token(
        context.endpoints,
        context.oauth.client_id(),
        TokenGrant::AuthorizationCode {
            code: code.as_str(),
            code_verifier: pkce.verifier(),
            redirect_uri: redirect_uri.as_str(),
        },
    )
    .await?;

    let session = downstream_chain(context, &tokens.access_token, progress).await?;

    progress(AuthPhase::SavingAccount);
    let record = commit_account(context, &session, &tokens.refresh_token)?;

    Ok(record)
}

// ---------------------------------------------------------------------------
// Session restoration
// ---------------------------------------------------------------------------

/// Returns a usable in-memory session for one account, restoring it from the
/// persisted refresh credential when the cached session is absent or expired.
///
/// The Microsoft refresh token is rotated on every use (the identity platform
/// returns a replacement on each redemption), so the newest token is
/// persisted immediately after a successful refresh — before the downstream
/// chain runs — keeping the stored credential valid even if Xbox or Minecraft
/// Services fail afterward.
pub async fn ensure_session(
    context: &AuthContext<'_>,
    account_id: &str,
    progress: &mut (dyn FnMut(AuthPhase) + Send),
) -> Result<Arc<MinecraftSession>, AuthError> {
    accounts::AccountId::validate(account_id)?;

    if let Some(cached) = SessionCache::usable(account_id) {
        return Ok(cached);
    }

    progress(AuthPhase::RestoringSession);
    let document = accounts::load(context.accounts_path)?;
    if document.find(account_id).is_none() {
        return Err(AuthError::AccountNotFound {
            account_id: account_id.to_owned(),
        });
    }

    let credential = context.credentials.load(account_id)?;
    let Some(credential) = credential else {
        return Err(AuthError::ReauthenticationRequired {
            reason: "no stored Microsoft credential remains".to_owned(),
        });
    };
    if credential.client_id() != context.oauth.client_id() {
        return Err(AuthError::ReauthenticationRequired {
            reason: "the stored credential belongs to a different application registration"
                .to_owned(),
        });
    }

    progress(AuthPhase::ExchangingMicrosoftToken);
    let tokens = match metadata::request_microsoft_token(
        context.endpoints,
        context.oauth.client_id(),
        TokenGrant::RefreshToken {
            refresh_token: credential.refresh_token(),
        },
    )
    .await
    {
        Ok(tokens) => tokens,
        Err(metadata::TokenExchangeError::Rejected(rejection))
            if rejection.error == "invalid_grant" =>
        {
            // The credential is provably dead (revoked or expired beyond
            // redemption). Remove it; the account record remains so the
            // user sees reauthentication-required rather than a vanished
            // account.
            context.credentials.delete(account_id)?;
            return Err(AuthError::ReauthenticationRequired {
                reason: "the stored Microsoft credential is no longer valid".to_owned(),
            });
        }
        Err(other) => return Err(other.into()),
    };

    // Persist the rotated refresh credential immediately.
    context.credentials.save(
        account_id,
        &PersistedCredential::new(context.oauth.client_id(), tokens.refresh_token.clone()),
    )?;

    let session = downstream_chain(context, &tokens.access_token, progress).await?;

    if session.profile().uuid() != account_id {
        // The credential now resolves to a different Minecraft account —
        // inconsistent state; remove the credential and demand a fresh sign-in.
        context.credentials.delete(account_id)?;
        return Err(AuthError::ReauthenticationRequired {
            reason: "the stored credential now resolves to a different Minecraft account"
                .to_owned(),
        });
    }

    update_record_name_if_changed(context.accounts_path, account_id, session.profile().name())?;

    Ok(SessionCache::put(session))
}

// ---------------------------------------------------------------------------
// Sign-out
// ---------------------------------------------------------------------------

/// Removes one account from Aurora: deletes the persisted credential,
/// removes the non-secret account record, clears the cached session, and
/// clears the selection when this account was selected.
///
/// This removes Aurora's local access. It does not claim to revoke the
/// Microsoft account session globally.
pub fn sign_out(context: &AuthContext<'_>, account_id: &str) -> Result<(), AuthError> {
    accounts::AccountId::validate(account_id)?;

    let _guard = accounts_lock();
    let mut document = accounts::load(context.accounts_path)?;
    if document.find(account_id).is_none() {
        return Err(AuthError::AccountNotFound {
            account_id: account_id.to_owned(),
        });
    }

    let existed = context.credentials.delete(account_id)?;
    if !existed {
        eprintln!("[aurora-launcher] sign-out for '{account_id}': no stored credential remained");
    }

    document
        .accounts_mut()
        .retain(|record| record.account_id() != account_id);
    if document.selected_account_id() == Some(account_id) {
        document.set_selected_account_id(None)?;
    }
    accounts::save(context.accounts_path, &document)?;

    SessionCache::remove(account_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared internals
// ---------------------------------------------------------------------------

/// The downstream chain from a fresh Microsoft access token to a usable
/// Minecraft session: Xbox Live authentication, XSTS authorization for the
/// Minecraft relying party, Minecraft Services authentication, entitlement
/// validation, and profile retrieval.
async fn downstream_chain(
    context: &AuthContext<'_>,
    microsoft_access_token: &SecretString,
    progress: &mut (dyn FnMut(AuthPhase) + Send),
) -> Result<MinecraftSession, AuthError> {
    progress(AuthPhase::AuthenticatingWithXbox);
    let xbox = metadata::authenticate_xbox(context.endpoints, microsoft_access_token).await?;

    progress(AuthPhase::AuthorizingXsts);
    let xsts = match metadata::authorize_xsts(context.endpoints, &xbox).await {
        Ok(xsts) => xsts,
        Err(XstsExchangeError::Denied { xerr }) => {
            return Err(AuthError::XstsDenied {
                xerr,
                reason: super::xsts_denial_reason(xerr),
            });
        }
        Err(other) => return Err(AuthError::Xsts(other)),
    };

    progress(AuthPhase::AuthenticatingWithMinecraft);
    let minecraft =
        metadata::login_with_minecraft(context.endpoints, &xsts.user_hash, &xsts.token).await?;

    progress(AuthPhase::CheckingEntitlement);
    let items =
        metadata::fetch_entitlement_items(context.endpoints, &minecraft.access_token).await?;
    if !metadata::owns_minecraft(&items) {
        return Err(AuthError::EntitlementMissing);
    }

    progress(AuthPhase::FetchingProfile);
    let profile = match metadata::fetch_profile(context.endpoints, &minecraft.access_token).await? {
        Some(profile) => profile,
        None => return Err(AuthError::ProfileMissing),
    };

    Ok(assemble_session(minecraft, profile))
}

fn assemble_session(
    minecraft: metadata::MinecraftTokenSet,
    profile: MinecraftProfileDocument,
) -> MinecraftSession {
    MinecraftSession::new(
        profile.id.clone(),
        MinecraftProfile::new(profile.id, profile.name),
        minecraft.access_token,
        Duration::from_secs(minecraft.expires_in_seconds),
    )
}

/// Persists the non-secret account record (creating or merging by account
/// identity) and then the refresh credential. Record-before-credential is
/// deliberate: if the credential save fails, the account exists in the
/// explicit reauthentication-required state and the next sign-in heals it —
/// the reverse order could leave a credential nothing addresses.
///
/// A first successfully added account becomes selected automatically; later
/// sign-ins never change an existing selection.
fn commit_account(
    context: &AuthContext<'_>,
    session: &MinecraftSession,
    refresh_token: &SecretString,
) -> Result<AccountRecord, AuthError> {
    let account_id = session.account_id();

    let _guard = accounts_lock();
    let mut document = accounts::load(context.accounts_path)?;

    let record = match document
        .accounts_mut()
        .iter_mut()
        .find(|record| record.account_id() == account_id)
    {
        Some(existing) => {
            existing.set_minecraft_name(session.profile().name().to_owned());
            existing.clone()
        }
        None => {
            let record = AccountRecord::new(account_id, session.profile().name());
            document.accounts_mut().push(record.clone());
            record
        }
    };

    if document.selected_account_id().is_none() {
        document.set_selected_account_id(Some(account_id))?;
    }

    accounts::save(context.accounts_path, &document)?;
    context.credentials.save(
        account_id,
        &PersistedCredential::new(context.oauth.client_id(), refresh_token.clone()),
    )?;

    SessionCache::put(session.clone());

    Ok(record)
}

/// Updates a renamed account's display name in the persisted record
/// (metadata only — identifiers never change).
fn update_record_name_if_changed(
    accounts_path: &Path,
    account_id: &str,
    name: &str,
) -> Result<(), AuthError> {
    let _guard = accounts_lock();
    let mut document = accounts::load(accounts_path)?;

    if let Some(record) = document
        .accounts_mut()
        .iter_mut()
        .find(|record| record.account_id() == account_id)
    {
        if record.minecraft_name() != name {
            record.set_minecraft_name(name.to_owned());
            accounts::save(accounts_path, &document)?;
        }
    }

    Ok(())
}

/// The honest per-account status for account listing: a present credential
/// means signed in; anything else means reauthentication is required.
pub enum AccountStatus {
    SignedIn,
    ReauthenticationRequired,
}

impl AccountStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SignedIn => "signedIn",
            Self::ReauthenticationRequired => "reauthenticationRequired",
        }
    }

    /// Derives the status by checking credential presence. A failing store
    /// is reported conservatively as reauthentication-required; the
    /// refresh-operation surfaces store errors honestly.
    pub fn of(credentials: &dyn CredentialStore, account_id: &str) -> Self {
        match credentials.load(account_id) {
            Ok(Some(_)) => Self::SignedIn,
            _ => Self::ReauthenticationRequired,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestResponse, TestServer};
    use base64ct::Encoding as _;
    use sha2::Digest as _;
    use std::net::TcpStream;
    use std::path::PathBuf;

    const ACCOUNT_A: &str = "986dec87b7ec47ff89ff033fdb95c4b5";
    const ACCOUNT_B: &str = "0b8e2c9b605f45d19f5b3f001ee66920";
    const AUTH_CODE: &str = "FIXTURE-AUTH-CODE";

    /// What the scripted "browser" does with the authorization URL.
    #[derive(Clone, Copy)]
    enum BrowserScript {
        /// Completes the redirect with the correct state.
        Complete,
        /// Redirects with a different (stale) state.
        StaleState,
        /// Redirects with a provider error.
        ProviderError { error: &'static str },
        /// Never redirects.
        Nothing,
        /// Fails to open at all.
        FailToOpen,
    }

    /// The scripted synthetic authentication world served on loopback.
    struct World {
        profile_id: String,
        profile_name: String,
        mc_expires_in: u64,
        entitled: bool,
        profile_present: bool,
        xerr: Option<i64>,
        reject_refresh: bool,
        refresh_count: usize,
        token_request_bodies: Vec<String>,
        xbox_request_bodies: Vec<String>,
        xsts_request_bodies: Vec<String>,
        mc_request_bodies: Vec<String>,
    }

    impl World {
        fn new() -> Self {
            Self {
                profile_id: ACCOUNT_A.to_owned(),
                profile_name: "HowDoesAuthWork".to_owned(),
                mc_expires_in: 86400,
                entitled: true,
                profile_present: true,
                xerr: None,
                reject_refresh: false,
                refresh_count: 0,
                token_request_bodies: Vec::new(),
                xbox_request_bodies: Vec::new(),
                xsts_request_bodies: Vec::new(),
                mc_request_bodies: Vec::new(),
            }
        }
    }

    fn handle(world: &World, request: &crate::test_support::TestRequest) -> TestResponse {
        let body = String::from_utf8_lossy(&request.body).into_owned();
        match request.path.as_str() {
            "/oauth2/v2.0/token" => {
                let is_refresh = body.contains("grant_type=refresh_token");
                if is_refresh {
                    if world.reject_refresh {
                        return TestResponse::ok(
                            br#"{"error":"invalid_grant","error_description":"token has been revoked"}"#,
                        )
                        .with_status(400);
                    }
                    // Refreshes rotate the credential; the serving closure has
                    // already counted this request, so R<n> matches the count.
                    let refresh = format!("FIXTURE-MSA-REFRESH-R{}", world.refresh_count);
                    return TestResponse::ok(
                        format!(
                            r#"{{"access_token":"FIXTURE-MSA-ACCESS-R{}","refresh_token":"{refresh}","expires_in":3600,"token_type":"Bearer"}}"#,
                            world.refresh_count
                        )
                        .as_bytes(),
                    );
                }
                TestResponse::ok(
                    br#"{"access_token":"FIXTURE-MSA-ACCESS-0","refresh_token":"FIXTURE-MSA-REFRESH-0","expires_in":3600,"token_type":"Bearer"}"#,
                )
            }
            "/user/authenticate" => TestResponse::ok(
                br#"{"Token":"FIXTURE-XBOX-TOKEN","DisplayClaims":{"xui":[{"uhs":"uhs-1"}]}}"#,
            ),
            "/xsts/authorize" => match world.xerr {
                Some(xerr) => TestResponse::ok(
                    format!(r#"{{"Identity":"X","XErr":{xerr},"Message":""}}"#).as_bytes(),
                )
                .with_status(401),
                None => TestResponse::ok(
                    br#"{"Token":"FIXTURE-XSTS-TOKEN","DisplayClaims":{"xui":[{"uhs":"uhs-1"}]}}"#,
                ),
            },
            "/authentication/login_with_xbox" => TestResponse::ok(
                format!(
                    r#"{{"username":"00000000000000000000000000000000","roles":[],"access_token":"FIXTURE-MC-TOKEN","token_type":"Bearer","expires_in":{}}}"#,
                    world.mc_expires_in
                )
                .as_bytes(),
            ),
            "/entitlements/mcstore" => {
                if world.entitled {
                    TestResponse::ok(br#"{"items":[{"name":"product_minecraft"},{"name":"product_minecraft_bedrock"}],"signature":"sig","keyId":"1"}"#)
                } else {
                    TestResponse::ok(br#"{"items":[{"name":"product_minecraft_bedrock"}],"signature":"sig","keyId":"1"}"#)
                }
            }
            "/minecraft/profile" => {
                if world.profile_present {
                    TestResponse::ok(
                        format!(
                            r#"{{"id":"{}","name":"{}","skins":[],"capes":[]}}"#,
                            world.profile_id, world.profile_name
                        )
                        .as_bytes(),
                    )
                } else {
                    TestResponse::ok(br#"{"error":"NOT_FOUND","errorMessage":"no profile"}"#)
                        .with_status(404)
                }
            }
            _ => TestResponse::status(404),
        }
    }

    fn scripted_browser(
        script: BrowserScript,
    ) -> impl Fn(&str) -> Result<(), BrowserOpenError> + Send + Sync {
        move |authorize_url: &str| -> Result<(), BrowserOpenError> {
            match script {
                BrowserScript::FailToOpen => {
                    return Err(BrowserOpenError::new("no browser available"));
                }
                BrowserScript::Nothing => return Ok(()),
                BrowserScript::ProviderError { error } => {
                    let (authority, path) = redirect_target_of(authorize_url);
                    let query = format!("error={error}");
                    std::thread::spawn(move || send_callback(&authority, &path, &query));
                    return Ok(());
                }
                _ => {}
            }

            let params: Vec<(String, String)> = url::form_urlencoded::parse(
                authorize_url
                    .split_once('?')
                    .map(|(_, query)| query)
                    .unwrap_or("")
                    .as_bytes(),
            )
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();

            let state = params
                .iter()
                .find(|(key, _)| key == "state")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let (redirect_authority, redirect_path) = redirect_target_of(authorize_url);

            let query = match script {
                BrowserScript::StaleState => {
                    format!("code={AUTH_CODE}&state=stale-state-from-an-old-tab")
                }
                _ => format!("code={AUTH_CODE}&state={state}"),
            };

            std::thread::spawn(move || send_callback(&redirect_authority, &redirect_path, &query));
            Ok(())
        }
    }

    /// The `(authority, path)` the scripted "browser" redirects to, taken
    /// from the authorization request's redirect_uri exactly as a real
    /// browser would follow it. An empty path normalizes to the root.
    fn redirect_target_of(authorize_url: &str) -> (String, String) {
        authorize_url
            .split_once("redirect_uri=")
            .and_then(|(_, rest)| rest.split('&').next())
            .map(|encoded| {
                // Re-key the encoded pair so form decoding yields the value.
                url::form_urlencoded::parse(format!("x={encoded}").as_bytes())
                    .next()
                    .map(|(_, value)| value.into_owned())
                    .unwrap_or_default()
            })
            .and_then(|uri| {
                uri.strip_prefix("http://")
                    .map(|rest| match rest.split_once('/') {
                        Some((authority, path)) => (authority.to_owned(), format!("/{path}")),
                        None => (rest.to_owned(), "/".to_owned()),
                    })
            })
            .unwrap_or_default()
    }

    fn send_callback(authority: &str, path: &str, query: &str) {
        let Ok(mut stream) = TcpStream::connect(authority) else {
            return;
        };
        let _ = std::io::Write::write_all(
            &mut stream,
            format!("GET {path}?{query} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes(),
        );
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut sink = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stream, &mut sink);
    }

    /// Serializes tests that exercise the process-global login-transaction
    /// slot and session cache, so parallel test threads cannot observe each
    /// other's login state.
    async fn serialized_flow_tests() -> tokio::sync::MutexGuard<'static, ()> {
        static SERIALIZE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        SERIALIZE.lock().await
    }

    struct TestSetup {
        server: TestServer,
        world: Arc<Mutex<World>>,
        credentials: super::super::credentials::MemoryCredentialStore,
        accounts_path: PathBuf,
        endpoints: AuthEndpoints,
        options: AuthFlowOptions,
    }

    impl TestSetup {
        fn new(name: &str) -> Self {
            SessionCache::clear();
            let world = Arc::new(Mutex::new(World::new()));
            let serving = Arc::clone(&world);
            let server = TestServer::spawn(Arc::new(move |request| {
                let mut world = serving.lock().expect("world is not poisoned");
                // Record request bodies for assertions.
                match request.path.as_str() {
                    "/oauth2/v2.0/token" => {
                        world
                            .token_request_bodies
                            .push(String::from_utf8_lossy(&request.body).into_owned());
                        if String::from_utf8_lossy(&request.body)
                            .contains("grant_type=refresh_token")
                        {
                            world.refresh_count += 1;
                        }
                    }
                    "/user/authenticate" => world
                        .xbox_request_bodies
                        .push(String::from_utf8_lossy(&request.body).into_owned()),
                    "/xsts/authorize" => world
                        .xsts_request_bodies
                        .push(String::from_utf8_lossy(&request.body).into_owned()),
                    "/authentication/login_with_xbox" => world
                        .mc_request_bodies
                        .push(String::from_utf8_lossy(&request.body).into_owned()),
                    _ => {}
                }
                handle(&world, request)
            }));

            let directory = std::env::temp_dir()
                .join(name)
                .join(std::process::id().to_string());
            std::fs::create_dir_all(&directory).unwrap();
            let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

            Self {
                server,
                world,
                credentials: super::super::credentials::MemoryCredentialStore::default(),
                accounts_path: directory.join("accounts.json"),
                endpoints,
                options: AuthFlowOptions {
                    login_timeout: Duration::from_secs(5),
                },
            }
        }

        fn context<'a>(
            &'a self,
            browser: &'a (dyn Fn(&str) -> Result<(), BrowserOpenError> + Send + Sync),
        ) -> AuthContext<'a> {
            AuthContext {
                endpoints: &self.endpoints,
                oauth: OAuthClientConfig::new("aurora-test-client-id"),
                credentials: &self.credentials,
                accounts_path: &self.accounts_path,
                options: self.options.clone(),
                browser,
            }
        }

        async fn sign_in(&self, script: BrowserScript) -> Result<AccountRecord, AuthError> {
            let browser = scripted_browser(script);
            let context = self.context(&browser);
            sign_in(&context, &mut |_phase| {}).await
        }
    }

    fn s256(verifier: &str) -> String {
        base64ct::Base64UrlUnpadded::encode_string(&sha2::Sha256::digest(verifier.as_bytes()))
    }

    #[tokio::test]
    async fn full_sign_in_completes_and_persists_only_non_secret_state() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-signin");
        let mut phases = Vec::new();
        let browser = scripted_browser(BrowserScript::Complete);
        let context = setup.context(&browser);

        let record = sign_in(&context, &mut |phase| {
            phases.push(phase.as_str().to_owned())
        })
        .await
        .expect("sign-in must complete");

        assert_eq!(record.account_id(), ACCOUNT_A);
        assert_eq!(record.minecraft_name(), "HowDoesAuthWork");
        assert!(phases.contains(&"waitingForMicrosoft".to_owned()));
        assert!(phases.last().is_some_and(|phase| phase == "savingAccount"));

        // The persisted document is non-secret.
        let document_text = std::fs::read_to_string(&setup.accounts_path).unwrap();
        assert!(document_text.contains("HowDoesAuthWork"));
        assert!(document_text.contains(ACCOUNT_A));
        for secret in [
            "FIXTURE-MSA-ACCESS",
            "FIXTURE-MSA-REFRESH",
            "FIXTURE-MC-TOKEN",
            AUTH_CODE,
        ] {
            assert!(
                !document_text.contains(secret),
                "accounts document leaked {secret}"
            );
        }

        // The credential store holds exactly the first refresh credential.
        let credential = setup.credentials.load(ACCOUNT_A).unwrap().unwrap();
        assert_eq!(credential.client_id(), "aurora-test-client-id");
        assert_eq!(credential.refresh_token().expose(), "FIXTURE-MSA-REFRESH-0");

        // A usable session is cached for launch assembly.
        let session = SessionCache::usable(ACCOUNT_A).expect("session must be cached");
        assert_eq!(
            session.minecraft_access_token().expose(),
            "FIXTURE-MC-TOKEN"
        );
        assert!(session.usable());

        // The first account became selected automatically.
        let document = accounts::load(&setup.accounts_path).unwrap();
        assert_eq!(document.selected_account_id(), Some(ACCOUNT_A));

        // The token request carried the code, the PKCE verifier whose S256
        // matches the authorization request, the redirect URI, and no secret.
        let bodies = setup.world.lock().unwrap().token_request_bodies.clone();
        assert_eq!(bodies.len(), 1);
        let body = &bodies[0];
        assert!(body.contains("grant_type=authorization_code"));
        assert!(body.contains(&format!("code={AUTH_CODE}")));
        assert!(body.contains("client_id=aurora-test-client-id"));
        assert!(!body.contains("client_secret"));

        // Downstream requests used the documented payloads. Each capture is
        // cloned out of the lock: borrowing through the guard would extend
        // the guard's lifetime and re-locking would deadlock the same thread.
        let xbox = setup.world.lock().unwrap().xbox_request_bodies[0].clone();
        assert!(xbox.contains(r#""RpsTicket":"d=FIXTURE-MSA-ACCESS-0""#));
        let xsts = setup.world.lock().unwrap().xsts_request_bodies[0].clone();
        assert!(xsts.contains(r#""RelyingParty":"rp://api.minecraftservices.com/""#));
        let mc = setup.world.lock().unwrap().mc_request_bodies[0].clone();
        assert!(mc.contains(r#""identityToken":"XBL3.0 x=uhs-1;FIXTURE-XSTS-TOKEN""#));
    }

    #[tokio::test]
    async fn the_verifier_matches_the_s256_challenge_of_the_authorization_request() {
        let _flow_tests = serialized_flow_tests().await;
        // The scripted browser received the authorization URL; recovering the
        // challenge requires the URL, so this test drives the browser itself
        // and asserts the verifier/challenge relationship from captures.
        let setup = TestSetup::new("aurora-auth-flow-pkce");

        let captured_url = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&captured_url);
        let browser = move |url: &str| -> Result<(), BrowserOpenError> {
            *sink.lock().unwrap() = url.to_owned();
            scripted_browser(BrowserScript::Complete)(url)
        };

        let context = setup.context(&browser);
        sign_in(&context, &mut |_| {}).await.unwrap();

        let authorize_url = captured_url.lock().unwrap().clone();
        assert!(authorize_url.contains("code_challenge_method=S256"));
        assert!(authorize_url.contains("response_type=code"));
        assert!(authorize_url.contains("scope=XboxLive.signin+offline_access"));
        assert!(authorize_url.contains("client_id=aurora-test-client-id"));

        let params: Vec<(String, String)> = url::form_urlencoded::parse(
            authorize_url
                .split_once('?')
                .map(|(_, query)| query)
                .unwrap_or("")
                .as_bytes(),
        )
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
        let challenge = params
            .iter()
            .find(|(key, _)| key == "code_challenge")
            .map(|(_, value)| value.clone())
            .unwrap();

        let body = setup.world.lock().unwrap().token_request_bodies[0].clone();
        let verifier = body
            .split("code_verifier=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned();

        assert_eq!(s256(&verifier), challenge);
    }

    #[tokio::test]
    async fn the_registered_loopback_root_reaches_both_exchange_ends_identically() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-redirect-uri");

        let captured_url = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&captured_url);
        let browser = move |url: &str| -> Result<(), BrowserOpenError> {
            *sink.lock().unwrap() = url.to_owned();
            scripted_browser(BrowserScript::Complete)(url)
        };

        let context = setup.context(&browser);
        sign_in(&context, &mut |_| {}).await.unwrap();

        // The authorization request advertised the registered loopback
        // redirect: localhost hostname, dynamically selected port, root path.
        let authorize_url = captured_url.lock().unwrap().clone();
        let params: Vec<(String, String)> = url::form_urlencoded::parse(
            authorize_url
                .split_once('?')
                .map(|(_, query)| query)
                .unwrap_or("")
                .as_bytes(),
        )
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
        let advertised = params
            .iter()
            .find(|(key, _)| key == "redirect_uri")
            .map(|(_, value)| value.clone())
            .unwrap();

        assert!(advertised.starts_with("http://localhost:"));
        assert!(
            advertised.ends_with('/'),
            "the root path carries no segment"
        );
        assert!(
            !advertised.contains("127.0.0.1"),
            "the bind address never enters the URI"
        );
        let port: u16 = advertised["http://localhost:".len()..]
            .trim_end_matches('/')
            .parse()
            .expect("the port is a bare number");
        assert_ne!(port, 0, "the dynamically selected port must be present");

        // The token exchange carried exactly the same redirect URI.
        let body = setup.world.lock().unwrap().token_request_bodies[0].clone();
        let encoded = body
            .split("redirect_uri=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let exchanged = url::form_urlencoded::parse(format!("x={encoded}").as_bytes())
            .next()
            .map(|(_, value)| value.into_owned())
            .unwrap();
        assert_eq!(exchanged, advertised);
    }

    #[tokio::test]
    async fn downstream_failures_persist_nothing() {
        let _flow_tests = serialized_flow_tests().await;
        for (name, mutate, expected_code) in [
            (
                "not-entitled",
                Box::new(|world: &mut World| world.entitled = false) as Box<dyn FnOnce(&mut World)>,
                "auth_entitlement_missing",
            ),
            (
                "no-profile",
                Box::new(|world: &mut World| world.profile_present = false),
                "auth_profile_missing",
            ),
            (
                "xsts-denied",
                Box::new(|world: &mut World| world.xerr = Some(2148916238)),
                "auth_xsts_failure",
            ),
        ] {
            let setup = TestSetup::new("aurora-auth-flow-downstream-failures");
            mutate(&mut setup.world.lock().unwrap());

            let error = setup
                .sign_in(BrowserScript::Complete)
                .await
                .expect_err("the flow must fail");

            assert_eq!(error.code(), expected_code, "case {name}");
            assert_eq!(
                accounts::load(&setup.accounts_path).unwrap().accounts(),
                &[]
            );
            assert_eq!(setup.credentials.load(ACCOUNT_A).unwrap(), None);
            assert!(SessionCache::usable(ACCOUNT_A).is_none());

            if name == "xsts-denied" {
                assert!(error.to_string().contains("family group"));
            }
        }
    }

    #[tokio::test]
    async fn provider_error_redirects_fail_with_oauth_failure() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-oauth-error");

        let error = setup
            .sign_in(BrowserScript::ProviderError {
                error: "access_denied",
            })
            .await
            .expect_err("provider errors fail");

        assert_eq!(error.code(), "auth_oauth_failure");
        assert!(error.to_string().contains("access_denied"));
        assert_eq!(
            accounts::load(&setup.accounts_path).unwrap().accounts(),
            &[]
        );
    }

    #[tokio::test]
    async fn a_browser_that_never_opens_fails_cleanly() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-browser-fail");

        let error = setup
            .sign_in(BrowserScript::FailToOpen)
            .await
            .expect_err("a browser failure must surface");

        assert_eq!(error.code(), "auth_browser_open_failure");
        // The transaction slot was released.
        assert!(!cancel_active_login());
    }

    #[tokio::test]
    async fn login_times_out_when_no_callback_arrives() {
        let _flow_tests = serialized_flow_tests().await;
        let mut setup = TestSetup::new("aurora-auth-flow-timeout");
        setup.options.login_timeout = Duration::from_millis(200);

        let error = setup
            .sign_in(BrowserScript::Nothing)
            .await
            .expect_err("no callback means timeout");

        assert_eq!(error.code(), "auth_login_timeout");
        assert_eq!(
            accounts::load(&setup.accounts_path).unwrap().accounts(),
            &[]
        );
        assert_eq!(setup.credentials.load(ACCOUNT_A).unwrap(), None);
        // The slot is free again and no listener survives the flow.
        assert!(!cancel_active_login());
    }

    #[tokio::test]
    async fn stale_state_callbacks_are_ignored_until_timeout() {
        let _flow_tests = serialized_flow_tests().await;
        let mut setup = TestSetup::new("aurora-auth-flow-stale-state");
        setup.options.login_timeout = Duration::from_millis(300);

        let error = setup
            .sign_in(BrowserScript::StaleState)
            .await
            .expect_err("a stale callback alone cannot complete the flow");

        assert_eq!(error.code(), "auth_login_timeout");
        assert_eq!(
            accounts::load(&setup.accounts_path).unwrap().accounts(),
            &[]
        );
    }

    #[tokio::test]
    async fn an_active_login_can_be_cancelled_and_its_state_discarded() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-cancel");
        let browser = scripted_browser(BrowserScript::Nothing);
        let context = setup.context(&browser);

        let canceller = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            cancel_active_login()
        });

        let error = sign_in(&context, &mut |_| {})
            .await
            .expect_err("cancelled flows fail");
        assert_eq!(error.code(), "auth_login_cancelled");
        assert!(canceller.await.expect("canceller settles"));
        assert!(!cancel_active_login(), "the slot must be free afterwards");
        assert_eq!(
            accounts::load(&setup.accounts_path).unwrap().accounts(),
            &[]
        );

        // A cancelled flow leaves no residue: a fresh login succeeds.
        let setup2 = TestSetup::new("aurora-auth-flow-cancel-then-retry");
        setup2
            .sign_in(BrowserScript::Complete)
            .await
            .expect("a fresh login works after a cancelled one");
    }

    #[tokio::test]
    async fn only_one_login_transaction_is_allowed_at_a_time() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-concurrency");
        let first_browser = scripted_browser(BrowserScript::Nothing);
        let first_context = setup.context(&first_browser);
        let second_browser = scripted_browser(BrowserScript::Complete);
        let second_context = setup.context(&second_browser);

        // join! polls its futures in order, so the first login acquires the
        // transaction slot (a synchronous step) before the second attempts
        // it — deterministic without sleeps. A canceller ends the first flow.
        let canceller = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            cancel_active_login()
        });

        let (first, second) = tokio::join!(
            async { sign_in(&first_context, &mut |_| {}).await },
            async { sign_in(&second_context, &mut |_| {}).await },
        );

        assert_eq!(
            second.expect_err("the second login must be refused").code(),
            "auth_login_in_progress"
        );
        assert_eq!(
            first.expect_err("first login cancelled").code(),
            "auth_login_cancelled"
        );
        canceller.await.expect("canceller settles");
    }

    #[tokio::test]
    async fn a_late_callback_cannot_resurrect_a_cancelled_flow() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-late-callback");

        let authorize_url = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&authorize_url);
        let browser = move |url: &str| -> Result<(), BrowserOpenError> {
            *sink.lock().unwrap() = url.to_owned();
            Ok(())
        };
        let context = setup.context(&browser);

        let canceller = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            cancel_active_login()
        });
        sign_in(&context, &mut |_| {}).await.expect_err("cancelled");
        canceller.await.expect("canceller settles");

        // The (long-gone) browser tab now delivers its callback. The
        // receiver is already closed, so the connection is refused (or
        // closed without an answer) and nothing whatsoever is persisted.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (redirect_authority, _redirect_path) =
            redirect_target_of(&authorize_url.lock().unwrap());
        if let Ok(mut stream) = TcpStream::connect(redirect_authority) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let mut response = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stream, &mut response);
        }

        assert_eq!(
            accounts::load(&setup.accounts_path).unwrap().accounts(),
            &[]
        );
        assert!(SessionCache::usable(ACCOUNT_A).is_none());
        assert_eq!(setup.credentials.load(ACCOUNT_A).unwrap(), None);
    }

    #[tokio::test]
    async fn refreshing_a_second_sign_in_updates_metadata_and_keeps_the_selection() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-re-signin");

        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("first sign-in");

        // A second, different account signs in; the selection must not move.
        setup.world.lock().unwrap().profile_id = ACCOUNT_B.to_owned();
        setup.world.lock().unwrap().profile_name = "SecondAccount".to_owned();
        let record = setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("second sign-in");

        assert_eq!(record.account_id(), ACCOUNT_B);
        let document = accounts::load(&setup.accounts_path).unwrap();
        assert_eq!(document.accounts().len(), 2);
        assert_eq!(document.selected_account_id(), Some(ACCOUNT_A));

        // Re-signing the first account updates its renamed metadata in place.
        setup.world.lock().unwrap().profile_id = ACCOUNT_A.to_owned();
        setup.world.lock().unwrap().profile_name = "RenamedLater".to_owned();
        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("re-sign-in");
        let document = accounts::load(&setup.accounts_path).unwrap();
        assert_eq!(document.accounts().len(), 2);
        assert!(
            document
                .accounts()
                .iter()
                .any(|record| record.account_id() == ACCOUNT_A
                    && record.minecraft_name() == "RenamedLater")
        );
    }

    #[tokio::test]
    async fn restoring_an_expired_session_refreshes_and_rotates_the_credential() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-restore");
        setup.world.lock().unwrap().mc_expires_in = 1; // below the skew margin
        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("sign-in");
        assert!(
            SessionCache::usable(ACCOUNT_A).is_none(),
            "the session must be immediately unusable"
        );

        // The profile was renamed upstream since the last sign-in, and the
        // restored session receives a normal lifetime again.
        let mut world = setup.world.lock().unwrap();
        world.profile_name = "RenamedName".to_owned();
        world.mc_expires_in = 86400;
        drop(world);

        let browser = scripted_browser(BrowserScript::Nothing);
        let context = setup.context(&browser);
        let session = ensure_session(&context, ACCOUNT_A, &mut |_| {})
            .await
            .expect("restoration must run the refresh chain");

        assert_eq!(session.profile().name(), "RenamedName");
        assert!(session.usable());
        assert_eq!(setup.world.lock().unwrap().refresh_count, 1);

        // The rotated refresh credential replaced the persisted one.
        let credential = setup.credentials.load(ACCOUNT_A).unwrap().unwrap();
        assert_eq!(
            credential.refresh_token().expose(),
            "FIXTURE-MSA-REFRESH-R1"
        );

        // The record's name followed the profile rename.
        let document = accounts::load(&setup.accounts_path).unwrap();
        assert!(
            document
                .accounts()
                .iter()
                .any(|record| record.minecraft_name() == "RenamedName")
        );
    }

    #[tokio::test]
    async fn a_usable_cached_session_short_circuits_without_network() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-cached");

        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("sign-in");
        let requests_before = setup.server.request_count();
        let refreshes_before = setup.world.lock().unwrap().refresh_count;

        let browser = scripted_browser(BrowserScript::Nothing);
        let context = setup.context(&browser);
        let session = ensure_session(&context, ACCOUNT_A, &mut |_| {})
            .await
            .expect("cached session is usable");

        assert_eq!(setup.server.request_count(), requests_before);
        assert_eq!(setup.world.lock().unwrap().refresh_count, refreshes_before);
        assert_eq!(session.account_id(), ACCOUNT_A);
    }

    #[tokio::test]
    async fn a_revoked_refresh_credential_requires_reauthentication_and_is_removed() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-revoked");
        setup.world.lock().unwrap().mc_expires_in = 1;
        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("sign-in");
        setup.world.lock().unwrap().reject_refresh = true;

        let browser = scripted_browser(BrowserScript::Nothing);
        let context = setup.context(&browser);
        let error = ensure_session(&context, ACCOUNT_A, &mut |_| {})
            .await
            .expect_err("revoked credentials fail");

        assert_eq!(error.code(), "auth_reauthentication_required");
        // The dead credential is gone; the account record deliberately stays.
        assert_eq!(setup.credentials.load(ACCOUNT_A).unwrap(), None);
        let document = accounts::load(&setup.accounts_path).unwrap();
        assert_eq!(document.accounts().len(), 1);
    }

    #[tokio::test]
    async fn a_missing_credential_requires_reauthentication() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-no-credential");
        setup.world.lock().unwrap().mc_expires_in = 1;
        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("sign-in");
        setup.credentials.delete(ACCOUNT_A).unwrap();

        let browser = scripted_browser(BrowserScript::Nothing);
        let context = setup.context(&browser);
        let error = ensure_session(&context, ACCOUNT_A, &mut |_| {})
            .await
            .expect_err("no credential means reauthentication");

        assert_eq!(error.code(), "auth_reauthentication_required");
    }

    #[tokio::test]
    async fn a_credential_resolving_to_another_account_is_removed() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-mismatch");
        setup.world.lock().unwrap().mc_expires_in = 1;
        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("sign-in");
        setup.world.lock().unwrap().profile_id = ACCOUNT_B.to_owned();

        let browser = scripted_browser(BrowserScript::Nothing);
        let context = setup.context(&browser);
        let error = ensure_session(&context, ACCOUNT_A, &mut |_| {})
            .await
            .expect_err("mismatched identity must fail");

        assert_eq!(error.code(), "auth_reauthentication_required");
        assert!(error.to_string().contains("different Minecraft account"));
        assert_eq!(setup.credentials.load(ACCOUNT_A).unwrap(), None);
    }

    #[tokio::test]
    async fn restoring_an_unknown_account_fails_explicitly() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-unknown-account");
        let browser = scripted_browser(BrowserScript::Nothing);
        let context = setup.context(&browser);

        let error = ensure_session(&context, ACCOUNT_B, &mut |_| {})
            .await
            .expect_err("unknown accounts fail");

        assert_eq!(error.code(), "auth_account_not_found");
    }

    #[tokio::test]
    async fn sign_out_removes_the_credential_record_and_cleared_selection() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-signout");

        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("first sign-in");
        setup.world.lock().unwrap().profile_id = ACCOUNT_B.to_owned();
        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("second sign-in");
        assert!(SessionCache::usable(ACCOUNT_B).is_some());

        let browser = scripted_browser(BrowserScript::Nothing);
        let context = setup.context(&browser);

        // Removing the selected account clears the selection.
        sign_out(&context, ACCOUNT_A).expect("sign out");
        let document = accounts::load(&setup.accounts_path).unwrap();
        assert_eq!(document.accounts().len(), 1);
        assert_eq!(document.selected_account_id(), None);
        assert_eq!(setup.credentials.load(ACCOUNT_A).unwrap(), None);
        assert!(SessionCache::usable(ACCOUNT_A).is_none());

        sign_out(&context, ACCOUNT_B).expect("second sign out");
        assert_eq!(
            accounts::load(&setup.accounts_path).unwrap().accounts(),
            &[]
        );

        let error = sign_out(&context, ACCOUNT_B).expect_err("unknown accounts fail");
        assert_eq!(error.code(), "auth_account_not_found");
    }

    #[tokio::test]
    async fn account_status_reflects_credential_presence() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-status");
        setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect("sign-in");

        assert_eq!(
            AccountStatus::of(&setup.credentials, ACCOUNT_A).as_str(),
            "signedIn"
        );

        setup.credentials.delete(ACCOUNT_A).unwrap();
        assert_eq!(
            AccountStatus::of(&setup.credentials, ACCOUNT_A).as_str(),
            "reauthenticationRequired"
        );
    }

    #[tokio::test]
    async fn no_fixture_secret_appears_in_any_error_or_debug_output() {
        let _flow_tests = serialized_flow_tests().await;
        let setup = TestSetup::new("aurora-auth-flow-redaction");
        setup.world.lock().unwrap().entitled = false;
        let entitled_error = setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect_err("not entitled");

        let denied_setup = TestSetup::new("aurora-auth-flow-redaction-xsts");
        denied_setup.world.lock().unwrap().xerr = Some(2148916233);
        let xsts_error = denied_setup
            .sign_in(BrowserScript::Complete)
            .await
            .expect_err("denied");

        let oauth_setup = TestSetup::new("aurora-auth-flow-redaction-oauth");
        let oauth_error = oauth_setup
            .sign_in(BrowserScript::ProviderError {
                error: "temporarily_unavailable",
            })
            .await
            .expect_err("oauth failure");

        let session = MinecraftSession::new(
            ACCOUNT_A,
            MinecraftProfile::new(ACCOUNT_A, "HowDoesAuthWork"),
            SecretString::new("FIXTURE-MC-TOKEN"),
            Duration::from_secs(3600),
        );

        for error in [&entitled_error, &xsts_error, &oauth_error] {
            let rendered = format!("{error}") + &format!("{error:?}");
            for secret in [
                "FIXTURE-MSA-ACCESS",
                "FIXTURE-MSA-REFRESH",
                "FIXTURE-XBOX-TOKEN",
                "FIXTURE-XSTS-TOKEN",
                "FIXTURE-MC-TOKEN",
                AUTH_CODE,
            ] {
                assert!(
                    !rendered.contains(secret),
                    "error output leaked {secret}: {rendered}"
                );
            }
        }

        let rendered = format!("{session:?}");
        assert!(!rendered.contains("FIXTURE-MC-TOKEN"));
    }

    #[test]
    fn the_production_registration_is_never_invented() {
        // When the release process provides no registration the value is
        // absent; when it does, it must be the real configuration, non-empty.
        match production_registration() {
            None => {}
            Some(config) => assert!(!config.client_id().is_empty()),
        }
    }
}
