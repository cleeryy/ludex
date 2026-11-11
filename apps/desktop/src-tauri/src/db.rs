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
    /// How the entry entered the local index: `ludex` (installed by this app),
    /// `adopted` (an existing folder linked in place, #129) or a launcher id
    /// (imported from another client, #13/#116).
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
/// An existing folder linked to a library game without copying or moving it
/// (#129). Ludex can launch it, track playtime and delete it only through the
/// confirmed `delete_adopted_installation` action.
pub const SOURCE_ADOPTED: &str = "adopted";

/// True for entries owned by another client (#13/#116): Ludex never deletes
/// their files and never adopts the folder they live in.
pub fn is_launcher_source(source: &str) -> bool {
    source != SOURCE_LUDEX && source != SOURCE_ADOPTED
}

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

const SCHEMA_VERSION: i64 = 12;

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

/// Automatic cloud save sync (#108). `auto_sync` is `NULL` when the game
/// inherits the global `cloudAutoSync` setting, so a global change applies to
/// every game that never opted out. `last_hash` is the SHA-256 of the last
/// archive uploaded from this machine; it makes an unchanged save a no-op.
/// Pending retry state is persisted here so a failed upload survives a restart.
const SCHEMA_V9: &str = r#"
CREATE TABLE IF NOT EXISTS cloud_save_prefs (
    game_id          INTEGER PRIMARY KEY,
    auto_sync        INTEGER,
    last_hash        TEXT,
    last_upload_at   TEXT,
    last_restore_at  TEXT,
    pending_attempts INTEGER NOT NULL DEFAULT 0,
    pending_next_at  TEXT,
    pending_error    TEXT,
    ignored_remote_hash TEXT
);

CREATE TABLE IF NOT EXISTS cloud_save_restore (
    game_id           INTEGER PRIMARY KEY,
    snapshot_id       INTEGER NOT NULL,
    restored_at       TEXT    NOT NULL,
    local_backup_path TEXT
);
"#;

/// Issue #115: per-entry checkpoints for resumable stream installs (v10).
///
/// `stream_completed` is a hex bitset of completed entry indexes (bit `i` set
/// when entry `i` is on disk). The manifest is kept in its own table so
/// updating the checkpoint after every entry does not rewrite a multi-megabyte
/// entry list. Both survive an app restart; the next install of the same
/// version adopts them when the fingerprint still matches the server manifest.
const SCHEMA_V10: &str = r#"
ALTER TABLE install_jobs ADD COLUMN stream_completed TEXT;

CREATE TABLE IF NOT EXISTS install_stream_manifests (
    job_id      TEXT PRIMARY KEY,
    game_id     INTEGER NOT NULL,
    version_id  INTEGER NOT NULL,
    format      TEXT NOT NULL,
    staging     TEXT NOT NULL,
    entries     TEXT NOT NULL,
    entry_count INTEGER NOT NULL,
    total_bytes INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS install_stream_manifests_version_idx
    ON install_stream_manifests (game_id, version_id);
"#;

/// Per-game game-settings edits (#105): every write keeps the original file in
/// `{appDataDir}/config-backups/game-<id>/` and records the line diff here, so
/// the UI can show history and restore a previous version. `file_path` is
/// relative to the install folder and `backup_path` is absolute on this
/// machine; nothing is synced to the server.
const SCHEMA_V11: &str = r#"
CREATE TABLE IF NOT EXISTS config_edits (
    id          TEXT    PRIMARY KEY,
    game_id     INTEGER NOT NULL,
    file_path   TEXT    NOT NULL,
    backup_path TEXT    NOT NULL,
    diff        TEXT    NOT NULL DEFAULT '',
    bytes       INTEGER NOT NULL DEFAULT 0,
    origin      TEXT    NOT NULL DEFAULT 'manual',
    created_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS config_edits_game_idx ON config_edits (game_id, created_at DESC);
"#;

/// Common config fields (#148): a per-game pinned key for a recognised field
/// (language, player name) and the opt-in that re-applies it on launch.
///
/// `config_field_mappings` overrides the built-in catalog: at most one row per
/// field, pointing at the exact relative file and key. `config_sync_prefs`
/// stores the last values the UI handed over, so a launch can apply them
/// without the web view; `apply_language`/`apply_player_name` let a game sync
/// only one of the two.
const SCHEMA_V12: &str = r#"
CREATE TABLE IF NOT EXISTS config_field_mappings (
    game_id    INTEGER NOT NULL,
    field      TEXT    NOT NULL,
    file_path  TEXT    NOT NULL,
    key        TEXT    NOT NULL,
    created_at TEXT    NOT NULL,
    PRIMARY KEY (game_id, field)
);

CREATE TABLE IF NOT EXISTS config_sync_prefs (
    game_id           INTEGER PRIMARY KEY,
    enabled           INTEGER NOT NULL DEFAULT 0,
    language          TEXT    NOT NULL DEFAULT '',
    player_name       TEXT    NOT NULL DEFAULT '',
    apply_language    INTEGER NOT NULL DEFAULT 1,
    apply_player_name INTEGER NOT NULL DEFAULT 1,
    last_applied_at   TEXT,
    updated_at        TEXT    NOT NULL
);
"#;

/// One recorded config write (#105/#106), shown in the game's Settings tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEditRecord {
    pub id: String,
    pub game_id: i64,
    /// Path relative to the install folder (`Saved/Config/Windows/GameUserSettings.ini`).
    pub file_path: String,
    /// Absolute backup file on this machine. Never sent to the server.
    pub backup_path: String,
    pub diff: String,
    pub bytes: i64,
    /// `manual` for a key edit, `adapter` for a typed control, `restore` for
    /// the safety copy taken before a restore.
    pub origin: String,
    pub created_at: String,
}

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
    if version < 9 {
        connection
            .execute_batch(SCHEMA_V9)
            .map_err(|e| format!("Could not add cloud save sync to the desktop database: {e}"))?;
    }
    if version < 10 {
        connection
            .execute_batch(SCHEMA_V10)
            .map_err(|e| format!("Could not add resumable stream checkpoints: {e}"))?;
    }
    if version < 11 {
        connection
            .execute_batch(SCHEMA_V11)
            .map_err(|e| format!("Could not add game config edits to the desktop database: {e}"))?;
    }
    if version < 12 {
        connection.execute_batch(SCHEMA_V12).map_err(|e| {
            format!("Could not add common config fields to the desktop database: {e}")
        })?;
    }

    if version < SCHEMA_VERSION {
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| format!("Could not record the desktop database version: {e}"))?;
    }

    // Future migrations: bump SCHEMA_VERSION and add `if version < N` blocks.
    Ok(())
}

/// A fresh in-memory index with every migration applied. Tests only.
#[cfg(test)]
pub(crate) fn open_in_memory_for_tests() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrate(&connection).unwrap();
    connection
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

/// Every external entry that already has a launch URI, keyed by URI (#116).
///
/// The unique index on `launch_uri` is the source of truth for "already
/// imported"; this map adds the row id and its source for the preview.
pub fn launch_uris(connection: &Connection) -> Result<Vec<(String, i64, String)>, String> {
    let mut statement = connection
        .prepare(
            r#"
            SELECT launch_uri, game_id, source
            FROM installed_games
            WHERE launch_uri IS NOT NULL AND TRIM(launch_uri) != ''
            "#,
        )
        .map_err(|e| format!("Could not read imported games: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("Could not read imported games: {e}"))?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row.map_err(|e| format!("Could not read imported games: {e}"))?);
    }
    Ok(entries)
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

/// Every indexed install as `(gameId, installPath, source)`.
///
/// Used by the adoption flow to refuse a folder another entry already owns and
/// to dedupe the same folder adopted twice (#129). Paths are compared by the
/// caller after canonicalization.
pub fn installed_paths(connection: &Connection) -> Result<Vec<(i64, String, String)>, String> {
    let mut statement = connection
        .prepare("SELECT game_id, install_path, source FROM installed_games")
        .map_err(|e| format!("Could not read installed games: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("Could not read installed games: {e}"))?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row.map_err(|e| format!("Could not read installed games: {e}"))?);
    }
    Ok(entries)
}

/// Record an adopted folder (#129).
///
/// `install_path` must already be the canonical folder. The desktop index keeps
/// one row per game, so the caller has to say whether replacing an existing
/// `ludex`/launcher row was confirmed (`replace`). An existing adopted row is
/// refreshed in place: the user is re-pointing their own folder, not replacing
/// a Ludex install.
pub fn record_adoption(
    connection: &Connection,
    game: &InstalledGame,
    replace: bool,
) -> Result<(), String> {
    if game.source != SOURCE_ADOPTED {
        return Err("Only adopted installs can be recorded through adoption.".to_string());
    }
    if let Some(existing) = get_installed(connection, game.game_id)? {
        if existing.source != SOURCE_ADOPTED && !replace {
            return Err(format!(
                "{} is already {}. Remove it from Ludex first.",
                existing.title,
                describe_source(&existing.source)
            ));
        }
    }

    // One folder can only back one entry. Launcher imports (#116) own their
    // folder: Ludex must never take it over silently.
    for (other_game_id, path, source) in installed_paths(connection)? {
        if other_game_id == game.game_id || path != game.install_path {
            continue;
        }
        if is_launcher_source(&source) {
            return Err(format!(
                "That folder belongs to a {source} import (#116). Imported games stay managed by their launcher."
            ));
        }
        return Err(format!(
            "That folder is already indexed as game {other_game_id} ({source}). Remove that entry first."
        ));
    }

    upsert_installed(connection, game)
}

/// Human wording for a source, used in actionable errors.
pub fn describe_source(source: &str) -> String {
    match source {
        SOURCE_LUDEX => "installed through Ludex".to_string(),
        SOURCE_ADOPTED => "linked as an adopted folder".to_string(),
        other => format!("imported from {other}"),
    }
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
    /// Hex bitset of completed entry indexes for a resumable stream install
    /// (#115). `None` for download/tar jobs.
    pub stream_completed: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn upsert_install_job(connection: &Connection, job: &InstallJobRecord) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_jobs
                (job_id, game_id, version_id, phase, bytes_done, bytes_total,
                 speed_bps, part_path, install_path, error, stream, stream_completed,
                 created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(job_id) DO UPDATE SET
                phase        = excluded.phase,
                bytes_done   = excluded.bytes_done,
                bytes_total  = excluded.bytes_total,
                speed_bps    = excluded.speed_bps,
                part_path    = excluded.part_path,
                install_path = excluded.install_path,
                error        = excluded.error,
                stream       = excluded.stream,
                stream_completed = excluded.stream_completed,
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
                job.stream_completed,
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
        stream_completed: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

const INSTALL_JOB_COLUMNS: &str = "job_id, game_id, version_id, phase, bytes_done, bytes_total, speed_bps, part_path, install_path, error, stream, stream_completed, created_at, updated_at";

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

/// Row shape of `install_stream_manifests` (#115).
#[derive(Debug, Clone)]
pub struct StreamManifestRecord {
    pub job_id: String,
    pub game_id: i64,
    pub version_id: i64,
    pub format: String,
    pub staging: String,
    pub entries: String,
    pub entry_count: i64,
    pub total_bytes: i64,
    pub fingerprint: String,
    pub created_at: String,
}

/// Saves (or replaces) the manifest of a resumable stream job.
pub fn upsert_stream_manifest(
    connection: &Connection,
    record: &StreamManifestRecord,
) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_stream_manifests
                (job_id, game_id, version_id, format, staging, entries,
                 entry_count, total_bytes, fingerprint, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(job_id) DO UPDATE SET
                format      = excluded.format,
                staging     = excluded.staging,
                entries     = excluded.entries,
                entry_count = excluded.entry_count,
                total_bytes = excluded.total_bytes,
                fingerprint = excluded.fingerprint
            "#,
            params![
                record.job_id,
                record.game_id,
                record.version_id,
                record.format,
                record.staging,
                record.entries,
                record.entry_count,
                record.total_bytes,
                record.fingerprint,
                record.created_at,
            ],
        )
        .map_err(|e| format!("Could not record the stream manifest: {e}"))?;
    Ok(())
}

fn map_stream_manifest(row: &rusqlite::Row<'_>) -> rusqlite::Result<StreamManifestRecord> {
    Ok(StreamManifestRecord {
        job_id: row.get(0)?,
        game_id: row.get(1)?,
        version_id: row.get(2)?,
        format: row.get(3)?,
        staging: row.get(4)?,
        entries: row.get(5)?,
        entry_count: row.get(6)?,
        total_bytes: row.get(7)?,
        fingerprint: row.get(8)?,
        created_at: row.get(9)?,
    })
}

const STREAM_MANIFEST_COLUMNS: &str = "job_id, game_id, version_id, format, staging, entries, entry_count, total_bytes, fingerprint, created_at";

/// Most recent manifest for a game version, whatever job wrote it. Used to
/// adopt a checkpoint when an install is restarted from the UI or after an app
/// restart (which creates a new job id).
pub fn latest_stream_manifest_for_version(
    connection: &Connection,
    game_id: i64,
    version_id: i64,
) -> Result<Option<StreamManifestRecord>, String> {
    connection
        .query_row(
            &format!(
                "SELECT {STREAM_MANIFEST_COLUMNS} FROM install_stream_manifests
                 WHERE game_id = ?1 AND version_id = ?2
                 ORDER BY created_at DESC LIMIT 1"
            ),
            params![game_id, version_id],
            map_stream_manifest,
        )
        .optional()
        .map_err(|e| format!("Could not read the stream manifest: {e}"))
}

/// Deletes every manifest row for a game version except `keep_job_id`.
pub fn prune_stream_manifests(
    connection: &Connection,
    game_id: i64,
    version_id: i64,
    keep_job_id: &str,
) -> Result<usize, String> {
    connection
        .execute(
            "DELETE FROM install_stream_manifests
             WHERE game_id = ?1 AND version_id = ?2 AND job_id <> ?3",
            params![game_id, version_id, keep_job_id],
        )
        .map_err(|e| format!("Could not prune the stream manifests: {e}"))
}

/// Drops the manifest of one job (completed, cancelled without progress, or
/// superseded).
pub fn delete_stream_manifest(connection: &Connection, job_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM install_stream_manifests WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Could not delete the stream manifest: {e}"))?;
    Ok(())
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
    // The stream manifest and its checkpoint are intentionally kept: they
    // describe files still present in the staging folder, so a later install
    // can adopt them even after the history row is gone (#115).
    Ok(affected > 0)
}

/// Remove every terminal row from Downloads history (#100). Returns the number
/// of rows deleted; install folders, archives and `.part` files stay.
pub fn clear_finished_install_jobs(connection: &Connection) -> Result<usize, String> {
    let affected = connection
        .execute(
            &format!("DELETE FROM install_jobs WHERE phase IN ({TERMINAL_INSTALL_PHASES})"),
            [],
        )
        .map_err(|e| format!("Could not clear finished install jobs: {e}"))?;
    Ok(affected)
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

/// Per-game cloud save state (#108).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudSavePrefs {
    pub game_id: i64,
    /// `None` inherits the global `cloudAutoSync` setting.
    pub auto_sync: Option<bool>,
    pub last_hash: Option<String>,
    pub last_upload_at: Option<String>,
    pub last_restore_at: Option<String>,
    pub pending_attempts: i64,
    pub pending_next_at: Option<String>,
    pub pending_error: Option<String>,
    /// Remote snapshot hash the user chose to ignore with `keep_local` (#108).
    /// Without it the same conflict would reappear forever when the ignored
    /// snapshot cannot be replaced by a byte-identical upload.
    pub ignored_remote_hash: Option<String>,
}

const CLOUD_SAVE_PREF_COLUMNS: &str =
    "game_id, auto_sync, last_hash, last_upload_at, last_restore_at,
     pending_attempts, pending_next_at, pending_error, ignored_remote_hash";

fn map_cloud_save_prefs(row: &rusqlite::Row<'_>) -> rusqlite::Result<CloudSavePrefs> {
    Ok(CloudSavePrefs {
        game_id: row.get(0)?,
        auto_sync: row.get::<_, Option<i64>>(1)?.map(|value| value != 0),
        last_hash: row.get(2)?,
        last_upload_at: row.get(3)?,
        last_restore_at: row.get(4)?,
        pending_attempts: row.get(5)?,
        pending_next_at: row.get(6)?,
        pending_error: row.get(7)?,
        ignored_remote_hash: row.get(8)?,
    })
}

/// Read the cloud save row for a game, creating an empty one on first use.
pub fn ensure_cloud_save_prefs(
    connection: &Connection,
    game_id: i64,
) -> Result<CloudSavePrefs, String> {
    connection
        .execute(
            "INSERT INTO cloud_save_prefs (game_id) VALUES (?1)
             ON CONFLICT(game_id) DO NOTHING",
            params![game_id],
        )
        .map_err(|e| format!("Could not prepare the cloud save state: {e}"))?;
    get_cloud_save_prefs(connection, game_id)?
        .ok_or_else(|| "Could not read the cloud save state.".to_string())
}

pub fn get_cloud_save_prefs(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<CloudSavePrefs>, String> {
    connection
        .query_row(
            &format!("SELECT {CLOUD_SAVE_PREF_COLUMNS} FROM cloud_save_prefs WHERE game_id = ?1"),
            params![game_id],
            map_cloud_save_prefs,
        )
        .optional()
        .map_err(|e| format!("Could not read the cloud save state: {e}"))
}

/// Store a per-game override, or `None` to inherit the global setting again.
pub fn set_cloud_save_auto_sync(
    connection: &Connection,
    game_id: i64,
    auto_sync: Option<bool>,
) -> Result<(), String> {
    ensure_cloud_save_prefs(connection, game_id)?;
    connection
        .execute(
            "UPDATE cloud_save_prefs SET auto_sync = ?2 WHERE game_id = ?1",
            params![game_id, auto_sync.map(i64::from)],
        )
        .map_err(|e| format!("Could not update the cloud save state: {e}"))?;
    Ok(())
}

/// Record a successful automatic upload and clear any pending retry.
pub fn record_cloud_save_upload(
    connection: &Connection,
    game_id: i64,
    hash: &str,
    uploaded_at: &str,
) -> Result<(), String> {
    ensure_cloud_save_prefs(connection, game_id)?;
    connection
        .execute(
            r#"UPDATE cloud_save_prefs SET
                   last_hash = ?2,
                   last_upload_at = ?3,
                   pending_attempts = 0,
                   pending_next_at = NULL,
                   pending_error = NULL,
                   ignored_remote_hash = NULL
               WHERE game_id = ?1"#,
            params![game_id, hash, uploaded_at],
        )
        .map_err(|e| format!("Could not record the cloud save upload: {e}"))?;
    Ok(())
}

/// Remember which remote snapshot the user chose to ignore (`keep_local`), so
/// the same conflict is not reported again. Cleared by the next upload or
/// remote restore.
pub fn record_cloud_save_ignored_remote(
    connection: &Connection,
    game_id: i64,
    hash: Option<&str>,
) -> Result<(), String> {
    ensure_cloud_save_prefs(connection, game_id)?;
    connection
        .execute(
            "UPDATE cloud_save_prefs SET ignored_remote_hash = ?2 WHERE game_id = ?1",
            params![game_id, hash],
        )
        .map_err(|e| format!("Could not record the cloud save conflict choice: {e}"))?;
    Ok(())
}

/// Remember a failed upload for the bounded retry worker.
pub fn record_cloud_save_failure(
    connection: &Connection,
    game_id: i64,
    attempts: i64,
    next_attempt_at: Option<&str>,
    error: &str,
) -> Result<(), String> {
    let current = ensure_cloud_save_prefs(connection, game_id)?;
    let attempts = attempts.max(current.pending_attempts);
    connection
        .execute(
            r#"UPDATE cloud_save_prefs SET
                   pending_attempts = ?2,
                   pending_next_at = ?3,
                   pending_error = ?4
               WHERE game_id = ?1"#,
            params![game_id, attempts, next_attempt_at, error],
        )
        .map_err(|e| format!("Could not record the cloud save retry: {e}"))?;
    Ok(())
}

pub fn clear_cloud_save_pending(connection: &Connection, game_id: i64) -> Result<(), String> {
    connection
        .execute(
            r#"UPDATE cloud_save_prefs SET
                   pending_attempts = 0,
                   pending_next_at = NULL,
                   pending_error = NULL
               WHERE game_id = ?1"#,
            params![game_id],
        )
        .map_err(|e| format!("Could not clear the cloud save retry: {e}"))?;
    Ok(())
}

/// Turning automatic sync off drops every queued retry: the worker must not
/// keep retrying uploads the user asked it to stop.
pub fn clear_all_cloud_save_pending(connection: &Connection) -> Result<(), String> {
    connection
        .execute(
            r#"UPDATE cloud_save_prefs SET
                   pending_attempts = 0,
                   pending_next_at = NULL,
                   pending_error = NULL
               WHERE pending_error IS NOT NULL OR pending_next_at IS NOT NULL"#,
            [],
        )
        .map_err(|e| format!("Could not clear the cloud save retries: {e}"))?;
    Ok(())
}

/// Games whose next retry is due at or before `now` (RFC 3339, UTC).
pub fn due_cloud_save_retries(connection: &Connection, now: &str) -> Result<Vec<i64>, String> {
    let mut statement = connection
        .prepare(
            "SELECT game_id FROM cloud_save_prefs
             WHERE pending_next_at IS NOT NULL AND pending_next_at <= ?1
             ORDER BY pending_next_at ASC",
        )
        .map_err(|e| format!("Could not read the cloud save retries: {e}"))?;
    let rows = statement
        .query_map(params![now], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("Could not read the cloud save retries: {e}"))?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row.map_err(|e| format!("Could not read the cloud save retries: {e}"))?);
    }
    Ok(games)
}

/// One completed restore, used to run a snapshot at most once (#108).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudSaveRestoreRecord {
    pub game_id: i64,
    pub snapshot_id: i64,
    pub restored_at: String,
    pub local_backup_path: Option<String>,
}

pub fn get_cloud_save_restore(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<CloudSaveRestoreRecord>, String> {
    connection
        .query_row(
            "SELECT game_id, snapshot_id, restored_at, local_backup_path
             FROM cloud_save_restore WHERE game_id = ?1",
            params![game_id],
            |row| {
                Ok(CloudSaveRestoreRecord {
                    game_id: row.get(0)?,
                    snapshot_id: row.get(1)?,
                    restored_at: row.get(2)?,
                    local_backup_path: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the cloud save restore: {e}"))
}

pub fn record_cloud_save_restore(
    connection: &Connection,
    game_id: i64,
    snapshot_id: i64,
    restored_at: &str,
    local_backup_path: Option<&str>,
) -> Result<(), String> {
    connection
        .execute(
            r#"INSERT INTO cloud_save_restore
                   (game_id, snapshot_id, restored_at, local_backup_path)
               VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT(game_id) DO UPDATE SET
                   snapshot_id       = excluded.snapshot_id,
                   restored_at       = excluded.restored_at,
                   local_backup_path = excluded.local_backup_path"#,
            params![game_id, snapshot_id, restored_at, local_backup_path],
        )
        .map_err(|e| format!("Could not record the cloud save restore: {e}"))?;
    ensure_cloud_save_prefs(connection, game_id)?;
    connection
        .execute(
            "UPDATE cloud_save_prefs SET last_restore_at = ?2, ignored_remote_hash = NULL
             WHERE game_id = ?1",
            params![game_id, restored_at],
        )
        .map_err(|e| format!("Could not record the cloud save restore: {e}"))?;
    Ok(())
}

/// Adopt the restored snapshot as the new upload baseline, so the content we
/// just wrote is not immediately uploaded again as a new snapshot.
pub fn record_cloud_save_restored_hash(
    connection: &Connection,
    game_id: i64,
    hash: &str,
) -> Result<(), String> {
    ensure_cloud_save_prefs(connection, game_id)?;
    connection
        .execute(
            "UPDATE cloud_save_prefs SET last_hash = ?2 WHERE game_id = ?1",
            params![game_id, hash],
        )
        .map_err(|e| format!("Could not record the cloud save baseline: {e}"))?;
    Ok(())
}

/// Record one config write (#105). The backup file already exists when this is
/// called; the row is what the Settings tab lists and what restore validates.
pub fn insert_config_edit(
    connection: &Connection,
    record: &ConfigEditRecord,
) -> Result<(), String> {
    connection
        .execute(
            r#"INSERT INTO config_edits
                   (id, game_id, file_path, backup_path, diff, bytes, origin, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            params![
                record.id,
                record.game_id,
                record.file_path,
                record.backup_path,
                record.diff,
                record.bytes,
                record.origin,
                record.created_at,
            ],
        )
        .map_err(|e| format!("Could not record the config edit: {e}"))?;
    Ok(())
}

fn map_config_edit(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConfigEditRecord> {
    Ok(ConfigEditRecord {
        id: row.get(0)?,
        game_id: row.get(1)?,
        file_path: row.get(2)?,
        backup_path: row.get(3)?,
        diff: row.get(4)?,
        bytes: row.get(5)?,
        origin: row.get(6)?,
        created_at: row.get(7)?,
    })
}

pub fn list_config_edits(
    connection: &Connection,
    game_id: i64,
    limit: usize,
) -> Result<Vec<ConfigEditRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, game_id, file_path, backup_path, diff, bytes, origin, created_at
             FROM config_edits WHERE game_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT ?2",
        )
        .map_err(|e| format!("Could not read the config history: {e}"))?;
    let rows = statement
        .query_map(params![game_id, limit as i64], map_config_edit)
        .map_err(|e| format!("Could not read the config history: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("Could not read the config history: {e}"))
}

pub fn get_config_edit(
    connection: &Connection,
    game_id: i64,
    id: &str,
) -> Result<Option<ConfigEditRecord>, String> {
    connection
        .query_row(
            "SELECT id, game_id, file_path, backup_path, diff, bytes, origin, created_at
             FROM config_edits WHERE game_id = ?1 AND id = ?2",
            params![game_id, id],
            map_config_edit,
        )
        .optional()
        .map_err(|e| format!("Could not read the config edit: {e}"))
}

/// One pinned common field for a game (#148). `field` is `language` or
/// `playerName`; the row overrides the built-in catalog and always wins when
/// the file still contains that key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFieldMapping {
    pub game_id: i64,
    pub field: String,
    /// Relative to the install folder (`Saved/Config/Windows/Engine.ini`).
    #[serde(rename = "file")]
    pub file_path: String,
    pub key: String,
    pub created_at: String,
}

/// Per-game "keep in sync" state (#148). Values are the last ones the UI sent;
/// a launch prefers the current app language/profile name and falls back to
/// these when the desktop settings carry none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSyncPrefs {
    pub game_id: i64,
    pub enabled: bool,
    pub language: String,
    pub player_name: String,
    pub apply_language: bool,
    pub apply_player_name: bool,
    pub last_applied_at: Option<String>,
    pub updated_at: String,
}

pub fn list_config_field_mappings(
    connection: &Connection,
    game_id: i64,
) -> Result<Vec<ConfigFieldMapping>, String> {
    let mut statement = connection
        .prepare(
            "SELECT game_id, field, file_path, key, created_at
             FROM config_field_mappings WHERE game_id = ?1 ORDER BY field ASC",
        )
        .map_err(|e| format!("Could not read the pinned config fields: {e}"))?;
    let rows = statement
        .query_map(params![game_id], |row| {
            Ok(ConfigFieldMapping {
                game_id: row.get(0)?,
                field: row.get(1)?,
                file_path: row.get(2)?,
                key: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(|e| format!("Could not read the pinned config fields: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| format!("Could not read the pinned config fields: {e}"))
}

/// Pin `field` to one file/key for this game. Replaces a previous mapping.
pub fn save_config_field_mapping(
    connection: &Connection,
    game_id: i64,
    field: &str,
    file_path: &str,
    key: &str,
    created_at: &str,
) -> Result<(), String> {
    connection
        .execute(
            r#"INSERT INTO config_field_mappings (game_id, field, file_path, key, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5)
               ON CONFLICT(game_id, field) DO UPDATE SET
                   file_path = excluded.file_path,
                   key = excluded.key,
                   created_at = excluded.created_at"#,
            params![game_id, field, file_path, key, created_at],
        )
        .map_err(|e| format!("Could not pin the config field: {e}"))?;
    Ok(())
}

pub fn delete_config_field_mapping(
    connection: &Connection,
    game_id: i64,
    field: &str,
) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM config_field_mappings WHERE game_id = ?1 AND field = ?2",
            params![game_id, field],
        )
        .map_err(|e| format!("Could not unpin the config field: {e}"))?;
    Ok(())
}

const CONFIG_SYNC_COLUMNS: &str = "game_id, enabled, language, player_name, apply_language,
     apply_player_name, last_applied_at, updated_at";

fn map_config_sync_prefs(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConfigSyncPrefs> {
    Ok(ConfigSyncPrefs {
        game_id: row.get(0)?,
        enabled: row.get::<_, i64>(1)? != 0,
        language: row.get(2)?,
        player_name: row.get(3)?,
        apply_language: row.get::<_, i64>(4)? != 0,
        apply_player_name: row.get::<_, i64>(5)? != 0,
        last_applied_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

pub fn get_config_sync_prefs(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<ConfigSyncPrefs>, String> {
    connection
        .query_row(
            &format!("SELECT {CONFIG_SYNC_COLUMNS} FROM config_sync_prefs WHERE game_id = ?1"),
            params![game_id],
            map_config_sync_prefs,
        )
        .optional()
        .map_err(|e| format!("Could not read the config sync state: {e}"))
}

/// Store the per-game sync opt-in. A missing row is created on first save.
#[allow(clippy::too_many_arguments)]
pub fn save_config_sync_prefs(
    connection: &Connection,
    game_id: i64,
    enabled: bool,
    language: &str,
    player_name: &str,
    apply_language: bool,
    apply_player_name: bool,
    updated_at: &str,
) -> Result<ConfigSyncPrefs, String> {
    connection
        .execute(
            r#"INSERT INTO config_sync_prefs
                   (game_id, enabled, language, player_name, apply_language, apply_player_name, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(game_id) DO UPDATE SET
                   enabled           = excluded.enabled,
                   language          = excluded.language,
                   player_name       = excluded.player_name,
                   apply_language    = excluded.apply_language,
                   apply_player_name = excluded.apply_player_name,
                   updated_at        = excluded.updated_at"#,
            params![
                game_id,
                i64::from(enabled),
                language,
                player_name,
                i64::from(apply_language),
                i64::from(apply_player_name),
                updated_at,
            ],
        )
        .map_err(|e| format!("Could not save the config sync state: {e}"))?;
    get_config_sync_prefs(connection, game_id)?
        .ok_or_else(|| "Could not read the config sync state.".to_string())
}

pub fn mark_config_sync_applied(
    connection: &Connection,
    game_id: i64,
    applied_at: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE config_sync_prefs SET last_applied_at = ?2 WHERE game_id = ?1",
            params![game_id, applied_at],
        )
        .map_err(|e| format!("Could not record the config sync: {e}"))?;
    Ok(())
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
        connection.execute_batch(SCHEMA_V9).unwrap();
        connection.execute_batch(SCHEMA_V10).unwrap();
        connection.execute_batch(SCHEMA_V11).unwrap();
        connection.execute_batch(SCHEMA_V12).unwrap();
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
    fn imported_launcher_entries_persist_and_are_listed_by_launch_uri() {
        let connection = memory_database();
        for (game_id, source, uri) in [
            (
                -1_234_567_890i64,
                "epic",
                "com.epicgames.launcher://apps/a?action=launch",
            ),
            (
                -1_234_567_891i64,
                "gog",
                "goggalaxy://openGameView/1207666623",
            ),
            (12, "steam", "steam://rungameid/440"),
            (7, SOURCE_LUDEX, ""),
        ] {
            let game = InstalledGame {
                game_id,
                version_id: 0,
                title: format!("{source} game"),
                version: None,
                install_path: "/games/x".into(),
                executable: None,
                size_bytes: 0,
                installed_at: "2026-11-10T00:00:00Z".into(),
                last_launched_at: None,
                archive_hash: None,
                install_root: None,
                source: source.to_string(),
                launch_uri: (!uri.is_empty()).then(|| uri.to_string()),
            };
            upsert_installed(&connection, &game).unwrap();
        }

        let mut uris = launch_uris(&connection).unwrap();
        uris.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(uris.len(), 3, "{uris:?}");
        assert!(uris.iter().any(|(uri, game_id, source)| uri
            .starts_with("com.epicgames.launcher://")
            && *game_id == -1_234_567_890
            && source == "epic"));
        assert!(uris.iter().any(
            |(uri, _, source)| uri == "goggalaxy://openGameView/1207666623" && source == "gog"
        ));
        // The Ludex install has no launch URI and stays out of the import map.
        assert!(uris.iter().all(|(uri, _, _)| !uri.is_empty()));
    }

    #[test]
    fn disabling_a_launcher_source_never_touches_its_imported_rows() {
        // #155: source selection is a scan filter. The index keeps every row,
        // including the ones imported from a source that is now disabled.
        let connection = memory_database();
        let epic = InstalledGame {
            game_id: -1_234_567_890,
            version_id: 0,
            title: "Control".into(),
            version: None,
            install_path: "C:/Program Files/Epic Games/Control".into(),
            executable: None,
            size_bytes: 0,
            installed_at: "2026-11-10T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: None,
            source: "epic".into(),
            launch_uri: Some("com.epicgames.launcher://apps/control".into()),
        };
        upsert_installed(&connection, &epic).unwrap();

        let enabled = crate::launchers::LauncherSet::from_ids(["steam"]);
        assert!(!enabled.contains(crate::launchers::Launcher::Epic));

        // Nothing in the settings or scan path deletes rows: the Epic import is
        // still indexed and still listed by its launch URI.
        assert_eq!(launch_uris(&connection).unwrap().len(), 1);
        let stored = get_installed(&connection, epic.game_id).unwrap().unwrap();
        assert_eq!(stored.source, "epic");
        assert_eq!(stored.launch_uri, epic.launch_uri);

        // Re-enabling the source does not duplicate or drop it either.
        let enabled = crate::launchers::LauncherSet::from_ids(["steam", "epic"]);
        assert!(enabled.contains(crate::launchers::Launcher::Epic));
        assert_eq!(launch_uris(&connection).unwrap().len(), 1);
    }

    #[test]
    fn adopted_installs_persist_and_refuse_other_owners() {
        let connection = memory_database();
        let adopted = InstalledGame {
            game_id: 42,
            version_id: 0,
            title: "Adopted".into(),
            version: None,
            install_path: "/games/adopted".into(),
            executable: Some("/games/adopted/game.exe".into()),
            size_bytes: 2048,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some("/games".into()),
            source: SOURCE_ADOPTED.into(),
            launch_uri: None,
        };
        record_adoption(&connection, &adopted, false).unwrap();
        let loaded = get_installed(&connection, 42).unwrap().unwrap();
        assert_eq!(loaded.source, SOURCE_ADOPTED);
        assert_eq!(loaded.version_id, 0);
        assert_eq!(loaded.archive_hash, None);

        // Another game cannot claim the same folder.
        let mut other = adopted.clone();
        other.game_id = 43;
        other.install_path = "/games/adopted".into();
        let error = record_adoption(&connection, &other, false).unwrap_err();
        assert!(error.contains("already indexed"), "{error}");

        // A launcher row owns its folder: adoption is refused.
        let mut steam = adopted.clone();
        steam.game_id = 44;
        steam.install_path = "/steam/hades".into();
        steam.source = SOURCE_STEAM.into();
        steam.title = "Hades".into();
        upsert_installed(&connection, &steam).unwrap();
        let mut wants_steam_folder = adopted.clone();
        wants_steam_folder.game_id = 45;
        wants_steam_folder.install_path = "/steam/hades".into();
        let error = record_adoption(&connection, &wants_steam_folder, false).unwrap_err();
        assert!(error.contains("launcher"), "{error}");
    }

    #[test]
    fn record_adoption_needs_replace_for_a_ludex_row_but_not_for_an_adopted_one() {
        let connection = memory_database();
        let ludex = InstalledGame {
            game_id: 7,
            version_id: 3,
            title: "Doom".into(),
            version: Some("1.0".into()),
            install_path: "/games/doom/1.0".into(),
            executable: None,
            size_bytes: 10,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: Some("sha256-sample:abc".into()),
            install_root: Some("/games".into()),
            source: SOURCE_LUDEX.into(),
            launch_uri: None,
        };
        upsert_installed(&connection, &ludex).unwrap();

        let adopted = InstalledGame {
            game_id: 7,
            version_id: 0,
            title: "Doom".into(),
            version: None,
            install_path: "/mnt/gamevault/Doom".into(),
            executable: None,
            size_bytes: 20,
            installed_at: "2026-02-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some("/mnt/gamevault".into()),
            source: SOURCE_ADOPTED.into(),
            launch_uri: None,
        };

        let error = record_adoption(&connection, &adopted, false).unwrap_err();
        assert!(error.contains("installed through Ludex"), "{error}");
        record_adoption(&connection, &adopted, true).unwrap();
        assert_eq!(
            get_installed(&connection, 7).unwrap().unwrap().source,
            SOURCE_ADOPTED
        );

        // Re-adopting the same row (new folder) never needs replace.
        let mut moved = adopted.clone();
        moved.install_path = "/mnt/gamevault/Doom 2".into();
        record_adoption(&connection, &moved, false).unwrap();
        assert_eq!(
            get_installed(&connection, 7).unwrap().unwrap().install_path,
            "/mnt/gamevault/Doom 2"
        );
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
            stream_completed: None,
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
            stream_completed: None,
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
            stream_completed: None,
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

    #[test]
    fn cloud_save_prefs_default_to_inherit() {
        let connection = memory_database();
        let prefs = ensure_cloud_save_prefs(&connection, 9).unwrap();
        assert_eq!(prefs.game_id, 9);
        assert_eq!(prefs.auto_sync, None);
        assert_eq!(prefs.last_hash, None);
        assert_eq!(prefs.pending_attempts, 0);
        assert!(prefs.pending_error.is_none());

        // Calling ensure twice keeps the same row and does not reset state.
        record_cloud_save_upload(&connection, 9, "sha-a", "2026-11-10T10:00:00Z").unwrap();
        let again = ensure_cloud_save_prefs(&connection, 9).unwrap();
        assert_eq!(again.last_hash.as_deref(), Some("sha-a"));
        assert_eq!(
            again.last_upload_at.as_deref(),
            Some("2026-11-10T10:00:00Z")
        );
    }

    #[test]
    fn cloud_save_auto_sync_override_roundtrip() {
        let connection = memory_database();
        set_cloud_save_auto_sync(&connection, 4, Some(false)).unwrap();
        assert_eq!(
            get_cloud_save_prefs(&connection, 4)
                .unwrap()
                .unwrap()
                .auto_sync,
            Some(false)
        );
        set_cloud_save_auto_sync(&connection, 4, Some(true)).unwrap();
        assert_eq!(
            get_cloud_save_prefs(&connection, 4)
                .unwrap()
                .unwrap()
                .auto_sync,
            Some(true)
        );
        // `None` clears the override and goes back to the global setting.
        set_cloud_save_auto_sync(&connection, 4, None).unwrap();
        assert_eq!(
            get_cloud_save_prefs(&connection, 4)
                .unwrap()
                .unwrap()
                .auto_sync,
            None
        );
    }

    #[test]
    fn cloud_save_retry_bookkeeping_is_persisted() {
        let connection = memory_database();
        record_cloud_save_failure(
            &connection,
            7,
            1,
            Some("2026-11-10T10:00:30Z"),
            "server offline",
        )
        .unwrap();
        let prefs = get_cloud_save_prefs(&connection, 7).unwrap().unwrap();
        assert_eq!(prefs.pending_attempts, 1);
        assert_eq!(
            prefs.pending_next_at.as_deref(),
            Some("2026-11-10T10:00:30Z")
        );
        assert_eq!(prefs.pending_error.as_deref(), Some("server offline"));

        // Only games whose retry is due are returned.
        assert!(due_cloud_save_retries(&connection, "2026-11-10T10:00:00Z")
            .unwrap()
            .is_empty());
        assert_eq!(
            due_cloud_save_retries(&connection, "2026-11-10T10:00:30Z").unwrap(),
            vec![7]
        );

        // A second failure keeps the higher attempt count and a later slot.
        record_cloud_save_failure(
            &connection,
            7,
            2,
            Some("2026-11-10T10:01:30Z"),
            "still offline",
        )
        .unwrap();
        let prefs = get_cloud_save_prefs(&connection, 7).unwrap().unwrap();
        assert_eq!(prefs.pending_attempts, 2);
        assert_eq!(prefs.pending_error.as_deref(), Some("still offline"));

        // A successful upload clears the queue entry.
        record_cloud_save_upload(&connection, 7, "sha-b", "2026-11-10T10:02:00Z").unwrap();
        let prefs = get_cloud_save_prefs(&connection, 7).unwrap().unwrap();
        assert_eq!(prefs.pending_attempts, 0);
        assert!(prefs.pending_next_at.is_none());
        assert!(prefs.pending_error.is_none());
        assert!(due_cloud_save_retries(&connection, "2030-01-01T00:00:00Z")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn clearing_all_retries_stops_the_worker() {
        let connection = memory_database();
        for game_id in [1, 2] {
            record_cloud_save_failure(
                &connection,
                game_id,
                1,
                Some("2026-11-10T10:00:30Z"),
                "offline",
            )
            .unwrap();
        }
        assert_eq!(
            due_cloud_save_retries(&connection, "2026-11-10T11:00:00Z")
                .unwrap()
                .len(),
            2
        );
        clear_all_cloud_save_pending(&connection).unwrap();
        assert!(due_cloud_save_retries(&connection, "2026-11-10T11:00:00Z")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn ignored_remote_hash_is_cleared_by_upload_or_restore() {
        let connection = memory_database();
        record_cloud_save_ignored_remote(&connection, 11, Some("remote-sha")).unwrap();
        assert_eq!(
            get_cloud_save_prefs(&connection, 11)
                .unwrap()
                .unwrap()
                .ignored_remote_hash
                .as_deref(),
            Some("remote-sha")
        );

        // A fresh upload establishes a new baseline.
        record_cloud_save_upload(&connection, 11, "local-sha", "2026-11-10T10:00:00Z").unwrap();
        assert!(get_cloud_save_prefs(&connection, 11)
            .unwrap()
            .unwrap()
            .ignored_remote_hash
            .is_none());

        record_cloud_save_ignored_remote(&connection, 11, Some("remote-sha")).unwrap();
        record_cloud_save_restore(&connection, 11, 8, "2026-11-10T11:00:00Z", None).unwrap();
        assert!(get_cloud_save_prefs(&connection, 11)
            .unwrap()
            .unwrap()
            .ignored_remote_hash
            .is_none());
    }

    #[test]
    fn cloud_save_restore_is_recorded_once_per_snapshot() {
        let connection = memory_database();
        assert!(get_cloud_save_restore(&connection, 3).unwrap().is_none());
        record_cloud_save_restore(
            &connection,
            3,
            41,
            "2026-11-10T09:00:00Z",
            Some("/data/backups/game-3.zip"),
        )
        .unwrap();
        let restore = get_cloud_save_restore(&connection, 3).unwrap().unwrap();
        assert_eq!(restore.snapshot_id, 41);
        assert_eq!(
            restore.local_backup_path.as_deref(),
            Some("/data/backups/game-3.zip")
        );
        assert_eq!(
            get_cloud_save_prefs(&connection, 3)
                .unwrap()
                .unwrap()
                .last_restore_at
                .as_deref(),
            Some("2026-11-10T09:00:00Z")
        );

        record_cloud_save_restore(&connection, 3, 42, "2026-11-11T09:00:00Z", None).unwrap();
        let restore = get_cloud_save_restore(&connection, 3).unwrap().unwrap();
        assert_eq!(restore.snapshot_id, 42);
        assert!(restore.local_backup_path.is_none());
    }

    #[test]
    fn stream_checkpoints_roundtrip_and_prune() {
        let connection = memory_database();
        let mut job = job_record("stream-1", "downloading");
        job.stream = true;
        job.stream_completed = Some("0500".into());
        upsert_install_job(&connection, &job).unwrap();
        assert_eq!(
            get_install_job(&connection, "stream-1")
                .unwrap()
                .unwrap()
                .stream_completed
                .as_deref(),
            Some("0500")
        );

        let manifest = StreamManifestRecord {
            job_id: "stream-1".into(),
            game_id: 9,
            version_id: 91,
            format: "zip".into(),
            staging: "/games/.slug.stream-9-91".into(),
            entries: "[]".into(),
            entry_count: 12,
            total_bytes: 4096,
            fingerprint: "abc123".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        upsert_stream_manifest(&connection, &manifest).unwrap();
        let loaded = latest_stream_manifest_for_version(&connection, 9, 91)
            .unwrap()
            .expect("manifest");
        assert_eq!(loaded.fingerprint, "abc123");
        assert_eq!(loaded.entry_count, 12);
        assert_eq!(loaded.staging, "/games/.slug.stream-9-91");

        // Same version, newer job: the latest row wins and prune keeps only it.
        upsert_install_job(&connection, &job_record("stream-2", "downloading")).unwrap();
        let mut second = manifest.clone();
        second.job_id = "stream-2".into();
        second.fingerprint = "def456".into();
        second.created_at = "2026-01-02T00:00:00Z".into();
        upsert_stream_manifest(&connection, &second).unwrap();
        assert_eq!(
            latest_stream_manifest_for_version(&connection, 9, 91)
                .unwrap()
                .unwrap()
                .fingerprint,
            "def456"
        );
        assert_eq!(
            prune_stream_manifests(&connection, 9, 91, "stream-2").unwrap(),
            1
        );
        assert_eq!(
            latest_stream_manifest_for_version(&connection, 9, 91)
                .unwrap()
                .unwrap()
                .job_id,
            "stream-2"
        );

        // Removing a history row keeps the manifest: the staging files it
        // points at are still adoptable by a later install (#115).
        connection
            .execute(
                "UPDATE install_jobs SET phase = 'cancelled' WHERE job_id = 'stream-2'",
                [],
            )
            .unwrap();
        assert!(delete_install_job(&connection, "stream-2").unwrap());
        assert_eq!(
            latest_stream_manifest_for_version(&connection, 9, 91)
                .unwrap()
                .unwrap()
                .job_id,
            "stream-2"
        );
    }

    #[test]
    fn config_edits_are_listed_newest_first_and_scoped_to_the_game() {
        let connection = memory_database();
        let record = |id: &str, game_id: i64, created_at: &str| ConfigEditRecord {
            id: id.to_string(),
            game_id,
            file_path: "GameUserSettings.ini".to_string(),
            backup_path: format!("/data/config-backups/game-{game_id}/{id}.bak"),
            diff: "-bUseVSync=False\n+bUseVSync=True".to_string(),
            bytes: 120,
            origin: "adapter".to_string(),
            created_at: created_at.to_string(),
        };
        insert_config_edit(&connection, &record("a", 7, "2026-11-10T10:00:00Z")).unwrap();
        insert_config_edit(&connection, &record("b", 7, "2026-11-10T11:00:00Z")).unwrap();
        insert_config_edit(&connection, &record("c", 8, "2026-11-10T12:00:00Z")).unwrap();

        let edits = list_config_edits(&connection, 7, 10).unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].id, "b");
        assert_eq!(edits[1].id, "a");
        assert!(get_config_edit(&connection, 7, "b").unwrap().is_some());
        assert!(get_config_edit(&connection, 7, "c").unwrap().is_none());
        assert_eq!(
            get_config_edit(&connection, 7, "a")
                .unwrap()
                .unwrap()
                .file_path,
            "GameUserSettings.ini"
        );
    }

    #[test]
    fn common_field_mappings_are_per_game_and_replaceable() {
        let connection = memory_database();
        save_config_field_mapping(
            &connection,
            7,
            "language",
            "Saved/Config/Windows/Engine.ini",
            "Internationalization/Culture",
            "2026-11-11T10:00:00Z",
        )
        .unwrap();
        save_config_field_mapping(
            &connection,
            7,
            "playerName",
            "steam_emu.ini",
            "Settings/UserName",
            "2026-11-11T10:01:00Z",
        )
        .unwrap();
        save_config_field_mapping(
            &connection,
            8,
            "language",
            "goggame-1.info",
            "language",
            "2026-11-11T10:02:00Z",
        )
        .unwrap();

        let mappings = list_config_field_mappings(&connection, 7).unwrap();
        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[0].field, "language");
        assert_eq!(mappings[0].file_path, "Saved/Config/Windows/Engine.ini");
        assert_eq!(mappings[1].game_id, 7);

        // Re-pinning the same field replaces the previous key, it never stacks.
        save_config_field_mapping(
            &connection,
            7,
            "language",
            "steam_emu.ini",
            "Settings/Language",
            "2026-11-11T10:03:00Z",
        )
        .unwrap();
        let mappings = list_config_field_mappings(&connection, 7).unwrap();
        assert_eq!(mappings.len(), 2);
        let language = mappings
            .iter()
            .find(|mapping| mapping.field == "language")
            .unwrap();
        assert_eq!(language.file_path, "steam_emu.ini");
        assert_eq!(language.key, "Settings/Language");

        delete_config_field_mapping(&connection, 7, "language").unwrap();
        let mappings = list_config_field_mappings(&connection, 7).unwrap();
        assert_eq!(mappings.len(), 1);
        assert_eq!(list_config_field_mappings(&connection, 8).unwrap().len(), 1);
    }

    #[test]
    fn config_sync_prefs_round_trip_and_default_to_off() {
        let connection = memory_database();
        assert!(get_config_sync_prefs(&connection, 7).unwrap().is_none());

        let saved = save_config_sync_prefs(
            &connection,
            7,
            true,
            "fr",
            "cleeryy",
            true,
            false,
            "2026-11-11T10:00:00Z",
        )
        .unwrap();
        assert!(saved.enabled);
        assert_eq!(saved.language, "fr");
        assert_eq!(saved.player_name, "cleeryy");
        assert!(saved.apply_language);
        assert!(!saved.apply_player_name);
        assert_eq!(saved.last_applied_at, None);

        mark_config_sync_applied(&connection, 7, "2026-11-11T10:05:00Z").unwrap();
        let read = get_config_sync_prefs(&connection, 7).unwrap().unwrap();
        assert_eq!(
            read.last_applied_at.as_deref(),
            Some("2026-11-11T10:05:00Z")
        );

        let disabled = save_config_sync_prefs(
            &connection,
            7,
            false,
            "fr",
            "cleeryy",
            true,
            true,
            "2026-11-11T10:06:00Z",
        )
        .unwrap();
        assert!(!disabled.enabled);
        assert_eq!(
            disabled.last_applied_at.as_deref(),
            Some("2026-11-11T10:05:00Z")
        );
    }
}
