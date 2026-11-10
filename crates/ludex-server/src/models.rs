//! Server-side DTOs and SQL row shapes.
//!
//! Response bodies reuse the shared `ludex_core::models` types so the web UI and
//! desktop app see exactly the contract's shapes.

use chrono::{DateTime, Utc};
use ludex_core::models::{LaunchInfo, ProgressState, Role, User};
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
    pub file_path: String,
    pub file_name: String,
    pub size: i64,
    pub version: Option<String>,
    pub release_date: Option<String>,
    pub early_access: i64,
    pub missing: i64,
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
}

impl UserRow {
    /// Public representation, hiding the password hash.
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

/// Scanner status exposed by `GET /api/library/scan`.
#[derive(Debug, Clone, Default, Serialize)]
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
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    /// Signed access token.
    pub token: String,
    /// Authenticated account.
    pub user: User,
}

/// Login request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    /// Login name.
    pub username: String,
    /// Plain-text password.
    pub password: String,
}

/// First-run setup request.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    /// Always `ok` when the request succeeds.
    pub status: &'static str,
    /// Server version.
    pub version: &'static str,
    /// Process uptime in seconds.
    pub uptime_seconds: u64,
}

/// Feature flags exposed by `GET /api/config`.
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
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
    /// Display name for the SSO button.
    pub oidc_display_name: String,
}

/// Password change request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    /// Current password.
    pub current_password: String,
    /// Replacement password.
    pub new_password: String,
}

/// Admin user creation request.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    /// New e-mail (`null` or `""` clears it).
    #[serde(default, deserialize_with = "deserialize_some")]
    pub email: Option<Option<String>>,
    /// New role.
    #[serde(default)]
    pub role: Option<Role>,
    /// New activation flag.
    #[serde(default)]
    pub activated: Option<bool>,
}

/// Query string for `GET /api/games` and `/api/games/random`.
#[derive(Debug, Default, Deserialize)]
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

/// Editor game metadata update.
#[derive(Debug, Default, Deserialize)]
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
    /// Launch configuration, replaces all launch fields.
    #[serde(default)]
    pub launch: Option<LaunchInfo>,
}

/// Progress state update.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressRequest {
    /// New state.
    pub state: ProgressState,
}

/// Playtime report.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaytimeRequest {
    /// Minutes played; clamped to 1..=1440.
    pub minutes: i64,
}

/// Bookmark update.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkRequest {
    /// Desired bookmark state.
    pub bookmarked: bool,
}

/// Bookmark response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkResponse {
    /// Current bookmark state.
    pub bookmarked: bool,
}

/// Download ticket request.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketRequest {
    /// Specific version, or the newest available one.
    #[serde(default)]
    pub version_id: Option<i64>,
}

/// Download ticket response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketResponse {
    /// Absolute-path URL including the short-lived ticket.
    pub url: String,
    /// Ticket expiry.
    pub expires_at: DateTime<Utc>,
}

/// Query string for the download route.
#[derive(Debug, Default, Deserialize)]
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
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamQuery {
    /// Short-lived download ticket, accepted instead of a Bearer token.
    #[serde(default)]
    pub token: Option<String>,
}

/// Local path response.
#[derive(Debug, Serialize)]
pub struct LocalPathResponse {
    /// Absolute path on the server's filesystem.
    pub path: String,
}

/// Scan trigger response.
#[derive(Debug, Serialize)]
pub struct ScanResponse {
    /// Whether a scan was queued.
    pub started: bool,
}

/// Metadata refresh response.
#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    /// Always true: enrichment runs in the background.
    pub queued: bool,
}

/// Library folder response.
#[derive(Debug, Serialize)]
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
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNewsRequest {
    /// Post title.
    pub title: String,
    /// Markdown body.
    pub body: String,
}

/// News update request.
#[derive(Debug, Default, Deserialize)]
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
#[derive(Debug, Serialize)]
pub struct UserResponse {
    /// Account.
    pub user: User,
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
    }

    #[test]
    fn scan_status_serialises_camel_case() {
        let status = ScanStatus::default();
        let json = serde_json::to_string(&status).expect("json");
        assert!(json.contains("\"metadataPending\":0"), "{json}");
        assert!(json.contains("\"startedAt\":null"), "{json}");
    }
}
