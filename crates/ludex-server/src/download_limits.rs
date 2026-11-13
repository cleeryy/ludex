//! Admission control for archive downloads (#217).
//!
//! A single-stream client keeps the per-address, per-version reconnect budget
//! introduced for #195 and sees no behaviour change. A client that fetches
//! several byte ranges in parallel names its job with `X-Ludex-Job-Id` (or
//! `?jobId=`) and gets a job-scoped budget instead:
//!
//! - [`JOB_REQUESTS_PER_MINUTE`] part requests per job and address per minute;
//! - [`IP_REQUESTS_PER_MINUTE`] part requests per address per minute, across
//!   every job, so inventing job ids does not multiply the budget;
//! - [`JOB_PARTS_MAX`] concurrently open part streams per job and address,
//!   advertised to clients as `parallelPartsMax`;
//! - [`IP_PARTS_MAX`] concurrently open part streams per address, across every
//!   job.
//!
//! The request budgets are well above a normal 4-8 part download, while a hot
//! reconnect loop still trips them and receives a `429` with `Retry-After`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::security::AttemptCounter;

/// Part requests one job may make per minute (#217).
pub const JOB_REQUESTS_PER_MINUTE: u32 = 240;
/// Part requests one address may make per minute across every job (#217).
pub const IP_REQUESTS_PER_MINUTE: u32 = 600;
/// Concurrent part streams one job may hold (#217).
pub const JOB_PARTS_MAX: usize = 8;
/// Concurrent part streams one address may hold across every job (#217).
pub const IP_PARTS_MAX: usize = 16;
/// Reconnect budget of a single-stream client, per address and version (#195).
pub const LEGACY_REQUESTS_PER_MINUTE: u32 = 120;
/// Longest accepted job id.
pub const JOB_ID_MAX_LEN: usize = 64;
/// Header carrying the download job id.
pub const JOB_ID_HEADER: &str = "x-ludex-job-id";

/// Length of the fixed window of the request budgets.
const WINDOW: Duration = Duration::from_secs(60);

/// Which budget refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitScope {
    /// The per-address, per-version budget of single-stream clients (#195).
    Legacy,
    /// The per-job request budget.
    Job,
    /// The per-address request budget shared by every job.
    Address,
    /// The concurrent-part cap.
    Parts,
}

/// A refused download admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitRejection {
    /// Budget that was exhausted.
    pub scope: LimitScope,
    /// Seconds until the request may be retried, for `Retry-After`.
    pub retry_after: u64,
}

impl LimitRejection {
    /// Client-facing message. Codes are unchanged from #195 so existing
    /// clients keep their handling.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self.scope {
            LimitScope::Legacy => {
                "Too many download attempts for this version; wait a few seconds and retry."
            }
            LimitScope::Job => {
                "Too many part requests for this download job; wait a few seconds and retry."
            }
            LimitScope::Address => {
                "Too many download attempts from this address; wait a few seconds and retry."
            }
            LimitScope::Parts => {
                "Too many concurrent download parts; wait for a part to finish and retry."
            }
        }
    }
}

/// An invalid `X-Ludex-Job-Id` / `?jobId=` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidJobId;

/// Validates a client-provided job id.
///
/// Empty and whitespace-only values mean "no job": the request is treated as a
/// single-stream download. Accepted ids are 1-64 ASCII alphanumerics, `-` or
/// `_`, which keeps them safe in log fields and in the limiter's key map.
pub fn parse_job_id(raw: Option<&str>) -> Result<Option<String>, InvalidJobId> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > JOB_ID_MAX_LEN
        || !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(InvalidJobId);
    }
    Ok(Some(trimmed.to_string()))
}

/// Counts the part streams currently open per job and per address.
#[derive(Debug, Default)]
struct PartCounts {
    jobs: HashMap<String, usize>,
    addresses: HashMap<String, usize>,
}

fn release(counts: &mut HashMap<String, usize>, key: &str) {
    if let Some(active) = counts.get_mut(key) {
        *active = active.saturating_sub(1);
        if *active == 0 {
            counts.remove(key);
        }
    }
}

/// RAII permit for one concurrently open part stream.
#[derive(Debug)]
struct PartSlot {
    counts: Arc<Mutex<PartCounts>>,
    job_key: String,
    address_key: String,
}

impl Drop for PartSlot {
    fn drop(&mut self) {
        if let Ok(mut counts) = self.counts.lock() {
            release(&mut counts.jobs, &self.job_key);
            release(&mut counts.addresses, &self.address_key);
        }
    }
}

/// Admission of one download request.
///
/// A job-scoped admission owns the permit of its part stream, so holding the
/// value for the lifetime of the response body keeps the concurrent-part count
/// accurate. A legacy admission owns nothing.
#[derive(Debug)]
pub struct DownloadAdmission {
    _part: Option<PartSlot>,
}

impl DownloadAdmission {
    /// Admission without a concurrency permit (single-stream or disabled).
    const fn unguarded() -> Self {
        Self { _part: None }
    }
}

/// Job-scoped download limiter (#217).
#[derive(Debug)]
pub struct DownloadLimiter {
    legacy: AttemptCounter,
    job_requests: AttemptCounter,
    address_requests: AttemptCounter,
    counts: Arc<Mutex<PartCounts>>,
    parts_max: usize,
    address_parts_max: usize,
}

impl Default for DownloadLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadLimiter {
    /// Limiter with the documented default budgets.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(
            JOB_REQUESTS_PER_MINUTE,
            IP_REQUESTS_PER_MINUTE,
            JOB_PARTS_MAX,
            IP_PARTS_MAX,
        )
    }

    /// Limiter with explicit budgets, used by tests. A request limit of `0`
    /// and/or `usize::MAX` part caps disables that dimension.
    #[must_use]
    pub fn with_limits(
        job_requests: u32,
        address_requests: u32,
        parts_max: usize,
        address_parts_max: usize,
    ) -> Self {
        Self {
            legacy: AttemptCounter::new(LEGACY_REQUESTS_PER_MINUTE, WINDOW),
            job_requests: AttemptCounter::new(job_requests, WINDOW),
            address_requests: AttemptCounter::new(address_requests, WINDOW),
            counts: Arc::new(Mutex::new(PartCounts::default())),
            parts_max,
            address_parts_max,
        }
    }

    /// A limiter that never refuses a request.
    #[must_use]
    pub fn disabled() -> Self {
        Self::with_limits(0, 0, usize::MAX, usize::MAX)
    }

    /// Admits one download request.
    ///
    /// Without a job id the request uses the #195 per-address/version budget
    /// and no concurrency permit, exactly like before #217. With a job id it
    /// checks the address budget, then the job budget, then reserves one part
    /// slot.
    pub fn admit(
        &self,
        address: IpAddr,
        version_id: i64,
        job_id: Option<&str>,
    ) -> Result<DownloadAdmission, LimitRejection> {
        let Some(job_id) = job_id else {
            return self
                .legacy
                .check(&format!("{address}:{version_id}"))
                .map(|()| DownloadAdmission::unguarded())
                .map_err(|retry_after| LimitRejection {
                    scope: LimitScope::Legacy,
                    retry_after,
                });
        };
        let address_key = address.to_string();
        self.address_requests
            .check(&address_key)
            .map_err(|retry_after| LimitRejection {
                scope: LimitScope::Address,
                retry_after,
            })?;
        let job_key = format!("{address_key}:{job_id}");
        self.job_requests
            .check(&job_key)
            .map_err(|retry_after| LimitRejection {
                scope: LimitScope::Job,
                retry_after,
            })?;
        self.reserve(&address_key, job_key)
            .map(|part| DownloadAdmission { _part: Some(part) })
    }

    fn reserve(&self, address_key: &str, job_key: String) -> Result<PartSlot, LimitRejection> {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let job_active = counts.jobs.get(&job_key).copied().unwrap_or(0);
        let address_active = counts.addresses.get(address_key).copied().unwrap_or(0);
        if job_active >= self.parts_max || address_active >= self.address_parts_max {
            return Err(LimitRejection {
                scope: LimitScope::Parts,
                retry_after: 1,
            });
        }
        *counts.jobs.entry(job_key.clone()).or_insert(0) += 1;
        *counts.addresses.entry(address_key.to_string()).or_insert(0) += 1;
        Ok(PartSlot {
            counts: Arc::clone(&self.counts),
            job_key,
            address_key: address_key.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn address() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10))
    }

    #[test]
    fn eight_parts_and_their_retries_stay_under_the_budget() {
        let limiter = DownloadLimiter::new();
        let mut open = Vec::new();
        for _ in 0..JOB_PARTS_MAX {
            open.push(
                limiter
                    .admit(address(), 7, Some("job-eight"))
                    .expect("part admitted"),
            );
        }
        drop(open);
        // Retries and Range reconnects stay far below 240/min.
        for _ in 0..32 {
            let admission = limiter
                .admit(address(), 7, Some("job-eight"))
                .expect("retry admitted");
            drop(admission);
        }
    }

    #[test]
    fn a_ninth_concurrent_part_is_refused_until_one_finishes() {
        let limiter = DownloadLimiter::new();
        let mut open = Vec::new();
        for _ in 0..JOB_PARTS_MAX {
            open.push(
                limiter
                    .admit(address(), 7, Some("job-parts"))
                    .expect("part admitted"),
            );
        }
        let rejection = limiter
            .admit(address(), 7, Some("job-parts"))
            .expect_err("ninth part");
        assert_eq!(rejection.scope, LimitScope::Parts);
        assert_eq!(rejection.retry_after, 1);
        open.pop();
        limiter
            .admit(address(), 7, Some("job-parts"))
            .expect("slot released");
    }

    #[test]
    fn the_address_budget_caps_every_job_together() {
        let limiter = DownloadLimiter::with_limits(10, 3, 8, 16);
        for job in ["a", "b", "c"] {
            drop(
                limiter
                    .admit(address(), 7, Some(job))
                    .expect("within address budget"),
            );
        }
        let rejection = limiter
            .admit(address(), 7, Some("d"))
            .expect_err("address budget exhausted");
        assert_eq!(rejection.scope, LimitScope::Address);
        assert!(rejection.retry_after >= 1);
    }

    #[test]
    fn job_budgets_are_isolated() {
        let limiter = DownloadLimiter::with_limits(2, 100, 8, 16);
        drop(limiter.admit(address(), 7, Some("hot")).expect("first"));
        drop(limiter.admit(address(), 7, Some("hot")).expect("second"));
        let rejection = limiter
            .admit(address(), 7, Some("hot"))
            .expect_err("job exhausted");
        assert_eq!(rejection.scope, LimitScope::Job);
        limiter
            .admit(address(), 7, Some("other"))
            .expect("other job unaffected");
    }

    #[test]
    fn single_stream_keeps_the_legacy_budget_and_keys() {
        let limiter = DownloadLimiter::new();
        for _ in 0..LEGACY_REQUESTS_PER_MINUTE {
            drop(
                limiter
                    .admit(address(), 7, None)
                    .expect("within #195 budget"),
            );
        }
        let rejection = limiter
            .admit(address(), 7, None)
            .expect_err("legacy budget exhausted");
        assert_eq!(rejection.scope, LimitScope::Legacy);
        assert!(rejection.retry_after >= 1);
        // Another version has its own legacy window.
        drop(limiter.admit(address(), 8, None).expect("other version"));
        // A job client on the same address/version has its own budget.
        drop(
            limiter
                .admit(address(), 7, Some("legacy-bypass"))
                .expect("job budget"),
        );
    }

    #[test]
    fn disabled_limiter_admits_everything() {
        let limiter = DownloadLimiter::disabled();
        for _ in 0..1_000 {
            drop(limiter.admit(address(), 7, Some("job")).expect("admitted"));
        }
    }

    #[test]
    fn job_ids_are_validated() {
        assert_eq!(parse_job_id(None), Ok(None));
        assert_eq!(parse_job_id(Some("  ")), Ok(None));
        assert_eq!(
            parse_job_id(Some(" 01930f2e-7d2a-7c3b-9f11-2f0d4f5c6a77 ")),
            Ok(Some("01930f2e-7d2a-7c3b-9f11-2f0d4f5c6a77".to_string()))
        );
        assert_eq!(
            parse_job_id(Some("job_42")).unwrap().as_deref(),
            Some("job_42")
        );
        assert_eq!(
            parse_job_id(Some(&"a".repeat(JOB_ID_MAX_LEN)))
                .unwrap()
                .map(|id| id.len()),
            Some(JOB_ID_MAX_LEN)
        );
        assert_eq!(
            parse_job_id(Some(&"a".repeat(JOB_ID_MAX_LEN + 1))),
            Err(InvalidJobId)
        );
        assert_eq!(parse_job_id(Some("job/../etc")), Err(InvalidJobId));
        assert_eq!(parse_job_id(Some("job id")), Err(InvalidJobId));
        assert_eq!(parse_job_id(Some("jöb")), Err(InvalidJobId));
    }
}
