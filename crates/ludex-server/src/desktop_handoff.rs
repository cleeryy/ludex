//! Desktop SSO handoff (#157).
//!
//! The desktop app cannot receive an OIDC redirect (no loopback listener, no
//! deep links), so the browser hands the session back through a short-lived
//! one-time code:
//!
//! 1. the desktop generates a random `state`, opens
//!    `{server}/#/login?desktop_state=<state>` in the system browser and starts
//!    polling the claim endpoint;
//! 2. the web page signs the user in (SSO or password) and, while
//!    authenticated, calls `POST /api/auth/desktop-handoff { state }`;
//! 3. the server stores `{ user_id, code }` under that state for two minutes
//!    and returns only the expiry;
//! 4. the desktop claims the state once and receives `{ code }`, then redeems
//!    it through the existing `POST /api/auth/oidc/exchange` — the only place a
//!    JWT is ever issued.
//!
//! Nothing is persisted: a restart drops every pending handoff. Neither the
//! state nor the code is ever logged, and no response of this module contains a
//! token.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use subtle::ConstantTimeEq;

use crate::error::ApiError;

/// Default lifetime of a pending desktop handoff.
pub const DEFAULT_TTL: Duration = Duration::from_secs(120);
/// Lifetime of a one-time code minted by the web OIDC callback.
pub const EXCHANGE_CODE_TTL: Duration = Duration::from_secs(60);
/// Maximum pending handoff entries kept in memory.
pub const MAX_HANDOFFS: usize = 1024;
/// Maximum pending exchange codes kept in memory.
const MAX_CODES: usize = 1024;
/// Shortest accepted `state`: 32 random bytes in base64url (43 characters).
pub const MIN_STATE_LEN: usize = 43;
/// Longest accepted `state`: 32 random bytes in hex (64) plus headroom.
pub const MAX_STATE_LEN: usize = 128;

/// Generates a fresh 32-byte state, base64url-encoded (43 characters).
#[must_use]
pub fn random_state() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generates a fresh 32-byte one-time code, base64url-encoded.
#[must_use]
pub fn random_code() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Validates the `state` submitted by the desktop.
///
/// The value is only ever used as an opaque key: it must look like high-entropy
/// randomness (length plus alphabet), which keeps a guessing attack against the
/// claim endpoint out of reach.
pub fn validate_state(value: &str) -> Result<(), ApiError> {
    let mut bytes = value.bytes();
    let acceptable = bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !acceptable || !(MIN_STATE_LEN..=MAX_STATE_LEN).contains(&value.len()) {
        return Err(ApiError::bad_request(
            "invalid_handoff_state",
            format!(
                "The handoff state must be {MIN_STATE_LEN}-{MAX_STATE_LEN} characters of A-Z, a-z, 0-9, '-' or '_'"
            ),
        ));
    }
    Ok(())
}

/// Converts an in-memory deadline into the RFC 3339 value returned to clients.
#[must_use]
pub fn expiry_datetime(deadline: Instant) -> chrono::DateTime<chrono::Utc> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    chrono::Utc::now() + chrono::Duration::from_std(remaining).unwrap_or_default()
}

/// A one-time login exchange code.
struct ExchangeCode {
    code: String,
    user_id: i64,
    expires_at: Instant,
}

/// In-memory store of one-time login exchange codes.
///
/// Shared by the web OIDC callback (`oidc_code` in the SPA URL) and the desktop
/// handoff. Codes are single use, short lived and compared in constant time;
/// redeeming one is the only step that issues a JWT.
#[derive(Default)]
pub struct ExchangeCodes {
    codes: Mutex<Vec<ExchangeCode>>,
}

impl ExchangeCodes {
    /// Builds an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Issues a code for `user_id`, valid for `ttl`.
    pub fn issue(&self, user_id: i64, ttl: Duration) -> Result<String, ApiError> {
        let code = random_code();
        let expires_at = Instant::now() + ttl;
        let now = Instant::now();
        let mut codes = lock(&self.codes);
        // Lazy cleanup on every access, then a hard bound so a stalled client
        // can never grow the process unboundedly.
        codes.retain(|entry| entry.expires_at > now);
        if codes.len() >= MAX_CODES {
            return Err(ApiError::too_many_requests(
                "sso_busy",
                "Too many pending sign-ins; try again shortly",
            ));
        }
        codes.push(ExchangeCode {
            code: code.clone(),
            user_id,
            expires_at,
        });
        Ok(code)
    }

    /// Consumes a code. Returns `None` when it is unknown, expired or used.
    #[must_use]
    pub fn take(&self, code: &str) -> Option<i64> {
        let now = Instant::now();
        let mut codes = lock(&self.codes);
        codes.retain(|entry| entry.expires_at > now);
        let index = codes
            .iter()
            .position(|entry| bool::from(entry.code.as_bytes().ct_eq(code.as_bytes())))?;
        Some(codes.remove(index).user_id)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// One pending desktop handoff.
struct Handoff {
    state: String,
    code: String,
    user_id: i64,
    expires_at: Instant,
}

/// A handoff claimed exactly once by the desktop.
#[derive(Debug, Clone)]
pub struct ClaimedHandoff {
    /// One-time code to redeem at `POST /api/auth/oidc/exchange`.
    pub code: String,
    /// Account that completed the web login.
    pub user_id: i64,
    /// When the code stops working.
    pub expires_at: Instant,
}

/// In-memory handoff store keyed by the desktop-generated `state`.
///
/// The store never touches the database: a handoff only has to survive the two
/// minutes between the web login and the poll that follows it.
pub struct DesktopHandoffs {
    ttl: Duration,
    entries: Mutex<Vec<Handoff>>,
}

impl DesktopHandoffs {
    /// Builds an empty store with the given lifetime.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Lifetime of a newly created handoff.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Binds `code` to `state` for the caller.
    ///
    /// A second submission of the same state by the same account refreshes the
    /// entry (a retried web login must still work). A state already bound to
    /// another account is rejected: that is either a bug or an attempt to
    /// hijack a pending handoff.
    pub fn insert(&self, state: &str, user_id: i64, code: &str) -> Result<Instant, ApiError> {
        let now = Instant::now();
        let expires_at = now + self.ttl;
        let mut entries = lock(&self.entries);
        entries.retain(|entry| entry.expires_at > now);
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| bool::from(entry.state.as_bytes().ct_eq(state.as_bytes())))
        {
            if existing.user_id != user_id {
                return Err(ApiError::conflict(
                    "handoff_state_conflict",
                    "This sign-in state is already in use; start the sign-in again from Ludex",
                ));
            }
            existing.code = code.to_string();
            existing.expires_at = expires_at;
            return Ok(expires_at);
        }
        if entries.len() >= MAX_HANDOFFS {
            return Err(ApiError::too_many_requests(
                "handoff_busy",
                "Too many pending desktop sign-ins; try again shortly",
            ));
        }
        entries.push(Handoff {
            state: state.to_string(),
            code: code.to_string(),
            user_id,
            expires_at,
        });
        Ok(expires_at)
    }

    /// Consumes the handoff bound to `state`.
    ///
    /// Returns `None` for an unknown, expired or already-claimed state; the
    /// caller answers the same way in every case so the endpoint reveals
    /// nothing about pending sign-ins.
    #[must_use]
    pub fn claim(&self, state: &str) -> Option<ClaimedHandoff> {
        let now = Instant::now();
        let mut entries = lock(&self.entries);
        entries.retain(|entry| entry.expires_at > now);
        let index = entries
            .iter()
            .position(|entry| bool::from(entry.state.as_bytes().ct_eq(state.as_bytes())))?;
        let entry = entries.remove(index);
        Some(ClaimedHandoff {
            code: entry.code,
            user_id: entry.user_id,
            expires_at: entry.expires_at,
        })
    }

    /// Number of live entries. Used by tests and diagnostics only.
    #[must_use]
    pub fn len(&self) -> usize {
        let now = Instant::now();
        let mut entries = lock(&self.entries);
        entries.retain(|entry| entry.expires_at > now);
        entries.len()
    }

    /// Whether the store holds no live entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(label: &str) -> String {
        format!("{label:0<64}")
    }

    #[test]
    fn random_states_are_high_entropy_and_valid() {
        let first = random_state();
        let second = random_state();
        assert_eq!(first.len(), 43);
        assert_ne!(first, second);
        assert!(validate_state(&first).is_ok());
        // The desktop uses hex for the same 32 bytes.
        assert!(validate_state(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn state_validation_rejects_weak_values() {
        assert!(validate_state("").is_err());
        assert!(validate_state(&"a".repeat(MIN_STATE_LEN - 1)).is_err());
        assert!(validate_state(&"a".repeat(MAX_STATE_LEN + 1)).is_err());
        assert!(validate_state(&format!("{} ", "a".repeat(43))).is_err());
        assert!(validate_state(&format!("{}!", "a".repeat(42))).is_err());
        assert!(validate_state("é".repeat(43).as_str()).is_err());
    }

    #[test]
    fn exchange_codes_are_single_use() {
        let store = ExchangeCodes::new();
        let code = store.issue(7, EXCHANGE_CODE_TTL).expect("issue");
        assert_eq!(store.take(&code), Some(7));
        assert_eq!(store.take(&code), None);
        assert_eq!(store.take("unknown"), None);
    }

    #[test]
    fn expired_exchange_codes_are_rejected() {
        let store = ExchangeCodes::new();
        let code = store.issue(9, Duration::from_millis(1)).expect("issue");
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(store.take(&code), None);
    }

    #[test]
    fn handoff_is_claimed_once() {
        let store = DesktopHandoffs::new(DEFAULT_TTL);
        let key = state("one");
        store.insert(&key, 3, "code-a").expect("insert");
        let claimed = store.claim(&key).expect("claim");
        assert_eq!(claimed.code, "code-a");
        assert_eq!(claimed.user_id, 3);
        assert!(store.claim(&key).is_none());
        assert!(store.is_empty());
    }

    #[test]
    fn handoff_requires_the_exact_state() {
        let store = DesktopHandoffs::new(DEFAULT_TTL);
        let key = state("right");
        store.insert(&key, 3, "code-a").expect("insert");
        assert!(store.claim(&state("wrong")).is_none());
        assert_eq!(store.len(), 1);
        assert!(store.claim(&key).is_some());
    }

    #[test]
    fn handoff_expires() {
        let store = DesktopHandoffs::new(Duration::from_millis(1));
        store.insert(&state("old"), 3, "code-a").expect("insert");
        std::thread::sleep(Duration::from_millis(10));
        assert!(store.claim(&state("old")).is_none());
        assert!(store.is_empty());
    }

    #[test]
    fn same_account_can_retry_the_same_state() {
        let store = DesktopHandoffs::new(DEFAULT_TTL);
        let key = state("retry");
        store.insert(&key, 4, "first").expect("insert");
        store.insert(&key, 4, "second").expect("refresh");
        assert_eq!(store.len(), 1);
        assert_eq!(store.claim(&key).expect("claim").code, "second");
    }

    #[test]
    fn another_account_cannot_hijack_a_state() {
        let store = DesktopHandoffs::new(DEFAULT_TTL);
        let key = state("taken");
        store.insert(&key, 4, "first").expect("insert");
        let error = store.insert(&key, 5, "second").expect_err("conflict");
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(error.code, "handoff_state_conflict");
        assert_eq!(store.claim(&key).expect("claim").user_id, 4);
    }

    #[test]
    fn handoff_store_is_bounded() {
        let store = DesktopHandoffs::new(DEFAULT_TTL);
        for index in 0..MAX_HANDOFFS {
            store
                .insert(&format!("{index:0>64}"), 1, "code")
                .expect("insert");
        }
        let error = store
            .insert(&"z".repeat(64), 1, "code")
            .expect_err("bounded");
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }
}
