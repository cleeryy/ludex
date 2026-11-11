//! Request rate limiting and login brute-force protection.
//!
//! Both structures are in-memory and process-local: Ludex targets a single
//! SQLite-backed server, and the counters are only meaningful for the running
//! process. Losing them on restart is acceptable (an attacker cannot restart
//! the server); keeping them out of the database avoids a write per request.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;

use crate::error::ApiError;
use crate::state::AppState;

/// Upper bound on tracked IPs before stale buckets are pruned.
const MAX_BUCKETS: usize = 20_000;
/// Buckets idle for longer than this are dropped when the map is pruned.
const BUCKET_IDLE: Duration = Duration::from_secs(600);
/// Upper bound on tracked login keys before stale entries are pruned.
const MAX_LOGIN_KEYS: usize = 10_000;
/// Maximum exponential backoff applied before a lockout, in seconds.
const MAX_BACKOFF_SECONDS: u64 = 60;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    updated: Instant,
}

/// Token-bucket limiter keyed by client IP.
#[derive(Debug)]
pub struct RateLimiter {
    rate_per_second: f64,
    burst: f64,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    /// Builds a limiter. `rate_per_second == 0` disables it.
    #[must_use]
    pub fn new(rate_per_second: u32, burst: u32) -> Self {
        let burst = burst.max(1);
        Self {
            rate_per_second: f64::from(rate_per_second),
            burst: f64::from(burst),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// A limiter that never rejects a request.
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(0, 0)
    }

    /// Whether the limiter is active.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.rate_per_second > 0.0
    }

    /// Consumes one token for `ip`.
    ///
    /// Returns `Ok(())` when the request may proceed, or the number of seconds
    /// the caller should wait.
    pub fn check(&self, ip: IpAddr) -> Result<(), u64> {
        if !self.enabled() {
            return Ok(());
        }
        let now = Instant::now();
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if buckets.len() > MAX_BUCKETS {
            buckets.retain(|_, bucket| now.duration_since(bucket.updated) < BUCKET_IDLE);
        }
        let bucket = buckets.entry(ip).or_insert(Bucket {
            tokens: self.burst,
            updated: now,
        });
        let elapsed = now.duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rate_per_second).min(self.burst);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let wait = (1.0 - bucket.tokens) / self.rate_per_second;
            Err(wait.ceil().max(1.0) as u64)
        }
    }
}

#[derive(Debug)]
struct LoginEntry {
    failures: u32,
    last_failure: Instant,
    locked_until: Option<Instant>,
}

impl LoginEntry {
    fn fresh() -> Self {
        Self {
            failures: 0,
            last_failure: Instant::now(),
            locked_until: None,
        }
    }
}

/// Per-IP + username login throttle with exponential backoff and lockout.
#[derive(Debug)]
pub struct LoginGuard {
    max_attempts: u32,
    lockout: Duration,
    base_delay: Duration,
    entries: Mutex<HashMap<String, LoginEntry>>,
}

impl LoginGuard {
    /// Builds a guard. `max_attempts == 0` disables the lockout.
    #[must_use]
    pub fn new(max_attempts: u32, lockout: Duration, base_delay: Duration) -> Self {
        Self {
            max_attempts,
            lockout,
            base_delay,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// A guard that never throttles.
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(0, Duration::ZERO, Duration::ZERO)
    }

    /// Whether the guard is active.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.max_attempts > 0
    }

    /// Builds the throttle key from a client IP and a submitted username.
    #[must_use]
    pub fn key(ip: Option<IpAddr>, username: &str) -> String {
        let ip = ip.map_or_else(|| "unknown".to_string(), |ip| ip.to_string());
        format!("{}|{}", ip, username.trim().to_ascii_lowercase())
    }

    /// Checks whether an attempt may run right now.
    ///
    /// Returns `Ok(())` when allowed, or the number of seconds to wait and
    /// whether the key is fully locked out.
    pub fn check(&self, key: &str) -> Result<(), (u64, bool)> {
        if !self.enabled() {
            return Ok(());
        }
        let now = Instant::now();
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(entry) = entries.get(key) else {
            return Ok(());
        };
        if let Some(until) = entry.locked_until {
            if until > now {
                return Err((seconds_until(now, until), true));
            }
        }
        if entry.failures >= 3 {
            let delay = self.backoff(entry.failures);
            let ready = entry.last_failure + delay;
            if ready > now {
                return Err((seconds_until(now, ready), false));
            }
        }
        Ok(())
    }

    /// Records a failed attempt and returns the resulting decision.
    pub fn record_failure(&self, key: &str) -> (u64, bool) {
        if !self.enabled() {
            return (0, false);
        }
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if entries.len() > MAX_LOGIN_KEYS {
            entries.retain(|_, entry| {
                entry.locked_until.is_some_and(|until| until > now)
                    || now.duration_since(entry.last_failure)
                        < self.lockout.max(Duration::from_secs(900))
            });
        }
        let entry = entries
            .entry(key.to_string())
            .or_insert_with(LoginEntry::fresh);
        entry.failures = entry.failures.saturating_add(1);
        entry.last_failure = now;
        if entry.failures >= self.max_attempts {
            let until = now + self.lockout;
            entry.locked_until = Some(until);
            (seconds_until(now, until), true)
        } else {
            (0, false)
        }
    }

    /// Clears the failure counter after a successful login.
    pub fn record_success(&self, key: &str) {
        if !self.enabled() {
            return;
        }
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(key);
    }

    fn backoff(&self, failures: u32) -> Duration {
        // The first two failures never delay the next attempt: a single typo
        // (or a password just changed on another device) must not make the
        // retry feel broken. From the third failure the delay doubles on every
        // attempt (500 ms, 1 s, 2 s, ... capped at 60 s).
        let exponent = failures.saturating_sub(3).min(8);
        let seconds =
            self.base_delay.as_secs_f64() * f64::powi(2.0, i32::try_from(exponent).unwrap_or(8));
        Duration::from_secs_f64(
            seconds.min(f64::from(u32::try_from(MAX_BACKOFF_SECONDS).unwrap_or(60))),
        )
    }
}

fn seconds_until(now: Instant, until: Instant) -> u64 {
    until
        .checked_duration_since(now)
        .map_or(1, |duration| duration.as_secs().max(1))
}

/// Upper bound on tracked keys of a fixed-window counter.
const MAX_COUNTER_KEYS: usize = 10_000;

#[derive(Debug)]
struct CounterWindow {
    used: u32,
    reset_at: Instant,
}

/// Fixed-window request counter keyed by an arbitrary string.
///
/// Used by the desktop SSO handoff (#157) where a token bucket is the wrong
/// shape: creating a handoff is rare and must be capped per account, while
/// claiming is polled once per second and must be capped per address.
#[derive(Debug)]
pub struct AttemptCounter {
    limit: u32,
    window: Duration,
    entries: Mutex<HashMap<String, CounterWindow>>,
}

impl AttemptCounter {
    /// Builds a counter. `limit == 0` disables it.
    #[must_use]
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// A counter that never rejects a request.
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(0, Duration::ZERO)
    }

    /// Whether the counter is active.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.limit > 0 && !self.window.is_zero()
    }

    /// Counts one request for `key`.
    ///
    /// Returns `Ok(())` when the request may proceed, or the number of seconds
    /// until the window resets.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        if !self.enabled() {
            return Ok(());
        }
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if entries.len() > MAX_COUNTER_KEYS {
            entries.retain(|_, window| window.reset_at > now);
        }
        let window = entries.entry(key.to_string()).or_insert(CounterWindow {
            used: 0,
            reset_at: now + self.window,
        });
        if window.reset_at <= now {
            window.used = 0;
            window.reset_at = now + self.window;
        }
        if window.used >= self.limit {
            return Err(seconds_until(now, window.reset_at));
        }
        window.used = window.used.saturating_add(1);
        Ok(())
    }
}

/// Global API rate-limit middleware.
///
/// Health checks, OpenAPI discovery and CORS preflights are exempt so probes
/// and documentation never consume a client's budget. The limit is keyed by
/// the TCP peer address; deployments behind a reverse proxy should apply their
/// own limit at the proxy as well, because every request then shares one IP.
pub async fn rate_limit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if !state.rate_limiter.enabled()
        || request.method() == Method::OPTIONS
        || matches!(
            request.uri().path(),
            "/health"
                | "/api/health"
                | "/openapi.json"
                | "/api/openapi.json"
                | "/docs"
                | "/api/docs"
        )
    {
        return Ok(next.run(request).await);
    }
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| peer.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    match state.rate_limiter.check(peer) {
        Ok(()) => Ok(next.run(request).await),
        Err(wait) => Err(ApiError::too_many_requests(
            "rate_limited",
            format!("Too many requests. Retry in {wait} seconds."),
        )
        .with_retry_after(wait)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_bucket_allows_burst_then_throttles() {
        let limiter = RateLimiter::new(1, 2);
        let ip: IpAddr = "203.0.113.4".parse().expect("ip");
        assert!(limiter.check(ip).is_ok());
        assert!(limiter.check(ip).is_ok());
        let wait = limiter.check(ip).expect_err("third request is throttled");
        assert!(wait >= 1);
    }

    #[test]
    fn disabled_limiter_never_throttles() {
        let limiter = RateLimiter::disabled();
        let ip: IpAddr = "203.0.113.4".parse().expect("ip");
        for _ in 0..10_000 {
            assert!(limiter.check(ip).is_ok());
        }
    }

    #[test]
    fn login_guard_locks_out_after_max_attempts() {
        let guard = LoginGuard::new(3, Duration::from_secs(60), Duration::from_millis(0));
        let key = LoginGuard::key(Some("203.0.113.4".parse().expect("ip")), "admin");
        assert!(guard.check(&key).is_ok());
        assert_eq!(guard.record_failure(&key), (0, false));
        assert_eq!(guard.record_failure(&key), (0, false));
        let (wait, locked) = guard.record_failure(&key);
        assert!(locked);
        assert!(wait >= 59);
        let (_, locked) = guard.check(&key).expect_err("locked");
        assert!(locked);
    }

    #[test]
    fn login_guard_backoff_grows_and_success_clears_it() {
        let guard = LoginGuard::new(10, Duration::from_secs(60), Duration::from_millis(50));
        let key = LoginGuard::key(None, "Admin");
        assert_eq!(guard.record_failure(&key), (0, false));
        // Two typos do not delay the next attempt.
        assert!(guard.check(&key).is_ok());
        assert_eq!(guard.record_failure(&key), (0, false));
        assert!(guard.check(&key).is_ok());
        assert_eq!(guard.record_failure(&key), (0, false));
        let (wait, locked) = guard.check(&key).expect_err("throttled");
        assert!(!locked);
        assert!(wait >= 1);
        guard.record_success(&key);
        assert!(guard.check(&key).is_ok());
    }

    #[test]
    fn attempt_counter_limits_within_a_window() {
        let counter = AttemptCounter::new(2, Duration::from_secs(60));
        assert!(counter.check("user:1").is_ok());
        assert!(counter.check("user:1").is_ok());
        let wait = counter.check("user:1").expect_err("throttled");
        assert!(wait >= 59);
        // A different key has its own window.
        assert!(counter.check("user:2").is_ok());
    }

    #[test]
    fn attempt_counter_window_resets_and_disabled_never_throttles() {
        let counter = AttemptCounter::new(1, Duration::from_millis(20));
        assert!(counter.check("ip").is_ok());
        assert!(counter.check("ip").is_err());
        std::thread::sleep(Duration::from_millis(30));
        assert!(counter.check("ip").is_ok());

        let disabled = AttemptCounter::disabled();
        for _ in 0..10_000 {
            assert!(disabled.check("ip").is_ok());
        }
    }

    #[test]
    fn keys_are_case_insensitive_on_the_username() {
        let ip = Some("198.51.100.7".parse().expect("ip"));
        assert_eq!(LoginGuard::key(ip, "Alice"), LoginGuard::key(ip, " alice "));
    }
}
