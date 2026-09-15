//! The in-memory authenticated Minecraft session.
//!
//! A session is Rust-owned: the Minecraft access token it holds is exposed
//! only within the native authentication boundary for future launch
//! assembly, never through frontend DTOs, events, or persisted JSON.
//! Sessions are regenerated on demand from the persisted refresh credential;
//! nothing here is persisted.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use tokio::time::Instant;

use super::credentials::SecretString;

/// Clock-skew margin subtracted from token lifetimes when deciding whether a
/// session is still usable, so expiry is noticed before the server enforces
/// it rather than through a launch-time failure.
const EXPIRY_SKEW: Duration = Duration::from_secs(300);

/// The normalized Minecraft profile Aurora needs — identity only; skins and
/// capes are ignored deliberately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftProfile {
    /// The profile UUID in canonical undashed lowercase-hex form.
    uuid: String,
    name: String,
}

impl MinecraftProfile {
    pub fn new(uuid: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            uuid: uuid.into(),
            name: name.into(),
        }
    }

    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// One usable Minecraft session.
#[derive(Clone)]
pub struct MinecraftSession {
    account_id: String,
    profile: MinecraftProfile,
    minecraft_access_token: SecretString,
    expires_at: Instant,
}

impl MinecraftSession {
    /// Builds a session with an explicit lifetime (seconds) as reported by
    /// the Minecraft Services token response.
    pub fn new(
        account_id: impl Into<String>,
        profile: MinecraftProfile,
        minecraft_access_token: SecretString,
        lifetime: Duration,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            profile,
            minecraft_access_token,
            expires_at: Instant::now() + lifetime,
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn profile(&self) -> &MinecraftProfile {
        &self.profile
    }

    /// Whether the session may still be used, honoring the skew margin.
    pub fn usable(&self) -> bool {
        Instant::now() + EXPIRY_SKEW < self.expires_at
    }

    /// The Minecraft access token. Native launch assembly (a future phase)
    /// is the only intended consumer; this is never serialized or emitted.
    pub fn minecraft_access_token(&self) -> &SecretString {
        &self.minecraft_access_token
    }
}

impl std::fmt::Debug for MinecraftSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is a SecretString and redacts itself; be explicit anyway
        // that sessions never render their credential.
        formatter
            .debug_struct("MinecraftSession")
            .field("account_id", &self.account_id)
            .field("profile", &self.profile)
            .field("minecraft_access_token", &"[redacted session token]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Process-wide session cache: at most one live session per account.
///
/// Refresh happens on demand at the flow boundary; this type is plain
/// storage with no background work.
#[derive(Default)]
pub struct SessionCache {
    sessions: RwLock<HashMap<String, Arc<MinecraftSession>>>,
}

fn cache() -> &'static SessionCache {
    static CACHE: OnceLock<SessionCache> = OnceLock::new();
    CACHE.get_or_init(SessionCache::default)
}

impl SessionCache {
    /// Returns the cached session for an account when one exists and is
    /// still usable.
    pub fn usable(account_id: &str) -> Option<Arc<MinecraftSession>> {
        let sessions = cache()
            .sessions
            .read()
            .expect("session cache is not poisoned");
        sessions
            .get(account_id)
            .filter(|session| session.usable())
            .cloned()
    }

    pub fn put(session: MinecraftSession) -> Arc<MinecraftSession> {
        let mut sessions = cache()
            .sessions
            .write()
            .expect("session cache is not poisoned");
        let shared = Arc::new(session);
        sessions.insert(shared.account_id().to_owned(), Arc::clone(&shared));
        shared
    }

    pub fn remove(account_id: &str) {
        let mut sessions = cache()
            .sessions
            .write()
            .expect("session cache is not poisoned");
        sessions.remove(account_id);
    }

    pub fn clear() {
        let mut sessions = cache()
            .sessions
            .write()
            .expect("session cache is not poisoned");
        sessions.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session(lifetime: Duration) -> MinecraftSession {
        MinecraftSession::new(
            "986dec87b7ec47ff89ff033fdb95c4b5",
            MinecraftProfile::new("986dec87b7ec47ff89ff033fdb95c4b5", "HowDoesAuthWork"),
            SecretString::new("FIXTURE-MINECRAFT-TOKEN"),
            lifetime,
        )
    }

    #[test]
    fn sessions_expire_with_the_skew_margin() {
        // A lifetime shorter than the skew margin is already unusable.
        assert!(!sample_session(Duration::from_secs(1)).usable());
        assert!(!sample_session(EXPIRY_SKEW).usable());
        assert!(sample_session(EXPIRY_SKEW + Duration::from_secs(60)).usable());
    }

    #[test]
    fn session_debug_output_never_contains_the_token() {
        let rendered = format!("{:?}", sample_session(Duration::from_secs(3600)));

        assert!(!rendered.contains("FIXTURE-MINECRAFT-TOKEN"));
        assert!(rendered.contains("[redacted session token]"));
        assert!(rendered.contains("HowDoesAuthWork"));
    }

    #[tokio::test]
    async fn expired_sessions_leave_the_cache() {
        SessionCache::clear();
        let account_id = "986dec87b7ec47ff89ff033fdb95c4b5";

        SessionCache::put(sample_session(Duration::from_secs(0)));
        // Already expired: not reported as usable, even though stored.
        assert!(SessionCache::usable(account_id).is_none());

        SessionCache::put(sample_session(Duration::from_secs(3600)));
        assert!(SessionCache::usable(account_id).is_some());

        SessionCache::remove(account_id);
        assert!(SessionCache::usable(account_id).is_none());
    }
}
