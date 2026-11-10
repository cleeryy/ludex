//! Server-side DTOs and SQL row shapes.
//!
//! Response bodies reuse the shared `ludex_core::models` types so the web UI and
//! desktop app see exactly the contract's shapes.

use chrono::{DateTime, Utc};
use ludex_core::models::{GameSummary, LaunchInfo, ProgressState, Role};
use serde::{Deserialize, Deserializer, Serialize};

/// Row returned by the game list/random queries.
#[derive(Debug, sqlx::FromRow)]
pub struct GameListRow {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub release_date: Option<String>,
    pub cover_path: Option<String>,
    pub background_path: Option<String>,
    pub rating: Option<f64>,
    pub genres_json: String,
    pub early_access: i64,
    pub alt_titles_json: String,
    pub max_players: Option<i64>,
    pub provider_slug: Option<String>,
    pub state: String,
    pub minutes_played: i64,
    pub bookmarked: i64,
    pub last_played_at: Option<String>,
    pub version_count: i64,
    pub total_size: i64,
}

/// Row returned by the game detail query.
#[derive(Debug, sqlx::FromRow)]
pub struct GameDetailRow {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub description: Option<String>,
    pub release_date: Option<String>,
    pub cover_path: Option<String>,
    pub background_path: Option<String>,
    pub screenshots_json: String,
    pub trailers_json: String,
    pub genres_json: String,
    pub developers_json: String,
    pub publishers_json: String,
    pub tags_json: String,
    pub rating: Option<f64>,
    pub age_rating: Option<i64>,
    pub early_access: i64,
    pub alt_titles_json: String,
    pub max_players: Option<i64>,
    pub provider_slug: Option<String>,
    pub provider_id: Option<String>,
    pub provider_url: Option<String>,
    pub launch_executable: Option<String>,
    pub launch_parameters: Option<String>,
    pub installer_executable: Option<String>,
    pub installer_parameters: Option<String>,
    pub uninstaller_executable: Option<String>,
    pub uninstaller_parameters: Option<String>,
    pub state: String,
    pub minutes_played: i64,
    pub bookmarked: i64,
    pub last_played_at: Option<String>,
    pub version_count: i64,
    pub total_size: i64,
}

/// Row of the `game_versions` table.
#[derive(Debug, sqlx::FromRow)]
pub struct VersionRow {
    pub id: i64,
    pub root_id: Option<i64>,
    pub file_path: String,
    pub file_name: String,
    pub size: i64,
    pub version: Option<String>,
    pub release_date: Option<String>,
    pub early_access: i64,
    pub missing: i64,
    pub root_unavailable: i64,
    pub archive_hash: Option<String>,
    pub updated_at: Option<String>,
    /// `local` or `s3` (#47).
    pub storage_backend: Option<String>,
    /// Object key inside the bucket when `storage_backend = "s3"`.
    pub object_key: Option<String>,
    /// ETag reported by the object store on upload.
    pub etag: Option<String>,
}

/// Where an account's identity comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum UserSource {
    /// Identity and password are managed by Ludex.
    Local,
    /// Identity is managed by the linked OIDC provider.
    Oidc,
}

impl UserSource {
    /// Stable wire identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            UserSource::Local => "local",
            UserSource::Oidc => "oidc",
        }
    }
}

/// Public representation of an account, never including secrets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct User {
    /// Database identifier.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Optional e-mail address.
    pub email: Option<String>,
    /// Authorisation level.
    pub role: Role,
    /// Whether the account has been activated by an administrator.
    pub activated: bool,
    /// Optional avatar URL served by the server.
    pub avatar_url: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Maximum age rating this account may access; `None` = unrestricted.
    #[schema(value_type = Option<u8>)]
    pub age_limit: Option<u8>,
    /// Identity source: `local` or `oidc`.
    pub source: UserSource,
    /// OIDC issuer when [`User::source`] is `oidc`.
    pub oidc_issuer: Option<String>,
}

impl User {
    /// Whether an identity provider owns this account's identity fields.
    #[must_use]
    pub fn is_oidc_managed(&self) -> bool {
        self.source == UserSource::Oidc
    }
}

/// Row of the `users` table, including the password hash.
#[derive(Debug, sqlx::FromRow)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub email: Option<String>,
    pub role: String,
    pub activated: i64,
    pub avatar_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub oidc_issuer: Option<String>,
    pub oidc_subject: Option<String>,
    pub age_limit: Option<i64>,
}

impl UserRow {
    /// Public representation, hiding the password hash and the OIDC subject.
    #[must_use]
    pub fn to_public(&self) -> User {
        User {
            id: self.id,
            username: self.username.clone(),
            email: self.email.clone(),
            role: self.role.parse::<Role>().unwrap_or(Role::User),
            activated: self.activated != 0,
            avatar_url: self
                .avatar_path
                .as_deref()
                .filter(|path| !path.is_empty())
                .map(crate::media::media_url),
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            age_limit: self.age_limit.and_then(|value| u8::try_from(value).ok()),
            source: self.source(),
            oidc_issuer: self.oidc_issuer.clone(),
        }
    }

    /// Whether an identity provider owns this account's identity fields.
    #[must_use]
    pub fn is_oidc_managed(&self) -> bool {
        self.source() == UserSource::Oidc
    }

    fn source(&self) -> UserSource {
        if self.oidc_issuer.is_some() || self.oidc_subject.is_some() {
            UserSource::Oidc
        } else {
            UserSource::Local
        }
    }
}

/// Row of the `news` table joined with its author's username.
#[derive(Debug, sqlx::FromRow)]
pub struct NewsRow {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub author: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Public representation of a cloud save snapshot.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveSnapshot {
    /// Snapshot id.
    pub id: i64,
    /// Game the snapshot belongs to.
    pub game_id: i64,
    /// Version the snapshot was taken from, when reported by the client.
    pub version_id: Option<i64>,
    /// User-supplied label.
    pub label: String,
    /// Sanitized client file name.
    pub file_name: String,
    /// Archive size in bytes.
    pub size: i64,
    /// Lowercase hex SHA-256 of the archive.
    pub sha256: String,
    /// Upload timestamp.
    pub created_at: DateTime<Utc>,
    /// `manual` for a user upload, `auto` for the post-exit sync (#108).
    pub source: String,
    /// Opaque identifier of the device that uploaded the archive (#108).
    pub device_id: Option<String>,
}

/// Row of the `save_snapshots` table.
#[derive(Debug, sqlx::FromRow)]
pub struct SaveSnapshotRow {
    pub id: i64,
    pub user_id: i64,
    pub game_id: i64,
    pub version_id: Option<i64>,
    pub label: String,
    pub file_name: String,
    pub size: i64,
    pub sha256: String,
    pub created_at: String,
    pub source: String,
    pub device_id: Option<String>,
}

impl SaveSnapshotRow {
    /// Public representation, hiding the owning account and version metadata.
    #[must_use]
    pub fn to_public(self) -> SaveSnapshot {
        SaveSnapshot {
            id: self.id,
            game_id: self.game_id,
            version_id: self.version_id,
            label: self.label,
            file_name: self.file_name,
            size: self.size,
            sha256: self.sha256,
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            source: self.source,
            device_id: self.device_id,
        }
    }
}

/// Scanner status exposed by `GET /api/library/scan`.
#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    /// Whether a scan is currently running.
    pub running: bool,
    /// When the current/last scan started.
    pub started_at: Option<DateTime<Utc>>,
    /// When the last scan finished.
    pub finished_at: Option<DateTime<Utc>>,
    /// Files examined.
    pub scanned: i64,
    /// New files indexed.
    pub added: i64,
    /// Existing files refreshed.
    pub updated: i64,
    /// Files marked missing.
    pub removed: i64,
    /// Games queued for metadata enrichment.
    pub metadata_pending: i64,
    /// Non-fatal errors encountered during the scan.
    pub errors: Vec<String>,
}

/// `POST /api/auth/login` and `POST /api/setup` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    /// Signed access token.
    pub token: String,
    /// Authenticated account.
    pub user: User,
    /// Token expiry, when the token belongs to a tracked session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Login request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    /// Login name.
    pub username: String,
    /// Plain-text password.
    pub password: String,
}

/// First-run setup request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    /// Administrator username.
    pub username: String,
    /// Administrator password.
    pub password: String,
    /// Optional server display name.
    #[serde(default)]
    pub server_name: Option<String>,
}

/// First-run setup status.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    /// True while the server has no accounts.
    pub needs_setup: bool,
    /// Configured display name.
    pub server_name: String,
    /// Server version.
    pub version: String,
}

/// `GET /api/health` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    /// Always `ok` when the request succeeds.
    pub status: &'static str,
    /// Server version.
    pub version: &'static str,
    /// Process uptime in seconds.
    pub uptime_seconds: u64,
}

/// `GET /api/health/details` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthDetails {
    /// `ok` or `degraded` when the database is unreachable.
    pub status: &'static str,
    /// Server version.
    pub version: &'static str,
    /// Build commit, `unknown` when not provided at build time.
    pub commit: String,
    /// Process uptime in seconds.
    pub uptime_seconds: u64,
    /// SQLite status and size.
    pub database: HealthDatabase,
    /// Free space on the configured volumes.
    pub disk: HealthDisk,
    /// Library counters.
    pub library: HealthLibrary,
    /// Ingest queue counters.
    pub ingest: HealthIngest,
    /// Folder watcher state.
    pub watcher: HealthWatcher,
    /// Last integrity report, when one has run.
    pub integrity: Option<crate::metrics::IntegritySummary>,
    /// Active archive storage backend (`local` or `s3`).
    pub storage_backend: String,
}

/// Database section of the health details.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthDatabase {
    /// Whether `SELECT 1` succeeded.
    pub ok: bool,
    /// Database size including WAL, in bytes.
    pub size_bytes: u64,
}

/// Disk section of the health details.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthDisk {
    /// Free bytes under the data directory.
    pub data_free_bytes: u64,
    /// Free bytes under the primary games directory.
    pub games_free_bytes: u64,
    /// Free bytes under the media directory.
    pub media_free_bytes: u64,
}

/// Library section of the health details.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthLibrary {
    /// Games in the library.
    pub games: i64,
    /// Versions available (`missing = 0`).
    pub versions: i64,
    /// Registered roots.
    pub roots_total: i64,
    /// Roots readable at the last scan.
    pub roots_available: i64,
    /// Cached media files.
    pub media_files: u64,
    /// Cached media bytes.
    pub media_bytes: u64,
}

/// Ingest queue section of the health details.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthIngest {
    /// Jobs waiting for a download slot.
    pub queued: i64,
    /// Jobs downloading or verifying.
    pub running: i64,
    /// Jobs stopped by an error.
    pub failed: i64,
    /// Completed jobs.
    pub done: i64,
    /// Cancelled jobs.
    pub cancelled: i64,
    /// All known jobs.
    pub total: i64,
}

/// Watcher section of the health details. Root paths stay private.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthWatcher {
    /// Whether the watcher task is running.
    pub enabled: bool,
    /// Whether a debounced scan is pending.
    pub pending: bool,
    /// Number of watched roots.
    pub roots: usize,
    /// Last relevant filesystem event.
    pub last_event_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Last watch error.
    pub last_error: Option<String>,
}

/// Feature flags exposed by `GET /api/config`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureFlags {
    /// IGDB credentials configured.
    pub igdb: bool,
    /// Steam Store fallback available (always true).
    pub steam: bool,
    /// Ingest download manager available.
    pub ingest: bool,
    /// Embedded/local mode.
    pub local_mode: bool,
    /// OIDC SSO available.
    pub oidc: bool,
}

/// `GET /api/config` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    /// Display name.
    pub server_name: String,
    /// Server version.
    pub version: &'static str,
    /// Optional features.
    pub features: FeatureFlags,
    /// Maximum accepted upload size.
    pub max_upload_bytes: u64,
    /// Display name for the SSO button (the configured label even while SSO
    /// is disabled, matching the historical response).
    pub oidc_display_name: String,
}

/// Password change request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    /// Current password.
    pub current_password: String,
    /// Replacement password.
    pub new_password: String,
}

/// Admin user creation request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    /// Login name.
    pub username: String,
    /// Initial password.
    pub password: String,
    /// Optional e-mail.
    #[serde(default)]
    pub email: Option<String>,
    /// Role, defaults to `user`.
    #[serde(default)]
    pub role: Option<Role>,
}

/// Admin user update request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    /// New login name (local accounts only).
    #[serde(default)]
    pub username: Option<String>,
    /// New e-mail (`null` or `""` clears it; local accounts only).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub email: Option<Option<String>>,
    /// New password, write-only (local accounts only).
    #[serde(default)]
    pub password: Option<String>,
    /// New role.
    #[serde(default)]
    pub role: Option<Role>,
    /// New activation flag.
    #[serde(default)]
    pub activated: Option<bool>,
    /// Maximum age rating (`null` clears the limit).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub age_limit: Option<Option<u8>>,
}

/// Query string for `GET /api/games` and `/api/games/random`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameQuery {
    /// Title substring.
    #[serde(default)]
    pub q: Option<String>,
    /// Exact genre label.
    #[serde(default)]
    pub genre: Option<String>,
    /// Progress state filter.
    #[serde(default)]
    pub state: Option<ProgressState>,
    /// Only bookmarked games.
    #[serde(default)]
    pub bookmarked: Option<bool>,
    /// Only early-access games.
    #[serde(default)]
    pub early_access: Option<bool>,
    /// Earliest release year (inclusive).
    #[serde(default)]
    pub year_from: Option<i64>,
    /// Latest release year (inclusive).
    #[serde(default)]
    pub year_to: Option<i64>,
    /// Minimum supported player count.
    #[serde(default)]
    pub players: Option<i64>,
    /// Minimum played minutes (inclusive).
    #[serde(default)]
    pub playtime_min: Option<i64>,
    /// Maximum played minutes (inclusive).
    #[serde(default)]
    pub playtime_max: Option<i64>,
    /// Minimum rating (0-100).
    #[serde(default)]
    pub min_rating: Option<f64>,
    /// Metadata provider slug (`igdb`, `steam`, ...).
    #[serde(default)]
    pub provider: Option<String>,
    /// Sort column.
    #[serde(default)]
    pub sort: Option<String>,
    /// Sort direction (`asc`/`desc`).
    #[serde(default)]
    pub order: Option<String>,
    /// 1-based page number.
    #[serde(default)]
    pub page: Option<i64>,
    /// Page size.
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Query string for `GET /api/games/suggest`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuggestQuery {
    /// Free-text query, usually a partial title with typos.
    #[serde(default)]
    pub q: Option<String>,
    /// Maximum number of suggestions (default 8, capped at 20).
    #[serde(default)]
    pub limit: Option<i64>,
}

/// One fuzzy-search suggestion with cover art and match metadata.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameSuggestion {
    /// Game id.
    pub id: i64,
    /// Display title.
    pub title: String,
    /// Release year, when known.
    pub year: Option<i32>,
    /// Cover media URL, when cached.
    pub cover_url: Option<String>,
    /// Field that matched (`title`, `sortTitle`, `altTitle`).
    pub matched_field: String,
    /// Title or alias text that matched.
    pub matched_title: String,
    /// Relevance score (0-1000).
    pub score: f64,
}

/// Response of `GET /api/games/suggest`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SuggestResponse {
    /// Normalised query echoed back.
    pub query: String,
    /// Ranked suggestions, best first.
    pub suggestions: Vec<GameSuggestion>,
}

/// Editor game metadata update.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGameRequest {
    /// New display title.
    #[serde(default)]
    pub title: Option<String>,
    /// New sort title.
    #[serde(default)]
    pub sort_title: Option<String>,
    /// Long description (`null` clears).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub description: Option<Option<String>>,
    /// Release date (`null` clears).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub release_date: Option<Option<String>>,
    /// Genre labels.
    #[serde(default)]
    pub genres: Option<Vec<String>>,
    /// Developer names.
    #[serde(default)]
    pub developers: Option<Vec<String>>,
    /// Publisher names.
    #[serde(default)]
    pub publishers: Option<Vec<String>>,
    /// Free-form tags.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Rating 0-100 (`null` clears).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub rating: Option<Option<f64>>,
    /// Age rating (`null` clears).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub age_rating: Option<Option<u8>>,
    /// Early-access flag.
    #[serde(default)]
    pub early_access: Option<bool>,
    /// Alternative titles used by fuzzy search.
    #[serde(default)]
    pub alt_titles: Option<Vec<String>>,
    /// Highest supported player count (`null` clears).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub players: Option<Option<u32>>,
    /// Launch configuration, replaces all launch fields.
    #[serde(default)]
    pub launch: Option<LaunchInfo>,
}

/// Progress state update.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProgressRequest {
    /// New state.
    pub state: ProgressState,
}

/// Playtime report.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlaytimeRequest {
    /// Minutes played; clamped to 1..=1440.
    pub minutes: i64,
}

/// Bookmark update.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkRequest {
    /// Desired bookmark state.
    pub bookmarked: bool,
}

/// Bookmark response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkResponse {
    /// Current bookmark state.
    pub bookmarked: bool,
}

/// Download ticket request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketRequest {
    /// Specific version, or the newest available one.
    #[serde(default)]
    pub version_id: Option<i64>,
}

/// Download ticket response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketResponse {
    /// Absolute-path URL including the short-lived ticket.
    pub url: String,
    /// Ticket expiry.
    pub expires_at: DateTime<Utc>,
}

/// Query string for the download route.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DownloadQuery {
    /// Version to stream.
    #[serde(default)]
    pub version_id: Option<i64>,
    /// Short-lived ticket, accepted instead of a Bearer token.
    #[serde(default)]
    pub token: Option<String>,
}

/// Query string for the ZIP stream-install route.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamQuery {
    /// Short-lived download ticket, accepted instead of a Bearer token.
    #[serde(default)]
    pub token: Option<String>,
}

/// Local path response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalPathResponse {
    /// Absolute path on the server's filesystem.
    pub path: String,
}

/// Scan trigger response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ScanResponse {
    /// Whether a scan was queued.
    pub started: bool,
}

/// Metadata refresh response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RefreshResponse {
    /// Always true: enrichment runs in the background.
    pub queued: bool,
}

/// `POST /api/library/roots` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateLibraryRootRequest {
    /// Optional label; defaults to the last path component.
    #[serde(default)]
    pub label: Option<String>,
    /// Absolute path on the server.
    pub path: String,
    /// Whether the scanner walks the root (default true).
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `PATCH /api/library/roots/{id}` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLibraryRootRequest {
    /// New label.
    #[serde(default)]
    pub label: Option<String>,
    /// New absolute path.
    #[serde(default)]
    pub path: Option<String>,
    /// Enable or disable the root.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// New ordering position.
    #[serde(default)]
    pub position: Option<i64>,
}

/// `POST /api/library/roots/{id}/scan` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RootScanResponse {
    /// Whether a scan was queued.
    pub started: bool,
    /// Root the request targeted.
    pub root_id: i64,
}

/// `POST /api/integrity/check` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityCheckRequest {
    /// `all` (default), `archives` or `media`.
    #[serde(default)]
    pub scope: Option<String>,
}

/// `POST /api/integrity/check` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityCheckResponse {
    /// Report id to poll with `GET /api/integrity/reports/{id}`.
    pub report_id: i64,
    /// Always `running` at creation time.
    pub status: String,
}

/// `GET /api/integrity/reports/{id}` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityReportDetail {
    /// Report summary.
    pub report: crate::integrity::IntegrityReport,
    /// Findings recorded by the report.
    pub findings: Vec<crate::integrity::IntegrityFinding>,
}

/// `POST /api/media/gc` query.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaGcQuery {
    /// Compute the plan without deleting anything (default true).
    #[serde(default)]
    pub dry_run: Option<bool>,
}

/// Library folder response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolders {
    /// Library volume.
    pub games_dir: String,
    /// Media cache.
    pub media_dir: String,
    /// Database directory.
    pub data_dir: String,
}

/// News creation request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewsRequest {
    /// Post title.
    pub title: String,
    /// Markdown body.
    pub body: String,
}

/// News update request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNewsRequest {
    /// New title.
    #[serde(default)]
    pub title: Option<String>,
    /// New body.
    #[serde(default)]
    pub body: Option<String>,
}

/// Response wrapping a single user.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserResponse {
    /// Account.
    pub user: User,
}

/// Row of the `api_keys` table.
#[derive(Debug, sqlx::FromRow)]
pub struct ApiKeyRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub prefix: String,
    pub secret_hash: String,
    pub scopes: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

impl ApiKeyRow {
    /// Public representation, never including the secret or its hash.
    #[must_use]
    pub fn to_public(&self) -> ApiKey {
        ApiKey {
            id: self.id,
            user_id: self.user_id,
            name: self.name.clone(),
            prefix: self.prefix.clone(),
            scopes: crate::api_keys::parse_scopes(&self.scopes)
                .iter()
                .map(|scope| scope.as_str().to_string())
                .collect(),
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            last_used_at: self.last_used_at.as_deref().and_then(crate::db::parse_time),
            revoked_at: self.revoked_at.as_deref().and_then(crate::db::parse_time),
        }
    }
}

/// API key metadata returned by `/api/keys`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiKey {
    /// Key id.
    pub id: i64,
    /// Owning account.
    pub user_id: i64,
    /// Human label.
    pub name: String,
    /// Non-secret display prefix.
    pub prefix: String,
    /// Granted scopes (`read`, `write`, `admin`).
    pub scopes: Vec<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last successful use, if any.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Revocation timestamp, if revoked.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// `POST /api/keys` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiKeyRequest {
    /// Human label.
    pub name: String,
    /// Requested scopes; defaults to `["read"]`.
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Admin only: create the key for another account.
    #[serde(default)]
    pub user_id: Option<i64>,
}

/// `POST /api/keys` response: the plaintext secret is shown exactly once.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateApiKeyResponse {
    /// Key metadata.
    #[serde(flatten)]
    pub key: ApiKey,
    /// Plaintext token, never stored server-side.
    pub secret: String,
}

/// Query string for `GET /api/keys`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyQuery {
    /// Admin only: list keys for every account.
    #[serde(default)]
    pub all: Option<bool>,
}

/// Row of the `audit_logs` table.
#[derive(Debug, sqlx::FromRow)]
pub struct AuditLogRow {
    pub id: i64,
    pub actor_id: Option<i64>,
    pub actor_kind: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub metadata: String,
    pub ip: Option<String>,
    pub created_at: String,
}

impl AuditLogRow {
    /// Public representation with parsed metadata.
    #[must_use]
    pub fn to_public(&self) -> AuditLog {
        AuditLog {
            id: self.id,
            actor_id: self.actor_id,
            actor_kind: self.actor_kind.clone(),
            action: self.action.clone(),
            target_type: self.target_type.clone(),
            target_id: self.target_id.clone(),
            metadata: serde_json::from_str(&self.metadata)
                .unwrap_or_else(|_| serde_json::json!({})),
            ip: self.ip.clone(),
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
        }
    }
}

/// One audit entry.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLog {
    /// Row id, also the pagination cursor.
    pub id: i64,
    /// User or API key id, depending on `actor_kind`.
    pub actor_id: Option<i64>,
    /// `user`, `api_key` or `system`.
    pub actor_kind: String,
    /// Action name.
    pub action: String,
    /// Target entity family.
    pub target_type: Option<String>,
    /// Target identifier.
    pub target_id: Option<String>,
    /// Structured, secret-free detail.
    pub metadata: serde_json::Value,
    /// Request address when known.
    pub ip: Option<String>,
    /// Timestamp.
    pub created_at: DateTime<Utc>,
}

/// Query string for `GET /api/audit`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditQuery {
    /// Page size, 1..=200 (default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Return rows with `id < before` (newest-first cursor).
    #[serde(default)]
    pub before: Option<i64>,
    /// Filter by actor id.
    #[serde(default)]
    pub actor_id: Option<i64>,
    /// Filter by actor kind (`user`, `api_key`, `system`).
    #[serde(default)]
    pub actor_kind: Option<String>,
    /// Filter by exact action.
    #[serde(default)]
    pub action: Option<String>,
    /// Filter by target type.
    #[serde(default)]
    pub target_type: Option<String>,
    /// Filter by target id.
    #[serde(default)]
    pub target_id: Option<String>,
}

/// One page of audit entries.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditPage {
    /// Newest-first entries.
    pub items: Vec<AuditLog>,
    /// Pass as `before` to fetch the next page; `null` on the last page.
    pub next_before: Option<i64>,
}

/// Row of the `webhooks` table.
#[derive(Debug, sqlx::FromRow)]
pub struct WebhookRow {
    pub id: i64,
    pub url: String,
    pub secret: String,
    pub events: String,
    pub enabled: i64,
    pub created_at: String,
}

impl WebhookRow {
    /// Public representation; the HMAC secret is never returned by list/update.
    #[must_use]
    pub fn to_public(&self) -> Webhook {
        Webhook {
            id: self.id,
            url: self.url.clone(),
            events: crate::webhooks::parse_events(&self.events),
            enabled: self.enabled != 0,
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
        }
    }
}

/// Webhook metadata.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Webhook {
    /// Webhook id.
    pub id: i64,
    /// Delivery URL.
    pub url: String,
    /// Subscribed event names.
    pub events: Vec<String>,
    /// Whether deliveries are attempted.
    pub enabled: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// `POST /api/webhooks` response: the signing secret is shown once.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateWebhookResponse {
    /// Webhook metadata.
    #[serde(flatten)]
    pub webhook: Webhook,
    /// HMAC-SHA256 signing secret.
    pub secret: String,
}

/// `POST /api/webhooks` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateWebhookRequest {
    /// Delivery URL (http/https, public unless explicitly allowed).
    pub url: String,
    /// Event names from the documented list.
    pub events: Vec<String>,
    /// Defaults to `true`.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `PATCH /api/webhooks/{id}` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWebhookRequest {
    /// Replacement URL.
    #[serde(default)]
    pub url: Option<String>,
    /// Replacement event list.
    #[serde(default)]
    pub events: Option<Vec<String>>,
    /// Enable or disable deliveries.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Rotate the signing secret and return the new value once.
    #[serde(default)]
    pub rotate_secret: Option<bool>,
}

/// `PATCH /api/webhooks/{id}` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWebhookResponse {
    /// Webhook metadata.
    #[serde(flatten)]
    pub webhook: Webhook,
    /// New signing secret when `rotateSecret` was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// `POST /api/webhooks/{id}/test` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookTestResponse {
    /// Queued delivery id.
    pub delivery_id: i64,
}

/// Kind of collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum CollectionKind {
    /// Explicit list of games.
    Manual,
    /// Rule tree evaluated at read time.
    Smart,
}

impl CollectionKind {
    /// Stable database and API value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Smart => "smart",
        }
    }

    /// Parses a database or API value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(Self::Manual),
            "smart" => Some(Self::Smart),
            _ => None,
        }
    }
}

/// One rule inside a smart collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SmartRule {
    /// `genre`, `year`, `players`, `playtime`, `rating`, `installed`,
    /// `provider` or `title`.
    pub field: String,
    /// Field-specific operator; defaults are documented in the contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Primary comparison value.
    pub value: serde_json::Value,
    /// Upper bound for `between` operators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value2: Option<serde_json::Value>,
}

/// Rule tree of a smart collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SmartRules {
    /// `all` (AND) or `any` (OR).
    #[serde(default = "default_match_mode", rename = "match")]
    pub match_mode: String,
    /// Rules to combine.
    #[serde(default)]
    pub rules: Vec<SmartRule>,
}

impl Default for SmartRules {
    fn default() -> Self {
        Self {
            match_mode: default_match_mode(),
            rules: Vec::new(),
        }
    }
}

fn default_match_mode() -> String {
    "all".to_string()
}

/// Owner of a collection, as exposed by the API.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionOwner {
    /// Account id.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Public display name, when the account opted in.
    pub display_name: Option<String>,
}

/// Collection metadata.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    /// Collection id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Whether other accounts can read it.
    pub is_public: bool,
    /// Manual or smart.
    pub kind: CollectionKind,
    /// Rule tree (empty for manual collections).
    pub rules: SmartRules,
    /// Manual member count, or the evaluated count for smart collections when
    /// it was cheap to compute.
    pub game_count: i64,
    /// Owning account.
    pub owner: CollectionOwner,
    /// Whether the requesting user owns it.
    pub owned: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Row of the `collections` table joined with its owner.
#[derive(Debug, sqlx::FromRow)]
pub struct CollectionRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_public: i64,
    pub kind: String,
    pub rules_json: String,
    pub created_at: String,
    pub updated_at: String,
    pub game_count: i64,
    pub owner_username: String,
    pub owner_display_name: Option<String>,
}

impl CollectionRow {
    /// Public representation for `viewer_id`.
    #[must_use]
    pub fn to_public(&self, viewer_id: i64) -> Collection {
        Collection {
            id: self.id,
            name: self.name.clone(),
            description: self.description.clone(),
            is_public: self.is_public != 0,
            kind: CollectionKind::parse(&self.kind).unwrap_or(CollectionKind::Manual),
            rules: serde_json::from_str(&self.rules_json).unwrap_or_default(),
            game_count: self.game_count,
            owner: CollectionOwner {
                id: self.user_id,
                username: self.owner_username.clone(),
                display_name: self.owner_display_name.clone(),
            },
            owned: self.user_id == viewer_id,
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            updated_at: crate::db::parse_time(&self.updated_at).unwrap_or_else(Utc::now),
        }
    }
}

/// `POST /api/collections` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateCollectionRequest {
    /// Display name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether other accounts can read it.
    #[serde(default)]
    pub is_public: Option<bool>,
    /// `manual` (default) or `smart`.
    #[serde(default)]
    pub kind: Option<CollectionKind>,
    /// Rule tree, required for smart collections.
    #[serde(default)]
    pub rules: Option<SmartRules>,
    /// Initial manual members.
    #[serde(default)]
    pub game_ids: Option<Vec<i64>>,
}

/// `PATCH /api/collections/{id}` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCollectionRequest {
    /// New display name.
    #[serde(default)]
    pub name: Option<String>,
    /// New description (`null` clears).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub description: Option<Option<String>>,
    /// Public visibility.
    #[serde(default)]
    pub is_public: Option<bool>,
    /// Manual or smart.
    #[serde(default)]
    pub kind: Option<CollectionKind>,
    /// Replacement rule tree.
    #[serde(default)]
    pub rules: Option<SmartRules>,
}

/// `POST /api/collections/{id}/games` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionGamesRequest {
    /// Games to add.
    pub game_ids: Vec<i64>,
}

/// Response of `POST /api/games/{id}/installed`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstalledReportResponse {
    /// Whether a new feed entry was written (deduped within an hour).
    pub recorded: bool,
}

/// `POST /api/collections/{id}/games` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionGamesResponse {
    /// Rows actually inserted (duplicates are ignored).
    pub added: i64,
    /// Members after the insert.
    pub total: i64,
}

/// Query string for `GET /api/collections/{id}/games`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CollectionGamesQuery {
    /// 1-based page number.
    #[serde(default)]
    pub page: Option<i64>,
    /// Page size.
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Account summary embedded in feed entries.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityUser {
    /// Account id.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Public display name.
    pub display_name: Option<String>,
    /// Avatar URL, when set.
    pub avatar_url: Option<String>,
}

/// Game summary embedded in feed entries.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityGame {
    /// Game id.
    pub id: i64,
    /// Display title.
    pub title: String,
    /// Cover URL, when cached.
    pub cover_url: Option<String>,
    /// Release year, when known.
    pub release_year: Option<i32>,
}

/// Collection summary embedded in feed entries.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityCollection {
    /// Collection id.
    pub id: i64,
    /// Display name.
    pub name: String,
}

/// One public activity entry.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    /// Row id, also the pagination cursor.
    pub id: i64,
    /// `game_added`, `game_installed`, `game_played`, `game_completed` or
    /// `collection_created`.
    pub kind: String,
    /// Event timestamp.
    pub created_at: DateTime<Utc>,
    /// Structured, secret-free detail.
    pub metadata: serde_json::Value,
    /// Account that generated the entry.
    pub user: ActivityUser,
    /// Related game, when the row survived.
    pub game: Option<ActivityGame>,
    /// Related collection, when the row survived.
    pub collection: Option<ActivityCollection>,
}

/// One page of feed entries.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPage {
    /// Newest-first entries.
    pub items: Vec<Activity>,
    /// Pass as `before` to fetch the next page; `null` on the last page.
    pub next_before: Option<i64>,
}

/// Query string for feed and activity routes.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActivityQuery {
    /// Page size, 1..=100 (default 20).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Return rows with `id < before` (newest-first cursor).
    #[serde(default)]
    pub before: Option<i64>,
}

/// Row of the `activities` table joined with actor, game and collection.
#[derive(Debug, sqlx::FromRow)]
pub struct ActivityRow {
    pub id: i64,
    pub kind: String,
    pub created_at: String,
    pub metadata: String,
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_path: Option<String>,
    pub game_id: Option<i64>,
    pub game_title: Option<String>,
    pub game_cover_path: Option<String>,
    pub game_release_date: Option<String>,
    pub collection_id: Option<i64>,
    pub collection_name: Option<String>,
}

impl ActivityRow {
    /// Public representation.
    #[must_use]
    pub fn to_public(&self) -> Activity {
        Activity {
            id: self.id,
            kind: self.kind.clone(),
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            metadata: serde_json::from_str(&self.metadata)
                .unwrap_or_else(|_| serde_json::json!({})),
            user: ActivityUser {
                id: self.user_id,
                username: self.username.clone(),
                display_name: self.display_name.clone(),
                avatar_url: self
                    .avatar_path
                    .as_deref()
                    .filter(|path| !path.is_empty())
                    .map(crate::media::media_url),
            },
            game: self
                .game_id
                .zip(self.game_title.clone())
                .map(|(id, title)| ActivityGame {
                    id,
                    title,
                    cover_url: self.game_cover_path.as_deref().map(crate::media::media_url),
                    release_year: self
                        .game_release_date
                        .as_deref()
                        .and_then(|date| date.get(0..4))
                        .and_then(|year| year.parse::<i32>().ok()),
                }),
            collection: self
                .collection_id
                .zip(self.collection_name.clone())
                .map(|(id, name)| ActivityCollection { id, name }),
        }
    }
}

/// Opt-in public profile.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicProfile {
    /// Account id.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Public display name.
    pub display_name: Option<String>,
    /// Free-form bio.
    pub bio: Option<String>,
    /// Avatar URL, when set.
    pub avatar_url: Option<String>,
    /// Whether the profile is public.
    pub public_profile: bool,
    /// Member since.
    pub created_at: DateTime<Utc>,
}

/// Aggregate counters shown on a profile.
#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicStats {
    /// Games with progress.
    pub games: i64,
    /// Games marked completed.
    pub completed: i64,
    /// Minutes played in total.
    pub minutes_played: i64,
    /// Bookmarked games.
    pub bookmarked: i64,
    /// Visible collections.
    pub collections: i64,
}

/// `GET /api/community/users/{id}` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicProfileResponse {
    /// Profile header.
    pub user: PublicProfile,
    /// Aggregate counters.
    pub stats: PublicStats,
    /// Games the account has progress on or bookmarked, most recent first.
    pub library: Vec<GameSummary>,
    /// Public collections (all of them for the owner).
    pub collections: Vec<Collection>,
}

/// `PATCH /api/me/profile` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProfileRequest {
    /// Opt in or out of the public directory and feed.
    #[serde(default)]
    pub public_profile: Option<bool>,
    /// Display name (`null` clears).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub display_name: Option<Option<String>>,
    /// Bio (`null` clears).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub bio: Option<Option<String>>,
}

/// Deserialises a present field, distinguishing `null` from an absent field.
///
/// `Option<Option<T>>` is `None` when the field is missing (thanks to
/// `#[serde(default)]`), `Some(None)` when it is explicitly `null`, and
/// `Some(Some(value))` otherwise.
fn deserialize_some<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_game_distinguishes_null_from_absent() {
        let update: UpdateGameRequest =
            serde_json::from_str(r#"{"description": null, "rating": 42.5}"#).expect("json");
        assert_eq!(update.description, Some(None));
        assert_eq!(update.rating, Some(Some(42.5)));
        assert!(update.release_date.is_none());
        assert!(update.title.is_none());
    }

    #[test]
    fn update_user_distinguishes_null_email() {
        let update: UpdateUserRequest = serde_json::from_str(r#"{"email": null}"#).expect("json");
        assert_eq!(update.email, Some(None));
        let update: UpdateUserRequest = serde_json::from_str("{}").expect("json");
        assert!(update.email.is_none());
        assert!(update.username.is_none());
        assert!(update.password.is_none());
    }

    #[test]
    fn update_user_accepts_identity_fields() {
        let update: UpdateUserRequest =
            serde_json::from_str(r#"{"username": "mara", "password": "password123"}"#)
                .expect("json");
        assert_eq!(update.username.as_deref(), Some("mara"));
        assert_eq!(update.password.as_deref(), Some("password123"));
    }

    #[test]
    fn user_source_serialises_lowercase() {
        assert_eq!(
            serde_json::to_string(&UserSource::Local).expect("json"),
            "\"local\""
        );
        assert_eq!(
            serde_json::to_string(&UserSource::Oidc).expect("json"),
            "\"oidc\""
        );
        assert_eq!(UserSource::Oidc.as_str(), "oidc");
    }

    #[test]
    fn scan_status_serialises_camel_case() {
        let status = ScanStatus::default();
        let json = serde_json::to_string(&status).expect("json");
        assert!(json.contains("\"metadataPending\":0"), "{json}");
        assert!(json.contains("\"startedAt\":null"), "{json}");
    }
}

/// One login session (`auth_sessions` row).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    /// Session id.
    pub id: i64,
    /// Client `User-Agent` recorded at login.
    pub user_agent: Option<String>,
    /// Client IP recorded at login.
    pub ip: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last request seen with this token.
    pub last_seen_at: DateTime<Utc>,
    /// Token expiry.
    pub expires_at: DateTime<Utc>,
    /// Revocation timestamp, if revoked.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Whether this is the session used by the current request.
    pub current: bool,
}

/// Row of the `auth_sessions` table.
#[derive(Debug, sqlx::FromRow)]
pub struct AuthSessionRow {
    pub id: i64,
    pub jti: String,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

impl AuthSessionRow {
    /// Public representation, hiding the token id.
    #[must_use]
    pub fn to_public(&self, current_jti: Option<&str>) -> AuthSession {
        AuthSession {
            id: self.id,
            user_agent: self.user_agent.clone(),
            ip: self.ip.clone(),
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            last_seen_at: crate::db::parse_time(&self.last_seen_at).unwrap_or_else(Utc::now),
            expires_at: crate::db::parse_time(&self.expires_at).unwrap_or_else(Utc::now),
            revoked_at: self.revoked_at.as_deref().and_then(crate::db::parse_time),
            current: current_jti == Some(self.jti.as_str()),
        }
    }
}

/// `POST /api/uploads` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateUploadRequest {
    /// Archive file name. Must be a supported archive extension.
    pub file_name: String,
    /// Total size in bytes.
    pub size: i64,
    /// Optional lowercase SHA-256 of the complete archive.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Progress of a resumable upload.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadSession {
    /// Upload id.
    pub id: String,
    /// Original file name.
    pub file_name: String,
    /// Declared total size.
    pub size: i64,
    /// Declared SHA-256, when provided.
    pub sha256: Option<String>,
    /// Chunk size the client must use.
    pub chunk_size: i64,
    /// Number of chunks expected.
    pub total_parts: i64,
    /// Bytes received so far.
    pub received_bytes: i64,
    /// Indices of chunks already stored.
    pub received_parts: Vec<i64>,
    /// `open`, `complete` or `failed`.
    pub status: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
    /// Indexed game id once complete.
    pub game_id: Option<i64>,
    /// Indexed version id once complete.
    pub version_id: Option<i64>,
    /// Failure reason, when `status` is `failed`.
    pub error: Option<String>,
}

/// Row of the `uploads` table.
#[derive(Debug, sqlx::FromRow)]
pub struct UploadRow {
    pub id: String,
    pub user_id: i64,
    pub file_name: String,
    pub size: i64,
    pub sha256: Option<String>,
    pub chunk_size: i64,
    pub status: String,
    pub game_id: Option<i64>,
    pub version_id: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl UploadRow {
    /// Public representation including on-disk progress.
    #[must_use]
    pub fn to_public(&self, received_parts: Vec<i64>) -> UploadSession {
        let complete = self.status == "complete";
        let received_bytes = if complete {
            self.size
        } else {
            received_parts
                .iter()
                .map(|index| part_len(self.size, self.chunk_size, *index))
                .sum()
        };
        UploadSession {
            id: self.id.clone(),
            file_name: self.file_name.clone(),
            size: self.size,
            sha256: self.sha256.clone(),
            chunk_size: self.chunk_size,
            total_parts: total_parts(self.size, self.chunk_size),
            received_bytes,
            received_parts,
            status: self.status.clone(),
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            updated_at: crate::db::parse_time(&self.updated_at).unwrap_or_else(Utc::now),
            game_id: self.game_id,
            version_id: self.version_id,
            error: self.error.clone(),
        }
    }
}

/// Length of one chunk, taking the final partial chunk into account.
#[must_use]
pub fn part_len(total: i64, chunk: i64, index: i64) -> i64 {
    if chunk <= 0 {
        return 0;
    }
    let start = index.saturating_mul(chunk);
    (total - start).clamp(0, chunk)
}

/// Number of chunks needed for `total` bytes at `chunk` bytes per part.
#[must_use]
pub fn total_parts(total: i64, chunk: i64) -> i64 {
    if chunk <= 0 {
        return 0;
    }
    (total + chunk - 1) / chunk
}

/// `PUT /api/uploads/{id}/parts/{n}` response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadPartResponse {
    /// Upload id.
    pub id: String,
    /// Stored chunk index.
    pub part: i64,
    /// Bytes received so far.
    pub received_bytes: i64,
    /// Number of chunks stored.
    pub received_parts: i64,
    /// Total number of chunks.
    pub total_parts: i64,
}

/// `POST /api/uploads/{id}/complete` response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteUploadResponse {
    /// Upload id.
    pub id: String,
    /// Indexed game id.
    pub game_id: i64,
    /// Indexed version id.
    pub version_id: i64,
    /// Stored file name (may be suffixed when the name was taken).
    pub file_name: String,
    /// Verified size.
    pub size: i64,
    /// Verified SHA-256, when the client provided one.
    pub sha256: Option<String>,
}
