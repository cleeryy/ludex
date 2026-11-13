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

/// An account on the server.
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
}

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
