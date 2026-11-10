//! Share links: expiring, revocable, optionally password-protected public
//! links to one game or one version (#9).
//!
//! A link is a bearer capability. Only the SHA-256 of the token is stored; the
//! plaintext URL is returned to the editor once, at creation. Public access
//! never returns account data: [`ShareGame`] is built from a dedicated query
//! that does not join `progress`, `bookmarks` or `play_sessions`.
//!
//! Downloads reuse the short-lived download ticket (`?token=`), so Range
//! requests, object storage and `Content-Disposition` work unchanged. The
//! ticket is scoped to the linked game and version and expires after 15
//! minutes, independently of the share link itself.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::auth;
use crate::db;
use crate::error::ApiError;
use crate::media;
use crate::state::AppState;

/// Prefix of every share token.
pub const TOKEN_PREFIX: &str = "lsx_";
/// Random bytes behind one token.
const TOKEN_BYTES: usize = 32;
/// Access attempts allowed per token and window.
pub const RATE_LIMIT_ATTEMPTS: usize = 30;
/// Length of the rate-limit window.
pub const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
/// Upper bound on tracked tokens, so a flood of random tokens cannot grow the
/// in-memory limiter without limit.
const MAX_TRACKED_TOKENS: usize = 4096;

const LINK_COLUMNS: &str = "s.id, s.game_id, s.version_id, s.label, s.created_by, s.created_at, \
s.expires_at, s.password_hash, s.max_downloads, s.download_count, s.revoked_at, \
s.last_accessed_at, g.title AS game_title";

const LINK_FROM: &str = " FROM share_links s LEFT JOIN games g ON g.id = s.game_id";

/// Generates a new share token: 32 random bytes as `lsx_<hex>`.
#[must_use]
pub fn generate_token() -> String {
    let mut bytes = [0_u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", hex::encode(bytes))
}

/// Hashes a token for storage and lookup.
#[must_use]
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Body of `POST /api/games/{id}/shares`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateShareRequest {
    /// Version to share; `None` serves the latest available version at access
    /// time.
    #[serde(default)]
    pub version_id: Option<i64>,
    /// Lifetime in hours (1..=8760); `None` never expires.
    #[serde(default)]
    pub expires_in_hours: Option<i64>,
    /// Optional access password (at least 4 characters).
    #[serde(default)]
    pub password: Option<String>,
    /// Maximum number of downloads; `None` is unlimited.
    #[serde(default)]
    pub max_downloads: Option<i64>,
    /// Editor-facing label, never shown on the public page.
    #[serde(default)]
    pub label: Option<String>,
}

/// Body of `POST /api/share/{token}/access`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareAccessRequest {
    /// Password when the link is protected.
    #[serde(default)]
    pub password: Option<String>,
}

/// Editor-facing view of a share link. The plaintext token is present only in
/// the creation response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareLink {
    /// Row id.
    pub id: i64,
    /// Plaintext token; only present on creation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Shared game.
    pub game_id: i64,
    /// Shared version, `None` for "latest at access time".
    pub version_id: Option<i64>,
    /// Game title, for the management list.
    pub game_title: Option<String>,
    /// Editor label.
    pub label: Option<String>,
    /// RFC 3339 expiry, `None` when the link never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the link requires a password.
    pub has_password: bool,
    /// Download cap, `None` when unlimited.
    pub max_downloads: Option<i64>,
    /// Successful accesses so far.
    pub download_count: i64,
    /// Revocation timestamp.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Public page: game/version metadata only, no account data.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    /// Public game metadata.
    pub game: ShareGame,
    /// Public version metadata.
    pub version: ShareVersion,
    /// Share expiry, `None` when it never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the access route needs a password.
    pub has_password: bool,
    /// Downloads left, `None` when unlimited.
    pub remaining_downloads: Option<i64>,
}

/// Public game metadata. Deliberately limited to what a store page shows:
/// progress, bookmarks, playtime, file paths and other games never leak here.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareGame {
    /// Game id.
    pub id: i64,
    /// Display title.
    pub title: String,
    /// Description, when known.
    pub description: Option<String>,
    /// Cover image URL, when cached.
    pub cover_url: Option<String>,
    /// Background image URL, when cached.
    pub background_url: Option<String>,
    /// Screenshot URLs.
    pub screenshots: Vec<String>,
    /// Release date, when known.
    pub release_date: Option<DateTime<Utc>>,
    /// Aggregated rating (0-100), when known.
    pub rating: Option<f64>,
    /// Genre labels.
    pub genres: Vec<String>,
    /// Developer names.
    pub developers: Vec<String>,
    /// Publisher names.
    pub publishers: Vec<String>,
    /// Minimum age, when known.
    pub age_rating: Option<u8>,
}

/// Public version metadata.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareVersion {
    /// Version id.
    pub id: i64,
    /// Version label parsed from the file name, when known.
    pub version: Option<String>,
    /// Archive size in bytes.
    pub size: i64,
    /// Version release date, when known.
    pub release_date: Option<DateTime<Utc>>,
    /// Whether this is an early-access build.
    pub early_access: bool,
}

/// Response of `POST /api/share/{token}/access`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareAccessResponse {
    /// Short-lived download URL carrying a scoped ticket.
    pub url: String,
    /// Ticket expiry (15 minutes).
    pub expires_at: DateTime<Utc>,
}

/// Why a share link or an access attempt was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareDenial {
    /// Unknown token.
    NotFound,
    /// The link was revoked.
    Revoked,
    /// The link expired.
    Expired,
    /// The download cap was reached.
    Exhausted,
}

impl ShareDenial {
    /// 404 used by both the metadata page and a revoked/unknown access.
    #[must_use]
    pub fn not_found_error(self) -> ApiError {
        ApiError::not_found(
            "share_not_found",
            "This share link is invalid, revoked or expired",
        )
    }

    /// Error returned by the access route.
    #[must_use]
    pub fn access_error(self) -> ApiError {
        match self {
            ShareDenial::NotFound | ShareDenial::Revoked => self.not_found_error(),
            ShareDenial::Expired => ApiError::new(
                axum::http::StatusCode::GONE,
                "share_expired",
                "This share link has expired",
            ),
            ShareDenial::Exhausted => ApiError::new(
                axum::http::StatusCode::GONE,
                "share_exhausted",
                "This share link reached its download limit",
            ),
        }
    }

    /// Stable reason recorded in the audit trail.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            ShareDenial::NotFound => "not_found",
            ShareDenial::Revoked => "revoked",
            ShareDenial::Expired => "expired",
            ShareDenial::Exhausted => "exhausted",
        }
    }
}

/// Row of the `share_links` table joined with the game title.
#[derive(Debug, sqlx::FromRow)]
pub struct ShareLinkRow {
    /// Row id.
    pub id: i64,
    /// Shared game.
    pub game_id: i64,
    /// Shared version, `None` for "latest at access time".
    pub version_id: Option<i64>,
    /// Editor label.
    pub label: Option<String>,
    /// Creator account.
    pub created_by: i64,
    /// Creation timestamp (RFC 3339 text).
    pub created_at: String,
    /// Expiry timestamp (RFC 3339 text).
    pub expires_at: Option<String>,
    /// Argon2 password hash, when protected.
    pub password_hash: Option<String>,
    /// Download cap.
    pub max_downloads: Option<i64>,
    /// Successful accesses so far.
    pub download_count: i64,
    /// Revocation timestamp.
    pub revoked_at: Option<String>,
    /// Last successful access.
    pub last_accessed_at: Option<String>,
    /// Game title from the join.
    pub game_title: Option<String>,
}

impl ShareLinkRow {
    /// Converts the row to the API shape.
    #[must_use]
    pub fn to_public(&self, token: Option<String>) -> ShareLink {
        ShareLink {
            id: self.id,
            token,
            game_id: self.game_id,
            version_id: self.version_id,
            game_title: self.game_title.clone(),
            label: self.label.clone(),
            expires_at: self.expires_at.as_deref().and_then(db::parse_time),
            has_password: self.password_hash.is_some(),
            max_downloads: self.max_downloads,
            download_count: self.download_count,
            revoked_at: self.revoked_at.as_deref().and_then(db::parse_time),
            created_at: db::parse_time(&self.created_at).unwrap_or_default(),
        }
    }
}

/// Values written by `POST /api/games/{id}/shares`.
#[derive(Debug)]
pub struct NewShare<'a> {
    /// SHA-256 of the plaintext token.
    pub token_hash: &'a str,
    /// Shared game.
    pub game_id: i64,
    /// Shared version, `None` for "latest at access time".
    pub version_id: Option<i64>,
    /// Editor label.
    pub label: Option<&'a str>,
    /// Creator account.
    pub created_by: i64,
    /// Expiry, `None` never.
    pub expires_at: Option<DateTime<Utc>>,
    /// Argon2 password hash, when protected.
    pub password_hash: Option<&'a str>,
    /// Download cap, `None` unlimited.
    pub max_downloads: Option<i64>,
}

/// Inserts a share link and returns its id.
pub async fn insert(pool: &SqlitePool, share: &NewShare<'_>) -> Result<i64, ApiError> {
    let result = sqlx::query(
        "INSERT INTO share_links
            (token_hash, game_id, version_id, label, created_by, created_at, expires_at,
             password_hash, max_downloads, download_count, revoked_at, last_accessed_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, NULL, NULL)",
    )
    .bind(share.token_hash)
    .bind(share.game_id)
    .bind(share.version_id)
    .bind(share.label)
    .bind(share.created_by)
    .bind(db::db_time(db::now()))
    .bind(share.expires_at.map(db::db_time))
    .bind(share.password_hash)
    .bind(share.max_downloads)
    .execute(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(result.last_insert_rowid())
}

/// Lists every share link, newest first, with the game title.
pub async fn list(pool: &SqlitePool) -> Result<Vec<ShareLinkRow>, ApiError> {
    let sql = format!("SELECT {LINK_COLUMNS}{LINK_FROM} ORDER BY s.created_at DESC, s.id DESC");
    sqlx::query_as::<_, ShareLinkRow>(&sql)
        .fetch_all(pool)
        .await
        .map_err(ApiError::from)
}

/// Reads one share link by id.
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<ShareLinkRow>, ApiError> {
    let sql = format!("SELECT {LINK_COLUMNS}{LINK_FROM} WHERE s.id = ?");
    sqlx::query_as::<_, ShareLinkRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)
}

/// Reads a share link by token hash. Revocation and expiry are not applied.
pub async fn find_by_token(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<ShareLinkRow>, ApiError> {
    let sql = format!("SELECT {LINK_COLUMNS}{LINK_FROM} WHERE s.token_hash = ?");
    sqlx::query_as::<_, ShareLinkRow>(&sql)
        .bind(token_hash)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)
}

/// Reads a share link that may still be shown publicly: unknown, revoked,
/// expired and exhausted links all answer `None` (404 on the metadata page).
pub async fn find_valid_by_token(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<ShareLinkRow>, ApiError> {
    let Some(row) = find_by_token(pool, token_hash).await? else {
        return Ok(None);
    };
    Ok(validate(&row, db::now()).ok().map(|()| row))
}

/// Soft-revokes a share link.
pub async fn revoke(pool: &SqlitePool, id: i64) -> Result<(), ApiError> {
    sqlx::query("UPDATE share_links SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(db::db_time(db::now()))
        .bind(id)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

/// Applies the lifecycle rules: revoked, expired, exhausted.
pub fn validate(share: &ShareLinkRow, now: DateTime<Utc>) -> Result<(), ShareDenial> {
    if share.revoked_at.is_some() {
        return Err(ShareDenial::Revoked);
    }
    if let Some(expires_at) = share.expires_at.as_deref().and_then(db::parse_time) {
        if expires_at <= now {
            return Err(ShareDenial::Expired);
        }
    }
    if share
        .max_downloads
        .is_some_and(|max| share.download_count >= max)
    {
        return Err(ShareDenial::Exhausted);
    }
    Ok(())
}

/// Downloads left before the cap, `None` when unlimited.
#[must_use]
pub fn remaining_downloads(share: &ShareLinkRow) -> Option<i64> {
    share
        .max_downloads
        .map(|max| (max - share.download_count).max(0))
}

/// Checks the presented password and always spends exactly one Argon2
/// verification, including for unprotected links and unknown tokens: response
/// time must not reveal whether a token exists or is password-protected.
///
/// Returns `false` only when a password is required and does not match.
#[must_use]
pub fn verify_access(share: Option<&ShareLinkRow>, password: Option<&str>) -> bool {
    match share {
        Some(share) => match share.password_hash.as_deref() {
            Some(hash) => password.is_some_and(|value| auth::verify_password(value, hash)),
            None => {
                spend_password_check();
                true
            }
        },
        None => {
            spend_password_check();
            false
        }
    }
}

/// Atomically reserves one download: increments `download_count`, stamps
/// `last_accessed_at` and re-checks expiry, revocation and the cap in the same
/// statement so two concurrent requests cannot exceed `max_downloads`.
///
/// `Ok(None)` means a rule changed since the caller loaded the row; the caller
/// re-reads it to pick the reason.
pub async fn claim_access(
    pool: &SqlitePool,
    share: &ShareLinkRow,
    now: DateTime<Utc>,
) -> Result<Option<i64>, ApiError> {
    let now_text = db::db_time(now);
    let result = sqlx::query(
        "UPDATE share_links
            SET download_count = download_count + 1, last_accessed_at = ?
          WHERE id = ?
            AND revoked_at IS NULL
            AND (expires_at IS NULL OR expires_at > ?)
            AND (max_downloads IS NULL OR download_count < max_downloads)",
    )
    .bind(&now_text)
    .bind(share.id)
    .bind(&now_text)
    .execute(pool)
    .await
    .map_err(ApiError::from)?;
    if result.rows_affected() == 1 {
        return Ok(Some(share.download_count.saturating_add(1)));
    }
    Ok(None)
}

/// Row of the `game_versions` columns a share exposes.
#[derive(Debug, sqlx::FromRow)]
pub struct ShareVersionRow {
    /// Version id.
    pub id: i64,
    /// Version label.
    pub version: Option<String>,
    /// Archive size.
    pub size: i64,
    /// Release date (RFC 3339 text).
    pub release_date: Option<String>,
    /// Early-access flag.
    pub early_access: i64,
}

impl ShareVersionRow {
    /// Converts the row to the API shape.
    #[must_use]
    pub fn to_public(&self) -> ShareVersion {
        ShareVersion {
            id: self.id,
            version: self.version.clone(),
            size: self.size,
            release_date: self.release_date.as_deref().and_then(db::parse_time),
            early_access: self.early_access != 0,
        }
    }
}

/// Resolves the shared version: the explicit one when set, otherwise the
/// newest available version at access time. Always constrained to `game_id`.
pub async fn resolve_version(
    pool: &SqlitePool,
    game_id: i64,
    version_id: Option<i64>,
) -> Result<Option<ShareVersionRow>, ApiError> {
    let row = match version_id {
        Some(version_id) => sqlx::query_as::<_, ShareVersionRow>(
            "SELECT id, version, size, release_date, early_access
                 FROM game_versions WHERE id = ? AND game_id = ? AND missing = 0",
        )
        .bind(version_id)
        .bind(game_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?,
        None => sqlx::query_as::<_, ShareVersionRow>(
            "SELECT id, version, size, release_date, early_access
                 FROM game_versions WHERE game_id = ? AND missing = 0
                 ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC LIMIT 1",
        )
        .bind(game_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?,
    };
    Ok(row)
}

/// Public game metadata. Dedicated query: no `progress`, `bookmarks`,
/// `play_sessions` or library-root joins, so no account data or file path can
/// leak even if the caller is anonymous.
pub async fn game_metadata(pool: &SqlitePool, game_id: i64) -> Result<Option<ShareGame>, ApiError> {
    #[derive(Debug, sqlx::FromRow)]
    struct ShareGameRow {
        id: i64,
        title: String,
        description: Option<String>,
        cover_path: Option<String>,
        background_path: Option<String>,
        screenshots_json: String,
        release_date: Option<String>,
        rating: Option<f64>,
        genres_json: String,
        developers_json: String,
        publishers_json: String,
        age_rating: Option<i64>,
    }

    let row = sqlx::query_as::<_, ShareGameRow>(
        "SELECT id, title, description, cover_path, background_path, screenshots_json,
                release_date, rating, genres_json, developers_json, publishers_json, age_rating
         FROM games WHERE id = ?",
    )
    .bind(game_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;

    Ok(row.map(|row| ShareGame {
        id: row.id,
        title: row.title,
        description: row.description,
        cover_url: row.cover_path.as_deref().map(media::media_url),
        background_url: row.background_path.as_deref().map(media::media_url),
        screenshots: db::parse_json_list(&row.screenshots_json)
            .iter()
            .map(|name| media::media_url(name))
            .collect(),
        release_date: row.release_date.as_deref().and_then(db::parse_time),
        rating: row.rating,
        genres: db::parse_json_list(&row.genres_json),
        developers: db::parse_json_list(&row.developers_json),
        publishers: db::parse_json_list(&row.publishers_json),
        age_rating: row.age_rating.and_then(|value| u8::try_from(value).ok()),
    }))
}

/// Issues a short-lived download ticket scoped to the shared game and version.
pub fn ticket_url(
    state: &AppState,
    share: &ShareLinkRow,
    version_id: i64,
) -> Result<(String, DateTime<Utc>), ApiError> {
    let (ticket, expires_at) = auth::issue_download_ticket(
        &state.jwt_secret,
        share.created_by,
        share.game_id,
        version_id,
    )?;
    let url = format!(
        "/api/games/{}/download?versionId={version_id}&token={ticket}",
        share.game_id
    );
    Ok((url, expires_at))
}

/// Whether a public access attempt for `token_hash` is allowed. The attempt is
/// consumed even when the password later turns out to be correct, so a link
/// cannot be hammered.
#[must_use]
pub fn allow_attempt(token_hash: &str) -> bool {
    limiter().allow_at(token_hash, Instant::now())
}

/// Runs one throwaway Argon2 verification against a fixed hash, used when a
/// request carries no real hash to check.
fn spend_password_check() {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY.get_or_init(|| auth::hash_password("ludex-share-dummy").unwrap_or_default());
    if !hash.is_empty() {
        let _ = auth::verify_password("ludex-share-dummy", hash);
    }
}

/// Per-token sliding-window limiter for public access attempts.
#[derive(Debug, Default)]
struct AttemptLimiter {
    windows: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl AttemptLimiter {
    fn allow_at(&self, token_hash: &str, now: Instant) -> bool {
        let Ok(mut windows) = self.windows.lock() else {
            // A poisoned lock must never lock the public route out.
            return true;
        };
        if !windows.contains_key(token_hash) && windows.len() >= MAX_TRACKED_TOKENS {
            prune(&mut windows, now);
            if windows.len() >= MAX_TRACKED_TOKENS {
                return false;
            }
        }
        let window = windows.entry(token_hash.to_string()).or_default();
        expire(window, now);
        if window.len() >= RATE_LIMIT_ATTEMPTS {
            return false;
        }
        window.push_back(now);
        true
    }
}

fn expire(window: &mut VecDeque<Instant>, now: Instant) {
    while window
        .front()
        .is_some_and(|attempt| now.saturating_duration_since(*attempt) >= RATE_LIMIT_WINDOW)
    {
        window.pop_front();
    }
}

fn prune(windows: &mut HashMap<String, VecDeque<Instant>>, now: Instant) {
    windows.retain(|_, window| {
        expire(window, now);
        !window.is_empty()
    });
}

fn limiter() -> &'static AttemptLimiter {
    static LIMITER: OnceLock<AttemptLimiter> = OnceLock::new();
    LIMITER.get_or_init(AttemptLimiter::default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    fn row(
        expires_at: Option<DateTime<Utc>>,
        revoked_at: Option<DateTime<Utc>>,
        max_downloads: Option<i64>,
        download_count: i64,
    ) -> ShareLinkRow {
        ShareLinkRow {
            id: 1,
            game_id: 2,
            version_id: None,
            label: None,
            created_by: 3,
            created_at: db::db_time(db::now()),
            expires_at: expires_at.map(db::db_time),
            password_hash: None,
            max_downloads,
            download_count,
            revoked_at: revoked_at.map(db::db_time),
            last_accessed_at: None,
            game_title: Some("Hades".to_string()),
        }
    }

    #[test]
    fn tokens_are_random_and_hashed_with_sha256() {
        let first = generate_token();
        let second = generate_token();
        assert!(first.starts_with(TOKEN_PREFIX));
        assert_eq!(first.len(), TOKEN_PREFIX.len() + TOKEN_BYTES * 2);
        assert_ne!(first, second);
        assert_eq!(hash_token(&first).len(), 64);
        assert_eq!(hash_token(&first), hash_token(&first));
        assert_ne!(hash_token(&first), hash_token(&second));
    }

    #[test]
    fn validation_rejects_revoked_expired_and_exhausted_links() {
        let now = db::now();
        let live = row(None, None, None, 0);
        assert_eq!(validate(&live, now), Ok(()));

        let revoked = row(None, Some(now), None, 0);
        assert_eq!(validate(&revoked, now), Err(ShareDenial::Revoked));

        let expired = row(Some(now - ChronoDuration::hours(1)), None, None, 0);
        assert_eq!(validate(&expired, now), Err(ShareDenial::Expired));

        let future = row(Some(now + ChronoDuration::hours(1)), None, None, 0);
        assert_eq!(validate(&future, now), Ok(()));

        let exhausted = row(None, None, Some(3), 3);
        assert_eq!(validate(&exhausted, now), Err(ShareDenial::Exhausted));
        let almost = row(None, None, Some(3), 2);
        assert_eq!(validate(&almost, now), Ok(()));
    }

    #[test]
    fn remaining_downloads_and_passwords_are_enforced() {
        let unlimited = row(None, None, None, 7);
        assert_eq!(remaining_downloads(&unlimited), None);
        let capped = row(None, None, Some(5), 2);
        assert_eq!(remaining_downloads(&capped), Some(3));
        let over = row(None, None, Some(5), 9);
        assert_eq!(remaining_downloads(&over), Some(0));

        // A link without a password accepts every request.
        assert!(verify_access(Some(&capped), None));

        let mut protected = row(None, None, None, 0);
        protected.password_hash = Some(auth::hash_password("open sesame").expect("hash"));
        assert!(!verify_access(Some(&protected), None));
        assert!(!verify_access(Some(&protected), Some("wrong")));
        assert!(verify_access(Some(&protected), Some("open sesame")));

        // Unknown tokens never validate, but still cost one verification.
        assert!(!verify_access(None, None));
    }

    #[test]
    fn access_attempts_are_rate_limited_per_token() {
        let limiter = AttemptLimiter::default();
        let start = Instant::now();
        for _ in 0..RATE_LIMIT_ATTEMPTS {
            assert!(limiter.allow_at("hash-a", start));
        }
        assert!(!limiter.allow_at("hash-a", start));
        assert!(limiter.allow_at("hash-b", start));

        // The window slides: once it elapses the token is usable again.
        let later = start + RATE_LIMIT_WINDOW + Duration::from_millis(1);
        for _ in 0..RATE_LIMIT_ATTEMPTS {
            assert!(limiter.allow_at("hash-a", later));
        }
        assert!(!limiter.allow_at("hash-a", later));
    }
}
