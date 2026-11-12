//! Mobile companion: remote install requests (#167).
//!
//! A phone cannot install a game itself: it has no library folder and no
//! launcher. Instead it queues a *request* on the server and a signed-in
//! desktop claims it, downloads the archive from the normal game endpoint and
//! reports progress back. The phone polls the same row and renders
//! queued/downloading/ready.
//!
//! Security model:
//! - every endpoint requires a session (JWT or a `write`-scoped API key);
//! - a request belongs to its founder: listing, cancelling and status updates
//!   only ever match `user_id` (admins can read, never mutate another row);
//! - one row per founder and game makes double taps idempotent;
//! - queueing is rate-limited per account, and every state transition is
//!   audited;
//! - no endpoint accepts a tunnel hostname or a device label as a credential:
//!   `device` is a label shown in the UI, nothing more.
//!
//! The transport stays optional: reach the server over HTTPS, a VPN or a
//! reverse proxy. See `docs/REMOTE-ACCESS.md`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db::{self, Database};
use crate::error::ApiError;
use crate::media::media_url;

/// Queue attempts allowed per account inside [`RATE_WINDOW`].
pub const RATE_LIMIT: u32 = 20;
/// Sliding window for [`RATE_LIMIT`].
pub const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Upper bound on tracked accounts before idle entries are dropped.
const MAX_RATE_KEYS: usize = 10_000;

/// Lifecycle of one remote install request.
///
/// `queued` and `claimed` are separate internally so the desktop can win the
/// race atomically, but the phone renders both as "waiting for the PC".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RemoteInstallStatus {
    /// Waiting for a desktop to claim it.
    Queued,
    /// A desktop claimed it and is preparing the download.
    Claimed,
    /// The desktop is downloading/extracting the archive.
    Downloading,
    /// The archive is installed and ready to play on the desktop.
    Ready,
    /// The desktop reported an error.
    Failed,
    /// The founder or an admin cancelled the request.
    Cancelled,
}

impl RemoteInstallStatus {
    /// Database representation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Downloading => "downloading",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses a database or query value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "queued" => Some(Self::Queued),
            "claimed" => Some(Self::Claimed),
            "downloading" => Some(Self::Downloading),
            "ready" => Some(Self::Ready),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Whether the request is still on its way to a desktop.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Claimed | Self::Downloading)
    }

    /// Whether a desktop may still transition into `target`.
    ///
    /// Repeating the current status is allowed so progress patches are
    /// idempotent; jumping straight from `queued` to `ready` is not, which
    /// keeps a lying or buggy client from skipping the download.
    #[must_use]
    pub fn can_transition(self, target: Self) -> bool {
        if self == target {
            return true;
        }
        match self {
            Self::Queued => matches!(target, Self::Claimed | Self::Cancelled),
            Self::Claimed => {
                matches!(target, Self::Downloading | Self::Failed | Self::Cancelled)
            }
            Self::Downloading => matches!(target, Self::Ready | Self::Failed | Self::Cancelled),
            Self::Ready | Self::Failed | Self::Cancelled => false,
        }
    }
}

/// One remote install request, as returned by the API.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInstall {
    /// Request id (UUID).
    pub id: String,
    /// Game the request targets.
    pub game_id: i64,
    /// Game title, snapshotted from the library.
    pub game_title: String,
    /// Cover URL (`/api/media/...`) or `null`.
    pub cover_url: Option<String>,
    /// Version index row, when it still exists.
    pub version_id: Option<i64>,
    /// Human-readable version label at request time.
    pub version: Option<String>,
    /// Archive size at request time, in bytes.
    pub size_bytes: Option<i64>,
    /// Free-form device label supplied by the phone.
    pub device: Option<String>,
    /// PC the phone targeted, or `null` for the account-wide queue.
    pub target_device_id: Option<String>,
    /// Display name of the target PC, resolved from the device registry.
    pub target_device_name: Option<String>,
    /// Device that actually claimed the request, when known.
    pub claimed_by_device_id: Option<String>,
    /// Current lifecycle status.
    pub status: RemoteInstallStatus,
    /// Download/extraction progress, 0..=100.
    pub progress: i64,
    /// Last message from the desktop (error text, "extracting", ...).
    pub message: Option<String>,
    /// When the phone queued the request.
    pub requested_at: String,
    /// Last state change.
    pub updated_at: String,
    /// When a desktop claimed the request.
    pub claimed_at: Option<String>,
    /// When the request reached a terminal state.
    pub completed_at: Option<String>,
}

impl RemoteInstall {
    /// Whether the request can still be cancelled by its founder.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status.is_active()
    }
}

/// Body of `POST /api/remote-installs`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRemoteInstallRequest {
    /// Game to install on the paired desktop.
    pub game_id: i64,
    /// Specific version to install; omit for the newest available one.
    #[serde(default)]
    pub version_id: Option<i64>,
    /// Human-readable device label, e.g. "Pixel 8". Never used for auth.
    #[serde(default)]
    pub device: Option<String>,
    /// Device registry id to target. Omit to keep the account-wide queue
    /// (any signed-in desktop may claim).
    #[serde(default)]
    pub target_device_id: Option<String>,
}

/// Body of `POST /api/remote-installs/{id}/status`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInstallStatusRequest {
    /// Next status. Repeating the current status is a no-op.
    pub status: RemoteInstallStatus,
    /// Progress percentage, 0..=100. Only meaningful while downloading.
    #[serde(default)]
    pub progress: Option<i64>,
    /// Short status line shown on the phone.
    #[serde(default)]
    pub message: Option<String>,
    /// Stable client id of the claiming desktop (#184). Required when the
    /// request is targeted, optional for the account-wide queue; the server
    /// resolves it to the registered device.
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug)]
struct Attempts {
    window_start: Instant,
    count: u32,
}

fn rate_buckets() -> &'static Mutex<HashMap<i64, Attempts>> {
    static BUCKETS: OnceLock<Mutex<HashMap<i64, Attempts>>> = OnceLock::new();
    BUCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Consumes one queue slot for `user_id`, returning `429` when exhausted.
///
/// In-memory on purpose: the counter only matters for the running process and
/// a restart must not lock a household out. The same choice is documented for
/// the request rate limiter in `security.rs`.
pub fn check_rate_limit(user_id: i64) -> Result<(), ApiError> {
    check_rate_limit_at(user_id, Instant::now())
}

fn check_rate_limit_at(user_id: i64, now: Instant) -> Result<(), ApiError> {
    let mut buckets = rate_buckets()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if buckets.len() > MAX_RATE_KEYS {
        buckets.retain(|_, attempt| now.duration_since(attempt.window_start) < RATE_WINDOW);
    }
    let attempt = buckets.entry(user_id).or_insert(Attempts {
        window_start: now,
        count: 0,
    });
    if now.duration_since(attempt.window_start) >= RATE_WINDOW {
        attempt.window_start = now;
        attempt.count = 0;
    }
    if attempt.count >= RATE_LIMIT {
        let retry = RATE_WINDOW.saturating_sub(now.duration_since(attempt.window_start));
        return Err(ApiError::too_many_requests(
            "rate_limited",
            format!(
                "Too many remote install requests; try again in {} seconds",
                retry.as_secs().max(1)
            ),
        ));
    }
    attempt.count += 1;
    Ok(())
}

/// Clears the in-memory queue counters. Tests use it so one case cannot leak
/// its budget into the next.
#[doc(hidden)]
pub fn reset_rate_limit() {
    rate_buckets()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}

/// A game resolved from the library for a new request.
struct GameTarget {
    age_rating: Option<u8>,
}

/// A version resolved for a new request.
struct VersionTarget {
    id: i64,
    label: Option<String>,
    size: Option<i64>,
}

async fn load_game(pool: &Database, game_id: i64) -> Result<GameTarget, ApiError> {
    let row = db::query_scalar::<Option<i64>>("SELECT age_rating FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;
    Ok(GameTarget {
        age_rating: row.and_then(|value| u8::try_from(value).ok()),
    })
}

/// Resolves the requested version, or the newest one that is still present.
async fn load_version(
    pool: &Database,
    game_id: i64,
    requested: Option<i64>,
) -> Result<VersionTarget, ApiError> {
    let row = if let Some(version_id) = requested {
        db::query_as::<(i64, Option<String>, i64)>(
            "SELECT id, version, size FROM game_versions
             WHERE id = ? AND game_id = ? AND missing = 0",
        )
        .bind(version_id)
        .bind(game_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_version",
                "This version does not belong to the game or is no longer available",
            )
        })?
    } else {
        db::query_as::<(i64, Option<String>, i64)>(
            "SELECT id, version, size FROM game_versions
             WHERE game_id = ? AND missing = 0
             ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC LIMIT 1",
        )
        .bind(game_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("version_not_found", "No downloadable version"))?
    };
    Ok(VersionTarget {
        id: row.0,
        label: row.1,
        size: Some(row.2).filter(|size| *size > 0),
    })
}

/// Flat row selected by [`list`] and [`get`]: request + game + version
/// snapshot, in column order.
#[derive(sqlx::FromRow)]
struct RemoteInstallRow {
    id: String,
    game_id: i64,
    game_title: String,
    cover_path: Option<String>,
    version_id: Option<i64>,
    version_label: Option<String>,
    size_bytes: Option<i64>,
    device: Option<String>,
    target_device_id: Option<String>,
    target_device_name: Option<String>,
    device_id: Option<String>,
    status: String,
    progress: i64,
    message: Option<String>,
    requested_at: String,
    updated_at: String,
    claimed_at: Option<String>,
    completed_at: Option<String>,
}

/// Lists a founder's requests, newest first.
///
/// `statuses` may hold several values (`queued,claimed`); an empty list means
/// every status. When `device_id` is set, only requests targeted at that device
/// plus the account-wide fallback are returned — that is what the desktop
/// polls, so a second PC never sees work meant for another one.
pub async fn list(
    pool: &Database,
    user_id: i64,
    statuses: &[RemoteInstallStatus],
    device_id: Option<&str>,
    limit: i64,
) -> Result<Vec<RemoteInstall>, ApiError> {
    let limit = limit.clamp(1, 200);
    let mut sql = String::from(
        "SELECT id, game_id, game_title, cover_path, version_id, version_label, size_bytes,
                device, target_device_id, target_device_name, device_id, status, progress,
                message, requested_at, updated_at, claimed_at, completed_at
         FROM (
             SELECT r.id, r.game_id, g.title AS game_title, g.cover_path, r.version_id,
                    r.version_label, r.size_bytes, r.device, r.target_device_id,
                    d.name AS target_device_name, r.device_id, r.status, r.progress,
                    r.message, r.requested_at, r.updated_at, r.claimed_at, r.completed_at,
                    r.user_id
             FROM remote_installs r
             JOIN games g ON g.id = r.game_id
             LEFT JOIN devices d ON d.id = r.target_device_id
         ) rows
         WHERE user_id = ?",
    );
    if !statuses.is_empty() {
        let placeholders = vec!["?"; statuses.len()].join(", ");
        sql.push_str(&format!(" AND status IN ({placeholders})"));
    }
    if device_id.is_some() {
        sql.push_str(" AND (target_device_id IS NULL OR target_device_id = ?)");
    }
    sql.push_str(" ORDER BY requested_at DESC, id DESC LIMIT ?");

    let mut query = db::query_as::<RemoteInstallRow>(&sql).bind(user_id);
    for status in statuses {
        query = query.bind(status.as_str());
    }
    if let Some(device_id) = device_id {
        query = query.bind(device_id);
    }
    let rows = query
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(ApiError::from)?;

    Ok(rows.into_iter().map(row_to_install).collect())
}

/// Reads one request owned by `user_id`.
pub async fn get(pool: &Database, user_id: i64, id: &str) -> Result<RemoteInstall, ApiError> {
    let row = db::query_as::<RemoteInstallRow>(
        "SELECT r.id, r.game_id, g.title AS game_title, g.cover_path, r.version_id,
                r.version_label, r.size_bytes, r.device, r.target_device_id,
                d.name AS target_device_name, r.device_id, r.status,
                r.progress, r.message, r.requested_at, r.updated_at, r.claimed_at,
                r.completed_at
         FROM remote_installs r
         JOIN games g ON g.id = r.game_id
         LEFT JOIN devices d ON d.id = r.target_device_id
         WHERE r.id = ? AND r.user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("remote_install_not_found", "Remote install not found"))?;
    Ok(row_to_install(row))
}

fn row_to_install(row: RemoteInstallRow) -> RemoteInstall {
    RemoteInstall {
        id: row.id,
        game_id: row.game_id,
        game_title: row.game_title,
        cover_url: row.cover_path.as_deref().map(media_url),
        version_id: row.version_id,
        version: row.version_label,
        size_bytes: row.size_bytes,
        device: row.device,
        target_device_id: row.target_device_id,
        target_device_name: row.target_device_name,
        claimed_by_device_id: row.device_id,
        status: RemoteInstallStatus::parse(&row.status).unwrap_or(RemoteInstallStatus::Queued),
        progress: row.progress,
        message: row.message,
        requested_at: row.requested_at,
        updated_at: row.updated_at,
        claimed_at: row.claimed_at,
        completed_at: row.completed_at,
    }
}

/// Result of queueing a request.
pub struct Queued {
    /// The row, new or already present.
    pub install: RemoteInstall,
    /// True when this call inserted (or re-queued) the row; used to audit a
    /// real state change rather than a repeated tap.
    pub created: bool,
    /// True only when a new row was inserted (HTTP 201 rather than 200).
    pub inserted: bool,
}

/// Queues an install, idempotently per founder and game.
///
/// A request that already exists in an active or `ready` state is returned
/// untouched. A cancelled or failed request is reset to `queued` so the phone
/// can retry without deleting history first.
pub async fn queue(
    pool: &Database,
    user_id: i64,
    age_limit: Option<u8>,
    request: &CreateRemoteInstallRequest,
) -> Result<Queued, ApiError> {
    if request.game_id <= 0 {
        return Err(ApiError::bad_request(
            "invalid_game_id",
            "gameId must be a positive game id",
        ));
    }
    let game = load_game(pool, request.game_id).await?;
    if matches!((age_limit, game.age_rating), (Some(limit), Some(rating)) if rating > limit) {
        // A restricted game is indistinguishable from a missing one (#39).
        return Err(ApiError::not_found("game_not_found", "Game not found"));
    }
    let version = load_version(pool, request.game_id, request.version_id).await?;
    let device = request
        .device
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(80).collect::<String>());
    let target_device_id = request
        .target_device_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(target) = &target_device_id {
        crate::devices::require_target(pool, user_id, target).await?;
    }
    let now = db::db_time(db::now());
    let id = uuid::Uuid::new_v4().to_string();

    let inserted = db::query(
        "INSERT INTO remote_installs
            (id, user_id, game_id, version_id, version_label, size_bytes, device,
             target_device_id, status, progress, requested_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'queued', 0, ?, ?)
         ON CONFLICT(user_id, game_id) DO NOTHING",
    )
    .bind(&id)
    .bind(user_id)
    .bind(request.game_id)
    .bind(version.id)
    .bind(&version.label)
    .bind(version.size)
    .bind(&device)
    .bind(&target_device_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(ApiError::from)?
    .rows_affected()
        > 0;

    // Read back by founder/game: after an insert this returns the new row,
    // after a unique collision it returns the existing one.
    let row = db::query_as::<(String, String)>(
        "SELECT id, status FROM remote_installs WHERE user_id = ? AND game_id = ?",
    )
    .bind(user_id)
    .bind(request.game_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)?;
    let status = RemoteInstallStatus::parse(&row.1).unwrap_or(RemoteInstallStatus::Queued);
    let mut created = inserted;
    if !inserted
        && matches!(
            status,
            RemoteInstallStatus::Failed | RemoteInstallStatus::Cancelled
        )
    {
        db::query(
            "UPDATE remote_installs
             SET status = 'queued', progress = 0, message = NULL, version_id = ?,
                 version_label = ?, size_bytes = ?, device = ?, target_device_id = ?,
                 device_id = NULL, updated_at = ?,
                 claimed_at = NULL, completed_at = NULL
             WHERE id = ? AND user_id = ?",
        )
        .bind(version.id)
        .bind(&version.label)
        .bind(version.size)
        .bind(&device)
        .bind(&target_device_id)
        .bind(&now)
        .bind(&row.0)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
        created = true;
    }
    let install = get(pool, user_id, &row.0).await?;
    Ok(Queued {
        install,
        created,
        inserted,
    })
}

/// Applies a desktop status update.
pub async fn update_status(
    pool: &Database,
    user_id: i64,
    id: &str,
    request: &RemoteInstallStatusRequest,
) -> Result<RemoteInstall, ApiError> {
    let current = get(pool, user_id, id).await?;
    if !current.status.can_transition(request.status) {
        return Err(ApiError::conflict(
            "invalid_transition",
            format!(
                "A {} request cannot become {}",
                current.status.as_str(),
                request.status.as_str()
            ),
        ));
    }
    if request.status == RemoteInstallStatus::Claimed {
        check_claim_device(pool, user_id, &current, request.client_id.as_deref()).await?;
    }
    let progress = request
        .progress
        .map(|value| value.clamp(0, 100))
        .unwrap_or_else(|| match request.status {
            RemoteInstallStatus::Ready => 100,
            _ => current.progress,
        });
    let now = db::db_time(db::now());
    let claimed_at = match (request.status, &current.claimed_at) {
        (RemoteInstallStatus::Claimed, None) => Some(now.clone()),
        (_, value) => value.clone(),
    };
    let completed_at = current
        .completed_at
        .clone()
        .or_else(|| (!request.status.is_active()).then(|| now.clone()));
    let message = request
        .message
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(500).collect::<String>())
        .or_else(|| match request.status {
            RemoteInstallStatus::Ready => None,
            _ => current.message.clone(),
        });
    let claimed_by_device_id = match request.status {
        RemoteInstallStatus::Claimed => match request.client_id.as_deref() {
            Some(client_id) if !client_id.trim().is_empty() => Some(
                crate::devices::resolve_client(pool, user_id, client_id)
                    .await?
                    .id,
            ),
            _ => current.claimed_by_device_id.clone(),
        },
        _ => current.claimed_by_device_id.clone(),
    };

    db::query(
        "UPDATE remote_installs
         SET status = ?, progress = ?, message = ?, updated_at = ?,
             claimed_at = ?, completed_at = ?, device_id = ?
         WHERE id = ? AND user_id = ?",
    )
    .bind(request.status.as_str())
    .bind(progress)
    .bind(&message)
    .bind(&now)
    .bind(&claimed_at)
    .bind(&completed_at)
    .bind(&claimed_by_device_id)
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(ApiError::from)?;

    get(pool, user_id, id).await
}

/// Rejects a claim when the request is targeted at another PC or the claiming
/// device is unknown/revoked.
///
/// Untargeted requests keep the historical any-desktop behaviour; a client id
/// is optional there but, when supplied, must still be an active device of the
/// account so a revoked client cannot claim work.
async fn check_claim_device(
    pool: &Database,
    user_id: i64,
    current: &RemoteInstall,
    client_id: Option<&str>,
) -> Result<(), ApiError> {
    let client_id = client_id.map(str::trim).filter(|value| !value.is_empty());
    if let Some(target) = current.target_device_id.as_deref() {
        let Some(client_id) = client_id else {
            return Err(ApiError::conflict(
                "wrong_device",
                "This install is reserved for another PC",
            ));
        };
        let device = crate::devices::resolve_client(pool, user_id, client_id).await?;
        if device.id != target {
            return Err(ApiError::conflict(
                "wrong_device",
                "This install is reserved for another PC",
            ));
        }
        crate::devices::require_target(pool, user_id, target).await?;
        return Ok(());
    }
    if let Some(client_id) = client_id {
        crate::devices::resolve_client(pool, user_id, client_id).await?;
    }
    Ok(())
}

/// Cancels or forgets a request.
///
/// Active rows become `cancelled` and stay visible as history; terminal rows
/// are deleted so the phone can clear its list.
pub async fn cancel(pool: &Database, user_id: i64, id: &str) -> Result<bool, ApiError> {
    let current = get(pool, user_id, id).await?;
    if current.status.is_active() {
        let now = db::db_time(db::now());
        db::query(
            "UPDATE remote_installs
             SET status = 'cancelled', message = NULL, updated_at = ?, completed_at = ?
             WHERE id = ? AND user_id = ?",
        )
        .bind(&now)
        .bind(&now)
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
        Ok(true)
    } else {
        db::query("DELETE FROM remote_installs WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(ApiError::from)?;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parsing_is_case_insensitive() {
        assert_eq!(
            RemoteInstallStatus::parse(" Downloading "),
            Some(RemoteInstallStatus::Downloading)
        );
        assert_eq!(RemoteInstallStatus::parse("nope"), None);
    }

    #[test]
    fn transitions_keep_a_desktop_honest() {
        use RemoteInstallStatus::*;
        assert!(Queued.can_transition(Claimed));
        assert!(Claimed.can_transition(Downloading));
        assert!(Downloading.can_transition(Ready));
        assert!(Downloading.can_transition(Downloading));
        assert!(!Queued.can_transition(Ready));
        assert!(!Ready.can_transition(Downloading));
        assert!(!Cancelled.can_transition(Queued));
    }

    #[test]
    fn rate_limit_counts_per_account_and_refills() {
        reset_rate_limit();
        let now = Instant::now();
        for _ in 0..RATE_LIMIT {
            assert!(check_rate_limit_at(7, now).is_ok());
        }
        let error = check_rate_limit_at(7, now).expect_err("limit reached");
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert!(check_rate_limit_at(8, now).is_ok());
        assert!(check_rate_limit_at(7, now + RATE_WINDOW).is_ok());
        reset_rate_limit();
    }
}
