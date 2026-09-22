//! Public-client OAuth 2.0 building blocks for Microsoft sign-in.
//!
//! Aurora is a desktop public client: it embeds no secret, proves itself
//! with PKCE (S256), and sends the user to the system browser. Everything in
//! this module is pure — endpoint URLs, randomness, and query parsing — so it
//! is fully testable without a browser or a network.

use std::fmt;

use base64ct::Encoding as _;
use sha2::Digest as _;
use url::Url;

/// The OAuth scope that authorizes the Xbox Live user-authentication
/// exchange downstream; Minecraft accounts are consumer Microsoft accounts.
pub const SCOPE_XBOXLIVE_SIGNIN: &str = "XboxLive.signin";

/// The OAuth scope that makes the Microsoft identity platform issue a
/// refresh token; without it no persisted session restoration is possible.
pub const SCOPE_OFFLINE_ACCESS: &str = "offline_access";

/// The complete scope set Aurora requests.
pub const OAUTH_SCOPES: &str = "XboxLive.signin offline_access";

/// Number of random bytes behind one PKCE verifier (43 base64url
/// characters — the minimum legal length).
const PKCE_VERIFIER_BYTES: usize = 32;

/// Number of random bytes behind one OAuth `state` value.
const STATE_BYTES: usize = 32;

/// The exact Microsoft application (client) ID Aurora signs in with.
///
/// A client ID is public configuration, not a secret, but it must be Aurora's
/// own registration: borrowing another application's client ID is forbidden.
/// The value is resolved at the command boundary, never invented here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientConfig {
    client_id: String,
}

impl OAuthClientConfig {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }
}

/// A PKCE verifier plus its S256 challenge.
///
/// The verifier is a secret-adjacent single-use value; it lives only for the
/// duration of one login transaction and its `Debug` output is redacted.
#[derive(Clone)]
pub struct PkcePair {
    verifier: String,
    challenge: String,
}

impl PkcePair {
    /// Generates a new verifier from OS entropy and derives the S256
    /// challenge (`BASE64URL(SHA256(ASCII(verifier)))` per RFC 7636 §4.2).
    pub fn generate() -> Result<Self, OAuthEntropyError> {
        let mut bytes = [0u8; PKCE_VERIFIER_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| OAuthEntropyError)?;
        let verifier = base64ct::Base64UrlUnpadded::encode_string(&bytes);
        Self::from_verifier(verifier)
    }

    /// Adopts an existing verifier (tests) and derives its challenge.
    pub fn from_verifier(verifier: String) -> Result<Self, OAuthEntropyError> {
        let valid_length = (43..=128).contains(&verifier.len());
        let valid_charset = verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'));
        if !valid_length || !valid_charset {
            return Err(OAuthEntropyError);
        }

        let digest = sha2::Sha256::digest(verifier.as_bytes());
        let challenge = base64ct::Base64UrlUnpadded::encode_string(&digest);

        Ok(Self {
            verifier,
            challenge,
        })
    }

    /// The S256 challenge sent in the authorization request.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    /// The verifier sent only to the token endpoint. Exposed narrowly to the
    /// token-exchange call site; never logged, serialized, or persisted.
    pub(crate) fn verifier(&self) -> &str {
        &self.verifier
    }
}

impl fmt::Debug for PkcePair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PkcePair")
            .field("verifier", &"[redacted pkce verifier]")
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// OS entropy was unavailable; login material cannot be generated safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthEntropyError;

impl fmt::Display for OAuthEntropyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the operating system entropy source failed, so OAuth login material could not be generated safely"
        )
    }
}

impl std::error::Error for OAuthEntropyError {}

/// The anti-CSRF `state` value for one login transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthState(String);

impl OAuthState {
    pub fn generate() -> Result<Self, OAuthEntropyError> {
        let mut bytes = [0u8; STATE_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| OAuthEntropyError)?;
        Ok(Self(base64ct::Base64UrlUnpadded::encode_string(&bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for OAuthState {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Builds the authorization URL opened in the system browser.
///
/// Parameters follow the Microsoft identity platform authorization-code +
/// PKCE request exactly: no secret, no prompt manipulation, `query` response
/// mode for the loopback redirect receiver. Only the fixed scope set Aurora
/// needs is requested.
pub fn build_authorization_url(
    authorize_endpoint: &Url,
    config: &OAuthClientConfig,
    redirect_uri: &str,
    state: &OAuthState,
    pkce: &PkcePair,
) -> Url {
    let mut url = authorize_endpoint.clone();
    url.query_pairs_mut()
        .clear()
        .append_pair("response_type", "code")
        .append_pair("client_id", config.client_id())
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", OAUTH_SCOPES)
        .append_pair("state", state.as_str())
        .append_pair("code_challenge", pkce.challenge())
        .append_pair("code_challenge_method", "S256")
        .append_pair("response_mode", "query");
    url
}

/// The outcome of parsing one OAuth redirect callback query string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackQuery {
    /// The provider redirected with an authorization code.
    Code { code: String, state: String },
    /// The provider redirected with an error (for example the user declined
    /// the consent or cancelled sign-in in the browser).
    Failure {
        error: String,
        error_description: Option<String>,
    },
}

/// Parses the query string of an OAuth redirect callback.
///
/// Returns `None` for shapes that are neither a code nor an error response
/// (for example a bare request without parameters). Values are
/// percent-decoded once; duplicate keys use the first occurrence.
pub fn parse_callback_query(query: &str) -> Option<CallbackQuery> {
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    let first = |name: &str| -> Option<String> {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };

    if let Some(error) = first("error") {
        if !error.is_empty() {
            return Some(CallbackQuery::Failure {
                error,
                error_description: first("error_description").filter(|value| !value.is_empty()),
            });
        }
    }

    match (first("code"), first("state")) {
        (Some(code), Some(state)) if !code.is_empty() && !state.is_empty() => {
            Some(CallbackQuery::Code { code, state })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256_base64url(bytes: &[u8]) -> String {
        base64ct::Base64UrlUnpadded::encode_string(&sha2::Sha256::digest(bytes))
    }

    #[test]
    fn generated_verifiers_are_legal_and_challenges_match_s256() {
        for _ in 0..16 {
            let pair = PkcePair::generate().unwrap();

            let verifier = pair.verifier().to_owned();
            assert!((43..=128).contains(&verifier.len()), "verifier length");
            assert!(
                verifier
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
            );
            assert_eq!(pair.challenge(), sha256_base64url(verifier.as_bytes()));
        }
    }

    #[test]
    fn generated_challenges_differ_between_pairs() {
        let first = PkcePair::generate().unwrap();
        let second = PkcePair::generate().unwrap();

        assert_ne!(first.challenge(), second.challenge());
        assert_ne!(first.verifier(), second.verifier());
    }

    #[test]
    fn known_verifier_derives_the_documented_s256_challenge() {
        // RFC 7636 appendix B vector.
        let pair =
            PkcePair::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned())
                .unwrap();

        assert_eq!(
            pair.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn illegal_verifiers_are_rejected() {
        for verifier in [
            String::new(),
            "short".to_owned(),
            "contains spaces and illegal characters !!!".to_owned(),
            "x".repeat(42),
            "x".repeat(129),
        ] {
            assert!(PkcePair::from_verifier(verifier).is_err());
        }
    }

    #[test]
    fn pkce_debug_output_never_contains_the_verifier() {
        let pair = PkcePair::generate().unwrap();
        let rendered = format!("{pair:?}");

        assert!(rendered.contains("[redacted pkce verifier]"));
        assert!(!rendered.contains(pair.verifier()));
    }

    #[test]
    fn generated_states_are_distinct_and_reasonably_long() {
        let first = OAuthState::generate().unwrap();
        let second = OAuthState::generate().unwrap();

        assert_ne!(first.as_str(), second.as_str());
        assert!(first.as_str().len() >= 32);
    }

    #[test]
    fn authorization_url_carries_exactly_the_public_client_parameters() {
        let endpoint = Url::parse("https://login.example.invalid/authorize").unwrap();
        let config = OAuthClientConfig::new("aurora-test-client-id");
        let state = OAuthState::from("state-value".to_owned());
        let pkce =
            PkcePair::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned())
                .unwrap();

        let url =
            build_authorization_url(&endpoint, &config, "http://localhost:49152/", &state, &pkce);

        assert_eq!(
            url.as_str(),
            "https://login.example.invalid/authorize?response_type=code&client_id=aurora-test-client-id&redirect_uri=http%3A%2F%2Flocalhost%3A49152%2F&scope=XboxLive.signin+offline_access&state=state-value&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256&response_mode=query"
        );

        // No secret parameter ever appears.
        assert!(!url.as_str().contains("client_secret"));
    }

    #[test]
    fn callback_queries_parse_codes_and_errors() {
        let code = parse_callback_query("code=M.C5_BLAH&state=abc").unwrap();
        assert_eq!(
            code,
            CallbackQuery::Code {
                code: "M.C5_BLAH".to_owned(),
                state: "abc".to_owned(),
            }
        );

        let failure =
            parse_callback_query("error=access_denied&error_description=user+cancelled").unwrap();
        assert_eq!(
            failure,
            CallbackQuery::Failure {
                error: "access_denied".to_owned(),
                error_description: Some("user cancelled".to_owned()),
            }
        );
    }

    #[test]
    fn callback_query_values_are_percent_decoded() {
        let code = parse_callback_query("code=M.C5%2FGx&state=a%20b").unwrap();

        assert_eq!(
            code,
            CallbackQuery::Code {
                code: "M.C5/Gx".to_owned(),
                state: "a b".to_owned(),
            }
        );
    }

    #[test]
    fn empty_or_partial_callback_queries_are_rejected() {
        assert_eq!(parse_callback_query(""), None);
        assert_eq!(parse_callback_query("code=only-code"), None);
        assert_eq!(parse_callback_query("state=only-state"), None);
        assert_eq!(parse_callback_query("code=&state=x"), None);
        assert_eq!(parse_callback_query("code=x&state="), None);
        assert_eq!(parse_callback_query("foo=bar"), None);
    }

    #[test]
    fn error_callback_takes_precedence_over_a_stray_code() {
        let failure = parse_callback_query("error=access_denied&code=stray").unwrap();

        assert!(matches!(failure, CallbackQuery::Failure { .. }));
    }

    #[test]
    fn empty_error_values_do_not_masquerade_as_failures() {
        let parsed = parse_callback_query("error=&code=real-code&state=st");

        assert_eq!(
            parsed,
            Some(CallbackQuery::Code {
                code: "real-code".to_owned(),
                state: "st".to_owned(),
            })
        );
    }
}
