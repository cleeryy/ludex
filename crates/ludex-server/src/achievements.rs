//! Achievements framework and local providers (#117, #118).
//!
//! Definitions live in `achievements` (per game, or global when `game_id` is
//! null), unlock state in `user_achievements`. Unlocks are immutable: a rescan
//! never moves the first timestamp and duplicate rows are impossible thanks to
//! the `UNIQUE (user_id, achievement_id)` constraint.
//!
//! Three local providers are built in:
//!
//! - [`GameFilesProvider`]: Steam emulation files (`UserStats/Stats.ini`,
//!   `steam_settings/achievements.ini`) and Goldberg definitions
//!   (`steam_settings/achievements.json`).
//! - [`GalaxyApiProvider`]: loopback JSON endpoints for launchers that expose
//!   achievements over HTTP (configurable URL patterns).
//! - [`GalaxySqliteProvider`]: the GOG Galaxy `galaxy-2.0.db` database when it
//!   is available locally.
//!
//! Scans are server-side and only ever read paths inside the configured
//! allowlist (`LUDEX_ACHIEVEMENT_SCAN_ROOTS`, defaulting to the library roots).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::db::Database;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::state::AppState;

/// Maximum number of definitions returned in one response.
pub const MAX_DEFINITIONS: usize = 2_000;
/// Maximum accepted length for an achievement key.
const MAX_KEY_CHARS: usize = 128;
/// Maximum accepted length for an achievement name.
const MAX_NAME_CHARS: usize = 200;
/// Maximum accepted length for an achievement description.
const MAX_DESCRIPTION_CHARS: usize = 1_000;
/// Maximum accepted length for an icon reference.
const MAX_ICON_CHARS: usize = 512;
/// Upper bound for the points value of one achievement.
const MAX_POINTS: i64 = 10_000;
/// Timeout for localhost achievement APIs.
const LOCAL_API_TIMEOUT: Duration = Duration::from_secs(5);
/// Rows read from a Galaxy database, to bound a corrupt file.
const GALAXY_ROW_LIMIT: usize = 10_000;

/* ------------------------------- providers ------------------------------- */

/// One achievement reported by a provider.
#[derive(Debug, Clone)]
pub struct ProviderAchievement {
    /// Stable key inside the game namespace.
    pub key: String,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Point value, clamped by the server.
    pub points: i64,
    /// Whether the definition itself is secret.
    pub hidden: bool,
    /// Icon reference (URL or plain file name).
    pub icon: Option<String>,
    /// Whether the account unlocked it.
    pub unlocked: bool,
    /// First unlock timestamp reported by the provider, when known.
    pub unlocked_at: Option<DateTime<Utc>>,
    /// Partial completion in `0..=1`; used for provider metadata only.
    pub progress: f64,
    /// Raw provider payload, stored for debugging mapping issues.
    pub raw: Value,
}

impl ProviderAchievement {
    fn new(key: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            name: name.into(),
            description: None,
            points: 0,
            hidden: false,
            icon: None,
            unlocked: false,
            unlocked_at: None,
            progress: 0.0,
            raw: Value::Null,
        }
    }
}

/// Error returned by a provider run.
#[derive(Debug)]
pub struct ProviderError {
    /// Human readable reason, safe to surface in a scan report.
    pub message: String,
}

impl ProviderError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Config and filesystem context handed to every provider.
pub struct ScanContext<'a> {
    /// Install directory reported by the desktop, already allowlist-checked.
    pub install_path: Option<&'a Path>,
    /// Store application id (Steam app id, GOG product id, ...).
    pub app_id: Option<&'a str>,
    /// Shared HTTP client for loopback APIs.
    pub http: &'a reqwest::Client,
    /// Configured loopback API URL patterns.
    pub local_api_urls: &'a [String],
    /// Explicit GOG Galaxy database path, when configured.
    pub galaxy_db: Option<&'a Path>,
    /// Allow API targets that are not loopback (tests/advanced setups).
    pub allow_remote_apis: bool,
}

/// Future returned by [`AchievementProvider::collect`].
pub type ProviderFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<Vec<ProviderAchievement>, ProviderError>>
            + Send
            + 'a,
    >,
>;

/// A local achievement provider.
pub trait AchievementProvider: Send + Sync {
    /// Stable provider id, stored with definitions and unlocks.
    fn id(&self) -> &'static str;

    /// Reads the provider, returning every achievement it can see.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError`] when the source is present but unreadable.
    /// Missing sources yield an empty list so a scan can try the next provider.
    fn collect<'a>(&'a self, context: &'a ScanContext<'a>) -> ProviderFuture<'a>;
}

/// Steam emulation and Goldberg files.
pub struct GameFilesProvider;

impl GameFilesProvider {
    fn definitions(path: &Path) -> (Vec<ProviderAchievement>, Vec<String>) {
        let mut warnings = Vec::new();
        let Ok(raw) = std::fs::read_to_string(path) else {
            return (Vec::new(), warnings);
        };
        let value: Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!(
                    "{}: invalid JSON ({error})",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                return (Vec::new(), warnings);
            }
        };
        let items = match &value {
            Value::Array(items) => items.clone(),
            Value::Object(map) => map
                .get("achievements")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut definitions = Vec::new();
        for item in items {
            let Some(object) = item.as_object() else {
                continue;
            };
            let Some(key) = text(object, &["name", "key", "id", "apiName"]) else {
                continue;
            };
            let mut achievement = ProviderAchievement::new(key.clone(), display_name(object, &key));
            achievement.description = text(object, &["description", "desc", "unlockedDescription"]);
            achievement.hidden = flag(object, &["hidden", "isHidden", "secret"]);
            achievement.points = number(object, &["points", "xp", "score"]);
            achievement.icon = text(object, &["icon", "iconUrl", "icon_url", "image"])
                .and_then(|value| sanitize_icon(&value));
            achievement.raw = item;
            definitions.push(achievement);
        }
        (definitions, warnings)
    }

    /// Reads `[Achievements] key=1` and `[KEY] Achieved=1` style files.
    fn unlocks(path: &Path) -> (BTreeMap<String, Option<DateTime<Utc>>>, Vec<String>) {
        let mut unlocks = BTreeMap::new();
        let mut warnings = Vec::new();
        let Ok(raw) = std::fs::read_to_string(path) else {
            return (unlocks, warnings);
        };

        let mut section = String::new();
        let mut unlocked = false;
        let mut unlocked_at: Option<DateTime<Utc>> = None;

        let flush = |unlocks: &mut BTreeMap<String, Option<DateTime<Utc>>>,
                     section: &str,
                     unlocked: bool,
                     unlocked_at: Option<DateTime<Utc>>| {
            if section.eq_ignore_ascii_case("achievements")
                || section.is_empty()
                || (!unlocked && unlocked_at.is_none())
            {
                return;
            }
            unlocks.insert(section.to_string(), unlocked_at);
        };

        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                flush(&mut unlocks, &section, unlocked, unlocked_at);
                section = name.trim().to_string();
                unlocked = false;
                unlocked_at = None;
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            if section.eq_ignore_ascii_case("achievements") || section.is_empty() {
                if truthy(value) {
                    unlocks.insert(normalize_key(key), None);
                }
            } else if key.eq_ignore_ascii_case("achieved") || key.eq_ignore_ascii_case("unlocked") {
                unlocked = truthy(value);
            } else if key.eq_ignore_ascii_case("unlocktime")
                || key.eq_ignore_ascii_case("unlockedat")
                || key.eq_ignore_ascii_case("date")
            {
                unlocked_at = parse_provider_time(value);
            }
        }
        flush(&mut unlocks, &section, unlocked, unlocked_at);

        if unlocks.is_empty() {
            warnings.push(format!(
                "{}: no unlock entries",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        (unlocks, warnings)
    }
}

impl AchievementProvider for GameFilesProvider {
    fn id(&self) -> &'static str {
        "game_files"
    }

    fn collect<'a>(&'a self, context: &'a ScanContext<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            let Some(root) = context.install_path else {
                return Ok(Vec::new());
            };
            if !root.is_dir() {
                return Ok(Vec::new());
            }

            let mut warnings = Vec::new();
            let mut definitions: BTreeMap<String, ProviderAchievement> = BTreeMap::new();
            for candidate in [
                root.join("steam_settings").join("achievements.json"),
                root.join("steam_settings")
                    .join("achievements")
                    .join("achievements.json"),
            ] {
                if candidate.is_file() {
                    let (found, mut messages) = Self::definitions(&candidate);
                    warnings.append(&mut messages);
                    for achievement in found {
                        definitions.insert(normalize_key(&achievement.key), achievement);
                    }
                }
            }

            let mut unlocks: BTreeMap<String, Option<DateTime<Utc>>> = BTreeMap::new();
            for candidate in [
                root.join("UserStats").join("Stats.ini"),
                root.join("steam_settings").join("achievements.ini"),
                root.join("steam_settings")
                    .join("stats")
                    .join("achievements.ini"),
                root.join("steam_settings")
                    .join("user_stats")
                    .join("achievements.ini"),
                root.join("steam_emu.ini"),
            ] {
                if candidate.is_file() {
                    let (found, mut messages) = Self::unlocks(&candidate);
                    warnings.append(&mut messages);
                    unlocks.extend(found);
                }
            }

            if definitions.is_empty() && unlocks.is_empty() {
                return Ok(Vec::new());
            }

            let mut achievements = Vec::new();
            for (key, definition) in &definitions {
                let mut achievement = definition.clone();
                achievement.key = key.clone();
                if let Some(unlocked_at) = unlocks.get(key) {
                    achievement.unlocked = true;
                    achievement.unlocked_at = *unlocked_at;
                    achievement.progress = 1.0;
                }
                achievements.push(achievement);
            }
            for (key, unlocked_at) in unlocks {
                if definitions.contains_key(&key) {
                    continue;
                }
                let mut achievement = ProviderAchievement::new(key.clone(), humanize_key(&key));
                achievement.unlocked = true;
                achievement.unlocked_at = unlocked_at;
                achievement.progress = 1.0;
                achievement.raw = json!({ "source": "stats.ini", "key": key });
                achievements.push(achievement);
            }

            if !warnings.is_empty() {
                tracing::debug!(warnings = warnings.len(), "achievement file warnings");
            }
            for warning in warnings {
                tracing::debug!(%warning, "achievement provider warning");
            }
            Ok(achievements)
        })
    }
}

/// Loopback JSON provider for launchers that expose achievements over HTTP.
pub struct GalaxyApiProvider;

impl AchievementProvider for GalaxyApiProvider {
    fn id(&self) -> &'static str {
        "gog_galaxy_api"
    }

    fn collect<'a>(&'a self, context: &'a ScanContext<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            if context.local_api_urls.is_empty() {
                return Ok(Vec::new());
            }
            let Some(app_id) = context.app_id else {
                return Ok(Vec::new());
            };
            let mut achievements = Vec::new();
            let mut last_error = None;
            for pattern in context.local_api_urls {
                let url = pattern
                    .replace("{appId}", &urlencode(app_id))
                    .replace("{gameId}", &urlencode(app_id));
                let Some(parsed) = validate_local_url(&url, context.allow_remote_apis) else {
                    last_error = Some(ProviderError::new(format!(
                        "{url}: target is not a loopback address"
                    )));
                    continue;
                };
                let request = context.http.get(parsed).timeout(LOCAL_API_TIMEOUT);
                let response = match request.send().await {
                    Ok(response) => response,
                    Err(error) => {
                        last_error = Some(ProviderError::new(format!("{url}: {error}")));
                        continue;
                    }
                };
                if !response.status().is_success() {
                    last_error = Some(ProviderError::new(format!(
                        "{url}: HTTP {}",
                        response.status().as_u16()
                    )));
                    continue;
                }
                let body = match response.text().await {
                    Ok(body) => body,
                    Err(error) => {
                        last_error = Some(ProviderError::new(format!("{url}: {error}")));
                        continue;
                    }
                };
                let value: Value = match serde_json::from_str(&body) {
                    Ok(value) => value,
                    Err(error) => {
                        last_error =
                            Some(ProviderError::new(format!("{url}: invalid JSON ({error})")));
                        continue;
                    }
                };
                achievements.extend(parse_api_achievements(&value));
            }
            if achievements.is_empty() {
                if let Some(error) = last_error {
                    return Err(error);
                }
            }
            Ok(achievements)
        })
    }
}

/// Reads the GOG Galaxy SQLite database.
pub struct GalaxySqliteProvider;

impl GalaxySqliteProvider {
    fn candidate_paths(context: &ScanContext<'_>) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(path) = context.galaxy_db {
            candidates.push(path.to_path_buf());
        }
        if let Some(root) = context.install_path {
            candidates.push(root.join("galaxy-2.0.db"));
            candidates.push(root.join("Galaxy").join("storage").join("galaxy-2.0.db"));
            if let Some(parent) = root.parent() {
                candidates.push(parent.join("galaxy-2.0.db"));
            }
        }
        candidates
    }

    async fn read_database(path: &Path) -> Result<Vec<ProviderAchievement>, ProviderError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .busy_timeout(Duration::from_secs(2));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|error| ProviderError::new(format!("{}: {error}", path.display())))?;

        let result = Self::read_tables(&pool).await;
        pool.close().await;
        result
    }

    async fn read_tables(pool: &SqlitePool) -> Result<Vec<ProviderAchievement>, ProviderError> {
        let tables = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type = 'table'
             AND lower(name) LIKE '%achievement%' ORDER BY name",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| ProviderError::new(format!("read schema: {error}")))?;

        let mut achievements = Vec::new();
        for table in tables {
            let columns = sqlx::query(&format!("PRAGMA table_info(\"{}\")", quote_ident(&table)))
                .fetch_all(pool)
                .await
                .map_err(|error| ProviderError::new(format!("read {table} columns: {error}")))?;
            let names: Vec<String> = columns
                .iter()
                .filter_map(|row| row.try_get::<String, _>("name").ok())
                .collect();
            let lower: Vec<String> = names.iter().map(|name| name.to_ascii_lowercase()).collect();

            let find = |candidates: &[&str]| -> Option<String> {
                candidates.iter().find_map(|candidate| {
                    lower
                        .iter()
                        .position(|name| name == candidate)
                        .map(|index| names[index].clone())
                })
            };

            let Some(key_column) = find(&[
                "achievement_id",
                "achievementid",
                "api_name",
                "apiname",
                "key",
                "name",
            ]) else {
                continue;
            };
            let name_column = find(&["name", "title", "display_name", "displayname"]);
            let description_column = find(&["description", "desc", "unlocked_description"]);
            let unlocked_column = find(&[
                "unlock_date",
                "unlocked_at",
                "date_unlocked",
                "dateunlocked",
                "unlocktime",
                "unlocked",
                "achieved",
                "is_unlocked",
                "isunlocked",
            ]);
            let points_column = find(&["points", "xp", "score"]);
            let hidden_column = find(&["hidden", "is_hidden", "secret", "visible"]);

            let select = format!(
                "SELECT CAST(\"{key}\" AS TEXT) AS key,
                        CAST(\"{name}\" AS TEXT) AS name,
                        CAST(\"{description}\" AS TEXT) AS description,
                        CAST(\"{unlocked}\" AS TEXT) AS unlocked,
                        CAST(\"{points}\" AS TEXT) AS points,
                        CAST(\"{hidden}\" AS TEXT) AS hidden
                 FROM \"{table}\" LIMIT {GALAXY_ROW_LIMIT}",
                key = quote_ident(&key_column),
                name = quote_ident(name_column.as_deref().unwrap_or(&key_column)),
                description = quote_ident(description_column.as_deref().unwrap_or(&key_column)),
                unlocked = quote_ident(unlocked_column.as_deref().unwrap_or(&key_column)),
                points = quote_ident(points_column.as_deref().unwrap_or(&key_column)),
                hidden = quote_ident(hidden_column.as_deref().unwrap_or(&key_column)),
                table = quote_ident(&table),
            );

            let rows = match sqlx::query(&select).fetch_all(pool).await {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::debug!(%error, table, "achievement table skipped");
                    continue;
                }
            };
            for row in rows {
                let key = row.try_get::<Option<String>, _>("key").ok().flatten();
                let Some(key) = key.filter(|value| !value.trim().is_empty()) else {
                    continue;
                };
                let name = row
                    .try_get::<Option<String>, _>("name")
                    .ok()
                    .flatten()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| humanize_key(&key));
                let unlocked_raw = row.try_get::<Option<String>, _>("unlocked").ok().flatten();
                let unlocked_at = unlocked_raw.as_deref().and_then(parse_provider_time);
                let unlocked = unlocked_raw
                    .as_deref()
                    .is_some_and(|value| truthy(value) || unlocked_at.is_some());
                let mut achievement = ProviderAchievement::new(key, name);
                achievement.description = row
                    .try_get::<Option<String>, _>("description")
                    .ok()
                    .flatten()
                    .filter(|value| !value.trim().is_empty());
                achievement.points = row
                    .try_get::<Option<String>, _>("points")
                    .ok()
                    .flatten()
                    .and_then(|value| value.trim().parse::<i64>().ok())
                    .unwrap_or(0);
                achievement.hidden = row
                    .try_get::<Option<String>, _>("hidden")
                    .ok()
                    .flatten()
                    .is_some_and(|value| {
                        if hidden_column
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case("visible"))
                        {
                            !truthy(&value)
                        } else {
                            truthy(&value)
                        }
                    });
                achievement.unlocked = unlocked;
                achievement.unlocked_at = unlocked_at;
                achievement.progress = if unlocked { 1.0 } else { 0.0 };
                achievement.raw = json!({ "table": table, "key": achievement.key });
                achievements.push(achievement);
            }
        }
        Ok(achievements)
    }
}

impl AchievementProvider for GalaxySqliteProvider {
    fn id(&self) -> &'static str {
        "gog_galaxy_sqlite"
    }

    fn collect<'a>(&'a self, context: &'a ScanContext<'a>) -> ProviderFuture<'a> {
        Box::pin(async move {
            let paths = Self::candidate_paths(context);
            let Some(path) = paths.iter().find(|path| path.is_file()) else {
                return Ok(Vec::new());
            };
            Self::read_database(path).await
        })
    }
}

/// Returns the built-in providers, in scan order.
#[must_use]
pub fn providers() -> Vec<Box<dyn AchievementProvider>> {
    vec![
        Box::new(GameFilesProvider),
        Box::new(GalaxyApiProvider),
        Box::new(GalaxySqliteProvider),
    ]
}

/// Enabled provider ids from configuration, intersected with request filters.
pub fn enabled_providers(config: &crate::config::Config) -> Vec<String> {
    config.achievement_providers.clone()
}

/* --------------------------------- scan ---------------------------------- */

/// Request body of `POST /api/games/{id}/achievements/scan`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    /// Install directory to read. Must live under an allowed scan root.
    #[serde(default)]
    pub install_path: Option<String>,
    /// Store application id used by API providers.
    #[serde(default)]
    pub app_id: Option<String>,
    /// Restrict the scan to these provider ids.
    #[serde(default)]
    pub providers: Option<Vec<String>>,
}

/// Per-provider outcome inside a scan report.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderReport {
    /// Provider id.
    pub provider: String,
    /// `ok`, `skipped` or `error`.
    pub status: String,
    /// Achievements seen.
    pub found: i64,
    /// New unlocks for the scanned account.
    pub unlocked: i64,
    /// Non-fatal message.
    pub message: Option<String>,
}

/// Result of one scan.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Game scanned.
    pub game_id: i64,
    /// Account the unlocks belong to.
    pub user_id: i64,
    /// When the scan ran.
    pub scanned_at: DateTime<Utc>,
    /// Definitions parsed across providers.
    pub found: i64,
    /// New unlocks recorded.
    pub unlocked: i64,
    /// Per-provider outcomes.
    pub providers: Vec<ProviderReport>,
    /// Non-fatal issues (malformed files, unreachable endpoints, ...).
    pub warnings: Vec<String>,
}

/// Checks that `candidate` is an existing directory under one allowed root.
///
/// # Errors
///
/// Returns `400 invalid_install_path` when the path is missing, not a
/// directory, or outside the allowlist.
pub fn validate_scan_path(
    candidate: &str,
    roots: &[PathBuf],
) -> Result<(PathBuf, Option<PathBuf>), ApiError> {
    let raw = PathBuf::from(candidate.trim());
    if candidate.trim().is_empty() {
        return Err(ApiError::bad_request(
            "invalid_install_path",
            "installPath is required",
        ));
    }
    let canonical = std::fs::canonicalize(&raw).map_err(|_| {
        ApiError::bad_request(
            "invalid_install_path",
            "The install path does not exist on the server",
        )
    })?;
    if !canonical.is_dir() {
        return Err(ApiError::bad_request(
            "invalid_install_path",
            "The install path is not a directory",
        ));
    }
    let mut matched = false;
    for root in roots {
        let Ok(root) = std::fs::canonicalize(root) else {
            continue;
        };
        if canonical.starts_with(&root) {
            matched = true;
            break;
        }
    }
    if !matched {
        return Err(ApiError::forbidden(
            "path_not_allowed",
            "The install path is outside LUDEX_ACHIEVEMENT_SCAN_ROOTS",
        ));
    }
    Ok((canonical, Some(raw)))
}

/// Runs every eligible provider for one game and account, then upserts rows.
///
/// # Errors
///
/// Returns 404 `game_not_found` for an unknown game and 400 for malformed
/// provider filters. Provider failures are reported, never fatal.
pub async fn scan(
    state: &AppState,
    user_id: i64,
    game_id: i64,
    request: ScanRequest,
) -> Result<ScanReport, ApiError> {
    let game = db::query_scalar::<i64>("SELECT id FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if game.is_none() {
        return Err(ApiError::not_found("game_not_found", "Game not found"));
    }

    let stored = db::query_as::<(Option<String>, Option<String>, Option<String>)>(
        "SELECT install_path, app_id, providers FROM achievement_sources
         WHERE game_id = ? AND user_id = ?",
    )
    .bind(game_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let install_path = request
        .install_path
        .clone()
        .or_else(|| stored.as_ref().and_then(|row| row.0.clone()));
    let app_id = request
        .app_id
        .clone()
        .or_else(|| stored.as_ref().and_then(|row| row.1.clone()));

    let validated = match install_path.as_deref() {
        Some(path) => Some(validate_scan_path(
            path,
            &state.config.achievement_scan_roots,
        )?),
        None => None,
    };

    let mut selected = request
        .providers
        .clone()
        .unwrap_or_else(|| enabled_providers(&state.config));
    selected.retain(|id| !id.trim().is_empty());
    for id in &selected {
        if !providers().iter().any(|provider| provider.id() == id) {
            return Err(ApiError::bad_request(
                "unknown_provider",
                format!("Unknown achievement provider: {id}"),
            ));
        }
    }

    let context = ScanContext {
        install_path: validated.as_ref().map(|(canonical, _)| canonical.as_path()),
        app_id: app_id.as_deref(),
        http: &state.http,
        local_api_urls: &state.config.achievement_local_api_urls,
        galaxy_db: state.config.achievement_galaxy_db.as_deref(),
        allow_remote_apis: state.config.achievement_allow_remote_apis,
    };

    let scanned_at = db::now();
    let mut report = ScanReport {
        game_id,
        user_id,
        scanned_at,
        found: 0,
        unlocked: 0,
        providers: Vec::new(),
        warnings: Vec::new(),
    };

    for provider in providers() {
        if !selected.iter().any(|id| id == provider.id()) {
            continue;
        }
        match provider.collect(&context).await {
            Ok(found) if found.is_empty() => {
                report.providers.push(ProviderReport {
                    provider: provider.id().to_string(),
                    status: "skipped".to_string(),
                    found: 0,
                    unlocked: 0,
                    message: Some("no achievement source found".to_string()),
                });
            }
            Ok(found) => {
                let mut unlocked = 0;
                for achievement in &found {
                    let (_, new_unlock) =
                        upsert(state, user_id, game_id, provider.id(), achievement).await?;
                    if new_unlock {
                        unlocked += 1;
                        state.events.emit(
                            Event::new(crate::audit::actions::ACHIEVEMENT_UNLOCKED, Actor::System)
                                .target("achievement", format!("{game_id}:{}", achievement.key))
                                .metadata(json!({
                                    "gameId": game_id,
                                    "userId": user_id,
                                    "key": achievement.key,
                                    "name": achievement.name,
                                    "source": provider.id(),
                                })),
                        );
                    }
                }
                report.found += i64::try_from(found.len()).unwrap_or(i64::MAX);
                report.unlocked += unlocked;
                report.providers.push(ProviderReport {
                    provider: provider.id().to_string(),
                    status: "ok".to_string(),
                    found: i64::try_from(found.len()).unwrap_or(i64::MAX),
                    unlocked,
                    message: None,
                });
            }
            Err(error) => {
                report
                    .warnings
                    .push(format!("{}: {}", provider.id(), error.message));
                report.providers.push(ProviderReport {
                    provider: provider.id().to_string(),
                    status: "error".to_string(),
                    found: 0,
                    unlocked: 0,
                    message: Some(error.message),
                });
            }
        }
    }

    let now = db::db_time(scanned_at);
    db::query(
        "INSERT INTO achievement_sources (game_id, user_id, install_path, app_id, providers, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(game_id, user_id) DO UPDATE SET
             install_path = excluded.install_path,
             app_id = excluded.app_id,
             providers = excluded.providers,
             updated_at = excluded.updated_at",
    )
    .bind(game_id)
    .bind(user_id)
    .bind(
        validated
            .as_ref()
            .and_then(|(_, raw)| raw.as_deref())
            .and_then(|raw| raw.to_str()),
    )
    .bind(&app_id)
    .bind(serde_json::to_string(&selected).unwrap_or_else(|_| "[]".to_string()))
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    for provider in &report.providers {
        db::query(
            "INSERT INTO achievement_scans
                (game_id, user_id, provider, status, found, unlocked, error, scanned_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(game_id)
        .bind(user_id)
        .bind(&provider.provider)
        .bind(&provider.status)
        .bind(provider.found)
        .bind(provider.unlocked)
        .bind(&provider.message)
        .bind(&now)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    }

    state.events.emit(
        Event::new(crate::audit::actions::ACHIEVEMENT_SCANNED, Actor::System)
            .target("game", game_id.to_string())
            .metadata(json!({
                "userId": user_id,
                "found": report.found,
                "unlocked": report.unlocked,
                "providers": report.providers.iter().map(|p| p.provider.clone()).collect::<Vec<_>>(),
            })),
    );

    Ok(report)
}

/// Upserts one definition and (when unlocked) one unlock row.
///
/// Returns `(created_definition, new_unlock)`. Both are idempotent: a second
/// call with the same payload never duplicates a row and never moves the
/// first unlock timestamp.
async fn upsert(
    state: &AppState,
    user_id: i64,
    game_id: i64,
    provider: &str,
    achievement: &ProviderAchievement,
) -> Result<(bool, bool), ApiError> {
    let key = normalize_key(&achievement.key);
    if key.is_empty() {
        return Ok((false, false));
    }
    let name = clamp_text(&achievement.name, MAX_NAME_CHARS)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| humanize_key(&key));
    let description = clamp_text(
        achievement.description.as_deref().unwrap_or_default(),
        MAX_DESCRIPTION_CHARS,
    )
    .filter(|value| !value.is_empty());
    let icon = achievement
        .icon
        .as_deref()
        .and_then(sanitize_icon)
        .map(|value| clamp_text(&value, MAX_ICON_CHARS).unwrap_or(value));
    let points = achievement.points.clamp(0, MAX_POINTS);
    let now = db::db_time(db::now());

    let existing =
        db::query_scalar::<i64>("SELECT id FROM achievements WHERE game_id = ? AND key = ?")
            .bind(game_id)
            .bind(&key)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?;

    let achievement_id = match existing {
        Some(id) => {
            db::query(
                "UPDATE achievements SET
                    name = ?, description = ?, icon_path = ?, points = ?, hidden = ?,
                    provider = ?, source_json = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(&name)
            .bind(&description)
            .bind(&icon)
            .bind(points)
            .bind(i64::from(achievement.hidden))
            .bind(provider)
            .bind(achievement.raw.to_string())
            .bind(&now)
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
            id
        }
        None => {
            let id = db::query(
                "INSERT INTO achievements
                    (game_id, key, name, description, icon_path, points, hidden, provider,
                     source_json, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(game_id)
            .bind(&key)
            .bind(&name)
            .bind(&description)
            .bind(&icon)
            .bind(points)
            .bind(i64::from(achievement.hidden))
            .bind(provider)
            .bind(achievement.raw.to_string())
            .bind(&now)
            .bind(&now)
            .returning_id(&state.pool)
            .await
            .map_err(ApiError::from)?;
            id
        }
    };

    let mut new_unlock = false;
    if achievement.unlocked {
        let unlocked_at = achievement
            .unlocked_at
            .map(db::db_time)
            .unwrap_or_else(|| db::db_time(db::now()));
        let result = db::query(
            "INSERT INTO user_achievements
                (user_id, achievement_id, unlocked_at, progress, source, raw_json, created_at)
             VALUES (?, ?, ?, 1.0, ?, ?, ?)
             ON CONFLICT(user_id, achievement_id) DO NOTHING",
        )
        .bind(user_id)
        .bind(achievement_id)
        .bind(&unlocked_at)
        .bind(provider)
        .bind(achievement.raw.to_string())
        .bind(&now)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
        new_unlock = result.rows_affected() > 0;
        if new_unlock && !achievement.hidden {
            crate::community::record(
                state,
                user_id,
                crate::community::kinds::ACHIEVEMENT_UNLOCKED,
                Some(game_id),
                None,
                json!({
                    "achievementKey": key,
                    "achievementName": name,
                    "points": points,
                    "provider": provider,
                }),
            )
            .await;
            crate::social::notify_achievement(
                state,
                user_id,
                game_id,
                achievement_id,
                &key,
                &name,
                points,
            )
            .await;
        }
    }

    Ok((existing.is_none(), new_unlock))
}

/* -------------------------------- queries -------------------------------- */

/// Row of the achievement queries, joined with the caller's unlock state.
#[derive(Debug, sqlx::FromRow)]
struct AchievementRow {
    id: i64,
    game_id: Option<i64>,
    game_title: Option<String>,
    cover_path: Option<String>,
    key: String,
    name: String,
    description: Option<String>,
    icon_path: Option<String>,
    points: i64,
    hidden: i64,
    provider: String,
    unlocked_at: Option<String>,
    progress: Option<f64>,
    unlock_source: Option<String>,
    global_unlocks: i64,
    total_users: i64,
}

/// One achievement as returned by the API.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Achievement {
    /// Row id.
    pub id: i64,
    /// Owning game, `null` for a global achievement.
    pub game_id: Option<i64>,
    /// Owning game title, filled on cross-game lists.
    pub game_title: Option<String>,
    /// Owning game cover, filled on cross-game lists.
    pub cover_url: Option<String>,
    /// Stable key inside the game namespace.
    pub key: String,
    /// Display name (`Hidden achievement` while hidden and locked).
    pub name: String,
    /// Description, hidden while secret.
    pub description: Option<String>,
    /// Icon URL, hidden while secret.
    pub icon_url: Option<String>,
    /// Point value.
    pub points: i64,
    /// Whether the definition is secret.
    pub hidden: bool,
    /// Provider that created the definition (`manual` for admin CRUD).
    pub provider: String,
    /// Whether the requested account unlocked it.
    pub unlocked: bool,
    /// First unlock timestamp.
    pub unlocked_at: Option<DateTime<Utc>>,
    /// Completion fraction, `0..=1`.
    pub progress: f64,
    /// Share of accounts that unlocked it (`0..=100`).
    pub unlock_percentage: f64,
    /// Source of the unlock (`manual`, `game_files`, ...).
    pub source: Option<String>,
}

impl AchievementRow {
    fn to_public(&self) -> Achievement {
        let unlocked = self.unlocked_at.is_some();
        let secret = self.hidden != 0 && !unlocked;
        let percentage = if self.total_users > 0 {
            (self.global_unlocks as f64 / self.total_users as f64) * 100.0
        } else {
            0.0
        };
        Achievement {
            id: self.id,
            game_id: self.game_id,
            game_title: self.game_title.clone(),
            cover_url: self
                .cover_path
                .as_deref()
                .filter(|path| !path.is_empty())
                .map(crate::media::media_url),
            key: self.key.clone(),
            name: if secret {
                "Hidden achievement".to_string()
            } else {
                self.name.clone()
            },
            description: if secret {
                None
            } else {
                self.description.clone()
            },
            icon_url: if secret {
                None
            } else {
                self.icon_path.as_deref().and_then(icon_url)
            },
            points: self.points,
            hidden: self.hidden != 0,
            provider: self.provider.clone(),
            unlocked,
            unlocked_at: self.unlocked_at.as_deref().and_then(db::parse_time),
            progress: if unlocked {
                1.0
            } else {
                self.progress.unwrap_or(0.0).clamp(0.0, 1.0)
            },
            unlock_percentage: (percentage * 10.0).round() / 10.0,
            source: self.unlock_source.clone(),
        }
    }
}

const ACHIEVEMENT_COLUMNS: &str = "a.id, a.game_id, g.title AS game_title, g.cover_path, \
a.key, a.name, a.description, a.icon_path, a.points, a.hidden, a.provider, \
ua.unlocked_at, ua.progress, ua.source AS unlock_source, \
(SELECT COUNT(*) FROM user_achievements x WHERE x.achievement_id = a.id) AS global_unlocks, \
(SELECT COUNT(*) FROM users WHERE activated = 1) AS total_users";

/// Summary of one game's achievement set for one account.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameAchievements {
    /// Game id.
    pub game_id: i64,
    /// Definitions known for the game.
    pub total: i64,
    /// Definitions unlocked by the account.
    pub unlocked: i64,
    /// Points available.
    pub points: i64,
    /// Points earned.
    pub unlocked_points: i64,
    /// Completion percentage (`0..=100`, one decimal).
    pub completion: f64,
    /// Definitions, richest first.
    pub achievements: Vec<Achievement>,
    /// Most recent scan for this account, when one exists.
    pub last_scan: Option<ScanSummary>,
}

/// Last scan recorded for a game/account pair.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    /// Timestamp of the latest run.
    pub scanned_at: DateTime<Utc>,
    /// Providers in that run.
    pub providers: Vec<String>,
    /// Definitions found.
    pub found: i64,
    /// New unlocks.
    pub unlocked: i64,
}

/// Cross-game achievement profile.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserAchievements {
    /// Account the list belongs to.
    pub user: AchievementUser,
    /// Definitions unlocked across the library.
    pub unlocked: i64,
    /// Points earned.
    pub unlocked_points: i64,
    /// Games with at least one unlock.
    pub games: i64,
    /// Unlocked achievements, newest first.
    pub achievements: Vec<Achievement>,
}

/// Minimal public account shape used by achievement responses.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AchievementUser {
    /// Account id.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Whether the community profile is public.
    pub public_profile: bool,
}

/// Loads one game's definitions with the caller's unlock state.
pub async fn for_game(
    pool: &Database,
    viewer_id: i64,
    game_id: i64,
) -> Result<GameAchievements, ApiError> {
    let sql = format!(
        "SELECT {ACHIEVEMENT_COLUMNS}
         FROM achievements a
         JOIN games g ON g.id = a.game_id
         LEFT JOIN user_achievements ua ON ua.achievement_id = a.id AND ua.user_id = ?
         WHERE a.game_id = ?
         ORDER BY a.points DESC, lower(a.name) ASC, a.id ASC
         LIMIT ?"
    );
    let rows = db::query_as::<AchievementRow>(&sql)
        .bind(viewer_id)
        .bind(game_id)
        .bind(i64::try_from(MAX_DEFINITIONS).unwrap_or(i64::MAX))
        .fetch_all(pool)
        .await
        .map_err(ApiError::from)?;

    let achievements: Vec<Achievement> = rows.iter().map(AchievementRow::to_public).collect();
    let total = i64::try_from(achievements.len()).unwrap_or(i64::MAX);
    let unlocked =
        i64::try_from(achievements.iter().filter(|item| item.unlocked).count()).unwrap_or(i64::MAX);
    let points: i64 = achievements.iter().map(|item| item.points).sum();
    let unlocked_points: i64 = achievements
        .iter()
        .filter(|item| item.unlocked)
        .map(|item| item.points)
        .sum();
    let completion = if total > 0 {
        (unlocked as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    let scans = db::query_as::<(String, String, i64, i64)>(
        "SELECT scanned_at, provider, found, unlocked FROM achievement_scans
         WHERE game_id = ? AND user_id = ? ORDER BY id DESC LIMIT 20",
    )
    .bind(game_id)
    .bind(viewer_id)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    let last_scan = scans.first().map(|(scanned_at, _, _, _)| ScanSummary {
        scanned_at: db::parse_time(scanned_at).unwrap_or_else(Utc::now),
        providers: scans
            .iter()
            .filter(|(stamp, _, _, _)| stamp == scanned_at)
            .map(|(_, provider, _, _)| provider.clone())
            .collect(),
        found: scans
            .iter()
            .filter(|(stamp, _, _, _)| stamp == scanned_at)
            .map(|(_, _, found, _)| *found)
            .sum(),
        unlocked: scans
            .iter()
            .filter(|(stamp, _, _, _)| stamp == scanned_at)
            .map(|(_, _, _, unlocked)| *unlocked)
            .sum(),
    });

    Ok(GameAchievements {
        game_id,
        total,
        unlocked,
        points,
        unlocked_points,
        completion: (completion * 10.0).round() / 10.0,
        achievements,
        last_scan,
    })
}

/// Loads the unlocked achievements of one account, newest first.
pub async fn for_user(
    pool: &Database,
    user_id: i64,
) -> Result<(AchievementUser, UserAchievements), ApiError> {
    let user = db::query_as::<(i64, String, Option<String>, i64)>(
        "SELECT id, username, display_name, public_profile FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("profile_not_found", "Profile not found"))?;

    let sql = format!(
        "SELECT {ACHIEVEMENT_COLUMNS}
         FROM user_achievements ua
         JOIN achievements a ON a.id = ua.achievement_id
         LEFT JOIN games g ON g.id = a.game_id
         WHERE ua.user_id = ?
         ORDER BY ua.unlocked_at DESC, ua.id DESC
         LIMIT ?"
    );
    let rows = db::query_as::<AchievementRow>(&sql)
        .bind(user_id)
        .bind(i64::try_from(MAX_DEFINITIONS).unwrap_or(i64::MAX))
        .fetch_all(pool)
        .await
        .map_err(ApiError::from)?;
    let achievements: Vec<Achievement> = rows.iter().map(AchievementRow::to_public).collect();
    let unlocked = i64::try_from(achievements.len()).unwrap_or(i64::MAX);
    let unlocked_points: i64 = achievements.iter().map(|item| item.points).sum();
    let games = achievements
        .iter()
        .filter_map(|item| item.game_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let games = i64::try_from(games).unwrap_or(i64::MAX);

    let public = AchievementUser {
        id: user.0,
        username: user.1,
        display_name: user.2,
        public_profile: user.3 != 0,
    };
    Ok((
        public.clone(),
        UserAchievements {
            user: public,
            unlocked,
            unlocked_points,
            games,
            achievements,
        },
    ))
}

/// Inserts a manual definition. Returns the created row id.
///
/// # Errors
///
/// 400 `invalid_achievement` for bad input, 409 `achievement_exists` on a
/// duplicate key.
pub async fn create_definition(
    pool: &Database,
    game_id: i64,
    request: &DefinitionInput,
) -> Result<i64, ApiError> {
    let key = validate_key(&request.key)?;
    let name = validate_name(&request.name)?;
    let description = clamp_text(
        request.description.as_deref().unwrap_or_default(),
        MAX_DESCRIPTION_CHARS,
    );
    let icon = request
        .icon_path
        .as_deref()
        .and_then(sanitize_icon)
        .map(|value| clamp_text(&value, MAX_ICON_CHARS).unwrap_or(value));
    let now = db::db_time(db::now());
    let created = db::query(
        "INSERT INTO achievements
            (game_id, key, name, description, icon_path, points, hidden, provider,
             source_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'manual', NULL, ?, ?)
         ON CONFLICT(game_id, key) WHERE game_id IS NOT NULL DO NOTHING",
    )
    .bind(game_id)
    .bind(&key)
    .bind(&name)
    .bind(&description)
    .bind(&icon)
    .bind(request.points.unwrap_or(0).clamp(0, MAX_POINTS))
    .bind(i64::from(request.hidden.unwrap_or(false)))
    .bind(&now)
    .bind(&now)
    .returning_id_optional(pool)
    .await
    .map_err(ApiError::from)?;
    created.ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "achievement_exists",
            "An achievement with this key already exists for the game",
        )
    })
}

/// Updates a manual definition.
///
/// # Errors
///
/// 404 `achievement_not_found` when the key is unknown for the game.
pub async fn update_definition(
    pool: &Database,
    game_id: i64,
    key: &str,
    request: &DefinitionPatch,
) -> Result<(), ApiError> {
    let key = normalize_key(key);
    let mut sets = Vec::new();
    let mut binds: Vec<crate::sql::Bind> = Vec::new();
    if let Some(name) = request.name.as_deref() {
        sets.push("name = ?");
        binds.push(crate::sql::Bind::Text(validate_name(name)?));
    }
    if let Some(description) = request.description.as_deref() {
        sets.push("description = ?");
        binds.push(crate::sql::Bind::OptText(
            clamp_text(description, MAX_DESCRIPTION_CHARS).filter(|value| !value.is_empty()),
        ));
    }
    if let Some(icon) = request.icon_path.as_deref() {
        sets.push("icon_path = ?");
        binds.push(crate::sql::Bind::OptText(
            sanitize_icon(icon).and_then(|value| clamp_text(&value, MAX_ICON_CHARS)),
        ));
    }
    if let Some(points) = request.points {
        sets.push("points = ?");
        binds.push(crate::sql::Bind::Int(points.clamp(0, MAX_POINTS)));
    }
    if let Some(hidden) = request.hidden {
        sets.push("hidden = ?");
        binds.push(crate::sql::Bind::Int(i64::from(hidden)));
    }
    if sets.is_empty() {
        return Ok(());
    }
    sets.push("updated_at = ?");
    binds.push(crate::sql::Bind::Text(db::db_time(db::now())));
    binds.push(crate::sql::Bind::Int(game_id));
    binds.push(crate::sql::Bind::Text(key));

    let sql = format!(
        "UPDATE achievements SET {} WHERE game_id = ? AND key = ?",
        sets.join(", ")
    );
    let result = crate::sql::bind_query(db::query(&sql), &binds)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "achievement_not_found",
            "Achievement not found",
        ));
    }
    Ok(())
}

/// Deletes a definition and every unlock pointing at it.
///
/// # Errors
///
/// 404 `achievement_not_found` when the key is unknown for the game.
pub async fn delete_definition(pool: &Database, game_id: i64, key: &str) -> Result<(), ApiError> {
    let result = db::query("DELETE FROM achievements WHERE game_id = ? AND key = ?")
        .bind(game_id)
        .bind(normalize_key(key))
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "achievement_not_found",
            "Achievement not found",
        ));
    }
    Ok(())
}

/// Unlocks one achievement for an account. Idempotent.
///
/// # Errors
///
/// 404 `achievement_not_found` when the key is unknown for the game.
pub async fn unlock(
    state: &AppState,
    user_id: i64,
    game_id: i64,
    key: &str,
    source: &str,
    actor: Actor,
) -> Result<(Achievement, bool), ApiError> {
    let key = normalize_key(key);
    let achievement_id =
        db::query_scalar::<i64>("SELECT id FROM achievements WHERE game_id = ? AND key = ?")
            .bind(game_id)
            .bind(&key)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found("achievement_not_found", "Achievement not found"))?;

    let now = db::now();
    let result = db::query(
        "INSERT INTO user_achievements
            (user_id, achievement_id, unlocked_at, progress, source, raw_json, created_at)
         VALUES (?, ?, ?, 1.0, ?, '{\"source\":\"manual\"}', ?)
         ON CONFLICT(user_id, achievement_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(achievement_id)
    .bind(db::db_time(now))
    .bind(source)
    .bind(db::db_time(now))
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let created = result.rows_affected() > 0;

    if created {
        let (name, hidden, points) = db::query_as::<(String, i64, i64)>(
            "SELECT name, hidden, points FROM achievements WHERE id = ?",
        )
        .bind(achievement_id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
        if hidden == 0 {
            crate::community::record(
                state,
                user_id,
                crate::community::kinds::ACHIEVEMENT_UNLOCKED,
                Some(game_id),
                None,
                json!({
                    "achievementKey": key,
                    "achievementName": name,
                    "provider": source,
                }),
            )
            .await;
            crate::social::notify_achievement(
                state,
                user_id,
                game_id,
                achievement_id,
                &key,
                &name,
                points,
            )
            .await;
        }
        state.events.emit(
            Event::new(crate::audit::actions::ACHIEVEMENT_UNLOCKED, actor)
                .target("achievement", format!("{game_id}:{key}"))
                .metadata(json!({
                    "gameId": game_id,
                    "userId": user_id,
                    "key": key,
                    "name": name,
                    "source": source,
                })),
        );
    }

    let sql = format!(
        "SELECT {ACHIEVEMENT_COLUMNS}
         FROM achievements a
         JOIN games g ON g.id = a.game_id
         LEFT JOIN user_achievements ua ON ua.achievement_id = a.id AND ua.user_id = ?
         WHERE a.id = ?"
    );
    let row = db::query_as::<AchievementRow>(&sql)
        .bind(user_id)
        .bind(achievement_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(ApiError::internal)?;
    Ok((row.to_public(), created))
}

/* --------------------------------- input --------------------------------- */

/// Body of `POST /api/games/{id}/achievements`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionInput {
    /// Stable key.
    pub key: String,
    /// Display name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional icon reference (`/api/media/...`, a file name or a URL).
    #[serde(default)]
    pub icon_path: Option<String>,
    /// Points, defaults to 0.
    #[serde(default)]
    pub points: Option<i64>,
    /// Secret until unlocked.
    #[serde(default)]
    pub hidden: Option<bool>,
}

/// Body of `PATCH /api/games/{id}/achievements/{key}`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionPatch {
    /// New display name.
    #[serde(default)]
    pub name: Option<String>,
    /// New description; empty clears it.
    #[serde(default)]
    pub description: Option<String>,
    /// New icon; empty clears it.
    #[serde(default)]
    pub icon_path: Option<String>,
    /// New point value.
    #[serde(default)]
    pub points: Option<i64>,
    /// New hidden flag.
    #[serde(default)]
    pub hidden: Option<bool>,
}

/// Body of `POST /api/games/{id}/achievements/{key}/unlock`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnlockRequest {
    /// Account to unlock for. Only administrators may target another account.
    #[serde(default)]
    pub user_id: Option<i64>,
    /// Free-form source label, `manual` by default.
    #[serde(default)]
    pub source: Option<String>,
}

/* -------------------------------- helpers -------------------------------- */

fn validate_key(raw: &str) -> Result<String, ApiError> {
    let key = normalize_key(raw);
    if key.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_achievement",
            "An achievement key is required",
        ));
    }
    if key.chars().count() > MAX_KEY_CHARS {
        return Err(ApiError::bad_request(
            "invalid_achievement",
            format!("Achievement keys are limited to {MAX_KEY_CHARS} characters"),
        ));
    }
    Ok(key)
}

fn validate_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::bad_request(
            "invalid_achievement",
            format!("Achievement names are required and limited to {MAX_NAME_CHARS} characters"),
        ));
    }
    Ok(name.to_string())
}

fn normalize_key(raw: &str) -> String {
    let mut key: String = raw
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':') {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while key.contains("__") {
        key = key.replace("__", "_");
    }
    key.trim_matches('_').to_string()
}

fn humanize_key(key: &str) -> String {
    let words: Vec<String> = key
        .split(['_', '-', '.'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        key.to_string()
    } else {
        words.join(" ")
    }
}

fn clamp_text(raw: &str, max: usize) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value.chars().any(char::is_control) {
        return None;
    }
    Some(value.chars().take(max).collect())
}

fn sanitize_icon(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("/api/media/")
    {
        return Some(value.to_string());
    }
    if value.contains('/') || value.contains('\\') || value.contains(':') || value.starts_with('.')
    {
        // Local filesystem paths are never exposed to other accounts.
        return None;
    }
    crate::media::safe_media_name(value).then(|| value.to_string())
}

fn icon_url(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("/api/media/")
    {
        Some(value.to_string())
    } else if crate::media::safe_media_name(value) {
        Some(crate::media::media_url(value))
    } else {
        None
    }
}

fn text(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object.get(*key).and_then(|value| match value {
            Value::String(text) => Some(text.trim().to_string()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
    })
}

fn display_name(object: &Map<String, Value>, fallback: &str) -> String {
    text(
        object,
        &["displayName", "display_name", "title", "name", "key"],
    )
    .unwrap_or_else(|| humanize_key(fallback))
}

fn number(object: &Map<String, Value>, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| {
            object.get(*key).and_then(|value| match value {
                Value::Number(number) => number.as_i64(),
                Value::String(text) => text.trim().parse::<i64>().ok(),
                _ => None,
            })
        })
        .unwrap_or(0)
}

fn flag(object: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|key| {
        object.get(*key).is_some_and(|value| match value {
            Value::Bool(value) => *value,
            Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
            Value::String(text) => truthy(text),
            _ => false,
        })
    })
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "unlocked" | "achieved"
    )
}

fn parse_provider_time(raw: &str) -> Option<DateTime<Utc>> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(parsed) = db::parse_time(value) {
        return Some(parsed);
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        // Unix seconds (Galaxy/Steam emulators) and milliseconds.
        let seconds = if timestamp > 1_000_000_000_000 {
            timestamp / 1_000
        } else {
            timestamp
        };
        return DateTime::from_timestamp(seconds, 0);
    }
    db::parse_date(value)
}

fn urlencode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        let character = byte as char;
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '~') {
            encoded.push(character);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn validate_local_url(raw: &str, allow_remote: bool) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(raw).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    if allow_remote {
        return Some(url);
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let loopback = host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    loopback.then_some(url)
}

fn parse_api_achievements(value: &Value) -> Vec<ProviderAchievement> {
    let items = match value {
        Value::Array(items) => items.clone(),
        Value::Object(map) => ["achievements", "items", "data", "results"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_array).cloned())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut achievements = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(key) = text(
            object,
            &[
                "key",
                "id",
                "achievementId",
                "achievement_id",
                "apiName",
                "name",
            ],
        ) else {
            continue;
        };
        let name = text(object, &["name", "title", "displayName", "display_name"])
            .unwrap_or_else(|| humanize_key(&key));
        let mut achievement = ProviderAchievement::new(key, name);
        achievement.description = text(object, &["description", "desc", "unlockedDescription"]);
        achievement.points = number(object, &["points", "xp", "score"]);
        achievement.hidden = flag(object, &["hidden", "isHidden", "secret"]);
        achievement.icon = text(object, &["icon", "iconUrl", "icon_url", "image"])
            .and_then(|value| sanitize_icon(&value));
        let unlocked_at = text(
            object,
            &[
                "unlockedAt",
                "unlockDate",
                "unlock_date",
                "dateUnlocked",
                "date_unlocked",
                "date",
            ],
        )
        .as_deref()
        .and_then(parse_provider_time);
        let flagged = flag(
            object,
            &["unlocked", "achieved", "isUnlocked", "is_unlocked"],
        );
        achievement.unlocked = flagged || unlocked_at.is_some();
        achievement.unlocked_at = unlocked_at;
        achievement.progress = if achievement.unlocked { 1.0 } else { 0.0 };
        achievement.raw = item;
        achievements.push(achievement);
    }
    achievements
}

fn quote_ident(value: &str) -> String {
    value.replace('"', "\"\"")
}

/* ------------------------------ background ------------------------------- */

/// Interval between automatic rescans; `None` disables the worker.
fn scan_interval(config: &crate::config::Config) -> Option<Duration> {
    (!config.achievement_scan_interval.is_zero()).then_some(config.achievement_scan_interval)
}

/// Rescans every recorded source on a schedule.
pub async fn scheduled_worker(state: AppState, mut cancel: tokio::sync::watch::Receiver<bool>) {
    let Some(interval) = scan_interval(&state.config) else {
        return;
    };
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if state
                    .achievement_busy
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                    )
                    .is_err()
                {
                    continue;
                }
                let result = rescan_sources(&state).await;
                state
                    .achievement_busy
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                match result {
                    Ok(scanned) => tracing::debug!(scanned, "scheduled achievement scan finished"),
                    Err(error) => tracing::warn!(%error, "scheduled achievement scan failed"),
                }
            }
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
            }
        }
    }
}

async fn rescan_sources(state: &AppState) -> Result<usize> {
    let sources = db::query_as::<(i64, i64, Option<String>, Option<String>, Option<String>)>(
        "SELECT game_id, user_id, install_path, app_id, providers FROM achievement_sources",
    )
    .fetch_all(&state.pool)
    .await
    .context("list achievement sources")?;

    let mut scanned = 0;
    for (game_id, user_id, install_path, app_id, providers) in sources {
        let request = ScanRequest {
            install_path,
            app_id,
            providers: providers.and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok()),
        };
        match scan(state, user_id, game_id, request).await {
            Ok(_) => scanned += 1,
            Err(error) => tracing::debug!(
                code = %error.code,
                game_id,
                user_id,
                "scheduled achievement scan skipped"
            ),
        }
    }
    Ok(scanned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("achievements")
            .join(name)
    }

    #[tokio::test]
    async fn steam_fixtures_merge_definitions_and_unlocks() {
        let root = fixture("steam");
        let client = reqwest::Client::new();
        let context = ScanContext {
            install_path: Some(&root),
            app_id: Some("480"),
            http: &client,
            local_api_urls: &[],
            galaxy_db: None,
            allow_remote_apis: false,
        };
        let found = GameFilesProvider.collect(&context).await.expect("scan");
        assert_eq!(found.len(), 3, "two definitions and one unknown key");
        let winner = found
            .iter()
            .find(|item| item.key == "ach_win")
            .expect("win");
        assert!(winner.unlocked);
        assert_eq!(winner.name, "First Win");
        assert_eq!(winner.points, 10);
        let hidden = found
            .iter()
            .find(|item| item.key == "ach_secret")
            .expect("secret");
        assert!(hidden.hidden);
        assert!(!hidden.unlocked);
        let unknown = found
            .iter()
            .find(|item| item.key == "extra_key")
            .expect("extra");
        assert!(unknown.unlocked);
        assert_eq!(unknown.name, "Extra Key");
    }

    #[tokio::test]
    async fn malformed_steam_json_is_ignored() {
        let root = fixture("malformed");
        let client = reqwest::Client::new();
        let context = ScanContext {
            install_path: Some(&root),
            app_id: None,
            http: &client,
            local_api_urls: &[],
            galaxy_db: None,
            allow_remote_apis: false,
        };
        let found = GameFilesProvider.collect(&context).await.expect("scan");
        assert!(found.is_empty());
    }

    #[test]
    fn api_payload_accepts_galaxy_and_generic_shapes() {
        let payload = serde_json::from_str::<Value>(include_str!(
            "../tests/fixtures/achievements/gog-api.json"
        ))
        .expect("json");
        let found = parse_api_achievements(&payload);
        assert_eq!(found.len(), 2);
        let first = found
            .iter()
            .find(|item| item.key == "ACH_FIRST")
            .expect("first");
        assert!(first.unlocked);
        assert!(first.unlocked_at.is_some());
        let second = found
            .iter()
            .find(|item| item.key == "ACH_SECOND")
            .expect("second");
        assert!(!second.unlocked);
    }

    #[test]
    fn keys_are_normalised_and_names_humanised() {
        assert_eq!(normalize_key("  ACH_WIN!! "), "ach_win");
        assert_eq!(normalize_key("ach__win"), "ach_win");
        assert_eq!(humanize_key("ach_win_first"), "Ach Win First");
    }

    #[tokio::test]
    async fn galaxy_fixture_is_read_and_mapped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("galaxy-2.0.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("db");
        let schema = include_str!("../tests/fixtures/achievements/galaxy-schema.sql");
        for statement in schema
            .split(';')
            .map(str::trim)
            .filter(|sql| !sql.is_empty())
        {
            sqlx::query(statement).execute(&pool).await.expect("schema");
        }
        sqlx::query(
            "INSERT INTO Achievements (achievementId, name, description, dateUnlocked, visible, points)
             VALUES ('ACH_FIRST', 'First', 'First achievement', '2026-01-02T03:04:05Z', 1, 5),
                    ('ACH_SECOND', 'Second', NULL, NULL, 0, 10)",
        )
        .execute(&pool)
        .await
        .expect("rows");
        pool.close().await;

        let found = GalaxySqliteProvider::read_database(&path)
            .await
            .expect("scan");
        assert_eq!(found.len(), 2);
        let first = found
            .iter()
            .find(|item| item.key == "ACH_FIRST")
            .expect("first");
        assert!(first.unlocked);
        assert_eq!(first.points, 5);
        let second = found
            .iter()
            .find(|item| item.key == "ACH_SECOND")
            .expect("second");
        assert!(!second.unlocked);
        assert!(second.hidden);
    }

    #[tokio::test]
    async fn scan_path_rejects_escape_and_accepts_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("games");
        let child = root.join("Game");
        std::fs::create_dir_all(&child).expect("child");
        let roots = vec![root.clone()];

        let (_, raw) = validate_scan_path(child.to_str().expect("utf8"), &roots).expect("child");
        assert_eq!(raw.as_deref(), Some(child.as_path()));

        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        let error =
            validate_scan_path(outside.to_str().expect("utf8"), &roots).expect_err("denied");
        assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);

        let error = validate_scan_path("", &roots).expect_err("empty");
        assert_eq!(error.code, "invalid_install_path");
    }

    #[test]
    fn local_api_urls_stay_on_loopback() {
        assert!(validate_local_url("http://localhost:29662/x", false).is_some());
        assert!(validate_local_url("http://127.0.0.1:1/x", false).is_some());
        assert!(validate_local_url("http://10.0.0.1:1/x", false).is_none());
        assert!(validate_local_url("http://10.0.0.1:1/x", true).is_some());
        assert!(validate_local_url("file:///etc/passwd", true).is_none());
    }

    #[test]
    fn sanitize_icon_never_leaks_paths() {
        assert_eq!(sanitize_icon("icon.jpg").as_deref(), Some("icon.jpg"));
        assert!(sanitize_icon("/home/me/icon.jpg").is_none());
        assert!(sanitize_icon("C:\\icons\\x.jpg").is_none());
        assert_eq!(
            sanitize_icon("https://cdn.example/icon.jpg").as_deref(),
            Some("https://cdn.example/icon.jpg")
        );
    }
}
