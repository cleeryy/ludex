//! Optional periodic VirusTotal re-check (#239).
//!
//! The manual scanner (#224) only runs when an administrator asks. This module
//! adds the server-side counterpart: one persisted policy, one background
//! worker per server, and a pass that walks every archive with a published
//! SHA-256 whose latest report is missing or older than the policy interval.
//!
//! Design constraints, inherited from the manual scanner and made explicit:
//!
//! * **Off by default.** Nothing runs until `enabled` is true and the interval
//!   is one of the offered values. `intervalHours = 0` keeps the policy for a
//!   manual run only: the worker never fires, and a run-now pass re-checks
//!   reports whose cache entry expired on its own TTL.
//! * **Hash only.** A pass calls the same `lookup_hash` path as a manual scan:
//!   one `GET /files/{sha256}` per unique hash, never an upload or extraction.
//! * **Same quota.** Every request goes through `virustotal_budget` and the
//!   in-memory minute window, paced at one request per slot. An exhausted
//!   day stops the pass; the remainder stays stale and is picked up by the
//!   next run, so a library bigger than 500 archives drains over several days.
//! * **Fail soft.** An outage, an exhausted quota or a rejected key stops the
//!   pass with a category-prefixed `lastError`, keeps the schedule moving and
//!   never fails the app. Verdicts are never rewritten to `clean`.
//! * **Restart-safe.** `next_run_at`, `last_run_at`, `cursor` and
//!   `scanned_last_run` live in `virustotal_rescan`, so a restart keeps the
//!   schedule instead of resetting it or firing a pass on boot.

use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::watch;

use crate::audit::actions;
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::state::AppState;
use crate::virustotal::{self, KeyState, ScanMode, Verdict};

/// Intervals an administrator may pick, in hours. `0` means manual only.
pub const INTERVALS: [i64; 4] = [0, 24, 168, 720];

/// Interval used before an administrator chooses one: one week.
pub const DEFAULT_INTERVAL_HOURS: i64 = 168;

/// Consecutive upstream failures that stop one pass. A VirusTotal outage must
/// not burn the whole queue (or the daily budget) against a dead endpoint.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Persisted policy plus the two derived numbers the admin card shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RescanStatus {
    /// Whether the worker runs on a schedule.
    pub enabled: bool,
    /// `0` (manual only), `24`, `168` or `720`.
    pub interval_hours: i64,
    /// Re-check a `malicious` report as well. Off by default: its cache entry
    /// is kept for 100 days on purpose and re-checking it burns quota.
    pub include_malicious: bool,
    /// End of the last pass.
    pub last_run_at: Option<String>,
    /// When the worker fires next; `null` when disabled or manual only.
    pub next_run_at: Option<String>,
    /// SHA-256 the last pass stopped on, as a progress marker.
    pub cursor: Option<String>,
    /// Last failure, prefixed `invalid_key:` / `quota_exceeded:` / `outage:` /
    /// `disabled:`. Never contains the API key.
    pub last_error: Option<String>,
    /// Hashes refreshed by the last pass.
    pub scanned_last_run: i64,
    /// Distinct stale hashes the policy would select right now.
    pub remaining: i64,
    /// Whether a scheduled run is due at this instant.
    pub due: bool,
}

/// `PUT /api/admin/integrations/virustotal/rescan` body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RescanPolicyInput {
    /// Whether the worker runs on a schedule.
    pub enabled: bool,
    /// `0` (manual only), `24`, `168` or `720`.
    pub interval_hours: i64,
    /// Include `malicious` verdicts. Defaults to false.
    #[serde(default)]
    pub include_malicious: bool,
}

/// One `virustotal_rescan` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct RescanRow {
    enabled: i64,
    interval_hours: i64,
    include_malicious: i64,
    last_run_at: Option<String>,
    next_run_at: Option<String>,
    cursor: Option<String>,
    last_error: Option<String>,
    scanned_last_run: i64,
}

/// A version whose archive needs a fresh report.
#[derive(Debug, Clone, sqlx::FromRow)]
struct CandidateRow {
    version_id: i64,
    game_id: i64,
    sha256: String,
    checked_at: Option<String>,
    ttl_seconds: Option<i64>,
    verdict: Option<String>,
}

impl RescanRow {
    fn status(&self, remaining: i64, due: bool) -> RescanStatus {
        RescanStatus {
            enabled: self.enabled != 0,
            interval_hours: self.interval_hours,
            include_malicious: self.include_malicious != 0,
            last_run_at: self.last_run_at.clone(),
            next_run_at: self.next_run_at.clone(),
            cursor: self.cursor.clone(),
            last_error: self.last_error.clone(),
            scanned_last_run: self.scanned_last_run,
            remaining,
            due,
        }
    }
}

impl Default for RescanRow {
    fn default() -> Self {
        Self {
            enabled: 0,
            interval_hours: DEFAULT_INTERVAL_HOURS,
            include_malicious: 0,
            last_run_at: None,
            next_run_at: None,
            cursor: None,
            last_error: None,
            scanned_last_run: 0,
        }
    }
}

/// Cycle of one pass, for the audit trail and the admin toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescanRun {
    /// Hashes refreshed (an API answer was received and cached).
    /// Hashes that were already fresh are not counted, and neither are
    /// failures.
    pub scanned: i64,
    /// Distinct stale hashes left when the pass ended.
    pub remaining: i64,
    /// Category-prefixed failure that stopped the pass early.
    pub error: Option<String>,
    /// True when the pass ended because the daily budget was exhausted.
    pub budget_exhausted: bool,
}

/// Validates one interval value.
fn validate_interval(hours: i64) -> Result<(), ApiError> {
    if INTERVALS.contains(&hours) {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "invalid_interval",
        "intervalHours must be 0 (manual only), 24 (daily), 168 (weekly) or 720 (monthly)",
    ))
}

/// Next run for a policy change.
///
/// Enabling or changing the interval starts a fresh period; saving the same
/// policy keeps the run already scheduled, so editing the include-malicious
/// flag cannot silently postpone a due pass.
#[must_use]
fn next_run_for(
    previous: Option<&RescanRow>,
    now: DateTime<Utc>,
    enabled: bool,
    interval_hours: i64,
) -> Option<String> {
    if !enabled || interval_hours <= 0 {
        return None;
    }
    if let Some(previous) = previous {
        let unchanged = previous.enabled != 0
            && previous.interval_hours == interval_hours
            && interval_hours > 0;
        if unchanged {
            if let Some(next) = previous
                .next_run_at
                .as_deref()
                .and_then(db::parse_time)
                .filter(|next| *next > now)
            {
                return Some(db::db_time(next));
            }
        }
    }
    Some(db::db_time(now + ChronoDuration::hours(interval_hours)))
}

/// Staleness cutoff for one pass: the policy interval, or the cache TTL when
/// the policy is manual only.
fn is_stale(row: &CandidateRow, now: DateTime<Utc>, interval_hours: i64) -> bool {
    let Some(checked_at) = row.checked_at.as_deref() else {
        return true;
    };
    let Some(checked_at) = db::parse_time(checked_at) else {
        return true;
    };
    if interval_hours > 0 {
        return checked_at < now - ChronoDuration::hours(interval_hours);
    }
    // Manual only: a report is due when its own cache entry has expired.
    match row.ttl_seconds {
        Some(ttl) => checked_at + ChronoDuration::seconds(ttl.max(0)) < now,
        None => true,
    }
}

fn is_malicious(row: &CandidateRow) -> bool {
    row.verdict.as_deref() == Some("malicious")
}

async fn load_row(state: &AppState) -> Result<Option<RescanRow>, ApiError> {
    db::query_as::<RescanRow>(
        "SELECT enabled, interval_hours, include_malicious, last_run_at, next_run_at,
                cursor, last_error, scanned_last_run
         FROM virustotal_rescan WHERE id = 1",
    )
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)
}

/// Current policy, with defaults when an administrator never saved one.
async fn policy(state: &AppState) -> Result<RescanRow, ApiError> {
    Ok(load_row(state).await?.unwrap_or_default())
}

/// One row per version that already has a published SHA-256, oldest report
/// first (never-checked archives before stale ones).
async fn candidate_rows(state: &AppState) -> Result<Vec<CandidateRow>, ApiError> {
    db::query_as::<CandidateRow>(
        "SELECT v.id AS version_id, v.game_id,
                LOWER(v.content_sha256) AS sha256,
                c.checked_at, c.ttl_seconds, c.verdict
         FROM game_versions v
         LEFT JOIN virustotal_cache c ON c.sha256 = LOWER(v.content_sha256)
         WHERE v.missing = 0 AND v.root_unavailable = 0
           AND v.content_sha256 IS NOT NULL AND v.content_sha256 <> ''
         ORDER BY (c.checked_at IS NOT NULL) ASC, c.checked_at ASC, v.id ASC",
    )
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)
}

/// Distinct hashes a pass would look up right now, in processing order.
///
/// Two games sharing one archive hash produce one candidate, never two
/// requests.
async fn selection(
    state: &AppState,
    row: &RescanRow,
    now: DateTime<Utc>,
) -> Result<Vec<CandidateRow>, ApiError> {
    let mut seen = std::collections::HashSet::new();
    let mut selected = Vec::new();
    for candidate in candidate_rows(state).await? {
        if !is_stale(&candidate, now, row.interval_hours) {
            continue;
        }
        if is_malicious(&candidate) && row.include_malicious == 0 {
            continue;
        }
        if !seen.insert(candidate.sha256.clone()) {
            continue;
        }
        selected.push(candidate);
    }
    Ok(selected)
}

fn is_due(row: &RescanRow, now: DateTime<Utc>) -> bool {
    if row.enabled == 0 || row.interval_hours <= 0 {
        return false;
    }
    match row.next_run_at.as_deref().and_then(db::parse_time) {
        Some(next) => next <= now,
        // Enabled with no schedule: treat it as due so a corrupt or partial
        // row self-heals on the next tick instead of never running again.
        None => true,
    }
}

/// Current policy and what a pass would do right now.
pub async fn status(state: &AppState) -> Result<RescanStatus, ApiError> {
    let row = policy(state).await?;
    let now = db::now();
    let remaining = i64::try_from(selection(state, &row, now).await?.len()).unwrap_or(i64::MAX);
    Ok(row.status(remaining, is_due(&row, now)))
}

/// Saves the policy (admin only). The interval is validated, the schedule is
/// persisted, and the last run/error history is kept.
pub async fn save_policy(
    state: &AppState,
    input: RescanPolicyInput,
) -> Result<RescanStatus, ApiError> {
    validate_interval(input.interval_hours)?;
    let previous = load_row(state).await?;
    let now = db::now();
    let next = next_run_for(previous.as_ref(), now, input.enabled, input.interval_hours);
    let mut row = previous.clone().unwrap_or_default();
    row.enabled = i64::from(input.enabled);
    row.interval_hours = input.interval_hours;
    row.include_malicious = i64::from(input.include_malicious);
    row.next_run_at = next;
    db::query(
        "INSERT INTO virustotal_rescan
            (id, enabled, interval_hours, include_malicious, last_run_at, next_run_at,
             cursor, last_error, scanned_last_run, updated_at)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET
            enabled = excluded.enabled,
            interval_hours = excluded.interval_hours,
            include_malicious = excluded.include_malicious,
            last_run_at = excluded.last_run_at,
            next_run_at = excluded.next_run_at,
            cursor = excluded.cursor,
            last_error = excluded.last_error,
            scanned_last_run = excluded.scanned_last_run,
            updated_at = excluded.updated_at",
    )
    .bind(row.enabled)
    .bind(row.interval_hours)
    .bind(row.include_malicious)
    .bind(row.last_run_at.as_deref())
    .bind(row.next_run_at.as_deref())
    .bind(row.cursor.as_deref())
    .bind(row.last_error.as_deref())
    .bind(row.scanned_last_run)
    .bind(db::db_time(now))
    .execute(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    let saved = load_row(state).await?.unwrap_or(row);
    let remaining = i64::try_from(selection(state, &saved, now).await?.len()).unwrap_or(i64::MAX);
    Ok(saved.status(remaining, is_due(&saved, now)))
}

async fn persist_outcome(
    state: &AppState,
    row: &RescanRow,
    now: DateTime<Utc>,
    scanned: i64,
    cursor: Option<&str>,
    error: Option<&str>,
) {
    let next = if row.enabled != 0 && row.interval_hours > 0 {
        Some(db::db_time(now + ChronoDuration::hours(row.interval_hours)))
    } else {
        None
    };
    let write = db::query(
        "UPDATE virustotal_rescan
         SET last_run_at = ?, next_run_at = ?, cursor = ?, last_error = ?,
             scanned_last_run = ?, updated_at = ?
         WHERE id = 1",
    )
    .bind(db::db_time(now))
    .bind(next.as_deref())
    .bind(cursor)
    .bind(error)
    .bind(scanned)
    .bind(db::db_time(now))
    .execute(&state.core.pool)
    .await;
    if let Err(error) = write {
        tracing::warn!(%error, "could not persist the VirusTotal re-check outcome");
    }
}

/// Category of a failure, for the audit metadata (never the raw message).
fn error_category(error: &str) -> &'static str {
    if error.starts_with("invalid_key") {
        "invalid_key"
    } else if error.starts_with("quota_exceeded") {
        "quota_exceeded"
    } else if error.starts_with("disabled") {
        "disabled"
    } else {
        "outage"
    }
}

/// Runs one pass over the current selection.
///
/// The caller guarantees a single pass at a time. Every lookup goes through
/// [`virustotal::lookup_hash`] with `force: true`, so a stale-but-cached
/// report is actually refreshed, and the shared minute window/daily budget
/// still applies. Requests are paced at one per slot so the pass never fights
/// the public 4/minute limit.
async fn execute(state: &AppState, row: &RescanRow, actor: Actor) -> Result<RescanRun, ApiError> {
    let now = db::now();
    if row.enabled == 0 {
        return Err(ApiError::conflict(
            "rescan_disabled",
            "Periodic VirusTotal re-checks are disabled",
        ));
    }
    let mut selected = selection(state, row, now).await?;
    let mut scanned: i64 = 0;
    let mut cursor = row.cursor.clone();
    let mut error: Option<String> = None;
    let mut budget_exhausted = false;

    let key = match virustotal::key_state(state).await {
        KeyState::Key(key) => Some(key),
        KeyState::Missing | KeyState::Unreadable(_) => None,
    };
    if key.is_none() {
        error = Some("disabled: no VirusTotal API key is configured".to_string());
        selected.clear();
    }

    let per_minute = state.core.config.virustotal_per_minute;
    let pace_ms = if per_minute == 0 {
        0
    } else {
        60_000 / u64::from(per_minute)
    };
    let daily_limit = state.core.config.virustotal_daily_limit;
    let mut consecutive_failures: u32 = 0;

    for (index, candidate) in selected.iter().enumerate() {
        if virustotal::used_today(state).await >= daily_limit {
            budget_exhausted = true;
            error = Some(
                "quota_exceeded: the daily VirusTotal request budget is exhausted; the remaining archives resume after the UTC reset"
                    .to_string(),
            );
            break;
        }
        if index > 0 && pace_ms > 0 {
            tokio::time::sleep(Duration::from_millis(pace_ms)).await;
        }
        let report = virustotal::lookup_hash(
            state,
            key.as_ref(),
            candidate.version_id,
            &candidate.sha256,
            true,
        )
        .await;
        virustotal::store_link(
            state,
            candidate.game_id,
            Some(candidate.version_id),
            &candidate.sha256,
            report.verdict,
            ScanMode::Hash,
            report.source,
        )
        .await;
        cursor = Some(candidate.sha256.clone());
        match report.verdict {
            Verdict::QuotaExceeded => {
                budget_exhausted = true;
                error = Some(
                    "quota_exceeded: the VirusTotal rate limit is reached; the remaining archives resume at the next run"
                        .to_string(),
                );
                break;
            }
            Verdict::Disabled => {
                error = Some("disabled: no VirusTotal API key is configured".to_string());
                break;
            }
            Verdict::Error => {
                error = Some("invalid_key: VirusTotal rejected the API key".to_string());
                break;
            }
            Verdict::Unknown => {
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    error = Some(
                        "outage: VirusTotal is unreachable; the pass stopped and resumes at the next run"
                            .to_string(),
                    );
                    break;
                }
            }
            // A fresh answer: clean, suspicious, malicious or never seen.
            Verdict::Clean | Verdict::Suspicious | Verdict::Malicious | Verdict::NotFound => {
                consecutive_failures = 0;
                scanned += 1;
            }
        }
    }

    let finalized = db::now();
    // A pass that ends on upstream failures without hitting the stop
    // threshold (a short queue) still records why nothing was refreshed.
    if error.is_none() && consecutive_failures > 0 {
        error = Some(
            "outage: VirusTotal is unreachable; the remaining archives resume at the next run"
                .to_string(),
        );
    }
    persist_outcome(
        state,
        row,
        finalized,
        scanned,
        cursor.as_deref(),
        error.as_deref(),
    )
    .await;
    let mut updated = row.clone();
    updated.last_run_at = Some(db::db_time(finalized));
    updated.next_run_at = if row.enabled != 0 && row.interval_hours > 0 {
        Some(db::db_time(
            finalized + ChronoDuration::hours(row.interval_hours),
        ))
    } else {
        None
    };
    updated.cursor = cursor;
    updated.last_error = error.clone();
    updated.scanned_last_run = scanned;
    let remaining =
        i64::try_from(selection(state, &updated, finalized).await?.len()).unwrap_or(i64::MAX);

    state.core.events.emit(
        Event::new(actions::VIRUSTOTAL_RESCAN_RUN, actor)
            .target("virustotal", "rescan")
            .metadata(json!({
                "scanned": scanned,
                "remaining": remaining,
                "budgetExhausted": budget_exhausted,
                "error": error.as_deref().map(error_category),
            })),
    );

    Ok(RescanRun {
        scanned,
        remaining,
        error,
        budget_exhausted,
    })
}

/// Runs a pass now (admin action), whether or not it is due.
pub async fn run_now(state: &AppState, actor: Actor) -> Result<RescanStatus, ApiError> {
    let row = policy(state).await?;
    if row.enabled == 0 {
        return Err(ApiError::conflict(
            "rescan_disabled",
            "Enable periodic re-checks before running a pass",
        ));
    }
    if !state.integrations.virustotal.claim_rescan() {
        return Err(ApiError::conflict(
            "rescan_running",
            "A VirusTotal re-check is already running",
        ));
    }
    let outcome = execute(state, &row, actor).await;
    state.integrations.virustotal.release_rescan();
    outcome?;
    status(state).await
}

/// Runs a pass when the persisted schedule says it is due. Returns the run
/// summary, or `None` when nothing ran.
pub async fn run_if_due(state: &AppState) -> Option<RescanRun> {
    let row = match policy(state).await {
        Ok(row) => row,
        Err(error) => {
            tracing::warn!(error = ?error, "could not read the VirusTotal re-check policy");
            return None;
        }
    };
    if !is_due(&row, db::now()) {
        return None;
    }
    if !state.integrations.virustotal.claim_rescan() {
        return None;
    }
    let outcome = execute(state, &row, Actor::System).await;
    state.integrations.virustotal.release_rescan();
    match outcome {
        Ok(run) => {
            if let Some(error) = run.error.as_deref() {
                tracing::warn!(
                    scanned = run.scanned,
                    remaining = run.remaining,
                    error,
                    "VirusTotal periodic re-check stopped early"
                );
            } else {
                tracing::info!(
                    scanned = run.scanned,
                    remaining = run.remaining,
                    "VirusTotal periodic re-check finished"
                );
            }
            Some(run)
        }
        Err(error) => {
            tracing::warn!(error = ?error, "VirusTotal periodic re-check failed");
            None
        }
    }
}

/// Background worker: one per server, started with the app.
///
/// The tick is short enough to honour `next_run_at` closely and cheap (one
/// policy row). A pass never blocks startup or the HTTP server.
pub async fn scheduled_worker(state: AppState, mut cancel: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(state.core.config.virustotal_rescan_tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let _ = run_if_due(&state).await;
            }
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
            }
        }
    }
}

/// Human label for an interval, shared by the audit payload and the docs.
#[must_use]
pub fn interval_label(hours: i64) -> &'static str {
    match hours {
        0 => "manual",
        24 => "daily",
        168 => "weekly",
        720 => "monthly",
        _ => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn row(enabled: i64, interval: i64, next: Option<&str>) -> RescanRow {
        RescanRow {
            enabled,
            interval_hours: interval,
            next_run_at: next.map(str::to_string),
            ..RescanRow::default()
        }
    }

    #[test]
    fn intervals_are_the_four_offered_values_only() {
        for hours in INTERVALS {
            assert!(validate_interval(hours).is_ok(), "{hours}");
        }
        for hours in [-24, 1, 2, 12, 48, 336, 721] {
            assert!(validate_interval(hours).is_err(), "{hours}");
        }
    }

    #[test]
    fn disabling_or_manual_clears_the_schedule() {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
        assert_eq!(next_run_for(None, now, false, 24), None);
        assert_eq!(next_run_for(None, now, true, 0), None);
        assert_eq!(
            next_run_for(None, now, true, 24),
            Some("2026-11-17T12:00:00Z".to_string())
        );
    }

    #[test]
    fn editing_the_policy_keeps_a_future_run() {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
        let previous = row(1, 24, Some("2026-11-17T12:00:00Z"));
        assert_eq!(
            next_run_for(Some(&previous), now, true, 24),
            Some("2026-11-17T12:00:00Z".to_string()),
            "the same enabled interval never postpones a due pass"
        );
        assert_eq!(
            next_run_for(Some(&previous), now, true, 168),
            Some("2026-11-23T12:00:00Z".to_string()),
            "a new interval starts a fresh period"
        );
        let past = row(1, 24, Some("2026-11-15T12:00:00Z"));
        assert_eq!(
            next_run_for(Some(&past), now, true, 24),
            Some("2026-11-17T12:00:00Z".to_string()),
            "a due or overdue run is rescheduled from now"
        );
    }

    #[test]
    fn restart_keeps_the_persisted_schedule() {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
        let persisted = row(1, 24, Some("2026-11-17T12:00:00Z"));
        assert!(!is_due(&persisted, now));
        assert!(is_due(&row(1, 24, Some("2026-11-16T11:59:00Z")), now));
        assert!(!is_due(&row(0, 24, Some("2026-11-16T11:59:00Z")), now));
        assert!(!is_due(&row(1, 0, Some("2026-11-16T11:59:00Z")), now));
        assert!(
            is_due(&row(1, 24, None), now),
            "an enabled row without a schedule self-heals"
        );
    }

    fn candidate(checked_at: Option<&str>, ttl: Option<i64>, verdict: &str) -> CandidateRow {
        CandidateRow {
            version_id: 1,
            game_id: 1,
            sha256: "a".repeat(64),
            checked_at: checked_at.map(str::to_string),
            ttl_seconds: ttl,
            verdict: Some(verdict.to_string()),
        }
    }

    #[test]
    fn staleness_follows_the_interval_then_the_cache_ttl() {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
        assert!(is_stale(&candidate(None, None, "clean"), now, 24));
        assert!(!is_stale(
            &candidate(Some("2026-11-16T11:00:00Z"), Some(604_800), "clean"),
            now,
            24
        ));
        assert!(is_stale(
            &candidate(Some("2026-11-15T11:00:00Z"), Some(604_800), "clean"),
            now,
            24
        ));
        // Manual only: the cache TTL decides.
        assert!(!is_stale(
            &candidate(Some("2026-11-10T12:00:00Z"), Some(604_800), "clean"),
            now,
            0
        ));
        assert!(is_stale(
            &candidate(Some("2026-11-08T12:00:00Z"), Some(86_400), "suspicious"),
            now,
            0
        ));
    }

    #[test]
    fn malicious_is_recognised_by_stored_verdict() {
        assert!(is_malicious(&candidate(None, None, "malicious")));
        assert!(!is_malicious(&candidate(None, None, "clean")));
    }

    #[test]
    fn failure_categories_never_leak_the_message() {
        assert_eq!(error_category("invalid_key: rejected"), "invalid_key");
        assert_eq!(error_category("quota_exceeded: 429"), "quota_exceeded");
        assert_eq!(error_category("disabled: no key"), "disabled");
        assert_eq!(error_category("outage: 503"), "outage");
    }

    #[test]
    fn interval_labels_match_the_contract() {
        assert_eq!(interval_label(0), "manual");
        assert_eq!(interval_label(24), "daily");
        assert_eq!(interval_label(168), "weekly");
        assert_eq!(interval_label(720), "monthly");
    }
}
