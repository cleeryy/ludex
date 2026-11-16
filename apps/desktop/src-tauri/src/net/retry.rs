//! Retry budget of one transfer (#195/#212).
//!
//! [`RetryPolicy`] is immutable configuration; [`RetryState`] is the mutable
//! connection counter. Keeping them separate is what lets the download loop ask
//! "may I reconnect?" without owning the answer, and what lets the tests pin
//! the window, the attempt cap and the circuit breaker independently.
//!
//! The backoff timing is unchanged from the pre-split code: exponential with
//! ±25 % jitter, capped by the policy, with the server's `Retry-After` as a
//! floor (never a shortening) and its own 30-minute cap.

use std::time::{Duration, Instant};

use crate::state::NetworkGate;

/// Read/write tuning for one resumable download (#195, #212).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy<'a> {
    /// Abort the response when no bytes arrive for this long. A transfer that
    /// keeps producing bytes is never judged a stall.
    pub idle_timeout: Duration,
    /// Connections per download, including the first one. `u32::MAX` for an
    /// unlimited retry window.
    pub max_attempts: u32,
    /// Give up on retries once this much wall-clock time was spent reconnecting.
    /// `None` keeps retrying while the job is active.
    pub max_retry_window: Option<Duration>,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    /// OS reachability (#212): retry waits park while offline.
    pub network: Option<&'a NetworkGate>,
    /// "Retry now" signal of the job (#212); skips one backoff sleep.
    pub retry_now: Option<&'a tokio::sync::Notify>,
}

impl Default for RetryPolicy<'_> {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(30),
            max_attempts: 50,
            max_retry_window: Some(Duration::from_secs(6 * 60 * 60)),
            base_backoff: Duration::from_secs(1),
            // A long-horizon retry must not hammer a dead link: five minutes
            // between attempts still recovers quickly after an outage (#212).
            max_backoff: Duration::from_secs(5 * 60),
            network: None,
            retry_now: None,
        }
    }
}

impl<'a> RetryPolicy<'a> {
    /// Build from the persisted settings, clamped so a bad value cannot turn
    /// every install into a reconnect storm or wait forever.
    pub(crate) fn from_settings(idle_secs: u64, retry_window_secs: u64) -> Self {
        let window = (retry_window_secs != 0)
            .then(|| Duration::from_secs(retry_window_secs.clamp(60, 7 * 24 * 60 * 60)));
        Self {
            idle_timeout: Duration::from_secs(idle_secs.clamp(5, 600)),
            // Unlimited retry: no attempt cap either, otherwise the count
            // would become the real limit and stop a working retry early.
            max_attempts: if window.is_some() { 50 } else { u32::MAX },
            max_retry_window: window,
            ..Self::default()
        }
    }

    /// Kept for call sites that only know the stall timeout (#195).
    #[cfg(test)]
    pub(crate) fn from_idle_secs(seconds: u64) -> Self {
        Self::from_settings(seconds, 6 * 60 * 60)
    }

    /// The next backoff after `attempt` failed connections.
    pub(crate) fn delay(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(10);
        let base = self
            .base_backoff
            .saturating_mul(1u32 << exponent)
            .min(self.max_backoff);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0);
        let factor = 0.75 + f64::from(nanos % 501) / 1000.0;
        base.mul_f64(factor).min(self.max_backoff)
    }

    /// The bigger of the exponential backoff and the server's `Retry-After`,
    /// capped so a bogus header cannot park a job for hours (#212).
    pub(crate) fn delay_with_hint(&self, attempt: u32, hint: Option<Duration>) -> Duration {
        const MAX_HINT: Duration = Duration::from_secs(30 * 60);
        self.delay(attempt)
            .max(hint.unwrap_or_default().min(MAX_HINT))
    }
}

/// Connection budget of one download: attempt count, retry window and the
/// circuit breaker on repeated identical permanent errors (#195, #212).
#[derive(Debug)]
pub(crate) struct RetryState {
    attempt: u32,
    retry_started: Option<Instant>,
    last_failure: Option<String>,
    identical_failures: u32,
}

impl RetryState {
    pub(crate) fn new() -> Self {
        Self {
            attempt: 0,
            retry_started: None,
            last_failure: None,
            identical_failures: 0,
        }
    }

    /// Connection attempt of the current transfer, starting at 1.
    pub(crate) fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The attempt number reported to the UI while waiting for a retry.
    pub(crate) fn next_attempt_number(&self) -> u32 {
        self.attempt.saturating_add(1)
    }

    pub(crate) fn next_attempt(&mut self) {
        self.attempt = self.attempt.saturating_add(1);
        if self.retry_started.is_none() && self.attempt > 1 {
            self.retry_started = Some(Instant::now());
        }
    }

    /// Record one failure. Returns a stop reason when the same *permanent*
    /// error repeated, so a flapping archive cannot spin the loop (#212).
    pub(crate) fn record_failure(&mut self, message: &str, permanent: bool) -> Option<String> {
        if self.last_failure.as_deref() == Some(message) {
            self.identical_failures = self.identical_failures.saturating_add(1);
        } else {
            self.last_failure = Some(message.to_string());
            self.identical_failures = 1;
        }
        if permanent && self.identical_failures >= 3 {
            return Some(format!(
                "the same permanent error repeated {} times",
                self.identical_failures
            ));
        }
        None
    }

    /// Whether another connection is allowed; adds a friendly reason when not.
    pub(crate) fn exhausted(&self, policy: RetryPolicy<'_>) -> Option<String> {
        if self.attempt >= policy.max_attempts {
            return Some(format!(
                "the connection failed {} times in a row",
                self.attempt
            ));
        }
        if let (Some(started), Some(window)) = (self.retry_started, policy.max_retry_window) {
            if started.elapsed() >= window {
                return Some(format!(
                    "the connection stayed unreliable for {}",
                    human_duration(window)
                ));
            }
        }
        None
    }

    /// Test hook: pretend the retry window started at a given instant.
    #[cfg(test)]
    pub(crate) fn set_retry_started(&mut self, started: Option<Instant>) {
        self.retry_started = started;
    }
}

impl Default for RetryState {
    fn default() -> Self {
        Self::new()
    }
}

/// `45 s` / `2 min` / `2 h` for retry reasons and extraction ETAs.
pub(crate) fn human_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!("{} h", seconds / 3600)
    } else if seconds >= 60 {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

/// RFC 3339 timestamp of a retry that will start in `delay` (#212).
pub(crate) fn retry_at(delay: Duration) -> String {
    (chrono::Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default()).to_rfc3339()
}

/// `Retry-After: 30` → 30 s. An HTTP-date is ignored rather than misparsed;
/// the reconnect limiter of the server uses delta-seconds (#212).
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_grows_and_stays_capped() {
        let policy = RetryPolicy {
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(800),
            ..RetryPolicy::default()
        };
        let first = policy.delay(1);
        let third = policy.delay(3);
        assert!(first >= Duration::from_millis(75), "{first:?}");
        assert!(third <= policy.max_backoff, "{third:?}");
        assert!(third >= Duration::from_millis(300), "{third:?}");
        assert_eq!(
            RetryPolicy::from_idle_secs(1).idle_timeout,
            Duration::from_secs(5)
        );
        assert_eq!(
            RetryPolicy::from_idle_secs(900).idle_timeout,
            Duration::from_secs(600)
        );
    }

    #[test]
    fn the_retry_window_defaults_to_six_hours_and_can_be_unlimited() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retry_window, Some(Duration::from_secs(6 * 3600)));
        assert_eq!(policy.max_attempts, 50);
        assert_eq!(policy.max_backoff, Duration::from_secs(5 * 60));

        let unlimited = RetryPolicy::from_settings(30, 0);
        assert_eq!(unlimited.max_retry_window, None);
        assert_eq!(
            unlimited.max_attempts,
            u32::MAX,
            "an unlimited window must not be capped by the attempt counter"
        );

        // The window boundary: one second past 6 h is exhausted, one second
        // before it still retries.
        let mut retry = RetryState::new();
        retry.next_attempt();
        retry.set_retry_started(Some(Instant::now() - Duration::from_secs(6 * 3600 + 1)));
        assert!(retry
            .exhausted(policy)
            .expect("window exhausted")
            .contains("6 h"));
        retry.set_retry_started(Some(Instant::now() - Duration::from_secs(6 * 3600 - 60)));
        assert!(
            retry.exhausted(policy).is_none(),
            "inside the window, retrying continues"
        );
    }

    #[test]
    fn repeated_identical_permanent_errors_trip_the_circuit_breaker() {
        let mut retry = RetryState::new();
        let message = "Download failed with HTTP 404: gone";
        assert!(retry.record_failure(message, true).is_none());
        assert!(retry.record_failure(message, true).is_none());
        let reason = retry
            .record_failure(message, true)
            .expect("third identical permanent error stops the loop");
        assert!(reason.contains("3 times"), "{reason}");
        // A transient error repeating is normal during an outage: no breaker.
        let mut transient = RetryState::new();
        for _ in 0..10 {
            assert!(transient
                .record_failure("connection reset by peer", false)
                .is_none());
        }
    }

    #[test]
    fn retry_after_is_honoured_and_capped() {
        assert_eq!(parse_retry_after(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("soon"), None);
        let policy = RetryPolicy {
            base_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(60),
            ..RetryPolicy::default()
        };
        assert_eq!(
            policy.delay_with_hint(1, Some(Duration::from_secs(45))),
            Duration::from_secs(45)
        );
        // A shorter Retry-After never shortens the exponential floor (the
        // floor carries ±25 % jitter, so assert the band, not one value).
        let floored = policy.delay_with_hint(1, Some(Duration::from_secs(1)));
        assert!(
            floored >= Duration::from_millis(1500) && floored <= Duration::from_millis(2500),
            "{floored:?} must stay the jittered 2 s floor"
        );
        // A bogus huge header is capped instead of parking the job for hours.
        assert_eq!(
            policy.delay_with_hint(1, Some(Duration::from_secs(24 * 3600))),
            Duration::from_secs(30 * 60)
        );
    }
}
