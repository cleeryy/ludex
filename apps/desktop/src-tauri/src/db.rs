//! Local SQLite index (`{appDataDir}/ludex-desktop.db`).
//!
//! This database is a rebuildable index of what the desktop app installed on
//! this machine, plus local play sessions waiting to be synced to the server.
//! It is not the server database.

use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::linux_gaming::{LaunchMode, LaunchOptions};

/// Mirrors `InstalledGame` from `docs/CONTRACT.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledGame {
    pub game_id: i64,
    pub version_id: i64,
    pub title: String,
    pub version: Option<String>,
    pub install_path: String,
    pub executable: Option<String>,
    pub size_bytes: i64,
    pub installed_at: String,
    pub last_launched_at: Option<String>,
    /// Fingerprint of the archive this install came from (`sha256-sample:…`),
    /// `null` for stream installs and installs made before fingerprinting.
    #[serde(default)]
    pub archive_hash: Option<String>,
    /// Absolute install root used for this game (not exposed to the web UI).
    #[serde(skip)]
    pub install_root: Option<String>,
    /// How the entry entered the local index: `ludex` (installed by this app)
    /// or `steam` (imported from a Steam library, #13).
    #[serde(default = "default_source")]
    pub source: String,
    /// External launch URI for imported entries (`steam://rungameid/…`).
    #[serde(default)]
    pub launch_uri: Option<String>,
}

fn default_source() -> String {
    SOURCE_LUDEX.to_string()
}

/// A game installed by the Ludex installer.
pub const SOURCE_LUDEX: &str = "ludex";
/// A Steam game imported into the local index (#13).
pub const SOURCE_STEAM: &str = "steam";

/// One recorded play session, as shown in the game page and the tray.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: String,
    pub game_id: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub minutes: i64,
    pub seconds: i64,
}

/// Aggregated playtime for one game.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamePlaytime {
    pub game_id: i64,
    pub total_seconds: i64,
    pub sessions: i64,
    pub first_played_at: Option<String>,
    pub last_played_at: Option<String>,
    /// Total per UTC day (`YYYY-MM-DD`), newest first, capped at 30 days.
    pub by_day: Vec<DayPlaytime>,
    /// Most recent sessions, newest first.
    pub recent_sessions: Vec<SessionRecord>,
}

/// Playtime for one UTC day.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayPlaytime {
    pub day: String,
    pub seconds: i64,
}

#[derive(Debug, Clone)]
pub struct PendingSession {
    pub id: String,
    pub game_id: i64,
    pub minutes: i64,
}

/// Projected rating cached by `protondb_rating` (#91).
#[derive(Debug, Clone, PartialEq)]
pub struct CachedRating {
    pub app_id: u32,
    pub tier: String,
    pub best_tier: Option<String>,
    pub trending_tier: Option<String>,
    pub confidence: Option<String>,
    pub total: i64,
    pub score: Option<f64>,
    pub fetched_at: String,
}

const SCHEMA_VERSION: i64 = 8;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS installed_games (
    game_id          INTEGER PRIMARY KEY,
    version_id       INTEGER NOT NULL,
    title            TEXT    NOT NULL,
    version          TEXT,
    install_path     TEXT    NOT NULL,
    executable       TEXT,
    size_bytes       INTEGER NOT NULL DEFAULT 0,
    installed_at     TEXT    NOT NULL,
    last_launched_at TEXT,
    install_root     TEXT
);

CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT    PRIMARY KEY,
    game_id    INTEGER NOT NULL,
    started_at TEXT    NOT NULL,
    ended_at   TEXT,
    minutes    INTEGER NOT NULL DEFAULT 0,
    synced     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS sessions_game_idx ON sessions (game_id);
CREATE INDEX IF NOT EXISTS sessions_pending_idx ON sessions (synced, minutes);
"#;

/// Persisted install jobs. The in-memory registry is authoritative while the
/// app runs; this table makes interrupted jobs visible after a restart and
/// keeps the `.part` path so a retry resumes with HTTP Range.
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS install_jobs (
    job_id       TEXT    PRIMARY KEY,
    game_id      INTEGER NOT NULL,
    version_id   INTEGER NOT NULL,
    phase        TEXT    NOT NULL,
    bytes_done   INTEGER NOT NULL DEFAULT 0,
    bytes_total  INTEGER,
    speed_bps    INTEGER NOT NULL DEFAULT 0,
    part_path    TEXT,
    install_path TEXT,
    error        TEXT,
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS install_jobs_game_idx ON install_jobs (game_id, version_id);
CREATE INDEX IF NOT EXISTS install_jobs_updated_idx ON install_jobs (updated_at);
"#;

/// File manifests for installed games (issue #37). `manifest` is the JSON
/// encoding of `manifest::FileManifest`; keeping it in the index means it never
/// shows up as an extra file inside the install folder and moving an install
/// only rewrites one row.
const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS install_manifests (
    game_id      INTEGER PRIMARY KEY,
    version_id   INTEGER NOT NULL,
    install_path TEXT    NOT NULL,
    manifest     TEXT    NOT NULL,
    files        INTEGER NOT NULL DEFAULT 0,
    bytes        INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL
);
"#;

/// Playtime tracking (#26) and install fingerprints (#30).
const SCHEMA_V4: &str = r#"
ALTER TABLE installed_games ADD COLUMN archive_hash TEXT;
ALTER TABLE sessions ADD COLUMN seconds INTEGER NOT NULL DEFAULT 0;
-- Existing sessions only had rounded minutes: keep their totals meaningful.
UPDATE sessions SET seconds = minutes * 60 WHERE seconds = 0 AND minutes > 0;
CREATE INDEX IF NOT EXISTS sessions_started_idx ON sessions (game_id, started_at DESC);
"#;

/// External library imports (#13): entries that live outside `installDir` and
/// launch through another client (`steam://rungameid/…`). A partial unique
/// index makes a second import of the same URI impossible.
const SCHEMA_V5: &str = r#"
ALTER TABLE installed_games ADD COLUMN source TEXT NOT NULL DEFAULT 'ludex';
ALTER TABLE installed_games ADD COLUMN launch_uri TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS installed_games_launch_uri_idx
    ON installed_games (launch_uri) WHERE launch_uri IS NOT NULL;
"#;

/// Issue #114: remember that a job used the stream-install path so the UI can
/// offer "Retry as download" without parsing an English error message.
const SCHEMA_V6: &str = r#"
ALTER TABLE install_jobs ADD COLUMN stream INTEGER NOT NULL DEFAULT 0;
"#;

/// Linux compatibility (#89, #91, #92): per-game launch options and the
/// ProtonDB cache. Both are local-only; nothing here is synced to the server.
const SCHEMA_V7: &str = r#"
CREATE TABLE IF NOT EXISTS game_launch_options (
    game_id          INTEGER PRIMARY KEY,
    mode             TEXT    NOT NULL DEFAULT 'auto',
    tool_id          TEXT    NOT NULL DEFAULT '',
    prefix_path      TEXT    NOT NULL DEFAULT '',
    env              TEXT    NOT NULL DEFAULT '{}',
    winetricks_verbs TEXT    NOT NULL DEFAULT '[]',
    steam_app_id     INTEGER,
    updated_at       TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS protondb_reports (
    app_id        INTEGER PRIMARY KEY,
    tier          TEXT    NOT NULL,
    best_tier     TEXT,
    trending_tier TEXT,
    confidence    TEXT,
    total         INTEGER NOT NULL DEFAULT 0,
    score         REAL,
    fetched_at    TEXT    NOT NULL
);
"#;

/// Save folder detection (#107). `save_paths` is the resolved mapping shown in
/// the UI (manual entries survive re-detection); `save_detection` keeps the last
/// preview, including sizes and notes, so `get_save_paths` never rescans the
/// disk. Host paths never leave this machine.
const SCHEMA_V8: &str = r#"
CREATE TABLE IF NOT EXISTS save_paths (
    game_id    INTEGER NOT NULL,
    path       TEXT    NOT NULL,
    kind       TEXT    NOT NULL,
    source     TEXT    NOT NULL,
    "primary"  INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    PRIMARY KEY (game_id, path)
);

CREATE UNIQUE INDEX IF NOT EXISTS save_paths_primary_idx
    ON save_paths (game_id) WHERE "primary" = 1;

CREATE TABLE IF NOT EXISTS save_detection (
    game_id          INTEGER PRIMARY KEY,
    version_id       INTEGER NOT NULL DEFAULT 0,
    manifest_version TEXT,
    detected_at      TEXT    NOT NULL,
    result_json      TEXT    NOT NULL
);
"#;

pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Could not create the desktop data folder {}: {e}",
                parent.display()
            )
        })?;
    }

    let connection = Connection::open(path).map_err(|e| {
        format!(
            "Could not open the desktop database {}: {e}",
            path.display()
        )
    })?;
    connection
        .busy_timeout(Duration::from_secs(10))
        .map_err(|e| format!("Could not configure the desktop database: {e}"))?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )
        .map_err(|e| format!("Could not initialise the desktop database: {e}"))?;
    migrate(&connection)?;
    Ok(connection)
}

fn migrate(connection: &Connection) -> Result<(), String> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| format!("Could not read the desktop database version: {e}"))?;

    if version < 1 {
        connection
            .execute_batch(SCHEMA_V1)
            .map_err(|e| format!("Could not create the desktop database schema: {e}"))?;
    }
    if version < 2 {
        connection
            .execute_batch(SCHEMA_V2)
            .map_err(|e| format!("Could not create the install job table: {e}"))?;
    }
    if version < 3 {
        connection
            .execute_batch(SCHEMA_V3)
            .map_err(|e| format!("Could not create the install manifest table: {e}"))?;
    }
    if version < 4 {
        connection
            .execute_batch(SCHEMA_V4)
            .map_err(|e| format!("Could not upgrade the desktop database for playtime: {e}"))?;
    }
    if version < 5 {
        connection
            .execute_batch(SCHEMA_V5)
            .map_err(|e| format!("Could not add external imports to the desktop database: {e}"))?;
    }
    if version < 6 {
        connection
            .execute_batch(SCHEMA_V6)
            .map_err(|e| format!("Could not add stream flags to the desktop database: {e}"))?;
    }
    if version < 7 {
        connection
            .execute_batch(SCHEMA_V7)
            .map_err(|e| format!("Could not add Linux compatibility tables: {e}"))?;
    }
    if version < 8 {
        connection
            .execute_batch(SCHEMA_V8)
            .map_err(|e| format!("Could not add save detection to the desktop database: {e}"))?;
    }

    if version < SCHEMA_VERSION {
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| format!("Could not record the desktop database version: {e}"))?;
    }

    // Future migrations: bump SCHEMA_VERSION and add `if version < N` blocks.
    Ok(())
}

pub fn upsert_installed(connection: &Connection, game: &InstalledGame) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO installed_games
                (game_id, version_id, title, version, install_path, executable,
                 size_bytes, installed_at, last_launched_at, archive_hash, install_root,
                 source, launch_uri)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(game_id) DO UPDATE SET
                version_id       = excluded.version_id,
                title            = excluded.title,
                version          = excluded.version,
                install_path     = excluded.install_path,
                executable       = excluded.executable,
                size_bytes       = excluded.size_bytes,
                installed_at     = excluded.installed_at,
                last_launched_at = excluded.last_launched_at,
                archive_hash     = excluded.archive_hash,
                install_root     = excluded.install_root,
                source           = excluded.source,
                launch_uri       = excluded.launch_uri
            "#,
            params![
                game.game_id,
                game.version_id,
                game.title,
                game.version,
                game.install_path,
                game.executable,
                game.size_bytes,
                game.installed_at,
                game.last_launched_at,
                game.archive_hash,
                game.install_root,
                game.source,
                game.launch_uri,
            ],
        )
        .map_err(|e| format!("Could not record the installed game: {e}"))?;
    Ok(())
}

fn map_installed(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstalledGame> {
    Ok(InstalledGame {
        game_id: row.get(0)?,
        version_id: row.get(1)?,
        title: row.get(2)?,
        version: row.get(3)?,
        install_path: row.get(4)?,
        executable: row.get(5)?,
        size_bytes: row.get(6)?,
        installed_at: row.get(7)?,
        last_launched_at: row.get(8)?,
        archive_hash: row.get(9)?,
        install_root: row.get(10)?,
        source: row.get(11)?,
        launch_uri: row.get(12)?,
    })
}

pub fn list_installed(connection: &Connection) -> Result<Vec<InstalledGame>, String> {
    let mut statement = connection
        .prepare(
            r#"
            SELECT game_id, version_id, title, version, install_path, executable,
                   size_bytes, installed_at, last_launched_at, archive_hash, install_root,
                   source, launch_uri
            FROM installed_games
            ORDER BY title COLLATE NOCASE ASC
            "#,
        )
        .map_err(|e| format!("Could not read installed games: {e}"))?;

    let rows = statement
        .query_map([], map_installed)
        .map_err(|e| format!("Could not read installed games: {e}"))?;

    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("Could not read installed games: {e}"))?);
    }
    Ok(games)
}

pub fn get_installed(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<InstalledGame>, String> {
    connection
        .query_row(
            r#"
            SELECT game_id, version_id, title, version, install_path, executable,
                   size_bytes, installed_at, last_launched_at, archive_hash, install_root,
                   source, launch_uri
            FROM installed_games
            WHERE game_id = ?1
            "#,
            params![game_id],
            map_installed,
        )
        .optional()
        .map_err(|e| format!("Could not read the installed game: {e}"))
}

pub fn remove_installed(connection: &Connection, game_id: i64) -> Result<bool, String> {
    connection
        .execute(
            "DELETE FROM game_launch_options WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not remove the launch options: {e}"))?;
    let affected = connection
        .execute(
            "DELETE FROM installed_games WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not remove the installed game: {e}"))?;
    Ok(affected > 0)
}

/// Record the executable chosen after an interactive or silent installer run
/// (#7), or from the exe picker.
pub fn set_executable(
    connection: &Connection,
    game_id: i64,
    executable: Option<&str>,
) -> Result<(), String> {
    let affected = connection
        .execute(
            "UPDATE installed_games SET executable = ?2 WHERE game_id = ?1",
            params![game_id, executable],
        )
        .map_err(|e| format!("Could not record the executable: {e}"))?;
    if affected == 0 {
        return Err(format!("Game {game_id} is not installed."));
    }
    Ok(())
}

/* ------------- per-game launch options (#89, #91, #92) ------------------ */

fn parse_mode(raw: &str) -> LaunchMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "native" => LaunchMode::Native,
        "proton" => LaunchMode::Proton,
        "umu" => LaunchMode::Umu,
        _ => LaunchMode::Auto,
    }
}

/// True when the game has saved launch options (used to apply the global
/// default mode only to games the user never configured).
pub fn has_launch_options(connection: &Connection, game_id: i64) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM game_launch_options WHERE game_id = ?1)",
            params![game_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .map_err(|e| format!("Could not read the launch options: {e}"))
}

/// Launch options for a game; defaults when nothing was saved yet.
pub fn get_launch_options(connection: &Connection, game_id: i64) -> Result<LaunchOptions, String> {
    let row = connection
        .query_row(
            "SELECT mode, tool_id, prefix_path, env, winetricks_verbs, steam_app_id
             FROM game_launch_options WHERE game_id = ?1",
            params![game_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the launch options: {e}"))?;

    let Some((mode, tool_id, prefix_path, env, verbs, app_id)) = row else {
        return Ok(LaunchOptions::default());
    };

    // Corrupt JSON must not make a game unlaunchable.
    let env = serde_json::from_str(&env).unwrap_or_default();
    let winetricks_verbs = serde_json::from_str(&verbs).unwrap_or_default();

    Ok(LaunchOptions {
        mode: parse_mode(&mode),
        tool_id,
        prefix_path,
        env,
        winetricks_verbs,
        steam_app_id: app_id.and_then(|value| u32::try_from(value).ok()),
    })
}

pub fn save_launch_options(
    connection: &Connection,
    game_id: i64,
    options: &LaunchOptions,
    updated_at: &str,
) -> Result<(), String> {
    let env = serde_json::to_string(&options.env)
        .map_err(|e| format!("Could not encode the environment overrides: {e}"))?;
    let verbs = serde_json::to_string(&options.winetricks_verbs)
        .map_err(|e| format!("Could not encode the winetricks verbs: {e}"))?;

    connection
        .execute(
            r#"
            INSERT INTO game_launch_options
                (game_id, mode, tool_id, prefix_path, env, winetricks_verbs, steam_app_id, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(game_id) DO UPDATE SET
                mode             = excluded.mode,
                tool_id          = excluded.tool_id,
                prefix_path      = excluded.prefix_path,
                env              = excluded.env,
                winetricks_verbs = excluded.winetricks_verbs,
                steam_app_id     = excluded.steam_app_id,
                updated_at       = excluded.updated_at
            "#,
            params![
                game_id,
                options.mode.as_str(),
                options.tool_id,
                options.prefix_path,
                env,
                verbs,
                options.steam_app_id.map(i64::from),
                updated_at,
            ],
        )
        .map_err(|e| format!("Could not save the launch options: {e}"))?;
    Ok(())
}

pub fn cached_rating(connection: &Connection, app_id: u32) -> Result<Option<CachedRating>, String> {
    connection
        .query_row(
            "SELECT app_id, tier, best_tier, trending_tier, confidence, total, score, fetched_at
             FROM protondb_reports WHERE app_id = ?1",
            params![i64::from(app_id)],
            |row| {
                Ok(CachedRating {
                    app_id: u32::try_from(row.get::<_, i64>(0)?).unwrap_or(app_id),
                    tier: row.get(1)?,
                    best_tier: row.get(2)?,
                    trending_tier: row.get(3)?,
                    confidence: row.get(4)?,
                    total: row.get(5)?,
                    score: row.get(6)?,
                    fetched_at: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the ProtonDB cache: {e}"))
}

pub fn store_rating(connection: &Connection, rating: &CachedRating) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO protondb_reports
                (app_id, tier, best_tier, trending_tier, confidence, total, score, fetched_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(app_id) DO UPDATE SET
                tier          = excluded.tier,
                best_tier     = excluded.best_tier,
                trending_tier = excluded.trending_tier,
                confidence    = excluded.confidence,
                total         = excluded.total,
                score         = excluded.score,
                fetched_at    = excluded.fetched_at
            "#,
            params![
                i64::from(rating.app_id),
                rating.tier,
                rating.best_tier,
                rating.trending_tier,
                rating.confidence,
                rating.total,
                rating.score,
                rating.fetched_at,
            ],
        )
        .map_err(|e| format!("Could not cache the ProtonDB rating: {e}"))?;
    Ok(())
}

/// Every entry imported from one source (`steam` today).
pub fn list_installed_by_source(
    connection: &Connection,
    source: &str,
) -> Result<Vec<InstalledGame>, String> {
    let mut statement = connection
        .prepare(
            r#"
            SELECT game_id, version_id, title, version, install_path, executable,
                   size_bytes, installed_at, last_launched_at, archive_hash, install_root,
                   source, launch_uri
            FROM installed_games
            WHERE source = ?1
            ORDER BY title COLLATE NOCASE ASC
            "#,
        )
        .map_err(|e| format!("Could not read imported games: {e}"))?;

    let rows = statement
        .query_map(params![source], map_installed)
        .map_err(|e| format!("Could not read imported games: {e}"))?;

    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("Could not read imported games: {e}"))?);
    }
    Ok(games)
}

pub fn touch_launched(
    connection: &Connection,
    game_id: i64,
    timestamp: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE installed_games SET last_launched_at = ?2 WHERE game_id = ?1",
            params![game_id, timestamp],
        )
        .map_err(|e| format!("Could not update the launch time: {e}"))?;
    Ok(())
}

pub fn insert_session(
    connection: &Connection,
    id: &str,
    game_id: i64,
    started_at: &str,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO sessions (id, game_id, started_at, minutes, synced) VALUES (?1, ?2, ?3, 0, 0)",
            params![id, game_id, started_at],
        )
        .map_err(|e| format!("Could not record the play session: {e}"))?;
    Ok(())
}

pub fn finish_session(
    connection: &Connection,
    id: &str,
    ended_at: &str,
    minutes: i64,
    seconds: i64,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE sessions SET ended_at = ?2, minutes = ?3, seconds = ?4 WHERE id = ?1",
            params![id, ended_at, minutes.max(0), seconds.max(0)],
        )
        .map_err(|e| format!("Could not record the play session: {e}"))?;
    Ok(())
}

/// Installed games ordered by the most recent activity (last launch, else the
/// import/install date), for the tray menu and Home. Imported Steam games have
/// never been launched through Ludex and must still be listed.
pub fn recent_installed(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<InstalledGame>, String> {
    let mut statement = connection
        .prepare(
            r#"
            SELECT game_id, version_id, title, version, install_path, executable,
                   size_bytes, installed_at, last_launched_at, archive_hash, install_root,
                   source, launch_uri
            FROM installed_games
            ORDER BY COALESCE(last_launched_at, installed_at) DESC, title COLLATE NOCASE ASC
            LIMIT ?1
            "#,
        )
        .map_err(|e| format!("Could not read recent games: {e}"))?;

    let rows = statement
        .query_map(params![limit as i64], map_installed)
        .map_err(|e| format!("Could not read recent games: {e}"))?;

    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("Could not read recent games: {e}"))?);
    }
    Ok(games)
}

/// Aggregated playtime for one game: totals, per-day buckets and the last
/// sessions. `days` caps the per-day buckets (1..=365) and `sessions` caps the
/// recent session list.
pub fn game_playtime(
    connection: &Connection,
    game_id: i64,
    days: i64,
    sessions: i64,
) -> Result<GamePlaytime, String> {
    let (total_seconds, session_count, first_played_at, last_played_at) = connection
        .query_row(
            "SELECT COALESCE(SUM(seconds), 0), COUNT(*), MIN(started_at), MAX(started_at)
             FROM sessions WHERE game_id = ?1",
            params![game_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .map_err(|e| format!("Could not read playtime: {e}"))?;

    let mut statement = connection
        .prepare(
            "SELECT substr(started_at, 1, 10) AS day, COALESCE(SUM(seconds), 0) AS seconds
             FROM sessions
             WHERE game_id = ?1 AND started_at >= date('now', ?2)
             GROUP BY day ORDER BY day DESC",
        )
        .map_err(|e| format!("Could not read daily playtime: {e}"))?;
    let window = format!("-{} days", days.clamp(1, 365));
    let rows = statement
        .query_map(params![game_id, window], |row| {
            Ok(DayPlaytime {
                day: row.get(0)?,
                seconds: row.get(1)?,
            })
        })
        .map_err(|e| format!("Could not read daily playtime: {e}"))?;
    let mut by_day = Vec::new();
    for row in rows {
        by_day.push(row.map_err(|e| format!("Could not read daily playtime: {e}"))?);
    }

    Ok(GamePlaytime {
        game_id,
        total_seconds,
        sessions: session_count,
        first_played_at,
        last_played_at,
        by_day,
        recent_sessions: recent_sessions(connection, game_id, sessions)?,
    })
}

/// Last `limit` sessions for a game, newest first.
pub fn recent_sessions(
    connection: &Connection,
    game_id: i64,
    limit: i64,
) -> Result<Vec<SessionRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, game_id, started_at, ended_at, minutes, seconds
             FROM sessions WHERE game_id = ?1
             ORDER BY started_at DESC, id DESC LIMIT ?2",
        )
        .map_err(|e| format!("Could not read play sessions: {e}"))?;
    let rows = statement
        .query_map(params![game_id, limit.clamp(1, 200)], |row| {
            Ok(SessionRecord {
                id: row.get(0)?,
                game_id: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
                minutes: row.get(4)?,
                seconds: row.get(5)?,
            })
        })
        .map_err(|e| format!("Could not read play sessions: {e}"))?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|e| format!("Could not read play sessions: {e}"))?);
    }
    Ok(records)
}

pub fn pending_sessions(connection: &Connection) -> Result<Vec<PendingSession>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, game_id, minutes FROM sessions WHERE synced = 0 AND minutes > 0 ORDER BY started_at ASC LIMIT 200",
        )
        .map_err(|e| format!("Could not read pending playtime: {e}"))?;

    let rows = statement
        .query_map([], |row| {
            Ok(PendingSession {
                id: row.get(0)?,
                game_id: row.get(1)?,
                minutes: row.get(2)?,
            })
        })
        .map_err(|e| format!("Could not read pending playtime: {e}"))?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|e| format!("Could not read pending playtime: {e}"))?);
    }
    Ok(sessions)
}

pub fn mark_session_synced(connection: &Connection, id: &str) -> Result<(), String> {
    connection
        .execute("UPDATE sessions SET synced = 1 WHERE id = ?1", params![id])
        .map_err(|e| format!("Could not mark playtime as synced: {e}"))?;
    Ok(())
}

/// Row shape of `install_jobs`.
#[derive(Debug, Clone)]
pub struct InstallJobRecord {
    pub job_id: String,
    pub game_id: i64,
    pub version_id: i64,
    pub phase: String,
    pub bytes_done: i64,
    pub bytes_total: Option<i64>,
    pub speed_bps: i64,
    pub part_path: Option<String>,
    pub install_path: Option<String>,
    pub error: Option<String>,
    /// True when the job used the non-resumable tar stream (#114).
    pub stream: bool,
    pub created_at: String,
    pub updated_at: String,
}

pub fn upsert_install_job(connection: &Connection, job: &InstallJobRecord) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_jobs
                (job_id, game_id, version_id, phase, bytes_done, bytes_total,
                 speed_bps, part_path, install_path, error, stream, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(job_id) DO UPDATE SET
                phase        = excluded.phase,
                bytes_done   = excluded.bytes_done,
                bytes_total  = excluded.bytes_total,
                speed_bps    = excluded.speed_bps,
                part_path    = excluded.part_path,
                install_path = excluded.install_path,
                error        = excluded.error,
                stream       = excluded.stream,
                updated_at   = excluded.updated_at
            "#,
            params![
                job.job_id,
                job.game_id,
                job.version_id,
                job.phase,
                job.bytes_done,
                job.bytes_total,
                job.speed_bps,
                job.part_path,
                job.install_path,
                job.error,
                i64::from(job.stream),
                job.created_at,
                job.updated_at,
            ],
        )
        .map_err(|e| format!("Could not record the install job: {e}"))?;
    Ok(())
}

fn map_install_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstallJobRecord> {
    Ok(InstallJobRecord {
        job_id: row.get(0)?,
        game_id: row.get(1)?,
        version_id: row.get(2)?,
        phase: row.get(3)?,
        bytes_done: row.get(4)?,
        bytes_total: row.get(5)?,
        speed_bps: row.get(6)?,
        part_path: row.get(7)?,
        install_path: row.get(8)?,
        error: row.get(9)?,
        stream: row.get::<_, i64>(10)? != 0,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

const INSTALL_JOB_COLUMNS: &str = "job_id, game_id, version_id, phase, bytes_done, bytes_total, speed_bps, part_path, install_path, error, stream, created_at, updated_at";

pub fn get_install_job(
    connection: &Connection,
    job_id: &str,
) -> Result<Option<InstallJobRecord>, String> {
    connection
        .query_row(
            &format!("SELECT {INSTALL_JOB_COLUMNS} FROM install_jobs WHERE job_id = ?1"),
            params![job_id],
            map_install_job,
        )
        .optional()
        .map_err(|e| format!("Could not read the install job: {e}"))
}

/// Most recent persisted job for a game version, used to resume a `.part`
/// file even when the download folder changed between attempts.
pub fn latest_install_job_for_version(
    connection: &Connection,
    game_id: i64,
    version_id: i64,
) -> Result<Option<InstallJobRecord>, String> {
    connection
        .query_row(
            &format!(
                "SELECT {INSTALL_JOB_COLUMNS} FROM install_jobs WHERE game_id = ?1 AND version_id = ?2 ORDER BY updated_at DESC LIMIT 1"
            ),
            params![game_id, version_id],
            map_install_job,
        )
        .optional()
        .map_err(|e| format!("Could not read the install job: {e}"))
}

/// Every persisted job, newest first. Used by `list_install_jobs` so the UI
/// can show active and historical jobs in one list.
pub fn all_install_jobs(connection: &Connection) -> Result<Vec<InstallJobRecord>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {INSTALL_JOB_COLUMNS} FROM install_jobs ORDER BY created_at DESC, job_id DESC"
        ))
        .map_err(|e| format!("Could not read install jobs: {e}"))?;

    let rows = statement
        .query_map([], map_install_job)
        .map_err(|e| format!("Could not read install jobs: {e}"))?;

    let mut jobs = Vec::new();
    for row in rows {
        jobs.push(row.map_err(|e| format!("Could not read install jobs: {e}"))?);
    }
    Ok(jobs)
}

/// Phases whose history row may be deleted (#100). Active and paused jobs are
/// still owned by the installer (queue slot, `.part` offset), so they are never
/// removed from here: the command refuses them first and returns an error.
const TERMINAL_INSTALL_PHASES: &str = "'done', 'cancelled', 'error', 'interrupted'";

/// Remove one terminal row from Downloads history (#100). Returns whether a row
/// was deleted; `Ok(false)` means the job is unknown or still active/paused.
/// Only the index row goes: install folders, archives and `.part` files stay.
pub fn delete_install_job(connection: &Connection, job_id: &str) -> Result<bool, String> {
    let affected = connection
        .execute(
            &format!(
                "DELETE FROM install_jobs WHERE job_id = ?1 AND phase IN ({TERMINAL_INSTALL_PHASES})"
            ),
            params![job_id],
        )
        .map_err(|e| format!("Could not remove the install job: {e}"))?;
    Ok(affected > 0)
}

/// Remove every terminal row from Downloads history (#100). Returns the number
/// of rows deleted; install folders, archives and `.part` files stay.
pub fn clear_finished_install_jobs(connection: &Connection) -> Result<usize, String> {
    connection
        .execute(
            &format!("DELETE FROM install_jobs WHERE phase IN ({TERMINAL_INSTALL_PHASES})"),
            [],
        )
        .map_err(|e| format!("Could not clear finished install jobs: {e}"))
}

/// Mark older non-terminal jobs for the same version as superseded when a new
/// attempt starts (for example resuming a paused job after a restart). Finished
/// and failed rows are kept as history.
pub fn supersede_install_jobs(
    connection: &Connection,
    game_id: i64,
    version_id: i64,
    now: &str,
) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE install_jobs SET phase = 'cancelled', error = 'Superseded by a new install', updated_at = ?3, speed_bps = 0
             WHERE game_id = ?1 AND version_id = ?2
               AND phase IN ('queued', 'downloading', 'extracting', 'finalizing', 'paused')",
            params![game_id, version_id, now],
        )
        .map_err(|e| format!("Could not supersede the previous install jobs: {e}"))
}

/// Called once at startup: any job left in an active phase was interrupted by
/// an app exit or crash. Keep the row (and its `.part` path) so it stays
/// visible and the next `install_game` resumes the download.
pub fn mark_interrupted_jobs(
    connection: &Connection,
    message: &str,
    now: &str,
) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE install_jobs SET phase = 'error', error = ?1, updated_at = ?2, speed_bps = 0 WHERE phase IN ('queued', 'downloading', 'extracting', 'finalizing')",
            params![message, now],
        )
        .map_err(|e| format!("Could not mark interrupted install jobs: {e}"))
}

/// Row shape of `install_manifests`.
#[derive(Debug, Clone)]
pub struct InstallManifestRecord {
    pub game_id: i64,
    pub version_id: i64,
    pub install_path: String,
    pub manifest: String,
    pub files: i64,
    pub bytes: i64,
    pub created_at: String,
    pub updated_at: String,
}

pub fn upsert_manifest(
    connection: &Connection,
    record: &InstallManifestRecord,
) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_manifests
                (game_id, version_id, install_path, manifest, files, bytes, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(game_id) DO UPDATE SET
                version_id   = excluded.version_id,
                install_path = excluded.install_path,
                manifest     = excluded.manifest,
                files        = excluded.files,
                bytes        = excluded.bytes,
                updated_at   = excluded.updated_at
            "#,
            params![
                record.game_id,
                record.version_id,
                record.install_path,
                record.manifest,
                record.files,
                record.bytes,
                record.created_at,
                record.updated_at,
            ],
        )
        .map_err(|e| format!("Could not record the install manifest: {e}"))?;
    Ok(())
}

pub fn get_manifest(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<InstallManifestRecord>, String> {
    connection
        .query_row(
            "SELECT game_id, version_id, install_path, manifest, files, bytes, created_at, updated_at
             FROM install_manifests WHERE game_id = ?1",
            params![game_id],
            |row| {
                Ok(InstallManifestRecord {
                    game_id: row.get(0)?,
                    version_id: row.get(1)?,
                    install_path: row.get(2)?,
                    manifest: row.get(3)?,
                    files: row.get(4)?,
                    bytes: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the install manifest: {e}"))
}

/// Keep the manifest attached to the folder after `move_install`.
pub fn update_manifest_path(
    connection: &Connection,
    game_id: i64,
    install_path: &str,
    now: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE install_manifests SET install_path = ?2, updated_at = ?3 WHERE game_id = ?1",
            params![game_id, install_path, now],
        )
        .map_err(|e| format!("Could not update the install manifest: {e}"))?;
    Ok(())
}

pub fn remove_manifest(connection: &Connection, game_id: i64) -> Result<bool, String> {
    let affected = connection
        .execute(
            "DELETE FROM install_manifests WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not remove the install manifest: {e}"))?;
    Ok(affected > 0)
}

/// One resolved save path for a game (#107). `source` is `manifest`, `manual`
/// or `override`; `primary` is the path a future cloud-save snapshot archives
/// first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavePathRecord {
    pub game_id: i64,
    pub path: String,
    pub kind: String,
    pub source: String,
    pub primary: bool,
    pub created_at: String,
}

const SAVE_PATH_COLUMNS: &str = "game_id, path, kind, source, \"primary\", created_at";

fn map_save_path(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavePathRecord> {
    Ok(SavePathRecord {
        game_id: row.get(0)?,
        path: row.get(1)?,
        kind: row.get(2)?,
        source: row.get(3)?,
        primary: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
    })
}

/// Every persisted save path for one game, primary first then path order.
pub fn list_save_paths(
    connection: &Connection,
    game_id: i64,
) -> Result<Vec<SavePathRecord>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {SAVE_PATH_COLUMNS} FROM save_paths WHERE game_id = ?1 ORDER BY \"primary\" DESC, path ASC"
        ))
        .map_err(|e| format!("Could not read save paths: {e}"))?;

    let rows = statement
        .query_map(params![game_id], map_save_path)
        .map_err(|e| format!("Could not read save paths: {e}"))?;

    let mut paths = Vec::new();
    for row in rows {
        paths.push(row.map_err(|e| format!("Could not read save paths: {e}"))?);
    }
    Ok(paths)
}

/// Replace the whole mapping for a game in one transaction. Duplicate paths are
/// dropped and only the first `primary` entry is kept, matching the partial
/// unique index.
pub fn replace_save_paths(
    connection: &Connection,
    game_id: i64,
    paths: &[SavePathRecord],
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| format!("Could not update the save paths: {e}"))?;
    transaction
        .execute(
            "DELETE FROM save_paths WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not update the save paths: {e}"))?;

    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut primary_used = false;
    for record in paths {
        if !seen.insert(record.path.as_str()) {
            continue;
        }
        let primary = record.primary && !primary_used;
        if primary {
            primary_used = true;
        }
        transaction
            .execute(
                r#"INSERT INTO save_paths (game_id, path, kind, source, "primary", created_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    game_id,
                    record.path,
                    record.kind,
                    record.source,
                    i64::from(primary),
                    record.created_at,
                ],
            )
            .map_err(|e| format!("Could not record the save path: {e}"))?;
    }

    transaction
        .commit()
        .map_err(|e| format!("Could not update the save paths: {e}"))
}

/// Remove every save path for a game. Returns the number of rows deleted.
pub fn delete_save_paths(connection: &Connection, game_id: i64) -> Result<usize, String> {
    connection
        .execute(
            "DELETE FROM save_paths WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not remove the save paths: {e}"))
}

/// Last detection preview for a game (#107).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveDetectionRecord {
    pub game_id: i64,
    pub version_id: i64,
    pub manifest_version: Option<String>,
    pub detected_at: String,
    /// JSON encoding of `saves_detect::Detection` (sizes, notes, warnings).
    pub result_json: String,
}

pub fn upsert_save_detection(
    connection: &Connection,
    record: &SaveDetectionRecord,
) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO save_detection
                (game_id, version_id, manifest_version, detected_at, result_json)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(game_id) DO UPDATE SET
                version_id       = excluded.version_id,
                manifest_version = excluded.manifest_version,
                detected_at      = excluded.detected_at,
                result_json      = excluded.result_json
            "#,
            params![
                record.game_id,
                record.version_id,
                record.manifest_version,
                record.detected_at,
                record.result_json,
            ],
        )
        .map_err(|e| format!("Could not record the save detection: {e}"))?;
    Ok(())
}

pub fn get_save_detection(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<SaveDetectionRecord>, String> {
    connection
        .query_row(
            "SELECT game_id, version_id, manifest_version, detected_at, result_json
             FROM save_detection WHERE game_id = ?1",
            params![game_id],
            |row| {
                Ok(SaveDetectionRecord {
                    game_id: row.get(0)?,
                    version_id: row.get(1)?,
                    manifest_version: row.get(2)?,
                    detected_at: row.get(3)?,
                    result_json: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the save detection: {e}"))
}

pub fn delete_save_detection(connection: &Connection, game_id: i64) -> Result<bool, String> {
    let affected = connection
        .execute(
            "DELETE FROM save_detection WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not remove the save detection: {e}"))?;
    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(SCHEMA_V2).unwrap();
        connection.execute_batch(SCHEMA_V3).unwrap();
        connection.execute_batch(SCHEMA_V4).unwrap();
        connection.execute_batch(SCHEMA_V5).unwrap();
        connection.execute_batch(SCHEMA_V6).unwrap();
        connection.execute_batch(SCHEMA_V7).unwrap();
        connection.execute_batch(SCHEMA_V8).unwrap();
        connection
    }

    #[test]
    fn migration_adds_external_import_columns() {
        let connection = memory_database();
        let columns: Vec<String> = connection
            .prepare("PRAGMA table_info(installed_games)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|name| name.unwrap())
            .collect();
        assert!(columns.iter().any(|name| name == "source"));
        assert!(columns.iter().any(|name| name == "launch_uri"));

        let uri = "steam://rungameid/440";
        connection
            .execute(
                "INSERT INTO installed_games (game_id, version_id, title, install_path, installed_at, launch_uri)
                 VALUES (-440, 0, 'TF2', '/steam/tf2', '2026-11-10T00:00:00Z', ?1)",
                params![uri],
            )
            .unwrap();
        // The partial unique index rejects a second import of the same URI.
        assert!(connection
            .execute(
                "INSERT INTO installed_games (game_id, version_id, title, install_path, installed_at, launch_uri)
                 VALUES (-441, 0, 'TF2 again', '/steam/tf2', '2026-11-10T00:00:00Z', ?1)",
                params![uri],
            )
            .is_err());
    }

    #[test]
    fn installed_games_roundtrip() {
        let connection = memory_database();
        let game = InstalledGame {
            game_id: 7,
            version_id: 3,
            title: "Test".into(),
            version: Some("1.0".into()),
            install_path: "/tmp/test".into(),
            executable: Some("/tmp/test/game".into()),
            size_bytes: 42,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: Some("sha256-sample:abc".into()),
            install_root: Some("/tmp".into()),
            source: SOURCE_LUDEX.into(),
            launch_uri: None,
        };
        upsert_installed(&connection, &game).unwrap();
        let loaded = get_installed(&connection, 7).unwrap().unwrap();
        assert_eq!(loaded.title, "Test");
        assert_eq!(loaded.archive_hash.as_deref(), Some("sha256-sample:abc"));
        assert_eq!(list_installed(&connection).unwrap().len(), 1);
        assert!(remove_installed(&connection, 7).unwrap());
        assert!(get_installed(&connection, 7).unwrap().is_none());
    }

    #[test]
    fn install_jobs_roundtrip_and_interrupt() {
        let connection = memory_database();
        let job = InstallJobRecord {
            job_id: "job-1".into(),
            game_id: 1,
            version_id: 2,
            phase: "downloading".into(),
            bytes_done: 1024,
            bytes_total: Some(4096),
            speed_bps: 512,
            part_path: Some("/tmp/game.part".into()),
            install_path: Some("/tmp/game".into()),
            error: None,
            stream: true,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        upsert_install_job(&connection, &job).unwrap();
        let loaded = get_install_job(&connection, "job-1").unwrap().unwrap();
        assert_eq!(loaded.bytes_done, 1024);
        assert_eq!(loaded.part_path.as_deref(), Some("/tmp/game.part"));
        assert!(loaded.stream);

        assert_eq!(
            mark_interrupted_jobs(&connection, "interrupted", "2026-01-01T00:10:00Z").unwrap(),
            1
        );
        let loaded = get_install_job(&connection, "job-1").unwrap().unwrap();
        assert_eq!(loaded.phase, "error");
        assert_eq!(loaded.error.as_deref(), Some("interrupted"));
        assert_eq!(all_install_jobs(&connection).unwrap().len(), 1);
        assert_eq!(all_install_jobs(&connection).unwrap().len(), 1);
    }

    #[test]
    fn open_migrates_and_is_idempotent() {
        let root = std::env::temp_dir().join(format!(
            "ludex-db-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("test.db");

        {
            let connection = open(&path).unwrap();
            let version: i64 = connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version, SCHEMA_VERSION);
        }
        {
            let connection = open(&path).unwrap();
            let jobs: i64 = connection
                .query_row("SELECT COUNT(*) FROM install_jobs", [], |row| row.get(0))
                .unwrap();
            assert_eq!(jobs, 0);
            let manifests: i64 = connection
                .query_row("SELECT COUNT(*) FROM install_manifests", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(manifests, 0);
            let save_paths: i64 = connection
                .query_row("SELECT COUNT(*) FROM save_paths", [], |row| row.get(0))
                .unwrap();
            assert_eq!(save_paths, 0);
            let detections: i64 = connection
                .query_row("SELECT COUNT(*) FROM save_detection", [], |row| row.get(0))
                .unwrap();
            assert_eq!(detections, 0);
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_manifests_roundtrip_and_follow_moves() {
        let connection = memory_database();
        let record = InstallManifestRecord {
            game_id: 4,
            version_id: 9,
            install_path: "/games/doom/1.0".into(),
            manifest: r#"{"entries":[],"totalBytes":0,"createdAt":"2026-01-01T00:00:00Z"}"#.into(),
            files: 0,
            bytes: 0,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        upsert_manifest(&connection, &record).unwrap();
        let loaded = get_manifest(&connection, 4).unwrap().unwrap();
        assert_eq!(loaded.install_path, "/games/doom/1.0");

        update_manifest_path(
            &connection,
            4,
            "/mnt/games/doom/1.0",
            "2026-01-02T00:00:00Z",
        )
        .unwrap();
        let loaded = get_manifest(&connection, 4).unwrap().unwrap();
        assert_eq!(loaded.install_path, "/mnt/games/doom/1.0");
        assert_eq!(loaded.updated_at, "2026-01-02T00:00:00Z");

        assert!(remove_manifest(&connection, 4).unwrap());
        assert!(get_manifest(&connection, 4).unwrap().is_none());
    }

    #[test]
    fn superseding_keeps_failed_history() {
        let connection = memory_database();
        let mut paused = InstallJobRecord {
            job_id: "paused".into(),
            game_id: 3,
            version_id: 4,
            phase: "paused".into(),
            bytes_done: 10,
            bytes_total: Some(20),
            speed_bps: 0,
            part_path: Some("/tmp/game.part".into()),
            install_path: None,
            error: None,
            stream: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        upsert_install_job(&connection, &paused).unwrap();
        paused.job_id = "failed".into();
        paused.phase = "error".into();
        paused.error = Some("boom".into());
        upsert_install_job(&connection, &paused).unwrap();

        assert_eq!(
            supersede_install_jobs(&connection, 3, 4, "2026-01-02T00:00:00Z").unwrap(),
            1
        );
        let paused = get_install_job(&connection, "paused").unwrap().unwrap();
        assert_eq!(paused.phase, "cancelled");
        let failed = get_install_job(&connection, "failed").unwrap().unwrap();
        assert_eq!(failed.phase, "error");
    }

    fn job_record(job_id: &str, phase: &str) -> InstallJobRecord {
        InstallJobRecord {
            job_id: job_id.into(),
            game_id: 9,
            version_id: 91,
            phase: phase.into(),
            bytes_done: 0,
            bytes_total: Some(100),
            speed_bps: 0,
            part_path: Some(format!("/tmp/{job_id}.part")),
            install_path: None,
            error: None,
            stream: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn deleting_a_terminal_install_job_removes_its_row() {
        let connection = memory_database();
        for phase in ["done", "cancelled", "error", "interrupted"] {
            let job_id = format!("job-{phase}");
            upsert_install_job(&connection, &job_record(&job_id, phase)).unwrap();
            assert!(
                delete_install_job(&connection, &job_id).unwrap(),
                "terminal phase {phase} must be deletable"
            );
            assert!(
                get_install_job(&connection, &job_id).unwrap().is_none(),
                "terminal phase {phase} row must be gone"
            );
        }
        // Unknown ids are not an error at the DB layer; the command maps this
        // to a clear "not in history" message.
        assert!(!delete_install_job(&connection, "job-missing").unwrap());
    }

    #[test]
    fn deleting_an_active_or_paused_install_job_keeps_its_row() {
        let connection = memory_database();
        for phase in [
            "queued",
            "downloading",
            "extracting",
            "finalizing",
            "paused",
        ] {
            let job_id = format!("job-{phase}");
            upsert_install_job(&connection, &job_record(&job_id, phase)).unwrap();
            assert!(
                !delete_install_job(&connection, &job_id).unwrap(),
                "{phase} is not terminal and must be refused"
            );
            let kept = get_install_job(&connection, &job_id).unwrap();
            assert_eq!(
                kept.expect("the row must survive").phase,
                phase,
                "{phase} row must stay"
            );
        }
    }

    #[test]
    fn clearing_finished_install_jobs_only_removes_terminal_rows() {
        let connection = memory_database();
        for phase in [
            "done",
            "cancelled",
            "error",
            "interrupted",
            "queued",
            "downloading",
            "paused",
        ] {
            upsert_install_job(&connection, &job_record(&format!("job-{phase}"), phase)).unwrap();
        }

        assert_eq!(clear_finished_install_jobs(&connection).unwrap(), 4);
        let remaining: Vec<String> = all_install_jobs(&connection)
            .unwrap()
            .into_iter()
            .map(|job| job.phase)
            .collect();
        assert_eq!(remaining.len(), 3);
        assert!(remaining
            .iter()
            .all(|phase| ["queued", "downloading", "paused"].contains(&phase.as_str())));
    }

    #[test]
    fn deleted_install_jobs_stay_gone_after_a_restart() {
        let connection = memory_database();
        upsert_install_job(&connection, &job_record("job-keep", "done")).unwrap();
        upsert_install_job(&connection, &job_record("job-drop", "error")).unwrap();

        assert!(delete_install_job(&connection, "job-drop").unwrap());

        // `all_install_jobs` is what `list_install_jobs` reads on the next
        // launch, so a deleted entry must not come back.
        let jobs = all_install_jobs(&connection).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, "job-keep");
        assert!(get_install_job(&connection, "job-drop").unwrap().is_none());
    }

    #[test]
    fn playtime_aggregates_totals_days_and_sessions() {
        let connection = memory_database();
        let today = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

        for (id, started, seconds) in [
            ("s1", yesterday.as_str(), 1800),
            ("s2", today.as_str(), 2700),
            ("s3", today.as_str(), 600),
        ] {
            insert_session(&connection, id, 5, started).unwrap();
            finish_session(&connection, id, started, seconds / 60, seconds).unwrap();
        }

        let playtime = game_playtime(&connection, 5, 30, 10).unwrap();
        assert_eq!(playtime.total_seconds, 5100);
        assert_eq!(playtime.sessions, 3);
        assert_eq!(playtime.by_day.len(), 2, "{:?}", playtime.by_day);
        assert_eq!(playtime.by_day[0].seconds, 3300);
        assert_eq!(playtime.by_day[1].seconds, 1800);
        assert_eq!(playtime.recent_sessions.len(), 3);
        assert!(playtime
            .recent_sessions
            .iter()
            .any(|session| session.id == "s1" && session.seconds == 1800));

        let limited = recent_sessions(&connection, 5, 1).unwrap();
        assert_eq!(limited.len(), 1);
        assert!(game_playtime(&connection, 6, 30, 10).unwrap().sessions == 0);
    }

    #[test]
    fn recent_games_follow_the_last_launch() {
        let connection = memory_database();
        let mut first = InstalledGame {
            game_id: 1,
            version_id: 1,
            title: "Alpha".into(),
            version: None,
            install_path: "/games/alpha".into(),
            executable: None,
            size_bytes: 1,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: Some("2026-01-01T00:00:00Z".into()),
            archive_hash: None,
            install_root: Some("/games".into()),
            source: SOURCE_LUDEX.into(),
            launch_uri: None,
        };
        upsert_installed(&connection, &first).unwrap();
        first.game_id = 2;
        first.title = "Beta".into();
        first.installed_at = "2026-01-02T00:00:00Z".into();
        first.last_launched_at = Some("2026-01-02T00:00:00Z".into());
        upsert_installed(&connection, &first).unwrap();
        first.game_id = 3;
        first.title = "Never played".into();
        first.installed_at = "2026-01-03T00:00:00Z".into();
        first.last_launched_at = None;
        upsert_installed(&connection, &first).unwrap();

        let recent = recent_installed(&connection, 10).unwrap();
        assert_eq!(
            recent
                .iter()
                .map(|game| game.title.as_str())
                .collect::<Vec<_>>(),
            // The never-launched entry still appears, by install date.
            vec!["Never played", "Beta", "Alpha"]
        );
        assert_eq!(recent_installed(&connection, 1).unwrap().len(), 1);
    }

    #[test]
    fn sessions_are_pending_until_synced() {
        let connection = memory_database();
        insert_session(&connection, "s1", 1, "2026-01-01T00:00:00Z").unwrap();
        assert!(pending_sessions(&connection).unwrap().is_empty());
        finish_session(&connection, "s1", "2026-01-01T00:30:00Z", 30, 1800).unwrap();
        let pending = pending_sessions(&connection).unwrap();
        assert_eq!(pending.len(), 1);
        mark_session_synced(&connection, "s1").unwrap();
        assert!(pending_sessions(&connection).unwrap().is_empty());
    }

    #[test]
    fn launch_options_roundtrip_and_default_to_auto() {
        let connection = memory_database();
        assert_eq!(
            get_launch_options(&connection, 9).unwrap(),
            LaunchOptions::default()
        );

        let mut options = LaunchOptions {
            mode: LaunchMode::Proton,
            tool_id: "proton:/games/GE-Proton9-20".into(),
            prefix_path: "/mnt/prefixes/nine".into(),
            ..LaunchOptions::default()
        };
        options.env.insert("DXVK_HUD".into(), "fps".into());
        options.winetricks_verbs = vec!["corefonts".into()];
        options.steam_app_id = Some(1145360);
        save_launch_options(&connection, 9, &options, "2026-11-10T00:00:00Z").unwrap();

        let back = get_launch_options(&connection, 9).unwrap();
        assert_eq!(back, options);

        // Corrupt JSON falls back to defaults instead of breaking the launch.
        connection
            .execute(
                "UPDATE game_launch_options SET env = 'not json' WHERE game_id = 9",
                [],
            )
            .unwrap();
        let broken = get_launch_options(&connection, 9).unwrap();
        assert!(broken.env.is_empty());
        assert_eq!(broken.mode, LaunchMode::Proton);
    }

    #[test]
    fn protondb_cache_roundtrips() {
        let connection = memory_database();
        assert!(cached_rating(&connection, 1145360).unwrap().is_none());

        let rating = CachedRating {
            app_id: 1145360,
            tier: "platinum".into(),
            best_tier: Some("platinum".into()),
            trending_tier: None,
            confidence: Some("strong".into()),
            total: 412,
            score: Some(0.82),
            fetched_at: "2026-11-10T00:00:00Z".into(),
        };
        store_rating(&connection, &rating).unwrap();
        let back = cached_rating(&connection, 1145360).unwrap().unwrap();
        assert_eq!(back, rating);
    }

    fn save_path(game_id: i64, path: &str, source: &str, primary: bool) -> SavePathRecord {
        SavePathRecord {
            game_id,
            path: path.into(),
            kind: "directory".into(),
            source: source.into(),
            primary,
            created_at: "2026-11-10T00:00:00Z".into(),
        }
    }

    #[test]
    fn save_paths_roundtrip_primary_and_delete() {
        let connection = memory_database();
        let paths = vec![
            save_path(7, "/home/player/.local/share/Game", "manifest", false),
            save_path(7, "/home/player/.config/Game", "manual", true),
        ];
        replace_save_paths(&connection, 7, &paths).unwrap();

        let loaded = list_save_paths(&connection, 7).unwrap();
        assert_eq!(loaded.len(), 2);
        // Primary first, then path order.
        assert_eq!(loaded[0].path, "/home/player/.config/Game");
        assert!(loaded[0].primary);
        assert_eq!(loaded[0].source, "manual");
        assert!(!loaded[1].primary);

        // Replace keeps only the new set and clears the old primary.
        replace_save_paths(&connection, 7, &paths[..1]).unwrap();
        let loaded = list_save_paths(&connection, 7).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(!loaded[0].primary);

        assert_eq!(delete_save_paths(&connection, 7).unwrap(), 1);
        assert!(list_save_paths(&connection, 7).unwrap().is_empty());
    }

    #[test]
    fn save_paths_are_unique_per_game_and_primary() {
        let connection = memory_database();
        // The composite primary key rejects a duplicate path for one game.
        connection
            .execute(
                r#"INSERT INTO save_paths (game_id, path, kind, source, "primary", created_at)
                   VALUES (1, '/saves/game', 'directory', 'manual', 1, '2026-11-10T00:00:00Z')"#,
                [],
            )
            .unwrap();
        assert!(connection
            .execute(
                r#"INSERT INTO save_paths (game_id, path, kind, source, "primary", created_at)
                   VALUES (1, '/saves/game', 'directory', 'manifest', 0, '2026-11-10T00:00:00Z')"#,
                [],
            )
            .is_err());
        // The partial unique index allows one primary per game, not per table.
        assert!(connection
            .execute(
                r#"INSERT INTO save_paths (game_id, path, kind, source, "primary", created_at)
                   VALUES (1, '/saves/other', 'directory', 'manifest', 1, '2026-11-10T00:00:00Z')"#,
                [],
            )
            .is_err());
        connection
            .execute(
                r#"INSERT INTO save_paths (game_id, path, kind, source, "primary", created_at)
                   VALUES (2, '/saves/other', 'directory', 'manifest', 1, '2026-11-10T00:00:00Z')"#,
                [],
            )
            .unwrap();

        // The helper de-duplicates paths and keeps at most one primary.
        let duplicates = vec![
            save_path(3, "/saves/a", "manual", true),
            save_path(3, "/saves/a", "manifest", true),
            save_path(3, "/saves/b", "manifest", true),
        ];
        replace_save_paths(&connection, 3, &duplicates).unwrap();
        let loaded = list_save_paths(&connection, 3).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.iter().filter(|row| row.primary).count(), 1);
    }

    #[test]
    fn save_detection_roundtrip_and_delete() {
        let connection = memory_database();
        let record = SaveDetectionRecord {
            game_id: 5,
            version_id: 52,
            manifest_version: Some("sha256:abc123456789".into()),
            detected_at: "2026-11-10T00:00:00Z".into(),
            result_json: r#"{"detected":true,"saves":[]}"#.into(),
        };
        upsert_save_detection(&connection, &record).unwrap();
        let loaded = get_save_detection(&connection, 5).unwrap().unwrap();
        assert_eq!(loaded.version_id, 52);
        assert_eq!(
            loaded.manifest_version.as_deref(),
            Some("sha256:abc123456789")
        );
        assert_eq!(loaded.result_json, r#"{"detected":true,"saves":[]}"#);

        let updated = SaveDetectionRecord {
            detected_at: "2026-11-11T00:00:00Z".into(),
            result_json: r#"{"detected":false,"saves":[]}"#.into(),
            ..record
        };
        upsert_save_detection(&connection, &updated).unwrap();
        let loaded = get_save_detection(&connection, 5).unwrap().unwrap();
        assert_eq!(loaded.detected_at, "2026-11-11T00:00:00Z");
        assert_eq!(loaded.result_json, r#"{"detected":false,"saves":[]}"#);

        assert!(delete_save_detection(&connection, 5).unwrap());
        assert!(get_save_detection(&connection, 5).unwrap().is_none());
    }

    #[test]
    fn save_detection_rows_are_per_game() {
        let connection = memory_database();
        for game_id in [1, 2] {
            upsert_save_detection(
                &connection,
                &SaveDetectionRecord {
                    game_id,
                    version_id: 0,
                    manifest_version: None,
                    detected_at: "2026-11-10T00:00:00Z".into(),
                    result_json: "{}".into(),
                },
            )
            .unwrap();
        }
        assert!(get_save_detection(&connection, 1).unwrap().is_some());
        assert!(get_save_detection(&connection, 2).unwrap().is_some());
        assert!(delete_save_detection(&connection, 1).unwrap());
        assert!(get_save_detection(&connection, 1).unwrap().is_none());
        assert!(get_save_detection(&connection, 2).unwrap().is_some());
    }
}
