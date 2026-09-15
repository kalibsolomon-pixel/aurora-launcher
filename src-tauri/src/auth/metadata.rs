//! External authentication services: pinned endpoints, wire DTOs, and the
//! bounded exchange boundary.
//!
//! Everything network-facing about Microsoft → Xbox → XSTS → Minecraft
//! Services authentication lives here. The rest of the authentication domain
//! consumes normalized results and never sees raw external JSON. Responses
//! are strictly parsed, bounded in memory, and sanitized: parse failures and
//! error mappings never quote response bodies (a malformed token response
//! could otherwise echo secret-bearing content into logs).
//!
//! Endpoint evidence (verified September 2026 against current Microsoft
//! identity-platform documentation and the community-documented Minecraft
//! launcher flow it underpins): the v2.0 consumer endpoints,
//! `user.auth.xboxlive.com`/`xsts.auth.xboxlive.com` with their exact
//! PascalCase payloads, and the three `api.minecraftservices.com` routes.

use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::downloads::{self, DownloadError};

use super::credentials::SecretString;

/// Hard cap on one authentication response body in memory.
const MAX_AUTH_RESPONSE_BYTES: usize = 256 * 1024;

/// Longest token lifetime this boundary accepts (30 days); anything larger
/// is treated as malformed rather than trusted.
const MAX_TOKEN_LIFETIME_SECONDS: u64 = 30 * 24 * 60 * 60;

/// Longest provider-supplied error string carried into diagnostics.
const MAX_PROVIDER_ERROR_DETAIL: usize = 200;

/// The pinned official authorization endpoint (consumer Microsoft accounts
/// only — Xbox authentication cannot consume work/school tokens).
pub const OFFICIAL_AUTHORIZE_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";

/// The pinned official token endpoint.
pub const OFFICIAL_TOKEN_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";

/// The pinned Xbox Live user-authentication endpoint.
pub const OFFICIAL_XBOX_USER_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";

/// The pinned Xbox XSTS authorization endpoint.
pub const OFFICIAL_XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";

/// The pinned Minecraft Services token endpoint.
pub const OFFICIAL_MINECRAFT_LOGIN_URL: &str =
    "https://api.minecraftservices.com/authentication/login_with_xbox";

/// The pinned Minecraft entitlements endpoint.
pub const OFFICIAL_MINECRAFT_ENTITLEMENTS_URL: &str =
    "https://api.minecraftservices.com/entitlements/mcstore";

/// The pinned Minecraft profile endpoint.
pub const OFFICIAL_MINECRAFT_PROFILE_URL: &str =
    "https://api.minecraftservices.com/minecraft/profile";

/// Every authentication endpoint, resolved once and passed around as data.
///
/// Production URLs are pinned constants; deterministic tests (and nothing
/// else) may point the whole set at a loopback server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthEndpoints {
    authorize: Url,
    token: Url,
    xbox_user_auth: Url,
    xsts: Url,
    minecraft_login: Url,
    minecraft_entitlements: Url,
    minecraft_profile: Url,
}

impl AuthEndpoints {
    pub fn official() -> Self {
        Self {
            authorize: Url::parse(OFFICIAL_AUTHORIZE_URL).expect("pinned official URL"),
            token: Url::parse(OFFICIAL_TOKEN_URL).expect("pinned official URL"),
            xbox_user_auth: Url::parse(OFFICIAL_XBOX_USER_AUTH_URL).expect("pinned official URL"),
            xsts: Url::parse(OFFICIAL_XSTS_URL).expect("pinned official URL"),
            minecraft_login: Url::parse(OFFICIAL_MINECRAFT_LOGIN_URL).expect("pinned official URL"),
            minecraft_entitlements: Url::parse(OFFICIAL_MINECRAFT_ENTITLEMENTS_URL)
                .expect("pinned official URL"),
            minecraft_profile: Url::parse(OFFICIAL_MINECRAFT_PROFILE_URL)
                .expect("pinned official URL"),
        }
    }

    /// Builds an endpoint set for the deterministic local test transport.
    /// Path shapes mirror the official endpoints.
    pub fn loopback_for_testing(base_url: &str) -> Self {
        let base = Url::parse(base_url).expect("test base URL");
        assert!(
            downloads::is_loopback_host(&base),
            "test authentication endpoints must stay on the loopback transport"
        );
        let join = |path: &str| base.join(path).expect("test endpoint path");

        Self {
            authorize: join("oauth2/v2.0/authorize"),
            token: join("oauth2/v2.0/token"),
            xbox_user_auth: join("user/authenticate"),
            xsts: join("xsts/authorize"),
            minecraft_login: join("authentication/login_with_xbox"),
            minecraft_entitlements: join("entitlements/mcstore"),
            minecraft_profile: join("minecraft/profile"),
        }
    }

    pub fn authorize(&self) -> &Url {
        &self.authorize
    }

    pub(crate) fn token(&self) -> &Url {
        &self.token
    }

    pub(crate) fn xbox_user_auth(&self) -> &Url {
        &self.xbox_user_auth
    }

    pub(crate) fn xsts(&self) -> &Url {
        &self.xsts
    }

    pub(crate) fn minecraft_login(&self) -> &Url {
        &self.minecraft_login
    }

    pub(crate) fn minecraft_entitlements(&self) -> &Url {
        &self.minecraft_entitlements
    }

    pub(crate) fn minecraft_profile(&self) -> &Url {
        &self.minecraft_profile
    }
}

// ---------------------------------------------------------------------------
// Bounded transport
// ---------------------------------------------------------------------------

/// Transport-level failures shared by every authentication exchange.
#[derive(Debug)]
pub enum AuthTransportError {
    Network(DownloadError),
    HttpStatus { status: u16 },
    ResponseTooLarge { limit_bytes: usize },
}

impl fmt::Display for AuthTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => write!(
                formatter,
                "the authentication service was unreachable: {error}"
            ),
            Self::HttpStatus { status } => write!(
                formatter,
                "the authentication service answered HTTP {status}"
            ),
            Self::ResponseTooLarge { limit_bytes } => write!(
                formatter,
                "the authentication response exceeded the {limit_bytes}-byte bound and was rejected"
            ),
        }
    }
}

impl std::error::Error for AuthTransportError {}

/// Builds the dedicated authentication client.
///
/// Distinct from the artifact downloader deliberately: tokens are not
/// artifacts and never flow through the cache. Redirects are refused (token
/// endpoints answer directly; silently following redirects on a credential
/// exchange is forbidden), HTTPS is transport policy, timeouts are bounded,
/// and the user agent is the shared launcher identity.
fn auth_client() -> reqwest::Client {
    downloads::ensure_rustls_crypto_provider();

    reqwest::Client::builder()
        .user_agent(downloads::user_agent())
        .connect_timeout(downloads::CONNECT_TIMEOUT)
        .read_timeout(downloads::IDLE_READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("authentication client configuration is valid")
}

/// Sends one request and returns `(status, bounded body)`.
async fn send(request: reqwest::RequestBuilder) -> Result<(u16, Vec<u8>), AuthTransportError> {
    let response = request
        .send()
        .await
        .map_err(|error| AuthTransportError::Network(DownloadError::from_transport(error)))?;

    let status = response.status().as_u16();
    if let Some(length) = response.content_length() {
        if length as usize > MAX_AUTH_RESPONSE_BYTES {
            return Err(AuthTransportError::ResponseTooLarge {
                limit_bytes: MAX_AUTH_RESPONSE_BYTES,
            });
        }
    }

    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| AuthTransportError::Network(DownloadError::from_transport(error)))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_AUTH_RESPONSE_BYTES {
            return Err(AuthTransportError::ResponseTooLarge {
                limit_bytes: MAX_AUTH_RESPONSE_BYTES,
            });
        }
    }

    Ok((status, body))
}

/// A provider-declared error on the token endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRejection {
    pub error: String,
    pub description: Option<String>,
}

/// Truncates provider-supplied detail to a bounded, non-secret summary.
fn bounded_detail(detail: Option<String>) -> Option<String> {
    detail.map(|value| {
        let truncated: String = value.chars().take(MAX_PROVIDER_ERROR_DETAIL).collect();
        truncated
    })
}

// ---------------------------------------------------------------------------
// Microsoft OAuth token exchange
// ---------------------------------------------------------------------------

/// The Microsoft credential set one exchange yields. The access token is
/// memory-only; the refresh token is the only value eligible for the
/// credential store.
#[derive(Debug)]
pub struct MicrosoftTokenSet {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub expires_in_seconds: u64,
}

/// The grant being redeemed.
pub enum TokenGrant<'a> {
    AuthorizationCode {
        code: &'a str,
        code_verifier: &'a str,
        redirect_uri: &'a str,
    },
    RefreshToken {
        refresh_token: &'a SecretString,
    },
}

#[derive(Debug)]
pub enum TokenExchangeError {
    Transport(AuthTransportError),
    /// The provider rejected the grant (for example `invalid_grant`).
    Rejected(ProviderRejection),
    /// The response parsed but did not contain the required fields.
    Malformed,
    /// The provider returned no refresh token; without `offline_access`
    /// there is nothing to persist and the login fails deliberately.
    MissingRefreshToken,
}

impl fmt::Display for TokenExchangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => {
                write!(formatter, "the Microsoft token exchange failed: {error}")
            }
            Self::Rejected(rejection) => write!(
                formatter,
                "Microsoft rejected the sign-in ({}{})",
                rejection.error,
                rejection
                    .description
                    .as_deref()
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default()
            ),
            Self::Malformed => write!(
                formatter,
                "the Microsoft token response was malformed and cannot be used"
            ),
            Self::MissingRefreshToken => write!(
                formatter,
                "Microsoft returned no refresh token, so the sign-in cannot be restored later; it was rejected"
            ),
        }
    }
}

impl std::error::Error for TokenExchangeError {}

#[derive(Deserialize)]
struct TokenResponseWire {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    token_type: String,
}

#[derive(Deserialize)]
struct TokenErrorWire {
    error: String,
    error_description: Option<String>,
}

/// Redeems an authorization code (with its PKCE verifier) or a refresh token
/// at the pinned Microsoft token endpoint.
///
/// The request is form-urlencoded with a public-client parameter set: client
/// ID, grant, and verifier — never a secret.
pub async fn request_microsoft_token(
    endpoints: &AuthEndpoints,
    client_id: &str,
    grant: TokenGrant<'_>,
) -> Result<MicrosoftTokenSet, TokenExchangeError> {
    // Build the form body in a scope so the non-Send serializer is dropped
    // before the request future is created (login futures must be Send).
    let form_body = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        match &grant {
            TokenGrant::AuthorizationCode {
                code,
                code_verifier,
                redirect_uri,
            } => {
                form.append_pair("grant_type", "authorization_code");
                form.append_pair("code", code);
                form.append_pair("code_verifier", code_verifier);
                form.append_pair("redirect_uri", redirect_uri);
            }
            TokenGrant::RefreshToken { refresh_token } => {
                form.append_pair("grant_type", "refresh_token");
                form.append_pair("refresh_token", refresh_token.expose());
            }
        }
        form.append_pair("client_id", client_id);
        form.append_pair("scope", super::oauth::OAUTH_SCOPES);
        form.finish()
    };

    let (status, body) = send(
        auth_client()
            .post(endpoints.token().clone())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body),
    )
    .await
    .map_err(TokenExchangeError::Transport)?;

    if status != 200 {
        let rejection = serde_json::from_slice::<TokenErrorWire>(&body)
            .ok()
            .filter(|wire| !wire.error.is_empty())
            .map(|wire| ProviderRejection {
                error: wire.error,
                description: bounded_detail(wire.error_description),
            })
            .unwrap_or(ProviderRejection {
                error: format!("http_{status}"),
                description: None,
            });
        return Err(TokenExchangeError::Rejected(rejection));
    }

    let wire: TokenResponseWire =
        serde_json::from_slice(&body).map_err(|_| TokenExchangeError::Malformed)?;
    if wire.access_token.is_empty()
        || !wire.token_type.eq_ignore_ascii_case("bearer")
        || wire.expires_in < 0
        || wire.expires_in as u64 > MAX_TOKEN_LIFETIME_SECONDS
    {
        return Err(TokenExchangeError::Malformed);
    }
    let refresh_token = wire.refresh_token.filter(|token| !token.is_empty());
    let refresh_token = match refresh_token {
        Some(token) => token,
        None => return Err(TokenExchangeError::MissingRefreshToken),
    };

    Ok(MicrosoftTokenSet {
        access_token: SecretString::new(wire.access_token),
        refresh_token: SecretString::new(refresh_token),
        expires_in_seconds: wire.expires_in as u64,
    })
}

// ---------------------------------------------------------------------------
// Xbox Live and XSTS
// ---------------------------------------------------------------------------

/// One Xbox-side credential: the token plus the user hash Minecraft Services
/// authentication requires. Both are memory-only.
#[derive(Debug)]
pub struct XboxToken {
    pub token: SecretString,
    pub user_hash: String,
}

#[derive(Debug)]
pub enum XboxExchangeError {
    Transport(AuthTransportError),
    Malformed,
    ServiceRejected { status: u16 },
}

impl fmt::Display for XboxExchangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => {
                write!(formatter, "Xbox Live authentication failed: {error}")
            }
            Self::Malformed => write!(
                formatter,
                "the Xbox Live authentication response was malformed and cannot be used"
            ),
            Self::ServiceRejected { status } => write!(
                formatter,
                "Xbox Live rejected the authentication with HTTP {status}"
            ),
        }
    }
}

impl std::error::Error for XboxExchangeError {}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct XboxAuthRequest {
    properties: XboxAuthProperties,
    relying_party: String,
    token_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct XboxAuthProperties {
    auth_method: String,
    site_name: String,
    rps_ticket: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxAuthResponse {
    token: String,
    display_claims: XboxDisplayClaims,
}

#[derive(Deserialize)]
struct XboxDisplayClaims {
    xui: Vec<XboxUserIdentity>,
}

#[derive(Deserialize)]
struct XboxUserIdentity {
    uhs: String,
}

/// The user-hash field of an Xbox token response, validated to be present
/// and non-empty (Minecraft Services authentication cannot proceed without
/// it).
fn validated_user_hash(response: &XboxAuthResponse) -> Result<String, ()> {
    let hash = response
        .display_claims
        .xui
        .first()
        .map(|identity| identity.uhs.trim())
        .unwrap_or("");
    if hash.is_empty() {
        return Err(());
    }
    Ok(hash.to_owned())
}

/// Exchanges the Microsoft access token for Xbox Live credentials
/// (`RPS` ticket with the documented `d=` prefix).
pub async fn authenticate_xbox(
    endpoints: &AuthEndpoints,
    microsoft_access_token: &SecretString,
) -> Result<XboxToken, XboxExchangeError> {
    let request = XboxAuthRequest {
        properties: XboxAuthProperties {
            auth_method: "RPS".to_owned(),
            site_name: "user.auth.xboxlive.com".to_owned(),
            rps_ticket: format!("d={}", microsoft_access_token.expose()),
        },
        relying_party: "http://auth.xboxlive.com".to_owned(),
        token_type: "JWT".to_owned(),
    };

    let (status, body) = send_json_post(endpoints.xbox_user_auth(), &request)
        .await
        .map_err(XboxExchangeError::Transport)?;

    if status != 200 {
        return Err(XboxExchangeError::ServiceRejected { status });
    }

    let wire: XboxAuthResponse =
        serde_json::from_slice(&body).map_err(|_| XboxExchangeError::Malformed)?;
    if wire.token.is_empty() {
        return Err(XboxExchangeError::Malformed);
    }
    let user_hash = validated_user_hash(&wire).map_err(|_| XboxExchangeError::Malformed)?;

    Ok(XboxToken {
        token: SecretString::new(wire.token),
        user_hash,
    })
}

#[derive(Debug)]
pub enum XstsExchangeError {
    Transport(AuthTransportError),
    Malformed,
    /// The account is not authorized for the Minecraft relying party. The
    /// numeric `XErr` code is the stable contract; the meaning attached to
    /// it at the flow boundary is evidence-based only.
    Denied {
        xerr: i64,
    },
    ServiceRejected {
        status: u16,
    },
}

impl fmt::Display for XstsExchangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => {
                write!(formatter, "Xbox XSTS authorization failed: {error}")
            }
            Self::Malformed => write!(
                formatter,
                "the XSTS authorization response was malformed and cannot be used"
            ),
            Self::Denied { xerr } => {
                write!(
                    formatter,
                    "Xbox XSTS authorization was denied (code {xerr})"
                )
            }
            Self::ServiceRejected { status } => write!(
                formatter,
                "Xbox XSTS authorization failed with HTTP {status}"
            ),
        }
    }
}

impl std::error::Error for XstsExchangeError {}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct XstsAuthRequest {
    properties: XstsAuthProperties,
    relying_party: String,
    token_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct XstsAuthProperties {
    sandbox_id: String,
    user_tokens: Vec<String>,
}

#[derive(Deserialize)]
struct XboxErrorWire {
    #[serde(rename = "XErr")]
    xerr: Option<i64>,
}

/// Performs the XSTS exchange for the Minecraft Services relying party,
/// preserving the user hash Minecraft authentication requires.
pub async fn authorize_xsts(
    endpoints: &AuthEndpoints,
    xbox_token: &XboxToken,
) -> Result<XboxToken, XstsExchangeError> {
    let request = XstsAuthRequest {
        properties: XstsAuthProperties {
            sandbox_id: "RETAIL".to_owned(),
            user_tokens: vec![xbox_token.token.expose().to_owned()],
        },
        relying_party: "rp://api.minecraftservices.com/".to_owned(),
        token_type: "JWT".to_owned(),
    };

    let (status, body) = send_json_post(endpoints.xsts(), &request)
        .await
        .map_err(XstsExchangeError::Transport)?;

    if status != 200 {
        // A 401 carries the provider's structured XErr code; parse it
        // without echoing anything else from the body.
        if let Some(xerr) = serde_json::from_slice::<XboxErrorWire>(&body)
            .ok()
            .and_then(|wire| wire.xerr)
        {
            return Err(XstsExchangeError::Denied { xerr });
        }
        return Err(XstsExchangeError::ServiceRejected { status });
    }

    let wire: XboxAuthResponse =
        serde_json::from_slice(&body).map_err(|_| XstsExchangeError::Malformed)?;
    if wire.token.is_empty() {
        return Err(XstsExchangeError::Malformed);
    }
    let user_hash = validated_user_hash(&wire).map_err(|_| XstsExchangeError::Malformed)?;

    Ok(XboxToken {
        token: SecretString::new(wire.token),
        user_hash,
    })
}

async fn send_json_post<T: Serialize>(
    url: &Url,
    body: &T,
) -> Result<(u16, Vec<u8>), AuthTransportError> {
    let payload = serde_json::to_vec(body).expect("authentication request serialization");
    send(
        auth_client()
            .post(url.clone())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(payload),
    )
    .await
}

// ---------------------------------------------------------------------------
// Minecraft Services
// ---------------------------------------------------------------------------

/// The Minecraft Services access token plus its declared lifetime.
#[derive(Debug)]
pub struct MinecraftTokenSet {
    pub access_token: SecretString,
    pub expires_in_seconds: u64,
}

#[derive(Debug)]
pub enum MinecraftExchangeError {
    Transport(AuthTransportError),
    Malformed,
    ServiceRejected { status: u16 },
}

impl fmt::Display for MinecraftExchangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => {
                write!(
                    formatter,
                    "Minecraft Services authentication failed: {error}"
                )
            }
            Self::Malformed => write!(
                formatter,
                "the Minecraft Services token response was malformed and cannot be used"
            ),
            Self::ServiceRejected { status } => write!(
                formatter,
                "Minecraft Services rejected the authentication with HTTP {status}"
            ),
        }
    }
}

impl std::error::Error for MinecraftExchangeError {}

#[derive(Serialize)]
struct MinecraftLoginRequest {
    #[serde(rename = "identityToken")]
    identity_token: String,
}

#[derive(Deserialize)]
struct MinecraftLoginResponse {
    access_token: String,
    token_type: Option<String>,
    expires_in: i64,
}

/// Exchanges the XSTS token for a Minecraft Services access token using the
/// documented `XBL3.0 x=<user hash>;<XSTS token>` identity token.
pub async fn login_with_minecraft(
    endpoints: &AuthEndpoints,
    user_hash: &str,
    xsts_token: &SecretString,
) -> Result<MinecraftTokenSet, MinecraftExchangeError> {
    let request = MinecraftLoginRequest {
        identity_token: format!("XBL3.0 x={user_hash};{}", xsts_token.expose()),
    };

    let (status, body) = send_json_post(endpoints.minecraft_login(), &request)
        .await
        .map_err(MinecraftExchangeError::Transport)?;

    if status != 200 {
        return Err(MinecraftExchangeError::ServiceRejected { status });
    }

    let wire: MinecraftLoginResponse =
        serde_json::from_slice(&body).map_err(|_| MinecraftExchangeError::Malformed)?;
    let bearer = wire.token_type.as_deref().unwrap_or("Bearer");
    if wire.access_token.is_empty()
        || !bearer.eq_ignore_ascii_case("bearer")
        || wire.expires_in < 0
        || wire.expires_in as u64 > MAX_TOKEN_LIFETIME_SECONDS
    {
        return Err(MinecraftExchangeError::Malformed);
    }

    Ok(MinecraftTokenSet {
        access_token: SecretString::new(wire.access_token),
        expires_in_seconds: wire.expires_in as u64,
    })
}

#[derive(Debug)]
pub enum EntitlementError {
    Transport(AuthTransportError),
    Malformed,
}

impl fmt::Display for EntitlementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => {
                write!(formatter, "the Minecraft entitlement check failed: {error}")
            }
            Self::Malformed => write!(
                formatter,
                "the Minecraft entitlement response was malformed and cannot be used"
            ),
        }
    }
}

impl std::error::Error for EntitlementError {}

#[derive(Deserialize)]
struct EntitlementsResponse {
    #[serde(default)]
    items: Vec<EntitlementItem>,
}

#[derive(Deserialize)]
struct EntitlementItem {
    name: String,
}

/// Fetches the account's entitlement item names (for example
/// `product_minecraft`, `game_minecraft`).
pub async fn fetch_entitlement_items(
    endpoints: &AuthEndpoints,
    minecraft_token: &SecretString,
) -> Result<Vec<String>, EntitlementError> {
    let (status, body) = send_bearer_get(endpoints.minecraft_entitlements(), minecraft_token)
        .await
        .map_err(EntitlementError::Transport)?;

    if status != 200 {
        return Err(EntitlementError::Transport(
            AuthTransportError::HttpStatus { status },
        ));
    }

    let wire: EntitlementsResponse =
        serde_json::from_slice(&body).map_err(|_| EntitlementError::Malformed)?;
    if wire.items.iter().any(|item| item.name.is_empty()) {
        return Err(EntitlementError::Malformed);
    }

    Ok(wire.items.into_iter().map(|item| item.name).collect())
}

/// Whether the entitlement items prove ownership of Minecraft: Java Edition.
///
/// Evidenced determination: `product_minecraft` (the store product) or
/// `game_minecraft` (the game license). Other item names (Bedrock, Dungeons,
/// Game Pass variants) do not by themselves establish Java ownership and are
/// deliberately not guessed into entitlement.
pub fn owns_minecraft(items: &[String]) -> bool {
    items
        .iter()
        .any(|name| name == "product_minecraft" || name == "game_minecraft")
}

#[derive(Debug)]
pub enum ProfileError {
    Transport(AuthTransportError),
    Malformed,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => {
                write!(formatter, "the Minecraft profile request failed: {error}")
            }
            Self::Malformed => write!(
                formatter,
                "the Minecraft profile response was malformed and cannot be used"
            ),
        }
    }
}

impl std::error::Error for ProfileError {}

/// The normalized Minecraft profile document: undashed lowercase UUID and a
/// validated name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftProfileDocument {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct MinecraftProfileResponse {
    id: String,
    name: String,
}

/// Fetches the authenticated profile.
///
/// Returns `Ok(None)` when the service answers 404 — the documented
/// "no profile exists" signal (an account that owns Minecraft but has never
/// created a profile).
pub async fn fetch_profile(
    endpoints: &AuthEndpoints,
    minecraft_token: &SecretString,
) -> Result<Option<MinecraftProfileDocument>, ProfileError> {
    let (status, body) = send_bearer_get(endpoints.minecraft_profile(), minecraft_token)
        .await
        .map_err(ProfileError::Transport)?;

    if status == 404 {
        return Ok(None);
    }
    if status != 200 {
        return Err(ProfileError::Transport(AuthTransportError::HttpStatus {
            status,
        }));
    }

    let wire: MinecraftProfileResponse =
        serde_json::from_slice(&body).map_err(|_| ProfileError::Malformed)?;

    let id = wire.id.trim().to_ascii_lowercase();
    let name = wire.name.trim();
    super::accounts::AccountId::validate(&id).map_err(|_| ProfileError::Malformed)?;
    super::accounts::validate_minecraft_name(name).map_err(|_| ProfileError::Malformed)?;

    Ok(Some(MinecraftProfileDocument {
        id,
        name: name.to_owned(),
    }))
}

async fn send_bearer_get(
    url: &Url,
    token: &SecretString,
) -> Result<(u16, Vec<u8>), AuthTransportError> {
    send(
        auth_client()
            .get(url.clone())
            .header("Authorization", format!("Bearer {}", token.expose()))
            .header("Accept", "application/json"),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestResponse, TestServer};
    use std::sync::Arc;

    fn token_body(access: &str, refresh: &str) -> String {
        format!(
            r#"{{"access_token":"{access}","refresh_token":"{refresh}","expires_in":3600,"token_type":"Bearer"}}"#
        )
    }

    #[tokio::test]
    async fn official_endpoints_are_pinned_to_https_authorities() {
        let endpoints = AuthEndpoints::official();

        for url in [
            endpoints.authorize(),
            endpoints.token(),
            endpoints.xbox_user_auth(),
            endpoints.xsts(),
            endpoints.minecraft_login(),
            endpoints.minecraft_entitlements(),
            endpoints.minecraft_profile(),
        ] {
            assert_eq!(url.scheme(), "https", "{} must be HTTPS", url);
        }
    }

    #[tokio::test]
    async fn authorization_code_exchange_sends_the_public_client_grant() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<TestRequest2>::new()));
        let seen = Arc::clone(&requests);
        let server = TestServer::spawn(Arc::new(move |request| {
            seen.lock().unwrap().push(TestRequest2 {
                method: request.method.clone(),
                path: request.path.clone(),
                body: String::from_utf8_lossy(&request.body).into_owned(),
            });
            TestResponse::ok(token_body("FIXTURE-MSA-ACCESS", "FIXTURE-MSA-REFRESH").as_bytes())
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let tokens = request_microsoft_token(
            &endpoints,
            "aurora-test-client-id",
            TokenGrant::AuthorizationCode {
                code: "FIXTURE-AUTH-CODE",
                code_verifier: "FIXTURE-VERIFIER-012345678901234567890123456789",
                redirect_uri: "http://localhost:49152/callback",
            },
        )
        .await
        .unwrap();

        assert_eq!(tokens.access_token.expose(), "FIXTURE-MSA-ACCESS");
        assert_eq!(tokens.refresh_token.expose(), "FIXTURE-MSA-REFRESH");
        assert_eq!(tokens.expires_in_seconds, 3600);

        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, "POST");
        let form = &sent[0].body;
        assert!(form.contains("grant_type=authorization_code"));
        assert!(form.contains("code=FIXTURE-AUTH-CODE"));
        assert!(form.contains("code_verifier=FIXTURE-VERIFIER"));
        assert!(form.contains("client_id=aurora-test-client-id"));
        assert!(form.contains("redirect_uri=http%3A%2F%2Flocalhost%3A49152%2Fcallback"));
        assert!(form.contains("scope=XboxLive.signin+offline_access"));
        assert!(!form.contains("client_secret"));
    }

    #[tokio::test]
    async fn refresh_grant_sends_the_refresh_token_without_a_secret() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<TestRequest2>::new()));
        let seen = Arc::clone(&requests);
        let server = TestServer::spawn(Arc::new(move |request| {
            seen.lock().unwrap().push(TestRequest2 {
                method: request.method.clone(),
                path: request.path.clone(),
                body: String::from_utf8_lossy(&request.body).into_owned(),
            });
            TestResponse::ok(token_body("FIXTURE-MSA-ACCESS-2", "FIXTURE-MSA-REFRESH-2").as_bytes())
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let tokens = request_microsoft_token(
            &endpoints,
            "aurora-test-client-id",
            TokenGrant::RefreshToken {
                refresh_token: &SecretString::new("FIXTURE-MSA-REFRESH-1"),
            },
        )
        .await
        .unwrap();

        assert_eq!(tokens.refresh_token.expose(), "FIXTURE-MSA-REFRESH-2");

        let form = requests.lock().unwrap()[0].body.clone();
        assert!(form.contains("grant_type=refresh_token"));
        assert!(form.contains("refresh_token=FIXTURE-MSA-REFRESH-1"));
        assert!(!form.contains("client_secret"));
    }

    #[tokio::test]
    async fn token_exchange_failures_map_to_structured_errors() {
        struct Case {
            status: u16,
            body: String,
            expected: fn(&TokenExchangeError) -> bool,
        }

        let cases = vec![
            Case {
                status: 400,
                body: r#"{"error":"invalid_grant","error_description":"token expired"}"#.to_owned(),
                expected: |error| {
                    matches!(
                        error,
                        TokenExchangeError::Rejected(rejection)
                            if rejection.error == "invalid_grant"
                            && rejection.description.as_deref() == Some("token expired")
                    )
                },
            },
            Case {
                status: 400,
                body: "{}".to_owned(),
                expected: |error| {
                    matches!(
                        error,
                        TokenExchangeError::Rejected(rejection) if rejection.error == "http_400"
                    )
                },
            },
            Case {
                status: 200,
                body: "{ not json".to_owned(),
                expected: |error| matches!(error, TokenExchangeError::Malformed),
            },
            Case {
                status: 200,
                body: r#"{"access_token":"x","expires_in":3600,"token_type":"Bearer"}"#.to_owned(),
                expected: |error| matches!(error, TokenExchangeError::MissingRefreshToken),
            },
            Case {
                status: 200,
                body: token_body("x", "y").replace("Bearer", "Basic"),
                expected: |error| matches!(error, TokenExchangeError::Malformed),
            },
        ];

        for case in cases {
            let (status, body, expected) = (case.status, case.body, case.expected);
            let server = TestServer::spawn(Arc::new(move |_| TestResponse {
                status,
                body: body.clone().into_bytes(),
                ..TestResponse::ok(&[])
            }));
            let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

            let error = request_microsoft_token(
                &endpoints,
                "aurora-test-client-id",
                TokenGrant::RefreshToken {
                    refresh_token: &SecretString::new("r"),
                },
            )
            .await
            .unwrap_err();

            assert!(
                expected(&error),
                "unexpected error for status {status}: {error}"
            );
        }
    }

    #[tokio::test]
    async fn token_rejection_messages_never_quote_the_response_body_verbatim() {
        let long_detail = "x".repeat(600);
        let body = format!(r#"{{"error":"invalid_grant","error_description":"{long_detail}"}}"#);
        let server = TestServer::spawn(Arc::new(move |_| {
            TestResponse::ok(body.as_bytes()).with_status(400)
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let error = request_microsoft_token(
            &endpoints,
            "aurora-test-client-id",
            TokenGrant::RefreshToken {
                refresh_token: &SecretString::new("r"),
            },
        )
        .await
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("invalid_grant"));
        assert!(!message.contains(&long_detail));
    }

    #[tokio::test]
    async fn oversized_token_responses_are_rejected() {
        let server = TestServer::spawn(Arc::new(|_| {
            let mut response = TestResponse::ok(&[]);
            response.body = vec![b'x'; MAX_AUTH_RESPONSE_BYTES + 1];
            response
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let error = request_microsoft_token(
            &endpoints,
            "aurora-test-client-id",
            TokenGrant::RefreshToken {
                refresh_token: &SecretString::new("r"),
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(
            error,
            TokenExchangeError::Transport(AuthTransportError::ResponseTooLarge { .. })
        ));
    }

    #[derive(Debug, Clone)]
    struct TestRequest2 {
        method: String,
        path: String,
        body: String,
    }

    fn xbox_success_body() -> String {
        r#"{"IssueInstant":"2026-01-01T00:00:00Z","NotAfter":"2026-01-15T00:00:00Z","Token":"FIXTURE-XBOX-TOKEN","DisplayClaims":{"xui":[{"uhs":"1234567890"}]}}"#.to_owned()
    }

    #[tokio::test]
    async fn xbox_exchange_sends_the_documented_rps_payload() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<TestRequest2>::new()));
        let seen = Arc::clone(&requests);
        let server = TestServer::spawn(Arc::new(move |request| {
            seen.lock().unwrap().push(TestRequest2 {
                method: request.method.clone(),
                path: request.path.clone(),
                body: String::from_utf8_lossy(&request.body).into_owned(),
            });
            TestResponse::ok(xbox_success_body().as_bytes())
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let xbox = authenticate_xbox(&endpoints, &SecretString::new("FIXTURE-MSA-ACCESS"))
            .await
            .unwrap();

        assert_eq!(xbox.token.expose(), "FIXTURE-XBOX-TOKEN");
        assert_eq!(xbox.user_hash, "1234567890");

        let sent = requests.lock().unwrap()[0].clone();
        assert_eq!(sent.path, "/user/authenticate");
        assert!(sent.body.contains(r#""AuthMethod":"RPS""#));
        assert!(sent.body.contains(r#""SiteName":"user.auth.xboxlive.com""#));
        assert!(sent.body.contains(r#""RpsTicket":"d=FIXTURE-MSA-ACCESS""#));
        assert!(
            sent.body
                .contains(r#""RelyingParty":"http://auth.xboxlive.com""#)
        );
        assert!(sent.body.contains(r#""TokenType":"JWT""#));
    }

    #[tokio::test]
    async fn xbox_failures_map_to_structured_errors() {
        let cases: Vec<(u16, String, fn(&XboxExchangeError) -> bool)> = vec![
            (400, "not json".to_owned(), |error| {
                matches!(error, XboxExchangeError::ServiceRejected { status: 400 })
            }),
            (
                200,
                r#"{"Token":"","DisplayClaims":{"xui":[{"uhs":"x"}]}}"#.to_owned(),
                |error| matches!(error, XboxExchangeError::Malformed),
            ),
            (
                200,
                r#"{"Token":"tok","DisplayClaims":{"xui":[]}}"#.to_owned(),
                |error| matches!(error, XboxExchangeError::Malformed),
            ),
            (
                200,
                r#"{"Token":"tok","DisplayClaims":{"xui":[{"uhs":""}]}}"#.to_owned(),
                |error| matches!(error, XboxExchangeError::Malformed),
            ),
        ];

        for (status, body, expected) in cases {
            let server = TestServer::spawn(Arc::new(move |_| TestResponse {
                status,
                body: body.clone().into_bytes(),
                ..TestResponse::ok(&[])
            }));
            let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

            let error = authenticate_xbox(&endpoints, &SecretString::new("msa"))
                .await
                .unwrap_err();
            assert!(expected(&error), "unexpected: {error}");
        }
    }

    #[tokio::test]
    async fn xsts_exchange_sends_the_documented_payload_and_parses_denials() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<TestRequest2>::new()));
        let seen = Arc::clone(&requests);
        let server = TestServer::spawn(Arc::new(move |request| {
            seen.lock().unwrap().push(TestRequest2 {
                method: request.method.clone(),
                path: request.path.clone(),
                body: String::from_utf8_lossy(&request.body).into_owned(),
            });
            TestResponse::ok(
                r#"{"Token":"FIXTURE-XSTS-TOKEN","DisplayClaims":{"xui":[{"uhs":"1234567890"}]}}"#
                    .as_bytes(),
            )
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let xbox = XboxToken {
            token: SecretString::new("FIXTURE-XBOX-TOKEN"),
            user_hash: "1234567890".to_owned(),
        };
        let xsts = authorize_xsts(&endpoints, &xbox).await.unwrap();

        assert_eq!(xsts.token.expose(), "FIXTURE-XSTS-TOKEN");
        assert_eq!(xsts.user_hash, "1234567890");

        let sent = requests.lock().unwrap()[0].clone();
        assert_eq!(sent.path, "/xsts/authorize");
        assert!(sent.body.contains(r#""SandboxId":"RETAIL""#));
        assert!(sent.body.contains(r#""UserTokens":["FIXTURE-XBOX-TOKEN"]"#));
        assert!(
            sent.body
                .contains(r#""RelyingParty":"rp://api.minecraftservices.com/""#)
        );

        // Denial with XErr.
        let denied = TestServer::spawn(Arc::new(|_| TestResponse {
            status: 401,
            body: br#"{"Identity":"X","XErr":2148916233,"Message":""}"#.to_vec(),
            ..TestResponse::ok(&[])
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(denied.base_url());
        let error = authorize_xsts(&endpoints, &xbox).await.unwrap_err();
        assert!(matches!(
            error,
            XstsExchangeError::Denied { xerr: 2148916233 }
        ));

        // 401 without a parsable XErr stays generic.
        let opaque = TestServer::spawn(Arc::new(|_| TestResponse::status(401)));
        let endpoints = AuthEndpoints::loopback_for_testing(opaque.base_url());
        let error = authorize_xsts(&endpoints, &xbox).await.unwrap_err();
        assert!(matches!(
            error,
            XstsExchangeError::ServiceRejected { status: 401 }
        ));
    }

    fn minecraft_login_body() -> String {
        r#"{"username":"986fec0ea8b44e1985f2a1c551f2b8b5","roles":[],"access_token":"FIXTURE-MC-TOKEN","token_type":"Bearer","expires_in":86400}"#.to_owned()
    }

    #[tokio::test]
    async fn minecraft_login_sends_the_identity_token_and_parses_expiry() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<TestRequest2>::new()));
        let seen = Arc::clone(&requests);
        let server = TestServer::spawn(Arc::new(move |request| {
            seen.lock().unwrap().push(TestRequest2 {
                method: request.method.clone(),
                path: request.path.clone(),
                body: String::from_utf8_lossy(&request.body).into_owned(),
            });
            TestResponse::ok(minecraft_login_body().as_bytes())
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let tokens = login_with_minecraft(
            &endpoints,
            "1234567890",
            &SecretString::new("FIXTURE-XSTS-TOKEN"),
        )
        .await
        .unwrap();

        assert_eq!(tokens.access_token.expose(), "FIXTURE-MC-TOKEN");
        assert_eq!(tokens.expires_in_seconds, 86400);

        let sent = requests.lock().unwrap()[0].clone();
        assert_eq!(sent.path, "/authentication/login_with_xbox");
        assert!(
            sent.body
                .contains(r#""identityToken":"XBL3.0 x=1234567890;FIXTURE-XSTS-TOKEN""#)
        );
    }

    #[tokio::test]
    async fn minecraft_login_failures_map_to_structured_errors() {
        for (status, body) in [(403, "{}".to_owned()), (200, "not json".to_owned())] {
            let server = TestServer::spawn(Arc::new(move |_| TestResponse {
                status,
                body: body.clone().into_bytes(),
                ..TestResponse::ok(&[])
            }));
            let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

            let error = login_with_minecraft(&endpoints, "uhs", &SecretString::new("xsts"))
                .await
                .unwrap_err();

            let message = error.to_string();
            assert!(!message.is_empty());
            match status {
                403 => assert!(matches!(
                    error,
                    MinecraftExchangeError::ServiceRejected { status: 403 }
                )),
                _ => assert!(matches!(error, MinecraftExchangeError::Malformed)),
            }
        }
    }

    #[tokio::test]
    async fn entitlements_are_fetched_with_bearer_and_evaluated_exactly() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<TestRequest2>::new()));
        let seen = Arc::clone(&requests);
        let server = TestServer::spawn(Arc::new(move |request| {
            seen.lock().unwrap().push(TestRequest2 {
                method: request.method.clone(),
                path: request.path.clone(),
                body: String::from_utf8_lossy(&request.body).into_owned(),
            });
            TestResponse::ok(
                r#"{"items":[{"name":"product_minecraft"},{"name":"product_minecraft_bedrock"}],"signature":"sig","keyId":"1"}"#.as_bytes(),
            )
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let items = fetch_entitlement_items(&endpoints, &SecretString::new("FIXTURE-MC-TOKEN"))
            .await
            .unwrap();

        assert_eq!(
            items,
            vec![
                "product_minecraft".to_owned(),
                "product_minecraft_bedrock".to_owned()
            ]
        );
        assert!(owns_minecraft(&items));

        assert_eq!(requests.lock().unwrap()[0].path, "/entitlements/mcstore");

        assert!(!owns_minecraft(&[
            "product_minecraft_bedrock".to_owned(),
            "product_game_pass_pc".to_owned()
        ]));
        assert!(!owns_minecraft(&[]));
        assert!(owns_minecraft(&["game_minecraft".to_owned()]));
    }

    #[tokio::test]
    async fn profiles_are_normalized_and_missing_profiles_are_distinguishable() {
        let server = TestServer::spawn(Arc::new(|request| {
            if request.path.starts_with("/minecraft/profile") {
                TestResponse::ok(
                    br#"{"id":"986DEC87B7EC47FF89FF033FDB95C4B5","name":"HowDoesAuthWork","skins":[],"capes":[]}"#,
                )
            } else {
                TestResponse::status(404)
            }
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(server.base_url());

        let profile = fetch_profile(&endpoints, &SecretString::new("FIXTURE-MC-TOKEN"))
            .await
            .unwrap()
            .expect("profile must parse");
        assert_eq!(profile.id, "986dec87b7ec47ff89ff033fdb95c4b5");
        assert_eq!(profile.name, "HowDoesAuthWork");

        let missing = TestServer::spawn(Arc::new(|_| TestResponse {
            status: 404,
            body: br#"{"error":"NOT_FOUND","errorMessage":"profile does not exist"}"#.to_vec(),
            ..TestResponse::ok(&[])
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(missing.base_url());
        assert_eq!(
            fetch_profile(&endpoints, &SecretString::new("FIXTURE-MC-TOKEN"))
                .await
                .unwrap(),
            None
        );

        let malformed = TestServer::spawn(Arc::new(|_| {
            TestResponse::ok(br#"{"id":"not-a-uuid","name":"HowDoesAuthWork"}"#)
        }));
        let endpoints = AuthEndpoints::loopback_for_testing(malformed.base_url());
        assert!(matches!(
            fetch_profile(&endpoints, &SecretString::new("FIXTURE-MC-TOKEN"))
                .await
                .unwrap_err(),
            ProfileError::Malformed
        ));
    }
}
