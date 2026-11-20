//! Server-side request DTOs and SQL row shapes.
//!
//! Response bodies live in `ludex_core::models` (the single definition shared by
//! the web UI and the desktop app) and are re-exported here so handlers keep
//! importing `crate::models::*`.

use chrono::{DateTime, Utc};
use ludex_core::models::{LaunchInfo, ProgressState, Role};
use serde::{Deserialize, Deserializer, Serialize};

pub use ludex_core::models::{
    Activity, ActivityCollection, ActivityGame, ActivityPage, ActivityUser, ApiKey, AuditLog,
    AuditPage, AuthResponse, AuthSession, BookmarkResponse, Collection, CollectionGamesResponse,
    CollectionKind, CollectionOwner, CollectionVisibility, CompleteUploadResponse,
    CreateApiKeyResponse, CreateWebhookResponse, CreatedInvitation, DeleteVersionResponse, Device,
    DeviceCapabilities, FeatureFlags, GameSuggestion, GameVersionInfo, GameVersionsResponse,
    InstalledReportResponse, IntegrityCheckResponse, Invitation, InvitationPreview,
    InvitationStatus, LibraryFolders, LocalPathResponse, MfaStatus, PublicProfile,
    PublicProfileResponse, PublicStats, RecoveryCodes, RefreshResponse, ResetPasswordResult,
    RootScanResponse, SaveSnapshot, ScanResponse, ScanStatus, ServerConfig, SetupStatus, SmartRule,
    SmartRules, SuggestResponse, TicketResponse, TotpActivated, TotpEnrollment,
    UpdateWebhookResponse, UploadPartResponse, UploadSession, User, UserResponse, UserSource,
    Webhook, WebhookTestResponse,
};

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
    pub available_version_count: i64,
    pub missing_version_count: i64,
    pub total_size: i64,
    pub kind: String,
    pub platform: Option<String>,
    pub emulator_id: Option<i64>,
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
    pub available_version_count: i64,
    pub missing_version_count: i64,
    pub total_size: i64,
    pub kind: String,
    pub platform: Option<String>,
    pub emulator_id: Option<i64>,
    /// Pinned version for downloads and updates (#168).
    pub default_version_id: Option<i64>,
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
    /// Full SHA-256 of the archive (#216); `None` until the background job ran.
    pub content_sha256: Option<String>,
    pub indexed_at: Option<String>,
    pub updated_at: Option<String>,
    /// `local` or `s3` (#47).
    pub storage_backend: Option<String>,
    /// Object key inside the bucket when `storage_backend = "s3"`.
    pub object_key: Option<String>,
    /// ETag reported by the object store on upload.
    pub etag: Option<String>,
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
    pub onboarding_completed_at: Option<String>,
    pub onboarding_step: i64,
    pub mfa_enabled: i64,
    pub must_change_password: i64,
    pub locale: Option<String>,
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
            onboarding_completed_at: self
                .onboarding_completed_at
                .as_deref()
                .and_then(crate::db::parse_time),
            onboarding_step: self.onboarding_step,
            mfa_enabled: self.mfa_enabled != 0,
            must_change_password: self.must_change_password != 0,
            locale: crate::locale::normalize_stored(self.locale.clone()),
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
    /// `1` once the snapshot is a restore point (#295). Kept as an integer so
    /// the same row decodes from SQLite INTEGER and PostgreSQL BIGINT.
    pub pinned: i64,
    pub client_mtime: Option<String>,
    pub clock_offset_secs: i64,
    pub save_seq: Option<i64>,
    pub conflict_of: Option<i64>,
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
            pinned: self.pinned != 0,
            client_mtime: self.client_mtime.as_deref().and_then(crate::db::parse_time),
            clock_offset_secs: self.clock_offset_secs,
            save_seq: self.save_seq,
            conflict_of: self.conflict_of,
        }
    }
}

/// One recorded restore of a snapshot (#295).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveRestoreReceipt {
    /// Receipt id.
    pub id: i64,
    /// Game the restored snapshot belongs to.
    pub game_id: i64,
    /// Snapshot that was restored.
    pub snapshot_id: i64,
    /// Device that performed the restore, when the client reported one.
    pub device_id: Option<String>,
    /// `server` for a cloud snapshot, `local` for a device restore point.
    pub kind: String,
    /// Restore timestamp.
    pub restored_at: DateTime<Utc>,
}

/// Row of `save_restore_receipts`.
#[derive(Debug, sqlx::FromRow)]
pub struct SaveRestoreReceiptRow {
    pub id: i64,
    pub game_id: i64,
    pub snapshot_id: i64,
    pub device_id: Option<String>,
    pub kind: String,
    pub restored_at: String,
}

impl SaveRestoreReceiptRow {
    /// Public representation, hiding the owning account.
    #[must_use]
    pub fn to_public(self) -> SaveRestoreReceipt {
        SaveRestoreReceipt {
            id: self.id,
            game_id: self.game_id,
            snapshot_id: self.snapshot_id,
            device_id: self.device_id,
            kind: self.kind,
            restored_at: crate::db::parse_time(&self.restored_at).unwrap_or_else(Utc::now),
        }
    }
}

/// Snapshot history and restore receipts for one game (#295).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveHistory {
    /// Snapshots, newest first, including pinned restore points.
    pub snapshots: Vec<SaveSnapshot>,
    /// Restore receipts, newest first.
    pub receipts: Vec<SaveRestoreReceipt>,
    /// Automatic snapshots the server keeps outside the pinned ones.
    pub auto_retention: i64,
}

/// Body of `POST /api/saves/{id}/restore` (#295).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoreSnapshotRequest {
    /// Device that performed the restore; stored for the history.
    #[serde(default)]
    pub device_id: Option<String>,
    /// `server` (default) or `local`.
    #[serde(default)]
    pub kind: Option<String>,
}

/// Result of `POST /api/saves/{id}/restore`: the snapshot is now a restore
/// point and the receipt is stored in the history.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoreSnapshotResponse {
    /// Pinned snapshot.
    pub snapshot: SaveSnapshot,
    /// Stored restore receipt.
    pub receipt: SaveRestoreReceipt,
}

/// Body of `POST /api/games/{id}/saves/prune` (#295).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PruneSavesRequest {
    /// Automatic snapshots to keep besides the pinned ones. Clamped to 1–50.
    #[serde(default)]
    pub keep: Option<i64>,
}

/// Result of a retention pass (#295). The newest snapshot and every pinned
/// restore point are never removed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SavePruneReport {
    /// Snapshots removed by this pass.
    pub removed: i64,
    /// Snapshots still stored for the game.
    pub kept: i64,
    /// Pinned restore points protected from the pass.
    pub protected: i64,
}

/// Public view of one open or resolved save conflict (#296).
///
/// Both snapshots stay on the server: resolving only relabels the losing side
/// as `save (conflict <device> <date>)`, it never deletes it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveConflictView {
    /// Conflict id.
    pub id: i64,
    /// Game the conflict belongs to.
    pub game_id: i64,
    /// `open` or `resolved`.
    pub status: String,
    /// `both_changed`, `clock_skew` or `counter_diverged`.
    pub reason: String,
    /// When the policy detected the conflict.
    pub detected_at: DateTime<Utc>,
    /// When the user chose a side.
    pub resolved_at: Option<DateTime<Utc>>,
    /// `keep_local` or `keep_remote`.
    pub strategy: Option<String>,
    /// Snapshot that was already on the server.
    pub existing: SaveSnapshot,
    /// Snapshot uploaded by the syncing device.
    pub candidate: SaveSnapshot,
}

/// Per-game conflict policy (#296): `ask` (default) or `prefer_device`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SavePolicyView {
    /// `ask` or `prefer_device`.
    pub policy: String,
    /// Device that always wins when `policy` is `prefer_device`.
    pub device_id: Option<String>,
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
    /// Active backend: `sqlite` or `postgres`.
    pub backend: String,
    /// Database size in bytes (SQLite file + WAL, or `pg_database_size`).
    pub size_bytes: u64,
    /// Live pooled connections.
    pub connections: u32,
    /// Idle pooled connections.
    pub idle_connections: u32,
    /// Configured pool ceiling.
    pub max_connections: u32,
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

/// Password change request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    /// Current password.
    pub current_password: String,
    /// Replacement password.
    pub new_password: String,
}

/// `PUT /api/auth/onboarding` request (#119).
///
/// Only the two onboarding preference fields can be changed here; the
/// administrator user PATCH ignores them.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingRequest {
    /// Set the completion flag, or clear it to restart the wizard.
    #[serde(default)]
    pub completed: Option<bool>,
    /// Canonical step to resume at, 0..=20.
    #[serde(default)]
    pub step: Option<i64>,
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
    /// Platform slug for ROM entries (`snes`, `gba`, ...).
    #[serde(default)]
    pub platform: Option<String>,
    /// Entry kind: `game` or `rom`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Availability filter (#210): `available` (default), `missing`, `partial`
    /// or `all`. The default hides metadata rows with no usable archive.
    #[serde(default)]
    pub status: Option<String>,
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
    /// Client-generated idempotency key (the desktop's local session id).
    ///
    /// The desktop keeps finished sessions in a local outbox and retries them
    /// until the server acknowledges; replaying the same id after a crash
    /// between "applied" and "marked synced" must not add the minutes twice.
    /// Omit it for additive behaviour (older clients, scripts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// Bookmark update.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkRequest {
    /// Desired bookmark state.
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
    /// Client download job shared by every range of one archive (#217).
    /// Equivalent to the `X-Ludex-Job-Id` header, which takes precedence.
    #[serde(default)]
    pub job_id: Option<String>,
    /// Zero-based index of this range within the job (#217), logged only.
    #[serde(default)]
    pub part_index: Option<u32>,
}

/// Query string for the ZIP stream-install route.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StreamQuery {
    /// Short-lived download ticket, accepted instead of a Bearer token.
    #[serde(default)]
    pub token: Option<String>,
    /// `restart=1`: serve the entry from byte 0 and ignore `Range` (#309).
    /// Used by the client when the server refuses to decompress a compressed
    /// entry up to a large offset.
    #[serde(default)]
    pub restart: Option<String>,
}

impl StreamQuery {
    /// Whether `?restart=1` (also `true`/`yes`) was requested.
    pub fn restart_requested(&self) -> bool {
        matches!(
            self.restart.as_deref().map(str::trim),
            Some("1" | "true" | "yes")
        )
    }
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
    /// `game` (default) or `rom`. ROM roots index emulator ROMs instead of
    /// game archives.
    #[serde(default)]
    pub kind: Option<String>,
    /// Free-space floor kept free on the root (#290), in bytes. Defaults to
    /// 5 GiB when omitted.
    #[serde(default)]
    pub min_free_bytes: Option<u64>,
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
    /// `game` or `rom`; cannot be changed while the root holds versions.
    #[serde(default)]
    pub kind: Option<String>,
    /// New free-space floor in bytes (#290). `0` disables the floor.
    #[serde(default)]
    pub min_free_bytes: Option<u64>,
}

/// `POST /api/integrity/check` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityCheckRequest {
    /// `all` (default), `archives` or `media`.
    #[serde(default)]
    pub scope: Option<String>,
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

/// Anonymous diagnostics report submitted by an opted-in client (#171).
///
/// The body is bounded by the route (24 KiB) and every string is redacted
/// again server-side before storage. `installId` is a random value generated
/// on the device; it is never derived from hardware or an account.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticsReportInput {
    /// Random install id generated locally (`[A-Za-z0-9_-]{8,64}`).
    pub install_id: String,
    /// App or server version that produced the report.
    pub app_version: String,
    /// `linux`, `windows`, `macos`, `web` or `other`.
    pub platform: String,
    /// Operating system name and version.
    pub os: String,
    /// UI locale, when known.
    #[serde(default)]
    pub locale: String,
    /// Release channel (`stable`, `early`, `dev`), when known.
    #[serde(default)]
    pub channel: String,
    /// `panic`, `startup`, `command`, `api`, `install`, `launch`, `update`,
    /// `web` or `other`.
    pub error_kind: String,
    /// One-line redacted error summary.
    pub summary: String,
    /// Structured, bounded context (redacted again server-side).
    #[serde(default)]
    pub detail: serde_json::Value,
}

/// Row of the `diagnostics_reports` table.
#[derive(Debug, sqlx::FromRow)]
pub struct DiagnosticsReportRow {
    pub id: i64,
    pub install_id: String,
    pub app_version: String,
    pub platform: String,
    pub os: String,
    pub locale: String,
    pub channel: String,
    pub error_kind: String,
    pub summary: String,
    pub detail_json: String,
    pub created_at: String,
}

impl DiagnosticsReportRow {
    /// Public representation; `detail_json` is parsed back to JSON.
    #[must_use]
    pub fn to_public(&self) -> DiagnosticsReport {
        DiagnosticsReport {
            id: self.id,
            install_id: self.install_id.clone(),
            app_version: self.app_version.clone(),
            platform: self.platform.clone(),
            os: self.os.clone(),
            locale: self.locale.clone(),
            channel: self.channel.clone(),
            error_kind: self.error_kind.clone(),
            summary: self.summary.clone(),
            detail: serde_json::from_str(&self.detail_json)
                .unwrap_or_else(|_| serde_json::json!({})),
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
        }
    }
}

/// One stored, already-redacted diagnostics report (#171).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    /// Row id, also the pagination cursor.
    pub id: i64,
    /// Random install id generated by the reporting device.
    pub install_id: String,
    /// App or server version.
    pub app_version: String,
    /// `linux`, `windows`, `macos`, `web` or `other`.
    pub platform: String,
    /// Operating system name and version.
    pub os: String,
    /// UI locale.
    pub locale: String,
    /// Release channel.
    pub channel: String,
    /// Error kind.
    pub error_kind: String,
    /// Redacted one-line summary.
    pub summary: String,
    /// Bounded, redacted context.
    pub detail: serde_json::Value,
    /// Submission time.
    pub created_at: DateTime<Utc>,
}

/// One value and how many reports carry it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsCount {
    /// Distinct value (version, platform or error kind).
    pub value: String,
    /// Number of reports with that value.
    pub count: i64,
}

/// Admin page of diagnostics reports (#171), with aggregate counts.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsPage {
    /// Newest-first reports.
    pub items: Vec<DiagnosticsReport>,
    /// Pass as `before` to fetch the next page; `null` on the last page.
    pub next_before: Option<i64>,
    /// Total reports currently stored.
    pub total: i64,
    /// Reports grouped by app version.
    pub by_version: Vec<DiagnosticsCount>,
    /// Reports grouped by error kind.
    pub by_kind: Vec<DiagnosticsCount>,
    /// Reports grouped by platform.
    pub by_platform: Vec<DiagnosticsCount>,
    /// Retention applied by the server, in days.
    pub retention_days: i64,
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

/// Row of the `collections` table joined with its owner.
#[derive(Debug, sqlx::FromRow)]
pub struct CollectionRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_public: i64,
    pub visibility: String,
    pub copied_from_id: Option<i64>,
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
        let visibility =
            CollectionVisibility::parse(&self.visibility).unwrap_or(if self.is_public != 0 {
                CollectionVisibility::Public
            } else {
                CollectionVisibility::Private
            });
        Collection {
            id: self.id,
            name: self.name.clone(),
            description: self.description.clone(),
            is_public: visibility == CollectionVisibility::Public,
            visibility,
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
    /// Whether other accounts can read it. Prefer `visibility`; `true` maps to
    /// `public`, `false` to `private`.
    #[serde(default)]
    pub is_public: Option<bool>,
    /// Who can read it (#173). Wins over `isPublic` when both are present.
    #[serde(default)]
    pub visibility: Option<CollectionVisibility>,
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
    /// Public visibility (kept for older clients; prefer `visibility`).
    #[serde(default)]
    pub is_public: Option<bool>,
    /// New visibility (#173). Wins over `isPublic` when both are present.
    #[serde(default)]
    pub visibility: Option<CollectionVisibility>,
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

/// `PATCH /api/me/profile` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProfileRequest {
    /// Opt in or out of the public directory and feed.
    #[serde(default)]
    pub public_profile: Option<bool>,
    /// Opt in or out of friend-visible presence (#161).
    #[serde(default)]
    pub presence_enabled: Option<bool>,
    /// Opt in or out of the friends-only activity feed (#161).
    #[serde(default)]
    pub friends_activity: Option<bool>,
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
pub(crate) fn deserialize_some<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
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
            device: crate::security::describe_user_agent(self.user_agent.as_deref()),
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

/* ---------------------- invitations and account security (#166) ---------- */

/// Row of the `invitations` table.
#[derive(Debug, sqlx::FromRow)]
pub struct InvitationRow {
    pub id: i64,
    pub role: String,
    pub email: Option<String>,
    pub note: Option<String>,
    pub created_by: Option<i64>,
    pub created_by_username: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub max_uses: i64,
    pub uses: i64,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

impl InvitationRow {
    /// Public representation with the lifecycle state computed at `now`.
    #[must_use]
    pub fn to_public(&self, now: DateTime<Utc>) -> Invitation {
        let expires_at = crate::db::parse_time(&self.expires_at).unwrap_or(now);
        let revoked_at = self.revoked_at.as_deref().and_then(crate::db::parse_time);
        let status = if revoked_at.is_some() {
            InvitationStatus::Revoked
        } else if self.uses >= self.max_uses {
            InvitationStatus::Used
        } else if expires_at <= now {
            InvitationStatus::Expired
        } else {
            InvitationStatus::Pending
        };
        Invitation {
            id: self.id,
            role: self.role.parse::<Role>().unwrap_or(Role::User),
            email: self.email.clone(),
            note: self.note.clone(),
            created_by: self.created_by,
            created_by_username: self.created_by_username.clone(),
            created_at: crate::db::parse_time(&self.created_at).unwrap_or(now),
            expires_at,
            max_uses: self.max_uses,
            uses: self.uses,
            last_used_at: self.last_used_at.as_deref().and_then(crate::db::parse_time),
            revoked_at,
            status,
            // The token is unknown at read time; the client builds the URL with
            // the token it received once at creation.
            path: "/register".to_string(),
        }
    }
}

/// `POST /api/invitations` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateInvitationRequest {
    /// Role granted on redemption, defaults to `user`.
    #[serde(default)]
    pub role: Option<Role>,
    /// Optional e-mail label.
    #[serde(default)]
    pub email: Option<String>,
    /// Optional note for administrators.
    #[serde(default)]
    pub note: Option<String>,
    /// Days until the link expires, defaults to 7 (1..=90).
    #[serde(default)]
    pub expires_in_days: Option<i64>,
    /// Maximum redemptions, defaults to 1 (1..=100).
    #[serde(default)]
    pub max_uses: Option<i64>,
}

/// `POST /api/invitations/{token}/accept` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcceptInvitationRequest {
    /// Login name for the new account.
    pub username: String,
    /// Initial password.
    pub password: String,
}

/// `POST /api/auth/mfa/totp/activate` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TotpActivationRequest {
    /// Six-digit code from the authenticator app.
    pub code: String,
}

/// `POST /api/auth/mfa/disable` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MfaDisableRequest {
    /// Current account password, re-checked before removing the factor.
    pub password: String,
}

/// `POST /api/auth/mfa/recovery-codes` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCodesRequest {
    /// Current account password.
    pub password: String,
}

/// `POST /api/auth/login/mfa` request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MfaLoginRequest {
    /// Challenge returned by the password step.
    pub challenge_token: String,
    /// Six-digit TOTP code, or a recovery code.
    pub code: String,
}

/// `POST /api/users/{id}/reset-password` request.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordRequest {
    /// Replacement password. Omit to let the server generate one; a generated
    /// password is returned once so the administrator can hand it over.
    #[serde(default)]
    pub password: Option<String>,
    /// Ask the account to choose a new password after signing in (default true).
    #[serde(default)]
    pub require_change: Option<bool>,
}

/* --------------------- device registry (#184) ----------------------------- */

/// Body of `POST /api/devices/heartbeat`.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceHeartbeatRequest {
    /// Stable, client-generated id stored in the desktop settings. Not a token.
    pub client_id: String,
    /// Display name ("Gaming PC"). Omitted keeps the current name.
    #[serde(default)]
    pub name: Option<String>,
    /// `windows`, `linux`, `macos`, ...
    #[serde(default)]
    pub platform: Option<String>,
    /// Desktop app version, e.g. `0.4.0`.
    #[serde(default)]
    pub app_version: Option<String>,
    /// What this PC can do; drives the phone's cards and a future launch.
    #[serde(default)]
    pub capabilities: DeviceCapabilities,
}

/// `PUT /api/games/{id}/versions/default` request.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetDefaultVersionRequest {
    /// Version to pin. It must belong to the game and still exist.
    pub version_id: i64,
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
