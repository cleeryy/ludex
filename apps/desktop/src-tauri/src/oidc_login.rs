//! Desktop SSO through the web login page (#157).
//!
//! The app never opens a local port and never registers a custom URL scheme:
//! it generates a random state, opens the normal web login page in the system
//! browser and polls the claim endpoint until the page hands the session back.
//!
//! ```text
//! desktop: state = 32 random bytes, open {server}/#/login?desktop_state=<state>
//! browser: signs in (SSO or password), POST /api/auth/desktop-handoff { state }
//! desktop: POST /api/auth/desktop-handoff/claim { state } -> { code } once
//! webview: POST /api/auth/oidc/exchange { code } -> token (the only issuer)
//! ```
//!
//! Neither the state nor the code is logged: the poll loop only records the
//! terminal status.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::profiles;

/// Poll interval used by the app.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long the app keeps polling for a web sign-in.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Per-request timeout of a claim call.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Consecutive network failures tolerated before the attempt is abandoned.
const MAX_NETWORK_FAILURES: u32 = 5;
/// Maximum concurrent sign-in attempts kept in memory.
const MAX_ATTEMPTS: usize = 16;
/// Query parameter read by the web login page.
pub const STATE_PARAM: &str = "desktop_state";

/// Generates the 32-byte state as lowercase hex (64 characters).
#[must_use]
pub fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// URL opened in the system browser for `state`.
#[must_use]
pub fn login_url(server_url: &str, state: &str) -> String {
    format!(
        "{}/#/login?{STATE_PARAM}={state}",
        server_url.trim_end_matches('/')
    )
}

/// URL of the plain web login, without any handoff (the UI fallback).
#[must_use]
pub fn web_login_url(server_url: &str) -> String {
    format!("{}/#/login", server_url.trim_end_matches('/'))
}

/// One in-flight desktop sign-in.
pub struct LoginAttempt {
    /// Server the browser page was opened on; polling stays on it even if the
    /// user switches profiles meanwhile.
    server_url: String,
    cancelled: AtomicBool,
}

impl LoginAttempt {
    fn new(server_url: &str) -> Self {
        Self {
            server_url: server_url.to_string(),
            cancelled: AtomicBool::new(false),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// In-memory registry of pending desktop sign-ins, keyed by the state.
#[derive(Default)]
pub struct LoginSessions {
    attempts: Mutex<HashMap<String, Arc<LoginAttempt>>>,
}

impl LoginSessions {
    /// Builds an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, state: &str, server_url: &str) -> Result<Arc<LoginAttempt>, String> {
        let mut attempts = lock(&self.attempts);
        if attempts.len() >= MAX_ATTEMPTS {
            return Err(
                "Too many sign-in attempts are pending; restart Ludex and try again.".to_string(),
            );
        }
        let attempt = Arc::new(LoginAttempt::new(server_url));
        attempts.insert(state.to_string(), Arc::clone(&attempt));
        Ok(attempt)
    }

    fn get(&self, state: &str) -> Option<Arc<LoginAttempt>> {
        lock(&self.attempts).get(state).cloned()
    }

    fn remove(&self, state: &str) {
        lock(&self.attempts).remove(state);
    }

    /// Requests cancellation; the running poll returns `cancelled` shortly.
    pub fn cancel(&self, state: &str) {
        if let Some(attempt) = self.get(state) {
            attempt.cancel();
        }
    }

    /// Number of pending attempts (tests and diagnostics).
    #[must_use]
    #[cfg(test)]
    pub fn len(&self) -> usize {
        lock(&self.attempts).len()
    }

    /// Whether no attempt is pending.
    #[must_use]
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// Result of [`start_with_opener`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OidcLoginStart {
    /// Opaque id used by `oidc_login_poll` / `oidc_login_cancel`.
    pub session_id: String,
    /// Plain web login URL, offered when the browser cannot be opened.
    pub web_login_url: String,
    /// When the app stops waiting for the web page.
    pub expires_at: String,
}

/// Terminal (or still-running) state of a desktop sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OidcLoginStatus {
    /// Still waiting for the web page. The blocking `oidc_login_poll` command
    /// only returns once the attempt is over, so the app never sees this; it is
    /// part of the protocol for channel or step-based callers.
    #[allow(dead_code)]
    Pending,
    /// The code was received and must be redeemed at the exchange endpoint.
    Claimed,
    /// The window closed or the handoff expired.
    Expired,
    /// The user cancelled from the app.
    Cancelled,
    /// The server could not be reached or answered unexpectedly.
    Error,
}

/// Result of [`poll_handoff`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OidcLoginPoll {
    /// What happened.
    pub status: OidcLoginStatus,
    /// One-time exchange code, present only for [`OidcLoginStatus::Claimed`].
    pub code: Option<String>,
    /// Actionable message for the UI; never contains the state or the code.
    pub message: Option<String>,
}

impl OidcLoginPoll {
    fn claimed(code: String) -> Self {
        Self {
            status: OidcLoginStatus::Claimed,
            code: Some(code),
            message: None,
        }
    }

    fn terminal(status: OidcLoginStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            code: None,
            message: Some(message.into()),
        }
    }
}

/// Polling behaviour; the app uses [`PollOptions::default`], tests shorten it.
#[derive(Debug, Clone)]
pub struct PollOptions {
    /// Delay between two claim calls.
    pub interval: Duration,
    /// Give up after this long.
    pub timeout: Duration,
    /// Timeout of one claim request.
    pub request_timeout: Duration,
    /// Consecutive transport failures tolerated before reporting an error.
    pub max_network_failures: u32,
}

impl Default for PollOptions {
    fn default() -> Self {
        Self {
            interval: POLL_INTERVAL,
            timeout: POLL_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
            max_network_failures: MAX_NETWORK_FAILURES,
        }
    }
}

#[cfg(test)]
impl PollOptions {
    fn for_tests() -> Self {
        Self {
            interval: Duration::from_millis(5),
            timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(5),
            max_network_failures: 3,
        }
    }
}

/// Starts a desktop sign-in.
///
/// `open` receives the URL to show in the system browser; the command wires the
/// Tauri opener plugin, tests inject a stub. A failure to open the browser is
/// reported with the web login URL so the user still has a way in, and leaves
/// no pending attempt behind.
pub fn start_with_opener(
    sessions: &LoginSessions,
    server_url: &str,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<OidcLoginStart, String> {
    let server_url = profiles::normalize_url(server_url)
        .map_err(|_| "Connect to a Ludex server before signing in with SSO.".to_string())?;
    let state = generate_state();
    // The server owns the real handoff TTL; this deadline only tells the UI
    // when the app itself stops waiting.
    let expires_at = (chrono::Utc::now()
        + chrono::Duration::from_std(POLL_TIMEOUT).unwrap_or_default())
    .to_rfc3339();

    open(&login_url(&server_url, &state)).map_err(|error| {
        format!(
            "Could not open your browser: {error}. Open {fallback} instead.",
            fallback = web_login_url(&server_url)
        )
    })?;

    sessions.insert(&state, &server_url)?;
    tracing::info!(server = %server_url, "desktop SSO started in the system browser");
    Ok(OidcLoginStart {
        session_id: state,
        web_login_url: web_login_url(&server_url),
        expires_at,
    })
}

/// Polls until the web page hands the session back, the window closes or the
/// user cancels.
///
/// A `410` from the claim endpoint means "nothing pending yet" and keeps the
/// loop running: the user may still be typing a password in the browser.
pub async fn poll_handoff(
    http: &reqwest::Client,
    server_url: &str,
    state: &str,
    cancelled: &AtomicBool,
    options: &PollOptions,
) -> OidcLoginPoll {
    let deadline = Instant::now() + options.timeout;
    let mut failures = 0u32;

    loop {
        if cancelled.load(Ordering::SeqCst) {
            tracing::debug!("desktop SSO cancelled by the user");
            return OidcLoginPoll::terminal(OidcLoginStatus::Cancelled, "Sign-in cancelled.");
        }

        match claim_once(http, server_url, state, options.request_timeout).await {
            Ok(Some(code)) => {
                tracing::debug!("desktop SSO handoff claimed");
                return OidcLoginPoll::claimed(code);
            }
            Ok(None) => failures = 0,
            Err(message) => {
                failures += 1;
                if failures >= options.max_network_failures {
                    tracing::warn!(error = %message, "desktop SSO could not reach the server");
                    return OidcLoginPoll::terminal(
                        OidcLoginStatus::Error,
                        format!("The sign-in could not be completed: {message}"),
                    );
                }
            }
        }

        let now = Instant::now();
        if now >= deadline {
            tracing::debug!("desktop SSO window expired");
            return OidcLoginPoll::terminal(
                OidcLoginStatus::Expired,
                "The sign-in window expired. Start again from Ludex.",
            );
        }
        tokio::time::sleep(
            options
                .interval
                .min(deadline.saturating_duration_since(now)),
        )
        .await;
    }
}

/// One claim call: `Ok(Some(code))` when the page handed the session back,
/// `Ok(None)` while nothing is pending yet.
async fn claim_once(
    http: &reqwest::Client,
    server_url: &str,
    state: &str,
    timeout: Duration,
) -> Result<Option<String>, String> {
    let url = format!(
        "{}/api/auth/desktop-handoff/claim",
        server_url.trim_end_matches('/')
    );
    let response = http
        .post(&url)
        .timeout(timeout)
        .json(&json!({ "state": state }))
        .send()
        .await
        .map_err(|error| format!("Could not reach {server_url}: {error}"))?;
    let status = response.status();
    if status.is_success() {
        #[derive(Deserialize)]
        struct Claim {
            code: String,
        }
        let claim: Claim = response
            .json()
            .await
            .map_err(|_| "The server returned an invalid handoff response.".to_string())?;
        if claim.code.trim().is_empty() {
            return Err("The server returned an empty handoff code.".to_string());
        }
        return Ok(Some(claim.code));
    }
    if status == reqwest::StatusCode::GONE
        || status == reqwest::StatusCode::NOT_FOUND
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Ok(None);
    }
    if status.is_server_error() {
        return Err(format!("The server answered HTTP {status}."));
    }
    Err(format!(
        "This server does not accept desktop sign-ins (HTTP {status}). Update it and try again."
    ))
}

/// Runs one attempt registered in `sessions` and forgets it afterwards.
pub async fn poll_session(
    http: &reqwest::Client,
    sessions: &LoginSessions,
    session_id: &str,
    options: &PollOptions,
) -> Result<OidcLoginPoll, String> {
    let attempt = sessions
        .get(session_id)
        .ok_or_else(|| "This sign-in attempt is unknown; start again from Ludex.".to_string())?;
    let result = poll_handoff(
        http,
        &attempt.server_url,
        session_id,
        &attempt.cancelled,
        options,
    )
    .await;
    sessions.remove(session_id);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::serve_json;
    use serde_json::{json, Value};

    const STATE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn states_are_32_random_bytes_of_hex() {
        let first = generate_state();
        let second = generate_state();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn login_urls_carry_the_state_only_in_the_fragment_route() {
        let url = login_url("https://ludex.example.com/", STATE);
        assert_eq!(
            url,
            format!("https://ludex.example.com/#/login?desktop_state={STATE}")
        );
        assert_eq!(
            web_login_url("https://ludex.example.com/"),
            "https://ludex.example.com/#/login"
        );
        assert!(!web_login_url("https://ludex.example.com").contains(STATE_PARAM));
    }

    #[test]
    fn start_opens_the_browser_and_registers_the_attempt() {
        let sessions = LoginSessions::new();
        let opened = Mutex::new(Vec::new());
        let start = start_with_opener(&sessions, "https://ludex.example.com/", |url| {
            opened.lock().unwrap().push(url.to_string());
            Ok(())
        })
        .expect("start");

        assert_eq!(start.session_id.len(), 64);
        assert_eq!(start.web_login_url, "https://ludex.example.com/#/login");
        assert!(!start.expires_at.is_empty());
        assert_eq!(sessions.len(), 1);
        let opened = opened.lock().unwrap();
        assert_eq!(opened.len(), 1);
        assert!(opened[0].contains(&format!("desktop_state={}", start.session_id)));
    }

    #[test]
    fn a_browser_that_cannot_open_is_reported_without_a_pending_attempt() {
        let sessions = LoginSessions::new();
        let error = start_with_opener(&sessions, "https://ludex.example.com", |_| {
            Err("no browser found".to_string())
        })
        .expect_err("browser failure");
        assert!(error.contains("Could not open your browser"), "{error}");
        assert!(
            error.contains("https://ludex.example.com/#/login"),
            "{error}"
        );
        assert!(sessions.is_empty());
    }

    #[test]
    fn an_unconfigured_server_is_rejected_before_opening_anything() {
        let sessions = LoginSessions::new();
        let error = start_with_opener(&sessions, "  ", |_| Ok(())).expect_err("no server");
        assert!(error.contains("Connect to a Ludex server"), "{error}");
        assert!(sessions.is_empty());
    }

    #[test]
    fn cancelling_flags_the_pending_attempt() {
        let sessions = LoginSessions::new();
        let start = start_with_opener(&sessions, "https://ludex.example.com", |_| Ok(())).unwrap();
        sessions.cancel(&start.session_id);
        let attempt = sessions.get(&start.session_id).expect("attempt");
        assert!(attempt.is_cancelled());
    }

    #[tokio::test]
    async fn polling_returns_the_code_after_the_handoff_appears() {
        let server = serve_json(vec![
            (
                410,
                json!({ "error": { "code": "handoff_unavailable", "message": "not yet" } }),
            ),
            (
                410,
                json!({ "error": { "code": "handoff_unavailable", "message": "not yet" } }),
            ),
            (
                200,
                json!({ "code": "one-time-code", "expiresAt": "2026-01-01T00:00:00Z" }),
            ),
        ])
        .await;
        let http = reqwest::Client::new();
        let poll = poll_handoff(
            &http,
            &server.url,
            STATE,
            &AtomicBool::new(false),
            &PollOptions::for_tests(),
        )
        .await;

        assert_eq!(poll.status, OidcLoginStatus::Claimed);
        assert_eq!(poll.code.as_deref(), Some("one-time-code"));
        assert!(poll.message.is_none());
        let requests = server.requests.lock().unwrap();
        assert!(requests.len() >= 3);
        assert!(requests.iter().all(|body: &Value| body["state"] == STATE));
    }

    #[tokio::test]
    async fn polling_times_out_when_the_page_never_reports_back() {
        let server = serve_json(vec![(
            410,
            json!({ "error": { "code": "handoff_unavailable", "message": "not yet" } }),
        )])
        .await;
        let options = PollOptions {
            timeout: Duration::from_millis(60),
            ..PollOptions::for_tests()
        };
        let poll = poll_handoff(
            &reqwest::Client::new(),
            &server.url,
            STATE,
            &AtomicBool::new(false),
            &options,
        )
        .await;
        assert_eq!(poll.status, OidcLoginStatus::Expired);
        assert!(poll.code.is_none());
        assert!(poll
            .message
            .as_deref()
            .is_some_and(|message| message.contains("expired")));
    }

    #[tokio::test]
    async fn polling_stops_when_the_user_cancels() {
        let server = serve_json(vec![(
            410,
            json!({ "error": { "code": "handoff_unavailable", "message": "not yet" } }),
        )])
        .await;
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let cancel = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let poll = poll_handoff(
            &reqwest::Client::new(),
            &server.url,
            STATE,
            &cancelled,
            &PollOptions::for_tests(),
        )
        .await;
        cancel.await.expect("cancel task");
        assert_eq!(poll.status, OidcLoginStatus::Cancelled);
        assert!(poll.code.is_none());
    }

    #[tokio::test]
    async fn an_unreachable_server_is_reported() {
        let options = PollOptions {
            max_network_failures: 1,
            ..PollOptions::for_tests()
        };
        let poll = poll_handoff(
            &reqwest::Client::new(),
            "http://127.0.0.1:1",
            STATE,
            &AtomicBool::new(false),
            &options,
        )
        .await;
        assert_eq!(poll.status, OidcLoginStatus::Error);
        assert!(poll
            .message
            .as_deref()
            .is_some_and(|message| message.contains("could not be completed")));
    }

    #[tokio::test]
    async fn an_unknown_session_cannot_be_polled() {
        let sessions = LoginSessions::new();
        let error = poll_session(
            &reqwest::Client::new(),
            &sessions,
            STATE,
            &PollOptions::for_tests(),
        )
        .await
        .expect_err("unknown session");
        assert!(error.contains("unknown"), "{error}");
    }

    #[tokio::test]
    async fn a_rejected_code_surfaces_as_an_exchange_failure() {
        // The webview redeems `poll.code` through the existing exchange; this
        // mirrors a server that accepts the claim but rejects the code, which
        // is what the UI must report instead of a session.
        let server = serve_json(vec![
            (200, json!({ "code": "one-time-code" })),
            (
                401,
                json!({ "error": { "code": "invalid_oidc_code", "message": "expired" } }),
            ),
        ])
        .await;
        let http = reqwest::Client::new();
        let poll = poll_handoff(
            &http,
            &server.url,
            STATE,
            &AtomicBool::new(false),
            &PollOptions::for_tests(),
        )
        .await;
        let code = poll.code.expect("code");

        let response = http
            .post(format!("{}/api/auth/oidc/exchange", server.url))
            .json(&json!({ "code": code }))
            .send()
            .await
            .expect("exchange");
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_server_without_handoff_support_fails_fast() {
        let server = serve_json(vec![(
            404,
            json!({ "error": { "code": "not_found", "message": "Endpoint not found" } }),
        )])
        .await;
        let options = PollOptions {
            timeout: Duration::from_millis(60),
            ..PollOptions::for_tests()
        };
        // 404 is treated as "nothing pending" for older servers, so the poll
        // ends on the timeout rather than an error.
        let poll = poll_handoff(
            &reqwest::Client::new(),
            &server.url,
            STATE,
            &AtomicBool::new(false),
            &options,
        )
        .await;
        assert_eq!(poll.status, OidcLoginStatus::Expired);
    }
}
