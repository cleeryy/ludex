//! Serialisable domain types shared by the Ludex server, web UI and desktop app.
//!
//! Every type serialises to `camelCase` JSON, matching `docs/CONTRACT.md`.
//! Timestamps are RFC 3339 strings (`chrono::DateTime<Utc>`).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// User role. Serialises as `admin`, `editor` or `user`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Full access, including user management and server settings.
    Admin,
    /// Can edit metadata, scan the library and manage ingests.
    Editor,
    /// Can browse, download, track progress and bookmark games.
    User,
}

impl Role {
    /// Stable lowercase name used in the database and JWTs.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Editor => "editor",
            Role::User => "user",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = ParseRoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Role::Admin),
            "editor" => Ok(Role::Editor),
            "user" => Ok(Role::User),
            other => Err(ParseRoleError(other.to_string())),
        }
    }
}

/// Error returned when parsing an unknown [`Role`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown role: {0}")]
pub struct ParseRoleError(pub String);

/// Per-user progress for a game. Serialises as `unplayed`, `playing`,
/// `completed`, `abandoned`, `onHold` or `planToPlay`.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ProgressState {
    /// Never launched.
    #[default]
    Unplayed,
    /// Currently being played.
    Playing,
    /// Finished the main content.
    Completed,
    /// Dropped before finishing.
    Abandoned,
    /// Paused, intending to resume later.
    OnHold,
    /// Owned but not started yet.
    PlanToPlay,
}

impl ProgressState {
    /// All states, in the order used by the UI.
    pub const ALL: [ProgressState; 6] = [
        ProgressState::Unplayed,
        ProgressState::Playing,
        ProgressState::Completed,
        ProgressState::Abandoned,
        ProgressState::OnHold,
        ProgressState::PlanToPlay,
    ];

    /// Stable camelCase name used in the database and JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            ProgressState::Unplayed => "unplayed",
            ProgressState::Playing => "playing",
            ProgressState::Completed => "completed",
            ProgressState::Abandoned => "abandoned",
            ProgressState::OnHold => "onHold",
            ProgressState::PlanToPlay => "planToPlay",
        }
    }
}

impl fmt::Display for ProgressState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProgressState {
    type Err = ParseProgressStateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unplayed" => Ok(ProgressState::Unplayed),
            "playing" => Ok(ProgressState::Playing),
            "completed" => Ok(ProgressState::Completed),
            "abandoned" => Ok(ProgressState::Abandoned),
            "onHold" => Ok(ProgressState::OnHold),
            "planToPlay" => Ok(ProgressState::PlanToPlay),
            other => Err(ParseProgressStateError(other.to_string())),
        }
    }
}

/// Error returned when parsing an unknown [`ProgressState`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown progress state: {0}")]
pub struct ParseProgressStateError(pub String);

/// A game as shown in library grids and search results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameSummary {
    /// Database identifier.
    pub id: i64,
    /// Display title.
    pub title: String,
    /// Title used for sorting (leading English articles removed).
    pub sort_title: String,
    /// Release date, when known.
    pub release_date: Option<DateTime<Utc>>,
    /// Cover image URL, when cached.
    pub cover_url: Option<String>,
    /// Background image URL, when cached.
    pub background_url: Option<String>,
    /// Aggregated rating (0-100), when known.
    pub rating: Option<f64>,
    /// Genre labels.
    pub genres: Vec<String>,
    /// Whether the game is still in early access.
    pub early_access: bool,
    /// Number of available archives backing this game (missing rows excluded).
    pub version_count: u32,
    /// Available versions: the same count as `version_count`, spelled out for
    /// clients that filter on availability (#210).
    #[serde(default)]
    pub available_version_count: u32,
    /// Versions whose archive is missing or on an offline root (#210).
    #[serde(default)]
    pub missing_version_count: u32,
    /// Availability derived from the two counts: `available`,
    /// `partially_missing` or `missing`. Metadata is never deleted by a scan.
    #[serde(default = "default_version_status")]
    pub status: String,
    /// Sum of the sizes of all available versions, in bytes.
    pub total_size: i64,
    /// Progress of the requesting user.
    pub state: ProgressState,
    /// Minutes played by the requesting user.
    pub minutes_played: i64,
    /// Whether the requesting user bookmarked this game.
    pub bookmarked: bool,
    /// Last launch timestamp for the requesting user.
    pub last_played_at: Option<DateTime<Utc>>,
    /// Highest supported player count, when known.
    #[serde(default)]
    pub max_players: Option<u32>,
    /// Field that matched the search query (`title`, `sortTitle`, `altTitle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_field: Option<String>,
    /// Title or alias text that matched the query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_title: Option<String>,
    /// Relevance score (0-1000) when the row came from a search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_score: Option<f64>,
    /// Entry kind: `game` for an archive, `rom` for an emulator ROM (#52).
    #[serde(default = "default_game_kind")]
    pub kind: String,
    /// Platform slug for ROM entries (`snes`, `gba`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Per-game emulator override; `null` uses the platform default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulator_id: Option<i64>,
}

fn default_game_kind() -> String {
    "game".to_string()
}

fn default_version_status() -> String {
    "available".to_string()
}

/// One downloadable file (archive or executable) of a game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameVersion {
    /// Database identifier.
    pub id: i64,
    /// File name on the library volume.
    pub file_name: String,
    /// File size in bytes.
    pub size: i64,
    /// Parsed version label, e.g. `1.2.3`.
    pub version: Option<String>,
    /// Release date attached to this version.
    pub release_date: Option<DateTime<Utc>>,
    /// Whether this version is an early-access build.
    pub early_access: bool,
    /// Cheap archive fingerprint (`sha256-sample:…`), see
    /// [`crate::fingerprint`]. `null` until the server has computed it.
    #[serde(default)]
    pub archive_hash: Option<String>,
    /// Full SHA-256 of the archive bytes (#216). `null` for legacy rows the
    /// background job has not reached yet; those stay downloadable unverified.
    #[serde(default)]
    pub content_sha256: Option<String>,
    /// Archive format guessed from the file name (`zip`, `7z`, `rar`), `null`
    /// when the extension is unknown or absent (#226). Never inspects the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_format: Option<String>,
    /// Whether the ZIP-only per-entry stream install can serve this version
    /// (#226). `false` means clients must download the archive and extract it.
    #[serde(default)]
    pub stream_supported: bool,
    /// Last time this version row changed (indexed, renamed or re-hashed).
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    /// The archive vanished at the last scan (#210). Never auto-deleted.
    #[serde(default)]
    pub missing: bool,
    /// The library root holding this archive was offline at the last scan.
    #[serde(default)]
    pub root_unavailable: bool,
    /// Download URL (`/api/games/{id}/download?versionId={id}`).
    pub download_url: String,
}

/// The newest available version of a game, used by clients to detect updates.
///
/// Returned by `GET /api/games/{id}/versions/latest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LatestVersion {
    /// Game this version belongs to.
    pub game_id: i64,
    /// Version row identifier.
    pub version_id: i64,
    /// Parsed version label (`1.2.3`), when the file name carried one.
    pub version_label: Option<String>,
    /// File name on the library volume.
    pub file_name: String,
    /// Cheap archive fingerprint, `null` until computed.
    pub archive_hash: Option<String>,
    /// Full SHA-256 of the archive bytes (#216), `null` while pending.
    #[serde(default)]
    pub content_sha256: Option<String>,
    /// Archive size in bytes.
    pub archive_size: i64,
    /// When the version row was last updated.
    pub updated_at: Option<DateTime<Utc>>,
    /// Whether this is an early-access build.
    pub early_access: bool,
    /// Download URL for this version.
    pub download_url: String,
}

/// Launch configuration for a game, editable by editors.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LaunchInfo {
    /// Main executable, relative to the install folder.
    pub executable: Option<String>,
    /// Arguments passed to the main executable.
    pub parameters: Option<String>,
    /// Installer executable, relative to the install folder.
    pub installer_executable: Option<String>,
    /// Arguments passed to the installer.
    pub installer_parameters: Option<String>,
    /// Uninstaller executable, relative to the install folder.
    pub uninstaller_executable: Option<String>,
    /// Arguments passed to the uninstaller.
    pub uninstaller_parameters: Option<String>,
}

/// A video trailer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Trailer {
    /// Trailer title.
    pub title: String,
    /// Video URL.
    pub url: String,
    /// Optional thumbnail URL.
    pub thumbnail_url: Option<String>,
    /// Provider-specific video id (the YouTube id for IGDB videos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_id: Option<String>,
    /// Playback provider (`youtube` for IGDB, `file` for direct media URLs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// Provider metadata reference for a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRef {
    /// Provider slug (`igdb`, `steam`, ...).
    pub slug: String,
    /// Identifier inside the provider.
    pub id: String,
    /// Public provider page, when available.
    pub url: Option<String>,
}

/// Full game payload, extending [`GameSummary`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameDetail {
    /// Summary fields, flattened into the same JSON object.
    #[serde(flatten)]
    pub summary: GameSummary,
    /// Long description.
    pub description: Option<String>,
    /// Screenshot media URLs.
    pub screenshots: Vec<String>,
    /// Trailers (kept for clients written against v0.2; prefer `videos`).
    pub trailers: Vec<Trailer>,
    /// Trailers and other videos, with provider metadata for in-app playback.
    #[serde(default)]
    pub videos: Vec<Trailer>,
    /// Developer names.
    pub developers: Vec<String>,
    /// Publisher names.
    pub publishers: Vec<String>,
    /// Free-form tags.
    pub tags: Vec<String>,
    /// Minimum age rating.
    pub age_rating: Option<u8>,
    /// Metadata provider reference.
    pub provider: Option<ProviderRef>,
    /// Alternative titles (provider aliases) used by fuzzy search.
    #[serde(default)]
    pub alt_titles: Vec<String>,
    /// Available archives backing this game.
    pub versions: Vec<GameVersion>,
    /// Rows whose archive vanished or whose root is offline (#210). Kept so a
    /// rescan can restore them; never offered for download.
    #[serde(default)]
    pub missing_versions: Vec<GameVersion>,
    /// Version used by downloads, fresh installs and the update check. When
    /// `null`, the newest available version is used (#168).
    #[serde(default)]
    pub default_version_id: Option<i64>,
    /// Launch configuration.
    pub launch: LaunchInfo,
}

/// A news post shown on the server home page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewsPost {
    /// Database identifier.
    pub id: i64,
    /// Post title.
    pub title: String,
    /// Markdown body.
    pub body: String,
    /// Author display name.
    pub author: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last edit timestamp.
    pub updated_at: DateTime<Utc>,
}

/// A page of results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paginated<T> {
    /// Items on this page.
    pub items: Vec<T>,
    /// Total number of matching items.
    pub total: i64,
    /// Current page, 1-based.
    pub page: i64,
    /// Page size.
    pub limit: i64,
}

/// Progress returned after a progress or playtime update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProgressUpdate {
    /// New progress state.
    pub state: ProgressState,
    /// Total minutes played.
    pub minutes_played: i64,
    /// Last launch timestamp.
    pub last_played_at: Option<DateTime<Utc>>,
}

/// Server-wide statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Number of games.
    pub games: i64,
    /// Number of files (versions).
    pub versions: i64,
    /// Total library size in bytes.
    pub total_bytes: i64,
    /// Number of user accounts.
    pub users: i64,
    /// Games per progress state.
    pub by_state: BTreeMap<ProgressState, i64>,
}

/// One archive in the editor version manager (#168).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameVersionInfo {
    /// Database identifier.
    pub id: i64,
    /// File name on the library volume.
    pub file_name: String,
    /// Archive size in bytes.
    pub size: i64,
    /// Parsed version label, e.g. `1.2.3`.
    pub version: Option<String>,
    /// Release date attached to this version.
    pub release_date: Option<DateTime<Utc>>,
    /// Whether this is an early-access build.
    pub early_access: bool,
    /// Cheap archive fingerprint, `null` until computed.
    pub archive_hash: Option<String>,
    /// Full SHA-256 of the archive (#216), `null` while a legacy row awaits
    /// the background hash job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    /// Archive format guessed from the file name (`zip`, `7z`, `rar`), `null`
    /// when the extension is unknown or absent (#226).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_format: Option<String>,
    /// Whether the ZIP-only per-entry stream install can serve this version
    /// (#226).
    #[serde(default)]
    pub stream_supported: bool,
    /// When the scanner first indexed the row.
    pub indexed_at: Option<DateTime<Utc>>,
    /// Last time the row changed.
    pub updated_at: Option<DateTime<Utc>>,
    /// The archive disappeared from its root; the row is kept for audit.
    pub missing: bool,
    /// The library root holding the archive is offline.
    pub root_unavailable: bool,
    /// Download URL for this version.
    pub download_url: String,
}

/// `GET /api/games/{id}/versions` and `PUT .../versions/default` response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameVersionsResponse {
    /// Game these versions belong to.
    pub game_id: i64,
    /// Pinned version, `null` when the newest available one is used.
    pub default_version_id: Option<i64>,
    /// Every row, newest first, including missing archives.
    pub versions: Vec<GameVersionInfo>,
}

/// `DELETE /api/games/{id}/versions/{version_id}` response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteVersionResponse {
    /// Deleted version row.
    pub version_id: i64,
    /// Bytes freed on disk (and removed from object storage, when used).
    pub freed_bytes: i64,
    /// Pinned version after the deletion; `null` when the game has none.
    pub default_version_id: Option<i64>,
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
    /// When the account finished or dismissed the first-run wizard (#119).
    pub onboarding_completed_at: Option<DateTime<Utc>>,
    /// Canonical step the wizard resumes at (#119), 0..=20.
    pub onboarding_step: i64,
    /// Whether a TOTP second factor is active for this account (#166).
    #[serde(default)]
    pub mfa_enabled: bool,
    /// Whether an administrator reset the password and the account should
    /// choose a new one (#166). Advisory for clients.
    #[serde(default)]
    pub must_change_password: bool,
    /// Interface language preference (#188). `None` or empty means inherit:
    /// the server default, then the browser, then English.
    #[serde(default)]
    pub locale: Option<String>,
}

impl User {
    /// Whether an identity provider owns this account's identity fields.
    #[must_use]
    pub fn is_oidc_managed(&self) -> bool {
        self.source == UserSource::Oidc
    }
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
    /// Newest save file mtime reported by the device, in device time (#296).
    pub client_mtime: Option<DateTime<Utc>>,
    /// Device clock minus server clock, in seconds, at upload time (#296).
    pub clock_offset_secs: i64,
    /// The game's own save counter, when the client knows it (#296).
    pub save_seq: Option<i64>,
    /// Snapshot this row was kept aside from as the losing conflict side
    /// (#296). Such a row is a restore point, never "the latest save".
    pub conflict_of: Option<i64>,
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
///
/// When the account has a second factor, the password step returns
/// `mfaRequired: true` with a short-lived `challengeToken` instead of a session:
/// `token` and `user` are absent, so a client cannot mistake the challenge for
/// a signed-in state (#166).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    /// Signed access token; absent until every factor is verified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Authenticated account; absent while a second factor is pending.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
    /// Token expiry, when the token belongs to a tracked session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// True when the password was accepted but a TOTP or recovery code is
    /// still required.
    #[serde(default)]
    pub mfa_required: bool,
    /// Single-use challenge to submit to `POST /api/auth/login/mfa`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_token: Option<String>,
    /// When the challenge expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_expires_at: Option<DateTime<Utc>>,
}

impl AuthResponse {
    /// Response for a completed sign-in.
    #[must_use]
    pub fn session(token: String, user: User, expires_at: Option<DateTime<Utc>>) -> Self {
        Self {
            token: Some(token),
            user: Some(user),
            expires_at,
            mfa_required: false,
            challenge_token: None,
            challenge_expires_at: None,
        }
    }

    /// Response that asks for the second factor.
    #[must_use]
    pub fn challenge(token: String, expires_at: DateTime<Utc>) -> Self {
        Self {
            token: None,
            user: None,
            expires_at: None,
            mfa_required: true,
            challenge_token: Some(token),
            challenge_expires_at: Some(expires_at),
        }
    }
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
    /// Full archive SHA-256 published per version (#216). Always true in this
    /// build; clients use it to know the header/field can be trusted.
    pub archive_sha256: bool,
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
    /// Server-wide default interface language (#188). Empty means System:
    /// signed-out visitors use their browser languages, then English.
    pub default_locale: String,
    /// Largest number of parallel byte ranges the server accepts for one
    /// download job (#217). The server also caps concurrent parts per address.
    pub parallel_parts_max: u32,
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

/// Bookmark response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkResponse {
    /// Current bookmark state.
    pub bookmarked: bool,
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

/// `POST /api/library/roots/{id}/scan` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RootScanResponse {
    /// Whether a scan was queued.
    pub started: bool,
    /// Root the request targeted.
    pub root_id: i64,
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

/// Response wrapping a single user.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UserResponse {
    /// Account.
    pub user: User,
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

/// One page of audit entries.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditPage {
    /// Newest-first entries.
    pub items: Vec<AuditLog>,
    /// Pass as `before` to fetch the next page; `null` on the last page.
    pub next_before: Option<i64>,
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

/// Who can read a collection (#173).
///
/// `Friends` is intentionally absent: Ludex has no friend graph yet (#161), so
/// the server only offers private, unlisted and public. When a friend graph
/// lands, a fourth variant can be added without changing the other three.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum CollectionVisibility {
    /// Owner only.
    #[default]
    Private,
    /// Owner plus anyone holding a live share token; hidden from listings.
    Unlisted,
    /// Every authenticated account, the community profile and the feed.
    Public,
}

impl CollectionVisibility {
    /// Stable database and API value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Unlisted => "unlisted",
            Self::Public => "public",
        }
    }

    /// Parses a database or API value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "private" => Some(Self::Private),
            "unlisted" => Some(Self::Unlisted),
            "public" => Some(Self::Public),
            _ => None,
        }
    }

    /// Whether the collection appears in directory and profile listings.
    #[must_use]
    pub fn is_listed(self) -> bool {
        self == Self::Public
    }
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
    /// Whether other accounts can read it (kept for older clients; mirrors
    /// `visibility == "public"`).
    pub is_public: bool,
    /// Who can read it (#173).
    pub visibility: CollectionVisibility,
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
    /// Whether friends may see online/last-seen presence (#161).
    pub presence_enabled: bool,
    /// Whether friends see this account in their activity and achievement
    /// feed (#161). Independent from the public profile.
    pub friends_activity: bool,
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
    /// Readable device label derived from the `User-Agent` (#166).
    pub device: Option<String>,
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

/// Lifecycle state of an invitation, computed at read time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum InvitationStatus {
    /// Still redeemable.
    Pending,
    /// Every allowed use was consumed.
    Used,
    /// Past its expiry timestamp.
    Expired,
    /// Revoked by an administrator.
    Revoked,
}

impl InvitationStatus {
    /// Stable wire identifier.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            InvitationStatus::Pending => "pending",
            InvitationStatus::Used => "used",
            InvitationStatus::Expired => "expired",
            InvitationStatus::Revoked => "revoked",
        }
    }
}

/// `GET /api/invitations` item. The token is never included: only its hash is
/// stored, and the plaintext link is shown once at creation.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    /// Database identifier.
    pub id: i64,
    /// Role granted on redemption.
    pub role: Role,
    /// Optional e-mail label, shown to the invitee and copied into the account.
    pub email: Option<String>,
    /// Optional note for the administrator.
    pub note: Option<String>,
    /// Account that created the invitation.
    pub created_by: Option<i64>,
    /// Username of the creator, when the account still exists.
    pub created_by_username: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Expiry timestamp.
    pub expires_at: DateTime<Utc>,
    /// Maximum redemptions.
    pub max_uses: i64,
    /// Redemptions so far.
    pub uses: i64,
    /// Last redemption, if any.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Revocation timestamp, if revoked.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Computed lifecycle state.
    pub status: InvitationStatus,
    /// Client route the administrator shares: `/register?invite=…`.
    pub path: String,
}

/// `POST /api/invitations` response: the invitation plus the plaintext token,
/// returned exactly once.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedInvitation {
    /// Created invitation.
    pub invitation: Invitation,
    /// Plaintext token. Not stored, not retrievable.
    pub token: String,
    /// Client route to share: `/register?invite=<token>`.
    pub path: String,
}

/// `GET /api/invitations/{token}` response, public.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationPreview {
    /// Role the account will receive.
    pub role: Role,
    /// E-mail label, prefilled into the form.
    pub email: Option<String>,
    /// Note left by the administrator.
    pub note: Option<String>,
    /// Expiry timestamp.
    pub expires_at: DateTime<Utc>,
    /// Always true today: redeeming sets a password. Reserved for SSO-only
    /// invitations.
    pub requires_password: bool,
}

/// `GET /api/auth/mfa` response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MfaStatus {
    /// True once a TOTP secret is confirmed.
    pub enabled: bool,
    /// Unused recovery codes left.
    pub recovery_codes_remaining: i64,
    /// When the second factor was enabled.
    pub enabled_at: Option<DateTime<Utc>>,
    /// Enabled methods, in display order.
    pub methods: Vec<String>,
    /// True when an enrolment is in progress but not confirmed.
    pub pending: bool,
}

/// `POST /api/auth/mfa/totp/enroll` response. The secret is returned once.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TotpEnrollment {
    /// Base32 secret for manual entry.
    pub secret: String,
    /// `otpauth://` URI for a QR encoder.
    pub otpauth_uri: String,
    /// Issuer label baked into the URI (the server name).
    pub issuer: String,
    /// Account label baked into the URI.
    pub account: String,
    /// Code length.
    pub digits: u32,
    /// Seconds per code.
    pub period: i64,
}

/// `POST /api/auth/mfa/totp/activate` response: the recovery codes, once.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TotpActivated {
    /// Refreshed second-factor state.
    pub mfa: MfaStatus,
    /// Single-use recovery codes. Not retrievable later.
    pub recovery_codes: Vec<String>,
}

/// `POST /api/auth/mfa/recovery-codes` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCodes {
    /// Freshly generated single-use codes; the previous set stops working.
    pub codes: Vec<String>,
}

/// `POST /api/users/{id}/reset-password` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordResult {
    /// Account whose password was reset.
    pub username: String,
    /// Generated password, returned once. `null` when the administrator chose it.
    pub password: Option<String>,
    /// Sessions revoked by the reset.
    pub revoked: i64,
    /// Whether the account is asked to choose a new password.
    pub must_change_password: bool,
}

/// Capabilities a desktop reports with its heartbeat (#184).
///
/// Every field is optional on the wire so an older client can send a partial
/// object; the server stores the sanitised copy and mirrors the three values
/// the phone lists today into their own columns.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct DeviceCapabilities {
    /// Installed game ids on that PC. Capped at 500 entries.
    pub installed_game_ids: Vec<i64>,
    /// Count when the client does not send the id list.
    pub installed_count: Option<i64>,
    /// Free bytes on the install volume, `null` when unknown.
    pub free_disk_bytes: Option<i64>,
    /// A supported streaming host (Sunshine) is installed on that PC.
    pub streaming_available: bool,
}

/// One registered device, as returned by `GET /api/devices` (#184).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Server-side device id (UUID). Only this id is used to target installs.
    pub id: String,
    /// Display name chosen by the user, or the platform fallback.
    pub name: String,
    /// Operating system reported by the app.
    pub platform: Option<String>,
    /// Desktop app version at the last heartbeat.
    pub app_version: Option<String>,
    /// Last heartbeat is newer than 60 s and the device is not revoked.
    pub online: bool,
    /// Last heartbeat, RFC 3339.
    pub last_seen_at: String,
    /// Installed games on that PC at the last heartbeat.
    pub installed_count: i64,
    /// Free bytes on the install volume, `null` when unknown.
    pub free_disk_bytes: Option<i64>,
    /// Sunshine (streaming) is installed on that PC.
    pub streaming_available: bool,
    /// Raw capabilities, echoed for future features.
    pub capabilities: DeviceCapabilities,
    /// First registration.
    pub created_at: String,
    /// Set when the user revoked the device; revoked devices cannot heartbeat.
    pub revoked_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> GameSummary {
        GameSummary {
            id: 1,
            title: "HITMAN 3".into(),
            sort_title: "HITMAN 3".into(),
            release_date: None,
            cover_url: None,
            background_url: None,
            rating: Some(84.5),
            genres: vec!["Stealth".into()],
            early_access: false,
            version_count: 2,
            available_version_count: 2,
            missing_version_count: 0,
            status: "available".into(),
            total_size: 42,
            state: ProgressState::Playing,
            minutes_played: 90,
            bookmarked: true,
            last_played_at: None,
            max_players: Some(1),
            matched_field: None,
            matched_title: None,
            match_score: None,
            kind: "game".into(),
            platform: None,
            emulator_id: None,
        }
    }

    #[test]
    fn serialises_camel_case() {
        let json = serde_json::to_string(&summary()).expect("serialise");
        assert!(json.contains("\"sortTitle\":\"HITMAN 3\""), "{json}");
        assert!(json.contains("\"versionCount\":2"), "{json}");
        assert!(json.contains("\"totalSize\":42"), "{json}");
        assert!(json.contains("\"minutesPlayed\":90"), "{json}");
        assert!(json.contains("\"lastPlayedAt\":null"), "{json}");
        assert!(json.contains("\"state\":\"playing\""), "{json}");
    }

    #[test]
    fn progress_state_roundtrip() {
        for state in ProgressState::ALL {
            let json = serde_json::to_string(&state).expect("serialise");
            assert_eq!(json, format!("\"{}\"", state.as_str()));
            let back: ProgressState = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(back, state);
        }
        assert_eq!("onHold".parse::<ProgressState>(), Ok(ProgressState::OnHold));
        assert!("paused".parse::<ProgressState>().is_err());
    }

    #[test]
    fn role_roundtrip() {
        for (role, text) in [
            (Role::Admin, "admin"),
            (Role::Editor, "editor"),
            (Role::User, "user"),
        ] {
            assert_eq!(role.as_str(), text);
            assert_eq!(text.parse::<Role>(), Ok(role));
            assert_eq!(
                serde_json::to_string(&role).expect("serialise"),
                format!("\"{text}\"")
            );
        }
        assert!("root".parse::<Role>().is_err());
    }

    #[test]
    fn detail_flattens_summary() {
        let detail = GameDetail {
            summary: summary(),
            description: Some("A game".into()),
            screenshots: vec!["/api/media/a.jpg".into()],
            trailers: vec![Trailer {
                title: "Launch".into(),
                url: "https://example.com/v.mp4".into(),
                thumbnail_url: None,
                video_id: None,
                provider: Some("file".into()),
            }],
            videos: vec![Trailer {
                title: "Launch".into(),
                url: "https://example.com/v.mp4".into(),
                thumbnail_url: None,
                video_id: None,
                provider: Some("file".into()),
            }],
            developers: vec!["IO Interactive".into()],
            publishers: vec![],
            tags: vec!["stealth".into()],
            age_rating: Some(18),
            provider: Some(ProviderRef {
                slug: "steam".into(),
                id: "1659040".into(),
                url: None,
            }),
            alt_titles: vec!["Hitman III".into()],
            versions: vec![GameVersion {
                id: 7,
                file_name: "HITMAN 3 (v3.10.1) (2021).7z".into(),
                size: 1000,
                version: Some("3.10.1".into()),
                release_date: None,
                early_access: false,
                archive_hash: Some("sha256-sample:abc".into()),
                content_sha256: None,
                archive_format: Some("7z".into()),
                stream_supported: false,
                updated_at: None,
                missing: false,
                root_unavailable: false,
                download_url: "/api/games/1/download?versionId=7".into(),
            }],
            missing_versions: Vec::new(),
            default_version_id: Some(7),
            launch: LaunchInfo {
                executable: Some("HITMAN3.exe".into()),
                ..LaunchInfo::default()
            },
        };
        let json = serde_json::to_string(&detail).expect("serialise");
        assert!(json.contains("\"id\":1"), "{json}");
        assert!(json.contains("\"title\":\"HITMAN 3\""), "{json}");
        assert!(json.contains("\"downloadUrl\""), "{json}");
        // A 7z version keeps its format but cannot stream per entry (#226).
        assert!(json.contains("\"archiveFormat\":\"7z\""), "{json}");
        assert!(json.contains("\"streamSupported\":false"), "{json}");
        assert!(json.contains("\"installerExecutable\":null"), "{json}");
        let back: GameDetail = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, detail);
    }

    #[test]
    fn stats_serialises_state_keys() {
        let mut by_state = BTreeMap::new();
        by_state.insert(ProgressState::Unplayed, 3);
        by_state.insert(ProgressState::OnHold, 1);
        let stats = Stats {
            games: 4,
            versions: 5,
            total_bytes: 100,
            users: 1,
            by_state,
        };
        let json = serde_json::to_string(&stats).expect("serialise");
        assert!(json.contains("\"onHold\":1"), "{json}");
        let back: Stats = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, stats);
    }

    #[test]
    fn timestamps_are_rfc3339() {
        let stamp = "2026-11-09T20:15:30Z"
            .parse::<DateTime<Utc>>()
            .expect("date");
        let update = ProgressUpdate {
            state: ProgressState::Completed,
            minutes_played: 120,
            last_played_at: Some(stamp),
        };
        let json = serde_json::to_string(&update).expect("serialise");
        assert!(json.contains("2026-11-09T20:15:30Z"), "{json}");
        let back: ProgressUpdate = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, update);
    }

    #[test]
    fn paginated_roundtrip() {
        let page = Paginated {
            items: vec![summary()],
            total: 12,
            page: 1,
            limit: 48,
        };
        let json = serde_json::to_string(&page).expect("serialise");
        assert!(json.starts_with("{\"items\":["), "{json}");
        let back: Paginated<GameSummary> = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, page);
    }
}
