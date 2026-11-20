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
use crate::runtime::{RuntimeAuditEvent, RuntimeGamePrefs, RuntimeInstallRow};
use crate::saves::registry::SaveCandidate;

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

/// Normalise a path read from or written to the index (#187).
///
/// Windows `fs::canonicalize` returns the verbatim `\\?\` form, which Win32
/// fail-fasts on when used as `lpApplicationName`; stripping it at the storage
/// boundary keeps old rows and new writes in the same plain form. A no-op on
/// other platforms.
fn normalize_stored_path(value: &str) -> String {
    crate::paths::normalize_native_path(Path::new(value))
        .to_string_lossy()
        .to_string()
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
    /// Failed deliveries so far; drives the bounded backoff.
    pub attempts: i64,
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

const SCHEMA_VERSION: i64 = 28;

/// Pre-migration snapshots kept next to the database (`<name>-backup-<stamp>.db`).
const BACKUP_RETENTION: usize = 3;

/// Maximum failed deliveries for one playtime outbox message.
///
/// After this many failures the row keeps its minutes and its error but is no
/// longer retried automatically; a later launch does not hammer a dead server.
pub const PLAYTIME_SYNC_MAX_ATTEMPTS: i64 = 5;

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

/// Save-path candidates from the OS/engine registry (#146). They are separate
/// from `save_paths` on purpose: a candidate is never archived until the user
/// confirms it, and a dismissed candidate keeps its `ignored` flag across
/// detection runs. Host paths never leave this machine.
const SCHEMA_V13: &str = r#"
CREATE TABLE IF NOT EXISTS save_candidates (
    game_id       INTEGER NOT NULL,
    path          TEXT    NOT NULL,
    kind          TEXT    NOT NULL,
    root_id       TEXT    NOT NULL,
    subpath       TEXT    NOT NULL,
    source        TEXT    NOT NULL,
    confidence    TEXT    NOT NULL,
    evidence      TEXT    NOT NULL,
    size_bytes    INTEGER NOT NULL DEFAULT 0,
    last_modified TEXT,
    first_seen    TEXT    NOT NULL,
    last_verified TEXT,
    confirmed     INTEGER NOT NULL DEFAULT 0,
    ignored       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (game_id, path)
);

CREATE INDEX IF NOT EXISTS save_candidates_game_idx
    ON save_candidates (game_id, ignored);
"#;

/// Automatic promotion of save folders (#180). `save_candidates.auto_status`
/// records the last automatic decision for a candidate (`saving`, `waiting` or
/// `skipped`); `auto_reason` carries the readable reason for a skip.
///
/// `save_candidate_snapshots` is the pre-launch measurement used to prove which
/// guessed folder a session actually wrote to. It is replaced on every launch,
/// so an exit hook never compares against a stale session.
const SCHEMA_V14: &str = r#"
ALTER TABLE save_candidates ADD COLUMN auto_status TEXT NOT NULL DEFAULT '';
ALTER TABLE save_candidates ADD COLUMN auto_reason TEXT;

CREATE TABLE IF NOT EXISTS save_candidate_snapshots (
    game_id     INTEGER NOT NULL,
    path        TEXT    NOT NULL,
    existed     INTEGER NOT NULL DEFAULT 0,
    files       INTEGER NOT NULL DEFAULT 0,
    bytes       INTEGER NOT NULL DEFAULT 0,
    newest      TEXT,
    captured_at TEXT    NOT NULL,
    PRIMARY KEY (game_id, path)
);
"#;

/// Guided runtime installer (#178): exactly what Ludex installed on this
/// machine, which runtime each game selected, and a local audit trail.
///
/// `runtime_installs` is machine-wide because a runtime is shared by every
/// game. `runtime_game_prefs` is the per-game choice that later launches use.
/// The audit rows never leave this device.
const SCHEMA_V15: &str = r#"
CREATE TABLE IF NOT EXISTS runtime_installs (
    provider_id  TEXT    NOT NULL,
    platform     TEXT    NOT NULL,
    version      TEXT    NOT NULL,
    path         TEXT    NOT NULL DEFAULT '',
    sha256       TEXT    NOT NULL DEFAULT '',
    size_bytes   INTEGER NOT NULL DEFAULT 0,
    installed_at TEXT    NOT NULL,
    PRIMARY KEY (provider_id, platform)
);

CREATE TABLE IF NOT EXISTS runtime_game_prefs (
    game_id          INTEGER PRIMARY KEY,
    provider_id      TEXT    NOT NULL DEFAULT '',
    provider_version TEXT    NOT NULL DEFAULT '',
    prefix_path      TEXT    NOT NULL DEFAULT '',
    prerequisites    TEXT    NOT NULL DEFAULT '[]',
    updated_at       TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_audit (
    id          TEXT    PRIMARY KEY,
    game_id     INTEGER NOT NULL DEFAULT 0,
    provider_id TEXT    NOT NULL,
    version     TEXT    NOT NULL,
    action      TEXT    NOT NULL,
    outcome     TEXT    NOT NULL,
    detail      TEXT    NOT NULL,
    created_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS runtime_audit_game_idx
    ON runtime_audit (game_id, created_at);
"#;

/// Per-game launch working directory (#187). Empty keeps the automatic rule
/// (adopted: the executable's folder, otherwise the install root). The stored
/// path is normalised on read/write so a verbatim `\\?\` path from an older
/// build never reaches `Command::current_dir`.
const SCHEMA_V16: &str = r#"
ALTER TABLE game_launch_options ADD COLUMN working_dir TEXT NOT NULL DEFAULT '';
"#;

/// Resumable installs (#203/#205/#208): the job row keeps the staging folder
/// and whether a real checkpoint exists, and `install_job_params` stores the
/// request that created the job so `resume_install` can rebuild a worker after
/// an app restart instead of starting a second job from zero.
///
/// The token is deliberately absent: it is read from the settings/secret store
/// when the job resumes, so no credential is copied into the install table.
const SCHEMA_V17: &str = r#"
ALTER TABLE install_jobs ADD COLUMN staging_path TEXT;
ALTER TABLE install_jobs ADD COLUMN resumable INTEGER NOT NULL DEFAULT 0;
ALTER TABLE install_jobs ADD COLUMN entries_total INTEGER;

CREATE TABLE IF NOT EXISTS install_job_params (
    job_id              TEXT    PRIMARY KEY,
    game_id             INTEGER NOT NULL,
    version_id          INTEGER NOT NULL,
    title               TEXT    NOT NULL,
    version             TEXT,
    executable          TEXT,
    install_dir         TEXT,
    download_dir        TEXT,
    install_location_id TEXT,
    download_location_id TEXT,
    expected_bytes      INTEGER,
    stream              INTEGER NOT NULL DEFAULT 0,
    archive_format      TEXT,
    replace_path        TEXT,
    server_url          TEXT    NOT NULL,
    created_at          TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS install_job_params_version_idx
    ON install_job_params (game_id, version_id);
"#;

/// Install queue order (#202): a stable position per waiting/active job, so
/// "move to top" survives a restart and the scheduler starts the lowest
/// position when a slot frees. Terminal rows carry NULL, so a finished or
/// cancelled job never leaves a gap in the queue.
const SCHEMA_V18: &str = r#"
ALTER TABLE install_jobs ADD COLUMN queue_position INTEGER;
CREATE INDEX IF NOT EXISTS install_jobs_queue_idx
    ON install_jobs (phase, queue_position);
"#;

/// Verified archive digest (#216/#223): the job keeps the SHA-256 it proved so
/// Downloads can show the result after a restart without rehashing 96 GB.
const SCHEMA_V19: &str = r#"
ALTER TABLE install_jobs ADD COLUMN verified_sha256 TEXT;
"#;

/// File-manifest transport (#231): the identity of the archive a job's files
/// were derived from, plus one row per committed file. Both are keyed by job
/// id, exactly like the stream checkpoint (#115), so a restart that reuses the
/// job id continues where it stopped.
///
/// Nothing here records an archive on the client: the rows describe the files
/// written into the staging folder, and a changed `archive_sha256` invalidates
/// the whole checkpoint.
const SCHEMA_V20: &str = r#"
CREATE TABLE IF NOT EXISTS install_file_manifests (
    job_id         TEXT    PRIMARY KEY,
    game_id        INTEGER NOT NULL,
    version_id     INTEGER NOT NULL,
    archive_sha256 TEXT    NOT NULL,
    staging        TEXT    NOT NULL,
    total_bytes    INTEGER NOT NULL,
    file_count     INTEGER NOT NULL,
    created_at     TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS install_file_manifests_version_idx
    ON install_file_manifests (game_id, version_id);

CREATE TABLE IF NOT EXISTS install_job_files (
    job_id       TEXT    NOT NULL,
    path         TEXT    NOT NULL,
    size         INTEGER NOT NULL,
    sha256       TEXT    NOT NULL,
    completed_at TEXT    NOT NULL,
    PRIMARY KEY (job_id, path)
);
"#;

/// Startup crash-loop breaker (#234). `resume_markers` holds one row per job
/// the desktop resumed by itself: the row is written *before* the worker
/// starts and deleted once the app has been healthy for 15 s. A row still
/// present at the next launch means the previous run died with that job in
/// flight, so `resume_failures` is incremented. Two such launches in a row park
/// the job (`error`) with a manual-retry message instead of resuming it again.
///
/// Both tables are local bookkeeping only: nothing here is synced to the
/// server, and a delete never touches the `.part`/staging files.
const SCHEMA_V21: &str = r#"
CREATE TABLE IF NOT EXISTS resume_markers (
    job_id TEXT PRIMARY KEY,
    at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS resume_failures (
    job_id     TEXT    PRIMARY KEY,
    failures   INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT    NOT NULL
);
"#;

/// The digest of the archive an install manifest was built from (#240).
///
/// The row already holds the per-file SHA-256 of every installed file; this
/// column records the archive digest those files came from when it was proved
/// before extraction (#216/#223), so support can answer "which archive is this
/// install" without re-reading anything. It is recorded, not re-proved: a later
/// verification compares the installed files with the per-file manifest, which
/// is the check that detects corruption. `NULL` means the install predates the
/// column or came from a transport without an archive (per-entry stream).
const SCHEMA_V22: &str = r#"
ALTER TABLE install_manifests ADD COLUMN trusted_archive_sha256 TEXT;
"#;

/// The playtime outbox (Wave 2 data safety).
///
/// A finished session is written locally with `synced = 0` and only flipped to
/// 1 once the server acknowledged it — the table *is* the queue, so a crash
/// never loses minutes. These columns make a retry observable and bounded:
///
/// * `sync_attempts` counts failed deliveries (bounded budget, see
///   `PLAYTIME_SYNC_MAX_ATTEMPTS`);
/// * `sync_next_at` is the RFC 3339 instant of the next allowed attempt, so a
///   server outage backs off instead of hammering;
/// * `sync_error` keeps the last failure for the UI/support, never dropped
///   because the row still holds unsent playtime.
///
/// `synced = 1` rows keep their columns for history; `mark_session_synced`
/// clears them.
const SCHEMA_V23: &str = r#"
ALTER TABLE sessions ADD COLUMN sync_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN sync_next_at  TEXT;
ALTER TABLE sessions ADD COLUMN sync_error    TEXT;

CREATE INDEX IF NOT EXISTS sessions_sync_due_idx
    ON sessions (synced, sync_next_at);
"#;

/// Per-game display profile through Gamescope (#287).
///
/// Stored as one JSON object so the profile can grow without another
/// migration; `{}` is the desktop default and deserialises to a strict no-op.
/// Nothing here is synced to the server.
const SCHEMA_V24: &str = r#"
ALTER TABLE game_launch_options ADD COLUMN display TEXT NOT NULL DEFAULT '{}';
"#;

/// Move-to-another-library journal (#290).
///
/// `install_moves` survives an interruption: the source stays untouched, the
/// staging copy is verified against the manifest, and only the rename that
/// publishes the destination swaps the index. `verify_state` is `pending` on a
/// freshly moved install so its first launch runs the digest check before the
/// game starts.
const SCHEMA_V25: &str = r#"
CREATE TABLE IF NOT EXISTS install_moves (
    game_id     INTEGER PRIMARY KEY,
    source      TEXT NOT NULL,
    destination TEXT NOT NULL,
    staging     TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'copying',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
ALTER TABLE installed_games ADD COLUMN verify_state TEXT;
"#;

/// Per-game audio output (#289).
///
/// The sink name is the full PulseAudio/PipeWire name, stored verbatim; an
/// empty value means the game follows the profile default. Nothing here is
/// synced to the server: a sink name belongs to this machine.
const SCHEMA_V26: &str = r#"
ALTER TABLE game_launch_options ADD COLUMN audio_sink TEXT NOT NULL DEFAULT '';
"#;

/// Local per-session restore points of the save manager (#295).
///
/// Every game exit captures the save files into one bounded zip under
/// `{appDataDir}/restore-points/game-<id>/`; the row names the archive and
/// carries the hash, size, device and session so the panel can show a
/// one-click rollback with date, device and size. Retention keeps the newest
/// `saveRestoreRetention` sessions (default 5) and never removes the newest
/// point, which is the last known-good copy.
const SCHEMA_V27: &str = r#"
CREATE TABLE IF NOT EXISTS save_restore_points (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id     INTEGER NOT NULL,
    session_id  TEXT,
    source      TEXT    NOT NULL DEFAULT 'session',
    label       TEXT    NOT NULL DEFAULT '',
    file_name   TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    sha256      TEXT    NOT NULL DEFAULT '',
    device_id   TEXT,
    created_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS save_restore_points_game_idx
    ON save_restore_points (game_id, created_at DESC);

ALTER TABLE cloud_save_prefs ADD COLUMN last_local_change_at TEXT;
"#;

/// The offline save-sync queue (#296).
///
/// The conflict decision is deliberately **not** persisted: when an upload is
/// queued because the device is offline, only the remote state observed at
/// queue time is recorded. On reconnect the sync engine re-reads the server
/// and runs the policy again, so a snapshot that changed while the device was
/// offline is judged on its current bytes instead of a stale verdict.
const SCHEMA_V28: &str = r#"
ALTER TABLE cloud_save_prefs ADD COLUMN queued_remote_sha256 TEXT;
ALTER TABLE cloud_save_prefs ADD COLUMN queued_at TEXT;
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
    prepare_and_migrate(&connection, path)?;
    Ok(connection)
}

/// Refuses a downgrade, snapshots the file, then migrates.
///
/// Order matters: the version is read first (a newer file must never be
/// touched), the snapshot is taken before any statement runs (a failed
/// migration must always have a restore point), and only then does
/// [`migrate`] open its transaction.
fn prepare_and_migrate(connection: &Connection, path: &Path) -> Result<(), String> {
    let version = read_user_version(connection)?;
    if version > SCHEMA_VERSION {
        return Err(downgrade_error(path, version));
    }
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    backup_before_migration(connection, path, version)?;
    migrate(connection)
}

/// Clear, actionable message for a database written by a newer build.
///
/// This is the edge-channel trap: the updater can move a user forward and then
/// a stable build opens the same file. SQLite happily reads a newer schema, so
/// without this guard the old code would run queries against columns it does
/// not know and corrupt rows silently. Never auto-downgrade.
fn downgrade_error(path: &Path, version: i64) -> String {
    let location = path
        .parent()
        .map(|directory| directory.display().to_string())
        .unwrap_or_else(|| path.display().to_string());
    format!(
        "This library was opened by a newer Ludex (database version {version}, this build supports {SCHEMA_VERSION}). \
         Update Ludex to open it again. If you need to go back to an older build, restore one of the pre-migration \
         backups from {location} (files named {prefix}*.db) over {} or move it aside to start with an empty library.",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "the database".to_string()),
        version = version,
        prefix = backup_prefix(path),
    )
}

fn read_user_version(connection: &Connection) -> Result<i64, String> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| format!("Could not read the desktop database version: {e}"))
}

/// `ludex-desktop-backup-<stamp>.db` next to `desktop.db`.
fn backup_prefix(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "ludex".to_string());
    format!("{stem}-backup-")
}

/// In-memory databases have no file to snapshot (and `VACUUM INTO` would just
/// waste a write); tests use them heavily.
fn is_memory_database(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.is_empty()
        || text == ":memory:"
        || text.starts_with("file::memory:")
        || text.contains("mode=memory")
}

/// Snapshots the index with `VACUUM INTO` before a migration touches it.
///
/// Only the last [`BACKUP_RETENTION`] snapshots are kept. A failing backup
/// aborts the migration on purpose: migrating without a restore point is how a
/// half-applied schema becomes a permanent startup brick. A brand-new database
/// (`version < 1`) is not backed up — there is nothing to lose yet.
fn backup_before_migration(
    connection: &Connection,
    path: &Path,
    version: i64,
) -> Result<(), String> {
    if version < 1 || is_memory_database(path) {
        return Ok(());
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
    backup_with_stamp(connection, path, version, &stamp)
}

fn backup_with_stamp(
    connection: &Connection,
    path: &Path,
    version: i64,
    stamp: &str,
) -> Result<(), String> {
    let Some(directory) = path.parent() else {
        return Ok(());
    };
    let target = directory.join(format!(
        "{}{}-v{:04}.db",
        backup_prefix(path),
        stamp,
        version
    ));
    connection
        .execute("VACUUM INTO ?1", params![target.to_string_lossy()])
        .map_err(|e| {
            format!(
                "Could not back up the desktop database before upgrading it ({}): {e}",
                target.display()
            )
        })?;
    prune_backups(directory, &backup_prefix(path));
    Ok(())
}

/// Keeps the newest [`BACKUP_RETENTION`] snapshots, deleting older ones.
///
/// Pruning is best-effort: a snapshot that cannot be removed is not a reason to
/// fail startup.
fn prune_backups(directory: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut backups: Vec<std::path::PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().starts_with(prefix))
                .unwrap_or(false)
                && path.extension().is_some_and(|extension| extension == "db")
        })
        .collect();
    // Names start with the UTC stamp, so lexical order is chronological.
    backups.sort();
    while backups.len() > BACKUP_RETENTION {
        let oldest = backups.remove(0);
        let _ = std::fs::remove_file(oldest);
    }
}

/// One schema step. The list is append-only: bump [`SCHEMA_VERSION`] and push
/// an entry; never edit or reorder a shipped `SCHEMA_Vn`.
struct Migration {
    version: i64,
    sql: &'static str,
    /// Prefix of the user-facing error (`Could not <context>: <sqlite error>`).
    context: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: SCHEMA_V1,
        context: "create the desktop database schema",
    },
    Migration {
        version: 2,
        sql: SCHEMA_V2,
        context: "create the install job table",
    },
    Migration {
        version: 3,
        sql: SCHEMA_V3,
        context: "create the install manifest table",
    },
    Migration {
        version: 4,
        sql: SCHEMA_V4,
        context: "upgrade the desktop database for playtime",
    },
    Migration {
        version: 5,
        sql: SCHEMA_V5,
        context: "add external imports to the desktop database",
    },
    Migration {
        version: 6,
        sql: SCHEMA_V6,
        context: "add stream flags to the desktop database",
    },
    Migration {
        version: 7,
        sql: SCHEMA_V7,
        context: "add Linux compatibility tables",
    },
    Migration {
        version: 8,
        sql: SCHEMA_V8,
        context: "add save detection to the desktop database",
    },
    Migration {
        version: 9,
        sql: SCHEMA_V9,
        context: "add cloud save sync to the desktop database",
    },
    Migration {
        version: 10,
        sql: SCHEMA_V10,
        context: "add resumable stream checkpoints",
    },
    Migration {
        version: 11,
        sql: SCHEMA_V11,
        context: "add game config edits to the desktop database",
    },
    Migration {
        version: 12,
        sql: SCHEMA_V12,
        context: "add common config fields to the desktop database",
    },
    Migration {
        version: 13,
        sql: SCHEMA_V13,
        context: "add save candidates to the desktop database",
    },
    Migration {
        version: 14,
        sql: SCHEMA_V14,
        context: "add automatic save-folder promotion to the desktop database",
    },
    Migration {
        version: 15,
        sql: SCHEMA_V15,
        context: "add runtime installs to the desktop database",
    },
    Migration {
        version: 16,
        sql: SCHEMA_V16,
        context: "add the launch working directory to the desktop database",
    },
    Migration {
        version: 17,
        sql: SCHEMA_V17,
        context: "add resumable install parameters to the desktop database",
    },
    Migration {
        version: 18,
        sql: SCHEMA_V18,
        context: "add the install queue order to the desktop database",
    },
    Migration {
        version: 19,
        sql: SCHEMA_V19,
        context: "add the verified archive digest to the desktop database",
    },
    Migration {
        version: 20,
        sql: SCHEMA_V20,
        context: "add file-manifest checkpoints to the desktop database",
    },
    Migration {
        version: 21,
        sql: SCHEMA_V21,
        context: "add the startup resume circuit breaker to the desktop database",
    },
    Migration {
        version: 22,
        sql: SCHEMA_V22,
        context: "add the trusted archive digest to the desktop database",
    },
    Migration {
        version: 23,
        sql: SCHEMA_V23,
        context: "add the playtime outbox to the desktop database",
    },
    Migration {
        version: 24,
        sql: SCHEMA_V24,
        context: "add the per-game display profile to the desktop database",
    },
    Migration {
        version: 25,
        sql: SCHEMA_V25,
        context: "add the install-move journal and the moved-install verify state",
    },
    Migration {
        version: 26,
        sql: SCHEMA_V26,
        context: "add the per-game audio output to the desktop database",
    },
    Migration {
        version: 27,
        sql: SCHEMA_V27,
        context: "add the local save restore points of the save manager (#295)",
    },
    Migration {
        version: 28,
        sql: SCHEMA_V28,
        context: "add the offline save-sync queue to the desktop database",
    },
];

/// Applies every pending step in one transaction.
///
/// SQLite has transactional DDL, so a failure anywhere (disk full, a duplicate
/// column left by an older crash, a corrupted page) rolls back both the schema
/// and the `user_version` write. The next launch retries from the same
/// version instead of re-running an `ALTER TABLE` that already half-applied.
fn migrate(connection: &Connection) -> Result<(), String> {
    migrate_with(connection, MIGRATIONS, SCHEMA_VERSION)
}

fn migrate_with(
    connection: &Connection,
    migrations: &[Migration],
    target: i64,
) -> Result<(), String> {
    let version = read_user_version(connection)?;
    if version > SCHEMA_VERSION {
        // Same guard as `prepare_and_migrate`, for direct callers and tests.
        return Err(format!(
            "Refusing to migrate a desktop database at version {version}: this build supports {SCHEMA_VERSION}"
        ));
    }
    if version >= target {
        return Ok(());
    }
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| format!("Could not start the desktop database upgrade: {e}"))?;
    for migration in migrations {
        if migration.version <= target && version < migration.version {
            transaction
                .execute_batch(migration.sql)
                .map_err(|e| format!("Could not {}: {e}", migration.context))?;
        }
    }
    transaction
        .pragma_update(None, "user_version", target)
        .map_err(|e| format!("Could not record the desktop database version: {e}"))?;
    transaction
        .commit()
        .map_err(|e| format!("Could not commit the desktop database upgrade: {e}"))
}

/// A fresh in-memory index with every migration applied. Tests only.
#[cfg(test)]
pub(crate) fn open_in_memory_for_tests() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrate(&connection).unwrap();
    connection
}

pub fn upsert_installed(connection: &Connection, game: &InstalledGame) -> Result<(), String> {
    let install_path = normalize_stored_path(&game.install_path);
    let executable = game.executable.as_deref().map(normalize_stored_path);
    let install_root = game.install_root.as_deref().map(normalize_stored_path);
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
                install_path,
                executable,
                game.size_bytes,
                game.installed_at,
                game.last_launched_at,
                game.archive_hash,
                install_root,
                game.source,
                game.launch_uri,
            ],
        )
        .map_err(|e| format!("Could not record the installed game: {e}"))?;
    Ok(())
}

fn map_installed(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstalledGame> {
    let install_path: String = row.get(4)?;
    let executable: Option<String> = row.get(5)?;
    let install_root: Option<String> = row.get(10)?;
    Ok(InstalledGame {
        game_id: row.get(0)?,
        version_id: row.get(1)?,
        title: row.get(2)?,
        version: row.get(3)?,
        install_path: normalize_stored_path(&install_path),
        executable: executable.map(|value| normalize_stored_path(&value)),
        size_bytes: row.get(6)?,
        installed_at: row.get(7)?,
        last_launched_at: row.get(8)?,
        archive_hash: row.get(9)?,
        install_root: install_root.map(|value| normalize_stored_path(&value)),
        source: row.get(11)?,
        launch_uri: row.get(12)?,
    })
}

/// One in-flight move between libraries (#290).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallMove {
    pub game_id: i64,
    pub source: String,
    pub destination: String,
    pub staging: String,
    /// `copying`, `copied` (verified staging, not swapped) or `swapped`.
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Records (or updates) the move journal for a game.
pub fn upsert_install_move(connection: &Connection, record: &InstallMove) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_moves
                (game_id, source, destination, staging, state, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(game_id) DO UPDATE SET
                source      = excluded.source,
                destination = excluded.destination,
                staging     = excluded.staging,
                state       = excluded.state,
                updated_at  = excluded.updated_at
            "#,
            params![
                record.game_id,
                record.source,
                record.destination,
                record.staging,
                record.state,
                record.created_at,
                record.updated_at,
            ],
        )
        .map(|_| ())
        .map_err(|e| format!("Could not record the install move: {e}"))
}

/// The journal row for one game, when a move is in flight.
pub fn install_move(connection: &Connection, game_id: i64) -> Result<Option<InstallMove>, String> {
    connection
        .query_row(
            r#"
            SELECT game_id, source, destination, staging, state, created_at, updated_at
            FROM install_moves WHERE game_id = ?1
            "#,
            params![game_id],
            |row| {
                Ok(InstallMove {
                    game_id: row.get(0)?,
                    source: row.get(1)?,
                    destination: row.get(2)?,
                    staging: row.get(3)?,
                    state: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the install move: {e}"))
}

/// Every journal row (startup resume, #290).
pub fn pending_install_moves(connection: &Connection) -> Result<Vec<InstallMove>, String> {
    let mut statement = connection
        .prepare(
            r#"
            SELECT game_id, source, destination, staging, state, created_at, updated_at
            FROM install_moves ORDER BY game_id ASC
            "#,
        )
        .map_err(|e| format!("Could not read pending install moves: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(InstallMove {
                game_id: row.get(0)?,
                source: row.get(1)?,
                destination: row.get(2)?,
                staging: row.get(3)?,
                state: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })
        .map_err(|e| format!("Could not read pending install moves: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Could not read pending install moves: {e}"))
}

/// Forgets a finished (or abandoned) move.
pub fn delete_install_move(connection: &Connection, game_id: i64) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM install_moves WHERE game_id = ?1",
            params![game_id],
        )
        .map(|_| ())
        .map_err(|e| format!("Could not clear the install move: {e}"))
}

/// Sets (or clears) the pending verification of a moved install (#290).
pub fn set_install_verify_state(
    connection: &Connection,
    game_id: i64,
    state: Option<&str>,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE installed_games SET verify_state = ?2 WHERE game_id = ?1",
            params![game_id, state],
        )
        .map(|_| ())
        .map_err(|e| format!("Could not update the install verify state: {e}"))
}

/// The pending verification flag of an install, `None` when it is ready.
pub fn install_verify_state(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT verify_state FROM installed_games WHERE game_id = ?1",
            params![game_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(|value| value.flatten())
        .map_err(|e| format!("Could not read the install verify state: {e}"))
}

pub fn installed_game_ids(connection: &Connection) -> Result<Vec<i64>, String> {
    let mut statement = connection
        .prepare("SELECT game_id FROM installed_games ORDER BY game_id ASC")
        .map_err(|error| format!("Could not read installed games: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("Could not read installed games: {error}"))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|error| format!("Could not read installed games: {error}"))?);
    }
    Ok(ids)
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
    let executable = executable.map(normalize_stored_path);
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
            "SELECT mode, tool_id, prefix_path, env, winetricks_verbs, steam_app_id, working_dir,
                    display, audio_sink
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
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the launch options: {e}"))?;

    let Some((mode, tool_id, prefix_path, env, verbs, app_id, working_dir, display, audio_sink)) =
        row
    else {
        return Ok(LaunchOptions::default());
    };

    // Corrupt JSON must not make a game unlaunchable.
    let env = serde_json::from_str(&env).unwrap_or_default();
    let winetricks_verbs = serde_json::from_str(&verbs).unwrap_or_default();
    let display = serde_json::from_str(&display).unwrap_or_default();

    Ok(LaunchOptions {
        mode: parse_mode(&mode),
        tool_id,
        // #187: a prefix or working directory saved before the normalisation
        // fix can be verbatim; strip it before it reaches a process API.
        prefix_path: normalize_stored_path(&prefix_path),
        env,
        winetricks_verbs,
        steam_app_id: app_id.and_then(|value| u32::try_from(value).ok()),
        working_dir: normalize_stored_path(&working_dir),
        display,
        // #289: a sink name is a host-local opaque string, never a path.
        audio_sink: audio_sink.trim().to_string(),
        // #297: an in-memory wrapper flag, never read from or written to the
        // database; the plan sets it on the launch copy only.
        gamemode: false,
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
    let display = serde_json::to_string(&options.display)
        .map_err(|e| format!("Could not encode the display profile: {e}"))?;
    let prefix_path = normalize_stored_path(&options.prefix_path);
    let working_dir = normalize_stored_path(&options.working_dir);

    connection
        .execute(
            r#"
            INSERT INTO game_launch_options
                (game_id, mode, tool_id, prefix_path, env, winetricks_verbs, steam_app_id,
                 working_dir, display, audio_sink, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(game_id) DO UPDATE SET
                mode             = excluded.mode,
                tool_id          = excluded.tool_id,
                prefix_path      = excluded.prefix_path,
                env              = excluded.env,
                winetricks_verbs = excluded.winetricks_verbs,
                steam_app_id     = excluded.steam_app_id,
                working_dir      = excluded.working_dir,
                display          = excluded.display,
                audio_sink       = excluded.audio_sink,
                updated_at       = excluded.updated_at
            "#,
            params![
                game_id,
                options.mode.as_str(),
                options.tool_id,
                prefix_path,
                env,
                verbs,
                options.steam_app_id.map(i64::from),
                working_dir,
                display,
                options.audio_sink.trim(),
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
    pending_sessions_due(connection, &crate::util::now_rfc3339())
}

/// Sessions the outbox should deliver at `now` (RFC 3339, UTC).
///
/// A session enters the queue with `sync_attempts = 0`; after a failure it is
/// retried only once `sync_next_at` is due. Rows whose attempt budget is spent
/// (`sync_attempts` at the cap, `sync_next_at` cleared) are kept with their
/// `sync_error` for support but are never retried automatically.
pub fn pending_sessions_due(
    connection: &Connection,
    now: &str,
) -> Result<Vec<PendingSession>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, game_id, minutes, sync_attempts FROM sessions
             WHERE synced = 0 AND minutes > 0
               AND (sync_attempts = 0
                    OR (sync_next_at IS NOT NULL AND sync_next_at <= ?1))
             ORDER BY started_at ASC LIMIT 200",
        )
        .map_err(|e| format!("Could not read pending playtime: {e}"))?;

    let rows = statement
        .query_map(params![now], |row| {
            Ok(PendingSession {
                id: row.get(0)?,
                game_id: row.get(1)?,
                minutes: row.get(2)?,
                attempts: row.get(3)?,
            })
        })
        .map_err(|e| format!("Could not read pending playtime: {e}"))?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|e| format!("Could not read pending playtime: {e}"))?);
    }
    Ok(sessions)
}

/// Marks one outbox message as acknowledged and clears its retry state.
///
/// The `synced = 0` guard makes a late duplicate a no-op: two workers that
/// both delivered the same message cannot un-sync it.
pub fn mark_session_synced(connection: &Connection, id: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE sessions SET synced = 1, sync_next_at = NULL, sync_error = NULL
              WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Could not mark playtime as synced: {e}"))?;
    Ok(())
}

/// Persists a failed delivery for the bounded retry worker.
///
/// `next_attempt_at = None` means the budget is spent (or the server rejected
/// the message for good): the row keeps its error and is no longer picked up.
pub fn record_session_sync_failure(
    connection: &Connection,
    id: &str,
    attempts: i64,
    next_attempt_at: Option<&str>,
    error: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE sessions SET sync_attempts = ?2, sync_next_at = ?3, sync_error = ?4
              WHERE id = ?1 AND synced = 0",
            params![id, attempts.max(0), next_attempt_at, error],
        )
        .map_err(|e| format!("Could not record the playtime retry: {e}"))?;
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
    /// Staging folder of the attempt (#203). A stream job keeps it so a later
    /// resume adopts the files already written.
    pub staging_path: Option<String>,
    pub error: Option<String>,
    /// True when the job used a stream install (#114).
    pub stream: bool,
    /// True when a real checkpoint exists (`.part` file, completed stream
    /// entries, or a paused job), so Resume continues instead of restarting.
    pub resumable: bool,
    /// Hex bitset of completed entry indexes for a resumable stream install
    /// (#115). `None` for download/tar jobs.
    pub stream_completed: Option<String>,
    /// Total file entries of a per-entry stream install (#203).
    pub entries_total: Option<i64>,
    /// Position in the install queue (#202). `None` for terminal rows: a
    /// finished job leaves no gap behind.
    pub queue_position: Option<i64>,
    /// Verified full archive SHA-256 (#216/#223).
    pub verified_sha256: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn upsert_install_job(connection: &Connection, job: &InstallJobRecord) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_jobs
                (job_id, game_id, version_id, phase, bytes_done, bytes_total,
                 speed_bps, part_path, install_path, staging_path, error, stream,
                 resumable, stream_completed, entries_total, queue_position,
                 verified_sha256, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
            ON CONFLICT(job_id) DO UPDATE SET
                phase        = excluded.phase,
                bytes_done   = excluded.bytes_done,
                bytes_total  = excluded.bytes_total,
                speed_bps    = excluded.speed_bps,
                part_path    = excluded.part_path,
                install_path = excluded.install_path,
                staging_path = excluded.staging_path,
                error        = excluded.error,
                stream       = excluded.stream,
                resumable    = excluded.resumable,
                stream_completed = excluded.stream_completed,
                entries_total = excluded.entries_total,
                queue_position = excluded.queue_position,
                verified_sha256 = excluded.verified_sha256,
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
                job.staging_path,
                job.error,
                i64::from(job.stream),
                i64::from(job.resumable),
                job.stream_completed,
                job.entries_total,
                job.queue_position,
                job.verified_sha256,
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
        staging_path: row.get(9)?,
        error: row.get(10)?,
        stream: row.get::<_, i64>(11)? != 0,
        resumable: row.get::<_, i64>(12)? != 0,
        stream_completed: row.get(13)?,
        entries_total: row.get(14)?,
        queue_position: row.get(15)?,
        verified_sha256: row.get(16)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
    })
}

const INSTALL_JOB_COLUMNS: &str = "job_id, game_id, version_id, phase, bytes_done, bytes_total, speed_bps, part_path, install_path, staging_path, error, stream, resumable, stream_completed, entries_total, queue_position, verified_sha256, created_at, updated_at";

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

/// Jobs interrupted by the previous app run (#212), in queue order so the
/// scheduler restores the previous queue. `mark_interrupted_jobs` stamps them
/// with the startup message; a real failure carries another one. The legacy
/// v0.4.5 message is accepted too, so an upgrade resumes an in-flight job.
pub fn interrupted_install_jobs(
    connection: &Connection,
    message: &str,
    legacy_message: &str,
) -> Result<Vec<InstallJobRecord>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {INSTALL_JOB_COLUMNS} FROM install_jobs
                 WHERE phase = 'error' AND error IN (?1, ?2)
                 ORDER BY CASE WHEN queue_position IS NULL THEN 1 ELSE 0 END,
                          queue_position, created_at"
        ))
        .map_err(|e| format!("Could not read interrupted install jobs: {e}"))?;
    let rows = statement
        .query_map(params![message, legacy_message], map_install_job)
        .map_err(|e| format!("Could not read interrupted install jobs: {e}"))?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|e| format!("Could not read interrupted install jobs: {e}"))?);
    }
    Ok(records)
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

/// Most recent job for a version that can still be resumed (#205): paused,
/// failed, interrupted or cancelled. Used so a retry reuses the existing job
/// row instead of inserting a second history entry.
pub fn latest_resumable_job_for_version(
    connection: &Connection,
    game_id: i64,
    version_id: i64,
    stream: bool,
) -> Result<Option<InstallJobRecord>, String> {
    connection
        .query_row(
            &format!(
                "SELECT {INSTALL_JOB_COLUMNS} FROM install_jobs
                 WHERE game_id = ?1 AND version_id = ?2 AND stream = ?3
                   AND phase IN ('paused', 'error', 'cancelled', 'interrupted')
                 ORDER BY updated_at DESC LIMIT 1"
            ),
            params![game_id, version_id, i64::from(stream)],
            map_install_job,
        )
        .optional()
        .map_err(|e| format!("Could not read the install job: {e}"))
}

/// Row shape of `install_job_params` (#203/#208).
///
/// Everything `resume_install` needs to rebuild an `InstallParams` after an app
/// restart. The token is not stored: it is read from the secret store/settings
/// when the job resumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallJobParamsRecord {
    pub job_id: String,
    pub game_id: i64,
    pub version_id: i64,
    pub title: String,
    pub version: Option<String>,
    pub executable: Option<String>,
    pub install_dir: Option<String>,
    pub download_dir: Option<String>,
    pub install_location_id: Option<String>,
    pub download_location_id: Option<String>,
    pub expected_bytes: Option<i64>,
    pub stream: bool,
    pub archive_format: Option<String>,
    pub replace_path: Option<String>,
    pub server_url: String,
    pub created_at: String,
}

/// Stores the request of a job. Immutable except for the stream flag and the
/// archive format, which an explicit retry or the stream manifest can refine.
pub fn upsert_install_job_params(
    connection: &Connection,
    record: &InstallJobParamsRecord,
) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_job_params
                (job_id, game_id, version_id, title, version, executable,
                 install_dir, download_dir, install_location_id,
                 download_location_id, expected_bytes, stream, archive_format,
                 replace_path, server_url, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ON CONFLICT(job_id) DO UPDATE SET
                stream         = excluded.stream,
                archive_format = COALESCE(excluded.archive_format, install_job_params.archive_format)
            "#,
            params![
                record.job_id,
                record.game_id,
                record.version_id,
                record.title,
                record.version,
                record.executable,
                record.install_dir,
                record.download_dir,
                record.install_location_id,
                record.download_location_id,
                record.expected_bytes,
                i64::from(record.stream),
                record.archive_format,
                record.replace_path,
                record.server_url,
                record.created_at,
            ],
        )
        .map_err(|e| format!("Could not record the install parameters: {e}"))?;
    Ok(())
}

fn map_install_job_params(row: &rusqlite::Row<'_>) -> rusqlite::Result<InstallJobParamsRecord> {
    Ok(InstallJobParamsRecord {
        job_id: row.get(0)?,
        game_id: row.get(1)?,
        version_id: row.get(2)?,
        title: row.get(3)?,
        version: row.get(4)?,
        executable: row.get(5)?,
        install_dir: row.get(6)?,
        download_dir: row.get(7)?,
        install_location_id: row.get(8)?,
        download_location_id: row.get(9)?,
        expected_bytes: row.get(10)?,
        stream: row.get::<_, i64>(11)? != 0,
        archive_format: row.get(12)?,
        replace_path: row.get(13)?,
        server_url: row.get(14)?,
        created_at: row.get(15)?,
    })
}

const INSTALL_JOB_PARAMS_COLUMNS: &str = "job_id, game_id, version_id, title, version, executable, install_dir, download_dir, install_location_id, download_location_id, expected_bytes, stream, archive_format, replace_path, server_url, created_at";

pub fn get_install_job_params(
    connection: &Connection,
    job_id: &str,
) -> Result<Option<InstallJobParamsRecord>, String> {
    connection
        .query_row(
            &format!(
                "SELECT {INSTALL_JOB_PARAMS_COLUMNS} FROM install_job_params WHERE job_id = ?1"
            ),
            params![job_id],
            map_install_job_params,
        )
        .optional()
        .map_err(|e| format!("Could not read the install parameters: {e}"))
}

/// Records the archive format once the server manifest reports it (#203).
pub fn set_install_job_archive_format(
    connection: &Connection,
    job_id: &str,
    format: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE install_job_params SET archive_format = ?2 WHERE job_id = ?1",
            params![job_id, format],
        )
        .map_err(|e| format!("Could not record the archive format: {e}"))?;
    Ok(())
}

/// Rewrites the server base URL of a job whose local server moved to another
/// port between app runs (#212 follow-up). Only the origin changes: the token is
/// read from the settings/secret store when the job resumes.
pub fn set_install_job_server_url(
    connection: &Connection,
    job_id: &str,
    server_url: &str,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE install_job_params SET server_url = ?2 WHERE job_id = ?1",
            params![job_id, server_url],
        )
        .map_err(|e| format!("Could not record the install server URL: {e}"))?;
    Ok(())
}

pub fn delete_install_job_params(connection: &Connection, job_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM install_job_params WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Could not delete the install parameters: {e}"))?;
    Ok(())
}

/// Every stored install request, newest first.
///
/// Used by the startup staging sweep (#225) to find install folders that no
/// longer appear in the settings (a location removed while a job still pointed
/// at it).
pub fn all_install_job_params(
    connection: &Connection,
) -> Result<Vec<InstallJobParamsRecord>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {INSTALL_JOB_PARAMS_COLUMNS} FROM install_job_params ORDER BY created_at DESC"
        ))
        .map_err(|e| format!("Could not read the install parameters: {e}"))?;
    let rows = statement
        .query_map([], map_install_job_params)
        .map_err(|e| format!("Could not read the install parameters: {e}"))?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|e| format!("Could not read the install parameters: {e}"))?);
    }
    Ok(records)
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

/// Header of one file-manifest checkpoint (#231).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileManifestRecord {
    pub job_id: String,
    pub game_id: i64,
    pub version_id: i64,
    pub archive_sha256: String,
    pub staging: String,
    pub total_bytes: i64,
    pub file_count: i64,
    pub created_at: String,
}

/// Saves (or replaces) the manifest header of a file-manifest job.
pub fn upsert_file_manifest(
    connection: &Connection,
    record: &FileManifestRecord,
) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_file_manifests
                (job_id, game_id, version_id, archive_sha256, staging,
                 total_bytes, file_count, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(job_id) DO UPDATE SET
                version_id     = excluded.version_id,
                archive_sha256 = excluded.archive_sha256,
                staging        = excluded.staging,
                total_bytes    = excluded.total_bytes,
                file_count     = excluded.file_count
            "#,
            params![
                record.job_id,
                record.game_id,
                record.version_id,
                record.archive_sha256,
                record.staging,
                record.total_bytes,
                record.file_count,
                record.created_at,
            ],
        )
        .map_err(|e| format!("Could not record the file manifest: {e}"))?;
    Ok(())
}

fn map_file_manifest(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileManifestRecord> {
    Ok(FileManifestRecord {
        job_id: row.get(0)?,
        game_id: row.get(1)?,
        version_id: row.get(2)?,
        archive_sha256: row.get(3)?,
        staging: row.get(4)?,
        total_bytes: row.get(5)?,
        file_count: row.get(6)?,
        created_at: row.get(7)?,
    })
}

const FILE_MANIFEST_COLUMNS: &str =
    "job_id, game_id, version_id, archive_sha256, staging, total_bytes, file_count, created_at";

/// The file-manifest checkpoint of one job, when it has one.
pub fn get_file_manifest(
    connection: &Connection,
    job_id: &str,
) -> Result<Option<FileManifestRecord>, String> {
    connection
        .query_row(
            &format!(
                "SELECT {FILE_MANIFEST_COLUMNS} FROM install_file_manifests WHERE job_id = ?1"
            ),
            params![job_id],
            map_file_manifest,
        )
        .optional()
        .map_err(|e| format!("Could not read the file manifest: {e}"))
}

/// Most recent file manifest for a game version, whatever job wrote it. The
/// caller only adopts it when the archive SHA-256 still matches.
pub fn latest_file_manifest_for_version(
    connection: &Connection,
    game_id: i64,
    version_id: i64,
) -> Result<Option<FileManifestRecord>, String> {
    connection
        .query_row(
            &format!(
                "SELECT {FILE_MANIFEST_COLUMNS} FROM install_file_manifests
                 WHERE game_id = ?1 AND version_id = ?2
                 ORDER BY created_at DESC LIMIT 1"
            ),
            params![game_id, version_id],
            map_file_manifest,
        )
        .optional()
        .map_err(|e| format!("Could not read the file manifest: {e}"))
}

/// Drops the manifest header and every completed file of a job.
pub fn delete_file_manifest(connection: &Connection, job_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM install_job_files WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Could not clear the completed files: {e}"))?;
    connection
        .execute(
            "DELETE FROM install_file_manifests WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Could not delete the file manifest: {e}"))?;
    Ok(())
}

/// Deletes every manifest row for a game version except `keep_job_id`.
pub fn prune_file_manifests(
    connection: &Connection,
    game_id: i64,
    version_id: i64,
    keep_job_id: &str,
) -> Result<usize, String> {
    let stale: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT job_id FROM install_file_manifests
                 WHERE game_id = ?1 AND version_id = ?2 AND job_id <> ?3",
            )
            .map_err(|e| format!("Could not read the file manifests: {e}"))?;
        let rows = statement
            .query_map(params![game_id, version_id, keep_job_id], |row| row.get(0))
            .map_err(|e| format!("Could not read the file manifests: {e}"))?;
        rows.filter_map(Result::ok).collect()
    };
    for job_id in &stale {
        delete_file_manifest(connection, job_id)?;
    }
    Ok(stale.len())
}

/// Records one committed file. Called once per file, after its rename.
pub fn record_job_file(
    connection: &Connection,
    job_id: &str,
    path: &str,
    size: u64,
    sha256: &str,
    completed_at: &str,
) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_job_files (job_id, path, size, sha256, completed_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(job_id, path) DO UPDATE SET
                size         = excluded.size,
                sha256       = excluded.sha256,
                completed_at = excluded.completed_at
            "#,
            params![
                job_id,
                path,
                i64::try_from(size).unwrap_or(i64::MAX),
                sha256,
                completed_at,
            ],
        )
        .map_err(|e| format!("Could not record {path}: {e}"))?;
    Ok(())
}

/// Completed files of one job, as `(relative path, size)`.
pub fn completed_job_files(
    connection: &Connection,
    job_id: &str,
) -> Result<Vec<(String, u64)>, String> {
    let mut statement = connection
        .prepare("SELECT path, size FROM install_job_files WHERE job_id = ?1 ORDER BY path")
        .map_err(|e| format!("Could not read the completed files: {e}"))?;
    let rows = statement
        .query_map(params![job_id], |row| {
            let path: String = row.get(0)?;
            let size: i64 = row.get(1)?;
            Ok((path, size.max(0) as u64))
        })
        .map_err(|e| format!("Could not read the completed files: {e}"))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// Drops one completed file whose copy is no longer on disk.
pub fn forget_job_file(connection: &Connection, job_id: &str, path: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM install_job_files WHERE job_id = ?1 AND path = ?2",
            params![job_id, path],
        )
        .map_err(|e| format!("Could not forget {path}: {e}"))?;
    Ok(())
}

/// Drops every completed file of a job (a changed archive, or a fresh start).
pub fn clear_job_files(connection: &Connection, job_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM install_job_files WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Could not clear the completed files: {e}"))?;
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

/// Writes the compacted install queue order in one transaction (#202).
///
/// `positions` is the full assignment produced by the scheduler: every active
/// job keeps a unique position in `0..n`. `updated_at` is deliberately left
/// alone, so a reorder never looks like install progress.
pub fn set_install_queue_positions(
    connection: &Connection,
    positions: &[(String, i64)],
) -> Result<(), String> {
    if positions.is_empty() {
        return Ok(());
    }
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| format!("Could not record the install queue order: {e}"))?;
    {
        let mut statement = transaction
            .prepare("UPDATE install_jobs SET queue_position = ?2 WHERE job_id = ?1")
            .map_err(|e| format!("Could not record the install queue order: {e}"))?;
        for (job_id, position) in positions {
            statement
                .execute(params![job_id, position])
                .map_err(|e| format!("Could not record the install queue order: {e}"))?;
        }
    }
    transaction
        .commit()
        .map_err(|e| format!("Could not record the install queue order: {e}"))
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
    if affected > 0 {
        let _ = delete_install_job_params(connection, job_id);
        // #234: a removed job has no automatic resume left to protect.
        let _ = delete_resume_marker(connection, job_id);
        let _ = reset_resume_failures(connection, job_id);
    }
    // The stream manifest and its checkpoint are intentionally kept: they
    // describe files still present in the staging folder, so a later install
    // can adopt them even after the history row is gone (#115).
    Ok(affected > 0)
}

/// Remove every terminal row from Downloads history (#100). Returns the number
/// of rows deleted; install folders, archives and `.part` files stay.
pub fn clear_finished_install_jobs(connection: &Connection) -> Result<usize, String> {
    connection
        .execute(
            "DELETE FROM install_job_params WHERE job_id IN (SELECT job_id FROM install_jobs WHERE phase IN ('done', 'cancelled', 'error', 'interrupted'))",
            [],
        )
        .map_err(|e| format!("Could not clear the install parameters: {e}"))?;
    let affected = connection
        .execute(
            &format!("DELETE FROM install_jobs WHERE phase IN ({TERMINAL_INSTALL_PHASES})"),
            [],
        )
        .map_err(|e| format!("Could not clear finished install jobs: {e}"))?;
    // #234: the breaker only describes jobs that still exist.
    connection
        .execute(
            "DELETE FROM resume_markers WHERE job_id NOT IN (SELECT job_id FROM install_jobs)",
            [],
        )
        .map_err(|e| format!("Could not clear the startup resume markers: {e}"))?;
    connection
        .execute(
            "DELETE FROM resume_failures WHERE job_id NOT IN (SELECT job_id FROM install_jobs)",
            [],
        )
        .map_err(|e| format!("Could not clear the automatic resume failures: {e}"))?;
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
               AND phase IN ('queued', 'waiting_for_space', 'downloading', 'verifying', 'extracting', 'finalizing', 'paused')",
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
            "UPDATE install_jobs SET phase = 'error', error = ?1, updated_at = ?2, speed_bps = 0 WHERE phase IN ('queued', 'waiting_for_space', 'downloading', 'verifying', 'extracting', 'finalizing')",
            params![message, now],
        )
        .map_err(|e| format!("Could not mark interrupted install jobs: {e}"))
}

/// One `{job_id, at}` row written before an automatic resume (#234).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeMarker {
    pub job_id: String,
    pub at: String,
}

/// Record that this job is about to be resumed by the startup path (#234).
///
/// Must be committed before the worker starts: a row that survives to the next
/// launch is the only evidence that the previous launch died with this job.
pub fn set_resume_marker(connection: &Connection, job_id: &str, at: &str) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO resume_markers (job_id, at) VALUES (?1, ?2)
             ON CONFLICT(job_id) DO UPDATE SET at = excluded.at",
            params![job_id, at],
        )
        .map_err(|e| format!("Could not write the startup resume marker: {e}"))?;
    Ok(())
}

/// Every marker left by the previous launch, oldest first.
pub fn resume_markers(connection: &Connection) -> Result<Vec<ResumeMarker>, String> {
    let mut statement = connection
        .prepare("SELECT job_id, at FROM resume_markers ORDER BY at ASC")
        .map_err(|e| format!("Could not read the startup resume markers: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(ResumeMarker {
                job_id: row.get(0)?,
                at: row.get(1)?,
            })
        })
        .map_err(|e| format!("Could not read the startup resume markers: {e}"))?;
    let mut markers = Vec::new();
    for row in rows {
        markers.push(row.map_err(|e| format!("Could not read the startup resume markers: {e}"))?);
    }
    Ok(markers)
}

/// Delete one marker: the app stayed healthy, or the resume never started.
pub fn delete_resume_marker(connection: &Connection, job_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM resume_markers WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Could not clear the startup resume marker: {e}"))?;
    Ok(())
}

/// Count one unclean startup for this job and return the new total (#234).
///
/// A marker whose job no longer exists is dropped instead of counted: the user
/// removed the row, nothing can be resumed. `0` therefore means "not counted".
pub fn bump_resume_failure(
    connection: &Connection,
    job_id: &str,
    now: &str,
) -> Result<u32, String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM install_jobs WHERE job_id = ?1)",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Could not inspect the install job: {e}"))?;
    if !exists {
        delete_resume_marker(connection, job_id)?;
        return Ok(0);
    }
    connection
        .execute(
            "INSERT INTO resume_failures (job_id, failures, updated_at) VALUES (?1, 1, ?2)
             ON CONFLICT(job_id) DO UPDATE SET
                 failures   = resume_failures.failures + 1,
                 updated_at = excluded.updated_at",
            params![job_id, now],
        )
        .map_err(|e| format!("Could not record a failed automatic resume: {e}"))?;
    connection
        .query_row(
            "SELECT failures FROM resume_failures WHERE job_id = ?1",
            params![job_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|failures| failures.max(0) as u32)
        .map_err(|e| format!("Could not read the automatic resume failure count: {e}"))
}

/// Reconcile the markers left by the previous launch (#234).
///
/// A marker still present at startup means that launch auto-resumed the job and
/// never reached the healthy window: count one unclean startup for it and
/// consume the marker. Returns the new counts as `(job_id, failures)`, oldest
/// marker first, so the caller can park the jobs that reached the limit.
pub fn reconcile_resume_markers(
    connection: &Connection,
    now: &str,
) -> Result<Vec<(String, u32)>, String> {
    let markers = resume_markers(connection)?;
    let mut counts = Vec::new();
    for marker in markers {
        let failures = bump_resume_failure(connection, &marker.job_id, now)?;
        delete_resume_marker(connection, &marker.job_id)?;
        if failures > 0 {
            counts.push((marker.job_id, failures));
        }
    }
    Ok(counts)
}

/// Consecutive unclean startups recorded for this job.
#[cfg(test)]
pub fn resume_failures(connection: &Connection, job_id: &str) -> Result<u32, String> {
    connection
        .query_row(
            "SELECT failures FROM resume_failures WHERE job_id = ?1",
            params![job_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|failures| failures.unwrap_or(0).max(0) as u32)
        .map_err(|e| format!("Could not read the automatic resume failure count: {e}"))
}

/// A manual Resume proved the user wants this job: the streak restarts (#234).
pub fn reset_resume_failures(connection: &Connection, job_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM resume_failures WHERE job_id = ?1",
            params![job_id],
        )
        .map_err(|e| format!("Could not reset the automatic resume failure count: {e}"))?;
    Ok(())
}

/// Park a job instead of auto-resuming it again (#234).
///
/// The row becomes `error` with the breaker message, which
/// `interrupted_install_jobs` ignores: a parked job can only come back through
/// an explicit Resume. `.part`, staging and the parameters are untouched, so
/// that Resume still continues the download.
pub fn park_install_job(
    connection: &Connection,
    job_id: &str,
    message: &str,
    now: &str,
) -> Result<bool, String> {
    let affected = connection
        .execute(
            "UPDATE install_jobs
                SET phase = 'error', error = ?2, updated_at = ?3, speed_bps = 0,
                    queue_position = NULL
              WHERE job_id = ?1",
            params![job_id, message, now],
        )
        .map_err(|e| format!("Could not park the interrupted install job: {e}"))?;
    Ok(affected > 0)
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
    /// Archive digest the per-file hashes were derived from, when it was
    /// proved before extraction (#240). `None` for a legacy install or a
    /// transport with no archive.
    pub trusted_archive_sha256: Option<String>,
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
                (game_id, version_id, install_path, manifest, files, bytes,
                 trusted_archive_sha256, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(game_id) DO UPDATE SET
                version_id             = excluded.version_id,
                install_path           = excluded.install_path,
                manifest               = excluded.manifest,
                files                  = excluded.files,
                bytes                  = excluded.bytes,
                trusted_archive_sha256 = excluded.trusted_archive_sha256,
                updated_at             = excluded.updated_at
            "#,
            params![
                record.game_id,
                record.version_id,
                record.install_path,
                record.manifest,
                record.files,
                record.bytes,
                record.trusted_archive_sha256,
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
            "SELECT game_id, version_id, install_path, manifest, files, bytes,
                    trusted_archive_sha256, created_at, updated_at
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
                    trusted_archive_sha256: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
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

/* ------------------------------------------------------- save candidates */

const SAVE_CANDIDATE_COLUMNS: &str = "path, kind, root_id, subpath, source, confidence, evidence,
     size_bytes, last_modified, first_seen, last_verified, confirmed, ignored, auto_status, auto_reason";

fn map_save_candidate(row: &rusqlite::Row<'_>) -> rusqlite::Result<SaveCandidate> {
    Ok(SaveCandidate {
        path: row.get(0)?,
        kind: row.get(1)?,
        root_id: row.get(2)?,
        subpath: row.get(3)?,
        source: row.get(4)?,
        confidence: row.get(5)?,
        evidence: row.get(6)?,
        size_bytes: row.get(7)?,
        last_modified: row.get(8)?,
        first_seen: row.get(9)?,
        last_verified: row.get(10)?,
        confirmed: row.get::<_, i64>(11)? != 0,
        ignored: row.get::<_, i64>(12)? != 0,
        auto_status: row.get(13)?,
        auto_reason: row.get(14)?,
    })
}

/// Every visible candidate for one game. Ignored rows stay in the database so
/// a re-scan does not resurface a path the user dismissed.
pub fn list_save_candidates(
    connection: &Connection,
    game_id: i64,
) -> Result<Vec<SaveCandidate>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {SAVE_CANDIDATE_COLUMNS} FROM save_candidates
             WHERE game_id = ?1 AND ignored = 0
             ORDER BY CASE confidence WHEN 'certain' THEN 0 WHEN 'likely' THEN 1 ELSE 2 END, path"
        ))
        .map_err(|e| format!("Could not read save candidates: {e}"))?;
    let rows = statement
        .query_map(params![game_id], map_save_candidate)
        .map_err(|e| format!("Could not read save candidates: {e}"))?;
    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row.map_err(|e| format!("Could not read save candidates: {e}"))?);
    }
    Ok(candidates)
}

/// Candidates grouped by installed game for Settings → Saves. Only games with
/// at least one visible candidate are returned.
pub fn list_save_candidate_groups(
    connection: &Connection,
) -> Result<Vec<(i64, String, Vec<SaveCandidate>)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT c.game_id, g.title, c.path, c.kind, c.root_id, c.subpath, c.source,
                    c.confidence, c.evidence, c.size_bytes, c.last_modified,
                    c.first_seen, c.last_verified, c.confirmed, c.ignored,
                    c.auto_status, c.auto_reason
             FROM save_candidates c
             JOIN installed_games g ON g.game_id = c.game_id
             WHERE c.ignored = 0
             ORDER BY g.title COLLATE NOCASE ASC, c.game_id,
                      CASE c.confidence WHEN 'certain' THEN 0 WHEN 'likely' THEN 1 ELSE 2 END,
                      c.path",
        )
        .map_err(|e| format!("Could not read save candidates: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                SaveCandidate {
                    path: row.get(2)?,
                    kind: row.get(3)?,
                    root_id: row.get(4)?,
                    subpath: row.get(5)?,
                    source: row.get(6)?,
                    confidence: row.get(7)?,
                    evidence: row.get(8)?,
                    size_bytes: row.get(9)?,
                    last_modified: row.get(10)?,
                    first_seen: row.get(11)?,
                    last_verified: row.get(12)?,
                    confirmed: row.get::<_, i64>(13)? != 0,
                    ignored: row.get::<_, i64>(14)? != 0,
                    auto_status: row.get(15)?,
                    auto_reason: row.get(16)?,
                },
            ))
        })
        .map_err(|e| format!("Could not read save candidates: {e}"))?;

    let mut groups: Vec<(i64, String, Vec<SaveCandidate>)> = Vec::new();
    for row in rows {
        let (game_id, title, candidate) =
            row.map_err(|e| format!("Could not read save candidates: {e}"))?;
        match groups.last_mut() {
            Some((last_id, _, candidates)) if *last_id == game_id => {
                candidates.push(candidate);
            }
            _ => groups.push((game_id, title, vec![candidate])),
        }
    }
    Ok(groups)
}

pub fn get_save_candidate(
    connection: &Connection,
    game_id: i64,
    path: &str,
) -> Result<Option<SaveCandidate>, String> {
    connection
        .query_row(
            &format!(
                "SELECT {SAVE_CANDIDATE_COLUMNS} FROM save_candidates
                 WHERE game_id = ?1 AND path = ?2"
            ),
            params![game_id, path],
            map_save_candidate,
        )
        .optional()
        .map_err(|e| format!("Could not read the save candidate: {e}"))
}

/// Persist a discovery run. `first_seen`, `confirmed` and `ignored` are
/// preserved for a path that already existed; rows that are no longer detected
/// and were never confirmed are removed.
pub fn upsert_save_candidates(
    connection: &Connection,
    game_id: i64,
    candidates: &[SaveCandidate],
    now: &str,
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| format!("Could not update the save candidates: {e}"))?;

    for candidate in candidates {
        let first_seen = candidate
            .first_seen
            .clone()
            .unwrap_or_else(|| now.to_string());
        let last_verified = candidate
            .last_verified
            .clone()
            .unwrap_or_else(|| now.to_string());
        transaction
            .execute(
                r#"INSERT INTO save_candidates
                       (game_id, path, kind, root_id, subpath, source, confidence, evidence,
                        size_bytes, last_modified, first_seen, last_verified, confirmed, ignored,
                        auto_status, auto_reason)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                   ON CONFLICT(game_id, path) DO UPDATE SET
                       kind          = excluded.kind,
                       root_id       = excluded.root_id,
                       subpath       = excluded.subpath,
                       source        = excluded.source,
                       confidence    = excluded.confidence,
                       evidence      = excluded.evidence,
                       size_bytes    = excluded.size_bytes,
                       last_modified = excluded.last_modified,
                       last_verified = excluded.last_verified,
                       confirmed     = CASE WHEN excluded.confirmed = 1
                                            THEN 1 ELSE save_candidates.confirmed END,
                       auto_status   = CASE WHEN excluded.auto_status <> ''
                                            THEN excluded.auto_status
                                            ELSE save_candidates.auto_status END,
                       auto_reason   = CASE WHEN excluded.auto_status <> ''
                                            THEN excluded.auto_reason
                                            ELSE save_candidates.auto_reason END"#,
                params![
                    game_id,
                    candidate.path,
                    candidate.kind,
                    candidate.root_id,
                    candidate.subpath,
                    candidate.source,
                    candidate.confidence,
                    candidate.evidence,
                    candidate.size_bytes,
                    candidate.last_modified,
                    first_seen,
                    last_verified,
                    i64::from(candidate.confirmed),
                    i64::from(candidate.ignored),
                    candidate.auto_status,
                    candidate.auto_reason,
                ],
            )
            .map_err(|e| format!("Could not record the save candidate: {e}"))?;
    }

    if candidates.is_empty() {
        transaction
            .execute(
                "DELETE FROM save_candidates WHERE game_id = ?1 AND confirmed = 0",
                params![game_id],
            )
            .map_err(|e| format!("Could not update the save candidates: {e}"))?;
    } else {
        let mut statement = String::from(
            "DELETE FROM save_candidates WHERE game_id = ?1 AND confirmed = 0 AND path NOT IN (",
        );
        for index in 0..candidates.len() {
            if index > 0 {
                statement.push_str(", ");
            }
            statement.push_str(&format!("?{}", index + 2));
        }
        statement.push(')');
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&game_id];
        for candidate in candidates {
            values.push(&candidate.path);
        }
        transaction
            .execute(&statement, values.as_slice())
            .map_err(|e| format!("Could not update the save candidates: {e}"))?;
    }

    transaction
        .commit()
        .map_err(|e| format!("Could not update the save candidates: {e}"))
}

pub fn set_save_candidate_confirmed(
    connection: &Connection,
    game_id: i64,
    path: &str,
    confirmed: bool,
) -> Result<bool, String> {
    let affected = connection
        .execute(
            "UPDATE save_candidates SET confirmed = ?1 WHERE game_id = ?2 AND path = ?3",
            params![i64::from(confirmed), game_id, path],
        )
        .map_err(|e| format!("Could not update the save candidate: {e}"))?;
    Ok(affected > 0)
}

pub fn set_save_candidate_ignored(
    connection: &Connection,
    game_id: i64,
    path: &str,
    ignored: bool,
) -> Result<bool, String> {
    let affected = connection
        .execute(
            "UPDATE save_candidates SET ignored = ?1 WHERE game_id = ?2 AND path = ?3",
            params![i64::from(ignored), game_id, path],
        )
        .map_err(|e| format!("Could not update the save candidate: {e}"))?;
    Ok(affected > 0)
}

/// Paths the user dismissed for one game (#180). They are invisible in the
/// normal list but still guard automatic promotion.
pub fn list_ignored_save_candidate_paths(
    connection: &Connection,
    game_id: i64,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("SELECT path FROM save_candidates WHERE game_id = ?1 AND ignored = 1")
        .map_err(|e| format!("Could not read save candidates: {e}"))?;
    let rows = statement
        .query_map(params![game_id], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Could not read save candidates: {e}"))?;
    let mut paths = Vec::new();
    for row in rows {
        paths.push(row.map_err(|e| format!("Could not read save candidates: {e}"))?);
    }
    Ok(paths)
}

/// Record the last automatic promotion decision for one candidate (#180).
pub fn set_save_candidate_status(
    connection: &Connection,
    game_id: i64,
    path: &str,
    status: &str,
    reason: Option<&str>,
) -> Result<bool, String> {
    let affected = connection
        .execute(
            "UPDATE save_candidates SET auto_status = ?1, auto_reason = ?2
             WHERE game_id = ?3 AND path = ?4",
            params![status, reason, game_id, path],
        )
        .map_err(|e| format!("Could not update the save candidate status: {e}"))?;
    Ok(affected > 0)
}

/* ------------------------------------------------ save candidate snapshots */

/// Pre-launch measurement of one candidate (#180). Compared with the post-exit
/// scan to prove which guessed folder a session wrote to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSnapshotRecord {
    pub game_id: i64,
    pub path: String,
    /// The path existed when the session started. A folder created during the
    /// session is not promoted from that same session.
    pub existed: bool,
    pub files: u64,
    pub bytes: u64,
    pub newest: Option<String>,
    pub captured_at: String,
}

pub fn replace_candidate_snapshots(
    connection: &Connection,
    game_id: i64,
    records: &[CandidateSnapshotRecord],
) -> Result<(), String> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(|e| format!("Could not update the save candidate snapshots: {e}"))?;
    transaction
        .execute(
            "DELETE FROM save_candidate_snapshots WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not update the save candidate snapshots: {e}"))?;
    for record in records {
        transaction
            .execute(
                r#"INSERT INTO save_candidate_snapshots
                       (game_id, path, existed, files, bytes, newest, captured_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                   ON CONFLICT(game_id, path) DO UPDATE SET
                       existed     = excluded.existed,
                       files       = excluded.files,
                       bytes       = excluded.bytes,
                       newest      = excluded.newest,
                       captured_at = excluded.captured_at"#,
                params![
                    game_id,
                    record.path,
                    i64::from(record.existed),
                    i64::try_from(record.files).unwrap_or(i64::MAX),
                    i64::try_from(record.bytes).unwrap_or(i64::MAX),
                    record.newest,
                    record.captured_at,
                ],
            )
            .map_err(|e| format!("Could not record a save candidate snapshot: {e}"))?;
    }
    transaction
        .commit()
        .map_err(|e| format!("Could not update the save candidate snapshots: {e}"))
}

pub fn list_candidate_snapshots(
    connection: &Connection,
    game_id: i64,
) -> Result<Vec<CandidateSnapshotRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT game_id, path, existed, files, bytes, newest, captured_at
             FROM save_candidate_snapshots WHERE game_id = ?1 ORDER BY path",
        )
        .map_err(|e| format!("Could not read the save candidate snapshots: {e}"))?;
    let rows = statement
        .query_map(params![game_id], |row| {
            Ok(CandidateSnapshotRecord {
                game_id: row.get(0)?,
                path: row.get(1)?,
                existed: row.get::<_, i64>(2)? != 0,
                files: row.get::<_, i64>(3)?.max(0) as u64,
                bytes: row.get::<_, i64>(4)?.max(0) as u64,
                newest: row.get(5)?,
                captured_at: row.get(6)?,
            })
        })
        .map_err(|e| format!("Could not read the save candidate snapshots: {e}"))?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|e| format!("Could not read the save candidate snapshots: {e}"))?);
    }
    Ok(records)
}

pub fn clear_candidate_snapshots(connection: &Connection, game_id: i64) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM save_candidate_snapshots WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not clear the save candidate snapshots: {e}"))?;
    Ok(())
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
    /// Remote snapshot observed when an offline upload was queued (#296).
    /// Informational only: the decision is re-run on reconnect.
    pub queued_remote_sha256: Option<String>,
    /// When that queue entry was recorded (#296).
    pub queued_at: Option<String>,
}

const CLOUD_SAVE_PREF_COLUMNS: &str =
    "game_id, auto_sync, last_hash, last_upload_at, last_restore_at,
     pending_attempts, pending_next_at, pending_error, ignored_remote_hash,
     queued_remote_sha256, queued_at";

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
        queued_remote_sha256: row.get(9)?,
        queued_at: row.get(10)?,
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
                   ignored_remote_hash = NULL,
                   queued_remote_sha256 = NULL,
                   queued_at = NULL
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
                   pending_error = NULL,
                   queued_remote_sha256 = NULL,
                   queued_at = NULL
               WHERE game_id = ?1"#,
            params![game_id],
        )
        .map_err(|e| format!("Could not clear the cloud save retry: {e}"))?;
    Ok(())
}

/// Remember the remote snapshot an offline upload saw (#296).
///
/// Only the observation is stored, never the policy verdict: `sync_game` reads
/// the server again on reconnect and re-evaluates the conflict against the
/// bytes that exist at that moment. Queueing a conflict also clears the retry
/// backoff: there is nothing to retry until the user chooses a side.
pub fn record_cloud_save_queued(
    connection: &Connection,
    game_id: i64,
    remote_sha256: Option<&str>,
    queued_at: &str,
) -> Result<(), String> {
    ensure_cloud_save_prefs(connection, game_id)?;
    connection
        .execute(
            r#"UPDATE cloud_save_prefs SET
                   queued_remote_sha256 = ?2,
                   queued_at = ?3,
                   pending_attempts = 0,
                   pending_next_at = NULL,
                   pending_error = NULL
               WHERE game_id = ?1"#,
            params![game_id, remote_sha256, queued_at],
        )
        .map_err(|e| format!("Could not record the queued save sync: {e}"))?;
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
                   pending_error = NULL,
                   queued_remote_sha256 = NULL,
                   queued_at = NULL
               WHERE pending_error IS NOT NULL OR pending_next_at IS NOT NULL
                  OR queued_at IS NOT NULL"#,
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

/* ------------------------------------------------- save restore points */

/// One local save restore point (#295). The archive lives under
/// `{appDataDir}/restore-points/game-<game_id>/` and never leaves the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveRestorePoint {
    pub id: i64,
    pub game_id: i64,
    /// Launch session that produced the save, when the snapshot came from an
    /// exit hook rather than a manual capture.
    pub session_id: Option<String>,
    /// `session`, `manual` or `pre_restore`.
    pub source: String,
    /// Human label shown in the panel, empty for automatic captures.
    pub label: String,
    /// Archive file name inside the game's restore-point folder.
    pub file_name: String,
    pub size_bytes: i64,
    pub sha256: String,
    /// Opaque identifier of the device that wrote the save (#108).
    pub device_id: Option<String>,
    pub created_at: String,
}

/// Values needed to insert a restore point; `id` is assigned by SQLite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSaveRestorePoint {
    pub game_id: i64,
    pub session_id: Option<String>,
    pub source: String,
    pub label: String,
    pub file_name: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub device_id: Option<String>,
    pub created_at: String,
}

const SAVE_RESTORE_POINT_COLUMNS: &str = "id, game_id, session_id, source, label, file_name,
     size_bytes, sha256, device_id, created_at";

fn map_save_restore_point(row: &rusqlite::Row<'_>) -> rusqlite::Result<SaveRestorePoint> {
    Ok(SaveRestorePoint {
        id: row.get(0)?,
        game_id: row.get(1)?,
        session_id: row.get(2)?,
        source: row.get(3)?,
        label: row.get(4)?,
        file_name: row.get(5)?,
        size_bytes: row.get(6)?,
        sha256: row.get(7)?,
        device_id: row.get(8)?,
        created_at: row.get(9)?,
    })
}

pub fn insert_save_restore_point(
    connection: &Connection,
    point: &NewSaveRestorePoint,
) -> Result<SaveRestorePoint, String> {
    connection
        .execute(
            r#"INSERT INTO save_restore_points
                   (game_id, session_id, source, label, file_name, size_bytes, sha256,
                    device_id, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            params![
                point.game_id,
                point.session_id,
                point.source,
                point.label,
                point.file_name,
                point.size_bytes,
                point.sha256,
                point.device_id,
                point.created_at,
            ],
        )
        .map_err(|e| format!("Could not record the save restore point: {e}"))?;
    let id = connection.last_insert_rowid();
    get_save_restore_point(connection, id)?
        .ok_or_else(|| "The save restore point disappeared after it was written.".to_string())
}

pub fn get_save_restore_point(
    connection: &Connection,
    id: i64,
) -> Result<Option<SaveRestorePoint>, String> {
    connection
        .query_row(
            &format!("SELECT {SAVE_RESTORE_POINT_COLUMNS} FROM save_restore_points WHERE id = ?1"),
            params![id],
            map_save_restore_point,
        )
        .optional()
        .map_err(|e| format!("Could not read the save restore point: {e}"))
}

/// Restore points for one game, newest first.
pub fn list_save_restore_points(
    connection: &Connection,
    game_id: i64,
) -> Result<Vec<SaveRestorePoint>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {SAVE_RESTORE_POINT_COLUMNS} FROM save_restore_points
             WHERE game_id = ?1
             ORDER BY created_at DESC, id DESC"
        ))
        .map_err(|e| format!("Could not read the save restore points: {e}"))?;
    let rows = statement
        .query_map(params![game_id], map_save_restore_point)
        .map_err(|e| format!("Could not read the save restore points: {e}"))?;
    let mut points = Vec::new();
    for row in rows {
        points.push(row.map_err(|e| format!("Could not read the save restore points: {e}"))?);
    }
    Ok(points)
}

/// Newest restore point, used to skip an unchanged session.
pub fn latest_save_restore_point(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<SaveRestorePoint>, String> {
    Ok(list_save_restore_points(connection, game_id)?
        .into_iter()
        .next())
}

pub fn count_save_restore_points(connection: &Connection, game_id: i64) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM save_restore_points WHERE game_id = ?1",
            params![game_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Could not count the save restore points: {e}"))
}

pub fn delete_save_restore_point(connection: &Connection, id: i64) -> Result<bool, String> {
    let deleted = connection
        .execute("DELETE FROM save_restore_points WHERE id = ?1", params![id])
        .map_err(|e| format!("Could not delete the save restore point: {e}"))?;
    Ok(deleted > 0)
}

/// Retention pass: keep the newest `keep` restore points and return the rows
/// that were removed so the caller can delete their archives.
///
/// `keep` is clamped to at least one: the newest point is the last known-good
/// copy and must survive every cleanup. Pinning is not needed for that
/// guarantee, so the rule holds even after the database lost its setting.
pub fn prune_save_restore_points(
    connection: &Connection,
    game_id: i64,
    keep: i64,
) -> Result<Vec<SaveRestorePoint>, String> {
    let keep = keep.max(1);
    let points = list_save_restore_points(connection, game_id)?;
    let mut removed = Vec::new();
    for point in points
        .into_iter()
        .skip(usize::try_from(keep).unwrap_or(usize::MAX))
    {
        if delete_save_restore_point(connection, point.id)? {
            removed.push(point);
        }
    }
    Ok(removed)
}

/// Remember when this device last captured a save, including games whose save
/// paths are not configured (the panel can still show a capture date).
pub fn record_local_save_snapshot(
    connection: &Connection,
    game_id: i64,
    at: &str,
) -> Result<(), String> {
    ensure_cloud_save_prefs(connection, game_id)?;
    connection
        .execute(
            "UPDATE cloud_save_prefs SET last_local_change_at = ?2 WHERE game_id = ?1",
            params![game_id, at],
        )
        .map_err(|e| format!("Could not record the local save change: {e}"))?;
    Ok(())
}

pub fn last_local_save_change(
    connection: &Connection,
    game_id: i64,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT last_local_change_at FROM cloud_save_prefs WHERE game_id = ?1",
            params![game_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(|value| value.flatten())
        .map_err(|e| format!("Could not read the local save change: {e}"))
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

/* ---------------------- Guided runtime installer (#178) ---------------------- */

pub fn list_runtime_installs(connection: &Connection) -> Result<Vec<RuntimeInstallRow>, String> {
    let mut statement = connection
        .prepare(
            "SELECT provider_id, platform, version, path, sha256, size_bytes, installed_at
             FROM runtime_installs ORDER BY provider_id",
        )
        .map_err(|e| format!("Could not read the runtime installs: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(RuntimeInstallRow {
                provider_id: row.get(0)?,
                platform: row.get(1)?,
                version: row.get(2)?,
                path: row.get(3)?,
                sha256: row.get(4)?,
                size_bytes: row.get::<_, i64>(5)?.max(0) as u64,
                installed_at: row.get(6)?,
            })
        })
        .map_err(|e| format!("Could not read the runtime installs: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Could not read the runtime installs: {e}"))
}

pub fn get_runtime_install(
    connection: &Connection,
    provider_id: &str,
) -> Result<Option<RuntimeInstallRow>, String> {
    connection
        .query_row(
            "SELECT provider_id, platform, version, path, sha256, size_bytes, installed_at
             FROM runtime_installs WHERE provider_id = ?1 ORDER BY installed_at DESC LIMIT 1",
            params![provider_id],
            |row| {
                Ok(RuntimeInstallRow {
                    provider_id: row.get(0)?,
                    platform: row.get(1)?,
                    version: row.get(2)?,
                    path: row.get(3)?,
                    sha256: row.get(4)?,
                    size_bytes: row.get::<_, i64>(5)?.max(0) as u64,
                    installed_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the runtime install: {e}"))
}

pub fn save_runtime_install(
    connection: &Connection,
    row: &RuntimeInstallRow,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO runtime_installs
                (provider_id, platform, version, path, sha256, size_bytes, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(provider_id, platform) DO UPDATE SET
                version = excluded.version,
                path = excluded.path,
                sha256 = excluded.sha256,
                size_bytes = excluded.size_bytes,
                installed_at = excluded.installed_at",
            params![
                row.provider_id,
                row.platform,
                row.version,
                row.path,
                row.sha256,
                row.size_bytes as i64,
                row.installed_at
            ],
        )
        .map_err(|e| format!("Could not save the runtime install: {e}"))?;
    Ok(())
}

pub fn delete_runtime_install(connection: &Connection, provider_id: &str) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM runtime_installs WHERE provider_id = ?1",
            params![provider_id],
        )
        .map_err(|e| format!("Could not remove the runtime install: {e}"))?;
    Ok(())
}

/// Per-game prefs, with a default row when the game has none yet.
pub fn get_runtime_prefs(
    connection: &Connection,
    game_id: i64,
) -> Result<RuntimeGamePrefs, String> {
    let stored = connection
        .query_row(
            "SELECT provider_id, provider_version, prefix_path, prerequisites, updated_at
             FROM runtime_game_prefs WHERE game_id = ?1",
            params![game_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("Could not read the runtime choice: {e}"))?;
    let Some((provider_id, provider_version, prefix_path, prerequisites, updated_at)) = stored
    else {
        return Ok(RuntimeGamePrefs {
            game_id,
            provider_id: String::new(),
            provider_version: String::new(),
            prefix_path: String::new(),
            prerequisites: Vec::new(),
            updated_at: String::new(),
        });
    };
    let prerequisites: Vec<String> = serde_json::from_str(&prerequisites).unwrap_or_default();
    Ok(RuntimeGamePrefs {
        game_id,
        provider_id,
        provider_version,
        prefix_path,
        prerequisites,
        updated_at,
    })
}

pub fn save_runtime_prefs(connection: &Connection, prefs: &RuntimeGamePrefs) -> Result<(), String> {
    let prerequisites = serde_json::to_string(&prefs.prerequisites)
        .map_err(|e| format!("Could not encode the prerequisites: {e}"))?;
    connection
        .execute(
            "INSERT INTO runtime_game_prefs
                (game_id, provider_id, provider_version, prefix_path, prerequisites, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(game_id) DO UPDATE SET
                provider_id = excluded.provider_id,
                provider_version = excluded.provider_version,
                prefix_path = excluded.prefix_path,
                prerequisites = excluded.prerequisites,
                updated_at = excluded.updated_at",
            params![
                prefs.game_id,
                prefs.provider_id,
                prefs.provider_version,
                prefs.prefix_path,
                prerequisites,
                prefs.updated_at
            ],
        )
        .map_err(|e| format!("Could not save the runtime choice: {e}"))?;
    Ok(())
}

pub fn record_runtime_audit(
    connection: &Connection,
    event: &RuntimeAuditEvent,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO runtime_audit
                (id, game_id, provider_id, version, action, outcome, detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.id,
                event.game_id,
                event.provider_id,
                event.version,
                event.action,
                event.outcome,
                event.detail,
                event.created_at
            ],
        )
        .map_err(|e| format!("Could not record the runtime audit entry: {e}"))?;
    Ok(())
}

pub fn list_runtime_audit(
    connection: &Connection,
    game_id: i64,
    limit: usize,
) -> Result<Vec<RuntimeAuditEvent>, String> {
    let limit = limit.clamp(1, 200) as i64;
    let mut statement = connection
        .prepare(
            "SELECT id, game_id, provider_id, version, action, outcome, detail, created_at
             FROM runtime_audit WHERE game_id = ?1
             ORDER BY created_at DESC, id DESC LIMIT ?2",
        )
        .map_err(|e| format!("Could not read the runtime audit: {e}"))?;
    let rows = statement
        .query_map(params![game_id, limit], |row| {
            Ok(RuntimeAuditEvent {
                id: row.get(0)?,
                game_id: row.get(1)?,
                provider_id: row.get(2)?,
                version: row.get(3)?,
                action: row.get(4)?,
                outcome: row.get(5)?,
                detail: row.get(6)?,
                created_at: row.get(7)?,
            })
        })
        .map_err(|e| format!("Could not read the runtime audit: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Could not read the runtime audit: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_database() -> Connection {
        // Always the real runner: a hand-written list of `SCHEMA_Vn` drifts the
        // moment a migration is added.
        open_in_memory_for_tests()
    }

    #[test]
    fn the_move_journal_round_trips_and_clears() {
        let connection = memory_database();
        let record = InstallMove {
            game_id: 12,
            source: "/games/doom/1.0".into(),
            destination: "/mnt/nas/games/doom/1.0".into(),
            staging: "/mnt/nas/games/doom/.1.0.ludex-move".into(),
            state: "copying".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        upsert_install_move(&connection, &record).unwrap();
        let stored = install_move(&connection, 12)
            .unwrap()
            .expect("a journal row");
        assert_eq!(stored.destination, record.destination);
        assert_eq!(pending_install_moves(&connection).unwrap().len(), 1);

        // A retry updates the state without duplicating the row.
        let mut updated = record.clone();
        updated.state = "copied".into();
        upsert_install_move(&connection, &updated).unwrap();
        assert_eq!(pending_install_moves(&connection).unwrap().len(), 1);
        assert_eq!(
            install_move(&connection, 12).unwrap().unwrap().state,
            "copied"
        );

        delete_install_move(&connection, 12).unwrap();
        assert!(install_move(&connection, 12).unwrap().is_none());
        assert!(pending_install_moves(&connection).unwrap().is_empty());
    }

    #[test]
    fn the_verify_state_is_pending_then_cleared() {
        let connection = memory_database();
        let game = InstalledGame {
            game_id: 21,
            version_id: 4,
            title: "Doom".into(),
            version: Some("1.0".into()),
            install_path: "/mnt/nas/games/doom/1.0".into(),
            executable: None,
            size_bytes: 2,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some("/mnt/nas/games".into()),
            source: SOURCE_LUDEX.into(),
            launch_uri: None,
        };
        upsert_installed(&connection, &game).unwrap();
        assert_eq!(install_verify_state(&connection, 21).unwrap(), None);

        set_install_verify_state(&connection, 21, Some("pending")).unwrap();
        assert_eq!(
            install_verify_state(&connection, 21).unwrap().as_deref(),
            Some("pending")
        );
        set_install_verify_state(&connection, 21, None).unwrap();
        assert_eq!(install_verify_state(&connection, 21).unwrap(), None);
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
            staging_path: Some("/tmp/game.staging".into()),
            error: None,
            stream: true,
            resumable: false,
            stream_completed: None,
            entries_total: Some(4),
            queue_position: Some(0),
            verified_sha256: None,
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
    fn interrupted_downloads_are_listed_in_queue_order_only() {
        let connection = memory_database();
        let record = |job_id: &str, phase: &str, error: Option<&str>, position: Option<i64>| {
            InstallJobRecord {
                job_id: job_id.into(),
                game_id: 1,
                version_id: 2,
                phase: phase.into(),
                bytes_done: 1024,
                bytes_total: Some(4096),
                speed_bps: 0,
                part_path: Some(format!("/tmp/{job_id}.part")),
                install_path: None,
                staging_path: None,
                error: error.map(str::to_string),
                stream: false,
                resumable: true,
                stream_completed: None,
                entries_total: None,
                queue_position: position,
                verified_sha256: None,
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
            }
        };
        // Two interrupted rows (one without a recorded slot) and one real
        // failure: only the interrupted ones may auto-resume.
        upsert_install_job(
            &connection,
            &record("job-b", "error", Some("Ludex closed early"), Some(1)),
        )
        .unwrap();
        upsert_install_job(
            &connection,
            &record("job-a", "error", Some("Ludex closed early"), Some(0)),
        )
        .unwrap();
        upsert_install_job(
            &connection,
            &record(
                "job-c",
                "error",
                Some("Download failed with HTTP 404"),
                Some(2),
            ),
        )
        .unwrap();
        upsert_install_job(&connection, &record("job-d", "paused", None, Some(3))).unwrap();

        let records =
            interrupted_install_jobs(&connection, "Ludex closed early", "Install again to resume")
                .unwrap();
        let ids: Vec<&str> = records.iter().map(|row| row.job_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["job-a", "job-b"],
            "queue order, no permanent error"
        );
    }

    #[test]
    fn install_queue_positions_persist_and_compact() {
        let connection = memory_database();

        // Schema v18 adds the column: an existing row without a position is
        // readable and sorts last.
        let columns: Vec<String> = connection
            .prepare("PRAGMA table_info(install_jobs)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|name| name.unwrap())
            .collect();
        assert!(columns.iter().any(|name| name == "queue_position"));

        for (job_id, position) in [("job-a", 0), ("job-b", 1), ("job-c", 2)] {
            let mut record = job_record(job_id, "queued");
            record.queue_position = Some(position);
            upsert_install_job(&connection, &record).unwrap();
        }
        // The order survives a reload: this is what `list_install_jobs` reads
        // after a restart.
        assert_eq!(
            get_install_job(&connection, "job-b")
                .unwrap()
                .unwrap()
                .queue_position,
            Some(1)
        );

        // Moving the third job to the top renumbers the queue without gaps.
        set_install_queue_positions(
            &connection,
            &[
                ("job-c".to_string(), 0),
                ("job-a".to_string(), 1),
                ("job-b".to_string(), 2),
            ],
        )
        .unwrap();
        let mut ordered: Vec<(String, i64)> = all_install_jobs(&connection)
            .unwrap()
            .into_iter()
            .map(|job| (job.job_id, job.queue_position.expect("active row")))
            .collect();
        ordered.sort_by_key(|(_, position)| *position);
        assert_eq!(
            ordered,
            vec![
                ("job-c".to_string(), 0),
                ("job-a".to_string(), 1),
                ("job-b".to_string(), 2),
            ]
        );

        // A finished row carries NULL and the remaining queue compacts to
        // 0..n, so the next start is the new lowest position.
        let mut done = job_record("job-c", "done");
        done.queue_position = None;
        upsert_install_job(&connection, &done).unwrap();
        set_install_queue_positions(
            &connection,
            &[("job-a".to_string(), 0), ("job-b".to_string(), 1)],
        )
        .unwrap();
        assert_eq!(
            get_install_job(&connection, "job-c")
                .unwrap()
                .unwrap()
                .queue_position,
            None
        );
        assert_eq!(
            get_install_job(&connection, "job-a")
                .unwrap()
                .unwrap()
                .queue_position,
            Some(0)
        );
        // An empty assignment is a no-op, not an error.
        set_install_queue_positions(&connection, &[]).unwrap();
    }

    #[test]
    fn install_job_params_roundtrip_and_resumable_lookup() {
        let connection = memory_database();
        let params = InstallJobParamsRecord {
            job_id: "job-params".into(),
            game_id: 12,
            version_id: 34,
            title: "Elden Ring".into(),
            version: Some("1.10".into()),
            executable: Some("Game/eldenring.exe".into()),
            install_dir: Some("/games".into()),
            download_dir: Some("/downloads".into()),
            install_location_id: Some("ssd".into()),
            download_location_id: None,
            expected_bytes: Some(69_200_000_000),
            stream: true,
            archive_format: None,
            replace_path: None,
            server_url: "https://ludex.example.com".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        upsert_install_job_params(&connection, &params).unwrap();
        let loaded = get_install_job_params(&connection, "job-params")
            .unwrap()
            .unwrap();
        assert_eq!(loaded, params);
        // The startup staging sweep (#225) reads every request to find install
        // folders that are no longer configured.
        let all = all_install_job_params(&connection).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].install_dir.as_deref(), Some("/games"));

        // The archive format is recorded once the manifest reports it and
        // never cleared by a later upsert that does not know it.
        set_install_job_archive_format(&connection, "job-params", "zip").unwrap();
        let mut again = params.clone();
        again.stream = true;
        upsert_install_job_params(&connection, &again).unwrap();
        assert_eq!(
            get_install_job_params(&connection, "job-params")
                .unwrap()
                .unwrap()
                .archive_format
                .as_deref(),
            Some("zip")
        );

        let mut failed = job_record("job-failed", "error");
        failed.game_id = 12;
        failed.version_id = 34;
        failed.stream = true;
        failed.resumable = true;
        failed.updated_at = "2026-01-02T00:00:00Z".into();
        upsert_install_job(&connection, &failed).unwrap();
        let mut done = failed.clone();
        done.job_id = "job-done".into();
        done.phase = "done".into();
        done.updated_at = "2026-01-03T00:00:00Z".into();
        upsert_install_job(&connection, &done).unwrap();

        let found = latest_resumable_job_for_version(&connection, 12, 34, true)
            .unwrap()
            .expect("failed job");
        assert_eq!(found.job_id, "job-failed");
        // A different stream mode must not adopt the row.
        assert!(latest_resumable_job_for_version(&connection, 12, 34, false)
            .unwrap()
            .is_none());

        delete_install_job_params(&connection, "job-params").unwrap();
        assert!(get_install_job_params(&connection, "job-params")
            .unwrap()
            .is_none());
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
        // A new database has nothing to lose: no snapshot is written.
        assert!(backup_files(&root).is_empty());
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
            trusted_archive_sha256: Some("a".repeat(64)),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        upsert_manifest(&connection, &record).unwrap();
        let loaded = get_manifest(&connection, 4).unwrap().unwrap();
        assert_eq!(loaded.install_path, "/games/doom/1.0");
        assert_eq!(
            loaded.trusted_archive_sha256,
            Some("a".repeat(64)),
            "the archive digest survives the round-trip"
        );

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
            staging_path: None,
            error: None,
            stream: false,
            resumable: true,
            stream_completed: None,
            entries_total: None,
            queue_position: None,
            verified_sha256: None,
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
            staging_path: None,
            error: None,
            stream: false,
            resumable: true,
            stream_completed: None,
            entries_total: None,
            queue_position: None,
            verified_sha256: None,
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
            working_dir: "/mnt/games/Uncharted".into(),
            display: crate::linux_gaming::DisplayProfile {
                mode: crate::linux_gaming::DisplayMode::Gamescope,
                width: 2560,
                height: 1440,
                refresh: 144,
                filter: "fsr".into(),
                adaptive_sync: true,
                hdr: false,
            },
            audio_sink: "alsa_output.pci-0000_01_00.1.hdmi-stereo".into(),
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
                "UPDATE game_launch_options SET env = 'not json', display = 'not json' WHERE game_id = 9",
                [],
            )
            .unwrap();
        let broken = get_launch_options(&connection, 9).unwrap();
        assert!(broken.env.is_empty());
        assert_eq!(broken.mode, LaunchMode::Proton);
        // #287: a corrupt display blob is the desktop default, never a failed launch.
        assert_eq!(
            broken.display,
            crate::linux_gaming::DisplayProfile::default()
        );
        // #289: the audio sink is a plain column and survives the bad JSON.
        assert_eq!(
            broken.audio_sink,
            "alsa_output.pci-0000_01_00.1.hdmi-stereo"
        );
    }

    /// #187: every stored path is normalised at the storage boundary, so a
    /// verbatim `\\?\` row from an older build (or a canonicalized path written
    /// on Windows) never reaches `Command::new`. The normaliser is a no-op off
    /// Windows, which is exactly what the expected value encodes.
    #[test]
    fn stored_paths_are_normalised_on_write_and_read() {
        let connection = memory_database();
        let raw_install = r"\\?\D:\Games\GameVault\Uncharted";
        let raw_exe = r"\\?\D:\Games\GameVault\Uncharted\u4.exe";
        let raw_root = r"\\?\D:\Games\GameVault";
        let game = InstalledGame {
            game_id: 42,
            version_id: 0,
            title: "Uncharted".into(),
            version: None,
            install_path: raw_install.into(),
            executable: Some(raw_exe.into()),
            size_bytes: 0,
            installed_at: "2026-11-12T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some(raw_root.into()),
            source: SOURCE_ADOPTED.into(),
            launch_uri: None,
        };
        upsert_installed(&connection, &game).unwrap();

        let expected_install = normalize_stored_path(raw_install);
        let expected_exe = normalize_stored_path(raw_exe);
        let expected_root = normalize_stored_path(raw_root);
        let stored = get_installed(&connection, 42).unwrap().unwrap();
        assert_eq!(stored.install_path, expected_install);
        assert_eq!(stored.executable.as_deref(), Some(expected_exe.as_str()));
        assert_eq!(stored.install_root.as_deref(), Some(expected_root.as_str()));

        // A path recorded later (#7 executable picker) goes through the same
        // normalisation.
        set_executable(&connection, 42, Some(raw_exe)).unwrap();
        let stored = get_installed(&connection, 42).unwrap().unwrap();
        assert_eq!(stored.executable.as_deref(), Some(expected_exe.as_str()));
    }

    /// #214: a repack install completes without an executable, then the guided
    /// flow records the program the setup produced. The stored value survives a
    /// reread unchanged (normalised like every other index path).
    #[test]
    fn repack_install_records_the_chosen_executable() {
        let connection = memory_database();
        let install_path = "/games/Hades/0.4.5";
        let game = InstalledGame {
            game_id: 77,
            version_id: 3,
            title: "Hades".into(),
            version: Some("0.4.5".into()),
            install_path: install_path.into(),
            executable: None,
            size_bytes: 0,
            installed_at: "2026-11-13T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some("/games/Hades".into()),
            source: SOURCE_LUDEX.into(),
            launch_uri: None,
        };
        upsert_installed(&connection, &game).unwrap();
        assert!(get_installed(&connection, 77)
            .unwrap()
            .unwrap()
            .executable
            .is_none());

        let chosen = format!("{install_path}/Hades.exe");
        set_executable(&connection, 77, Some(&chosen)).unwrap();
        let stored = get_installed(&connection, 77).unwrap().unwrap();
        let expected = normalize_stored_path(&chosen);
        assert_eq!(stored.executable.as_deref(), Some(expected.as_str()));
    }

    /// #187: the launch-option paths (Wine prefix, working directory) use the
    /// same boundary. A value that needs long-path support keeps its prefix.
    #[test]
    fn launch_option_paths_are_normalised() {
        let connection = memory_database();
        let mut options = LaunchOptions {
            prefix_path: r"\\?\D:\Prefixes\uncharted".into(),
            working_dir: r"\\?\D:\Games\Uncharted".into(),
            ..LaunchOptions::default()
        };
        save_launch_options(&connection, 7, &options, "2026-11-12T00:00:00Z").unwrap();
        let back = get_launch_options(&connection, 7).unwrap();
        assert_eq!(
            back.prefix_path,
            normalize_stored_path(&options.prefix_path)
        );
        assert_eq!(
            back.working_dir,
            normalize_stored_path(&options.working_dir)
        );

        // Long paths keep the verbatim prefix: stripping would break >MAX_PATH.
        let long_plain = format!(r"D:\Games\{}game.exe", "segment\\".repeat(40));
        let long_verbatim = format!(r"\\?\{}", long_plain);
        options.working_dir = long_verbatim.clone();
        save_launch_options(&connection, 7, &options, "2026-11-12T00:00:01Z").unwrap();
        let back = get_launch_options(&connection, 7).unwrap();
        assert_eq!(back.working_dir, long_verbatim);
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
    fn cloud_save_queue_roundtrip_keeps_only_the_observation() {
        let connection = memory_database();
        record_cloud_save_queued(&connection, 5, Some("remote-a"), "2026-11-19T12:00:00Z").unwrap();
        let prefs = get_cloud_save_prefs(&connection, 5).unwrap().unwrap();
        assert_eq!(prefs.queued_remote_sha256.as_deref(), Some("remote-a"));
        assert_eq!(prefs.queued_at.as_deref(), Some("2026-11-19T12:00:00Z"));

        // The queue records what the server held, never a decision: the retry
        // worker re-evaluates against the then-current remote (#296).
        record_cloud_save_queued(&connection, 5, Some("remote-b"), "2026-11-19T13:00:00Z").unwrap();
        let prefs = get_cloud_save_prefs(&connection, 5).unwrap().unwrap();
        assert_eq!(prefs.queued_remote_sha256.as_deref(), Some("remote-b"));

        clear_cloud_save_pending(&connection, 5).unwrap();
        let prefs = get_cloud_save_prefs(&connection, 5).unwrap().unwrap();
        assert!(prefs.queued_remote_sha256.is_none());
        assert!(prefs.queued_at.is_none());

        // A successful upload also drops the queued observation.
        record_cloud_save_queued(&connection, 5, Some("remote-c"), "2026-11-19T14:00:00Z").unwrap();
        record_cloud_save_upload(&connection, 5, "uploaded", "2026-11-19T14:05:00Z").unwrap();
        let prefs = get_cloud_save_prefs(&connection, 5).unwrap().unwrap();
        assert!(prefs.queued_remote_sha256.is_none());
        assert_eq!(prefs.last_hash.as_deref(), Some("uploaded"));
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

    fn restore_point(game_id: i64, name: &str, created_at: &str) -> NewSaveRestorePoint {
        NewSaveRestorePoint {
            game_id,
            session_id: Some("session-1".into()),
            source: "session".into(),
            label: String::new(),
            file_name: name.into(),
            size_bytes: 2048,
            sha256: "a".repeat(64),
            device_id: Some("device-a".into()),
            created_at: created_at.into(),
        }
    }

    #[test]
    fn save_restore_points_roundtrip_and_retention_keeps_the_newest() {
        let connection = memory_database();
        let first = insert_save_restore_point(
            &connection,
            &restore_point(7, "a.zip", "2026-11-01T10:00:00Z"),
        )
        .unwrap();
        assert_eq!(first.game_id, 7);
        assert_eq!(first.size_bytes, 2048);
        assert_eq!(first.device_id.as_deref(), Some("device-a"));
        for (name, at) in [
            ("b.zip", "2026-11-02T10:00:00Z"),
            ("c.zip", "2026-11-03T10:00:00Z"),
            ("d.zip", "2026-11-04T10:00:00Z"),
        ] {
            insert_save_restore_point(&connection, &restore_point(7, name, at)).unwrap();
        }
        let points = list_save_restore_points(&connection, 7).unwrap();
        assert_eq!(points.len(), 4);
        assert_eq!(points[0].file_name, "d.zip", "newest first");
        assert_eq!(count_save_restore_points(&connection, 7).unwrap(), 4);

        let removed = prune_save_restore_points(&connection, 7, 2).unwrap();
        assert_eq!(
            removed
                .iter()
                .map(|p| p.file_name.as_str())
                .collect::<Vec<_>>(),
            vec!["b.zip", "a.zip"],
            "older sessions are pruned"
        );
        let kept = list_save_restore_points(&connection, 7).unwrap();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].file_name, "d.zip");
        assert_eq!(kept[1].file_name, "c.zip");

        // A zero/negative window still keeps the last known-good copy.
        let removed = prune_save_restore_points(&connection, 7, 0).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].file_name, "c.zip");
        let last = list_save_restore_points(&connection, 7).unwrap();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].file_name, "d.zip");
        assert!(prune_save_restore_points(&connection, 7, 0)
            .unwrap()
            .is_empty());

        assert!(delete_save_restore_point(&connection, last[0].id).unwrap());
        assert!(list_save_restore_points(&connection, 7).unwrap().is_empty());
        assert!(get_save_restore_point(&connection, last[0].id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn save_restore_points_survive_a_schema_reopen() {
        let root = temp_database_dir("restore-points");
        let path = root.join("ludex-desktop.db");
        {
            let connection = open(&path).unwrap();
            insert_save_restore_point(
                &connection,
                &restore_point(4, "session-4.zip", "2026-11-05T10:00:00Z"),
            )
            .unwrap();
            record_local_save_snapshot(&connection, 4, "2026-11-05T10:00:05Z").unwrap();
        }
        let connection = open(&path).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let points = list_save_restore_points(&connection, 4).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].file_name, "session-4.zip");
        assert_eq!(
            last_local_save_change(&connection, 4).unwrap().as_deref(),
            Some("2026-11-05T10:00:05Z")
        );
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
    fn save_candidate(path: &str, confidence: &str) -> SaveCandidate {
        SaveCandidate {
            path: path.to_string(),
            kind: "directory".to_string(),
            root_id: "saved_games".to_string(),
            subpath: "Hades".to_string(),
            source: "engine".to_string(),
            confidence: confidence.to_string(),
            evidence: "Game title folders".to_string(),
            size_bytes: 0,
            last_modified: None,
            first_seen: Some("2026-11-11T00:00:00Z".to_string()),
            last_verified: Some("2026-11-11T00:00:00Z".to_string()),
            confirmed: false,
            ignored: false,
            auto_status: String::new(),
            auto_reason: None,
        }
    }

    #[test]
    fn save_candidates_preserve_confirmation_dismissal_and_first_seen() {
        let connection = memory_database();
        let first = save_candidate("/home/player/Saved Games/Hades", "likely");
        upsert_save_candidates(&connection, 7, &[first], "2026-11-11T00:00:00Z").unwrap();

        let listed = list_save_candidates(&connection, 7).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "/home/player/Saved Games/Hades");
        assert!(!listed[0].confirmed);
        assert!(!listed[0].ignored);

        // A confirmation survives a re-scan, including first_seen.
        set_save_candidate_confirmed(&connection, 7, "/home/player/Saved Games/Hades", true)
            .unwrap();
        let mut refreshed = save_candidate("/home/player/Saved Games/Hades", "likely");
        refreshed.first_seen = Some("2026-11-12T00:00:00Z".to_string());
        refreshed.last_verified = Some("2026-11-12T00:00:00Z".to_string());
        refreshed.evidence = "Game title folders: refreshed".to_string();
        upsert_save_candidates(&connection, 7, &[refreshed], "2026-11-12T00:00:00Z").unwrap();
        let listed = list_save_candidates(&connection, 7).unwrap();
        assert!(listed[0].confirmed);
        assert_eq!(
            listed[0].first_seen.as_deref(),
            Some("2026-11-11T00:00:00Z")
        );
        assert!(listed[0].evidence.contains("refreshed"));

        // A dismissed candidate does not resurface, but is kept so a later
        // scan cannot recreate it as new.
        upsert_save_candidates(
            &connection,
            7,
            &[
                save_candidate("/home/player/Saved Games/Hades", "likely"),
                save_candidate("/home/player/Documents/Hades", "guess"),
            ],
            "2026-11-13T00:00:00Z",
        )
        .unwrap();
        set_save_candidate_ignored(&connection, 7, "/home/player/Documents/Hades", true).unwrap();
        let listed = list_save_candidates(&connection, 7).unwrap();
        assert_eq!(listed.len(), 1, "{listed:?}");
        upsert_save_candidates(
            &connection,
            7,
            &[
                save_candidate("/home/player/Saved Games/Hades", "likely"),
                save_candidate("/home/player/Documents/Hades", "guess"),
            ],
            "2026-11-14T00:00:00Z",
        )
        .unwrap();
        assert_eq!(list_save_candidates(&connection, 7).unwrap().len(), 1);
        assert!(
            get_save_candidate(&connection, 7, "/home/player/Documents/Hades")
                .unwrap()
                .unwrap()
                .ignored
        );

        // Paths that disappear and were never confirmed are cleaned up; a
        // confirmed path stays so its history is not lost.
        upsert_save_candidates(&connection, 7, &[], "2026-11-15T00:00:00Z").unwrap();
        let listed = list_save_candidates(&connection, 7).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].confirmed);
    }

    #[test]
    fn save_candidate_groups_join_installed_games() {
        let connection = memory_database();
        let game = InstalledGame {
            game_id: 7,
            version_id: 1,
            title: "Hades".into(),
            version: None,
            install_path: "/games/hades".into(),
            executable: None,
            size_bytes: 0,
            installed_at: "2026-11-10T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: None,
            source: crate::db::SOURCE_LUDEX.into(),
            launch_uri: None,
        };
        upsert_installed(&connection, &game).unwrap();
        upsert_save_candidates(
            &connection,
            7,
            &[
                save_candidate("/home/player/Documents/Hades", "guess"),
                save_candidate("/home/player/Saved Games/Hades", "likely"),
            ],
            "2026-11-11T00:00:00Z",
        )
        .unwrap();
        let groups = list_save_candidate_groups(&connection).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1, "Hades");
        assert_eq!(groups[0].2.len(), 2);
        assert_eq!(groups[0].2[0].confidence, "likely");
        assert_eq!(groups[0].2[1].confidence, "guess");
    }

    #[test]
    fn candidate_auto_status_survives_a_rescan_and_is_cleared_by_saving() {
        let connection = memory_database();
        let mut candidate = save_candidate("/home/player/Documents/Hades", "guess");
        candidate.auto_status = "skipped".to_string();
        candidate.auto_reason = Some("too broad".to_string());
        upsert_save_candidates(&connection, 7, &[candidate.clone()], "2026-11-11T00:00:00Z")
            .unwrap();

        // A plain discovery run has an empty status and must not erase the
        // stored decision.
        let mut rediscovered = candidate.clone();
        rediscovered.auto_status = String::new();
        rediscovered.auto_reason = None;
        upsert_save_candidates(&connection, 7, &[rediscovered], "2026-11-12T00:00:00Z").unwrap();
        let listed = list_save_candidates(&connection, 7).unwrap();
        assert_eq!(listed[0].auto_status, "skipped");
        assert_eq!(listed[0].auto_reason.as_deref(), Some("too broad"));

        set_save_candidate_status(&connection, 7, &candidate.path, "saving", None).unwrap();
        let listed = list_save_candidates(&connection, 7).unwrap();
        assert_eq!(listed[0].auto_status, "saving");
        assert_eq!(listed[0].auto_reason, None);
    }

    #[test]
    fn candidate_snapshots_are_replaced_per_game() {
        let connection = memory_database();
        let before = CandidateSnapshotRecord {
            game_id: 7,
            path: "/home/player/Documents/Hades".to_string(),
            existed: true,
            files: 2,
            bytes: 120,
            newest: Some("2026-11-11T10:00:00Z".to_string()),
            captured_at: "2026-11-11T10:00:00Z".to_string(),
        };
        replace_candidate_snapshots(&connection, 7, std::slice::from_ref(&before)).unwrap();
        let listed = list_candidate_snapshots(&connection, 7).unwrap();
        assert_eq!(listed, vec![before.clone()]);

        let next = CandidateSnapshotRecord {
            bytes: 240,
            captured_at: "2026-11-12T10:00:00Z".to_string(),
            ..before
        };
        replace_candidate_snapshots(&connection, 7, std::slice::from_ref(&next)).unwrap();
        assert_eq!(
            list_candidate_snapshots(&connection, 7).unwrap(),
            vec![next]
        );

        clear_candidate_snapshots(&connection, 7).unwrap();
        assert!(list_candidate_snapshots(&connection, 7).unwrap().is_empty());
    }

    #[test]
    fn runtime_install_prefs_and_audit_round_trip() {
        let connection = memory_database();
        let row = RuntimeInstallRow {
            provider_id: "umu-launcher".into(),
            platform: "linux".into(),
            version: "1.4.4".into(),
            path: "/home/player/.local/share/ludex/runtimes/umu-launcher/1.4.4".into(),
            sha256: "abc".into(),
            size_bytes: 430_080,
            installed_at: "2026-11-12T00:00:00Z".into(),
        };
        save_runtime_install(&connection, &row).unwrap();
        assert_eq!(
            list_runtime_installs(&connection).unwrap(),
            vec![row.clone()]
        );
        assert_eq!(
            get_runtime_install(&connection, "umu-launcher").unwrap(),
            Some(row.clone())
        );

        // Upsert replaces the row instead of duplicating it.
        let mut updated = row.clone();
        updated.version = "1.4.5".into();
        save_runtime_install(&connection, &updated).unwrap();
        assert_eq!(list_runtime_installs(&connection).unwrap().len(), 1);
        assert_eq!(
            get_runtime_install(&connection, "umu-launcher")
                .unwrap()
                .unwrap()
                .version,
            "1.4.5"
        );

        let prefs = get_runtime_prefs(&connection, 7).unwrap();
        assert!(prefs.provider_id.is_empty());
        assert!(prefs.prerequisites.is_empty());

        let prefs = RuntimeGamePrefs {
            game_id: 7,
            provider_id: "umu-launcher".into(),
            provider_version: "1.4.4".into(),
            prefix_path: "/games/test/.prefix".into(),
            prerequisites: vec!["dotnet".into(), "directx".into()],
            updated_at: "2026-11-12T00:00:00Z".into(),
        };
        save_runtime_prefs(&connection, &prefs).unwrap();
        assert_eq!(get_runtime_prefs(&connection, 7).unwrap(), prefs);

        let event = RuntimeAuditEvent {
            id: "audit-1".into(),
            game_id: 7,
            provider_id: "umu-launcher".into(),
            version: "1.4.4".into(),
            action: "install".into(),
            outcome: "ok".into(),
            detail: "verified and extracted".into(),
            created_at: "2026-11-12T00:00:00Z".into(),
        };
        record_runtime_audit(&connection, &event).unwrap();
        assert_eq!(list_runtime_audit(&connection, 7, 10).unwrap(), vec![event]);

        delete_runtime_install(&connection, "umu-launcher").unwrap();
        assert!(list_runtime_installs(&connection).unwrap().is_empty());
    }

    fn file_manifest_row(job_id: &str, sha: &str) -> FileManifestRecord {
        FileManifestRecord {
            job_id: job_id.into(),
            game_id: 3,
            version_id: 4,
            archive_sha256: sha.into(),
            staging: format!("/games/.elden-ring.staging-{job_id}"),
            total_bytes: 1_000,
            file_count: 2,
            created_at: "2026-11-15T00:00:00Z".into(),
        }
    }

    /// #231: the per-file checkpoint survives a restart keyed by job id, and a
    /// different archive invalidates it.
    #[test]
    fn file_manifest_checkpoints_round_trip_and_prune() {
        let connection = open_in_memory_for_tests();
        let sha = "a".repeat(64);
        upsert_file_manifest(&connection, &file_manifest_row("job-files", &sha)).unwrap();
        record_job_file(
            &connection,
            "job-files",
            "Game/bin/a.bin",
            512,
            &"b".repeat(64),
            "2026-11-15T00:00:10Z",
        )
        .unwrap();
        record_job_file(
            &connection,
            "job-files",
            "Game/bin/b.bin",
            488,
            &"c".repeat(64),
            "2026-11-15T00:00:11Z",
        )
        .unwrap();

        let stored = get_file_manifest(&connection, "job-files")
            .unwrap()
            .unwrap();
        assert_eq!(stored.archive_sha256, sha);
        assert_eq!(stored.file_count, 2);
        assert_eq!(
            completed_job_files(&connection, "job-files").unwrap(),
            vec![
                ("Game/bin/a.bin".to_string(), 512),
                ("Game/bin/b.bin".to_string(), 488),
            ]
        );

        // A file that is no longer on disk is forgotten without touching the
        // winner.
        forget_job_file(&connection, "job-files", "Game/bin/b.bin").unwrap();
        assert_eq!(
            completed_job_files(&connection, "job-files").unwrap(),
            vec![("Game/bin/a.bin".to_string(), 512)]
        );

        // A second job of the same version prunes the first one's rows, so a
        // restart cannot adopt a stale staging folder.
        upsert_file_manifest(&connection, &file_manifest_row("job-new", &"d".repeat(64))).unwrap();
        assert_eq!(
            prune_file_manifests(&connection, 3, 4, "job-new").unwrap(),
            1
        );
        assert!(get_file_manifest(&connection, "job-files")
            .unwrap()
            .is_none());
        assert!(completed_job_files(&connection, "job-files")
            .unwrap()
            .is_empty());

        // A changed archive clears the completed files but keeps the header so
        // the next attest sees the new identity.
        record_job_file(
            &connection,
            "job-new",
            "Game/bin/a.bin",
            512,
            &"e".repeat(64),
            "2026-11-15T00:00:12Z",
        )
        .unwrap();
        clear_job_files(&connection, "job-new").unwrap();
        assert!(completed_job_files(&connection, "job-new")
            .unwrap()
            .is_empty());
        assert_eq!(
            get_file_manifest(&connection, "job-new")
                .unwrap()
                .unwrap()
                .archive_sha256,
            "d".repeat(64)
        );

        delete_file_manifest(&connection, "job-new").unwrap();
        assert!(get_file_manifest(&connection, "job-new").unwrap().is_none());
    }

    /// Store a job row the startup path would see (#234).
    fn store_job(
        connection: &Connection,
        job_id: &str,
        phase: &str,
        error: &str,
        part: Option<&str>,
    ) {
        upsert_install_job(
            connection,
            &InstallJobRecord {
                job_id: job_id.to_string(),
                game_id: 7,
                version_id: 3,
                phase: phase.to_string(),
                bytes_done: 4096,
                bytes_total: Some(1_000_000),
                speed_bps: 0,
                part_path: part.map(str::to_string),
                install_path: None,
                staging_path: None,
                error: Some(error.to_string()),
                stream: false,
                resumable: true,
                stream_completed: None,
                entries_total: None,
                queue_position: Some(0),
                verified_sha256: None,
                created_at: "2026-11-16T00:00:00Z".to_string(),
                updated_at: "2026-11-16T00:00:00Z".to_string(),
            },
        )
        .unwrap();
    }

    #[test]
    fn resume_marker_lifecycle_counts_only_unclean_launches() {
        let connection = memory_database();
        store_job(
            &connection,
            "job-breaker",
            "error",
            crate::install::INTERRUPTED_MESSAGE,
            None,
        );
        // Clean launch: the marker is written, the app reaches the healthy
        // window and the marker is cleared again. No failure is recorded.
        set_resume_marker(&connection, "job-breaker", "2026-11-16T10:00:00Z").unwrap();
        assert_eq!(resume_markers(&connection).unwrap().len(), 1);
        delete_resume_marker(&connection, "job-breaker").unwrap();
        assert!(resume_markers(&connection).unwrap().is_empty());
        assert_eq!(resume_failures(&connection, "job-breaker").unwrap(), 0);
        assert_eq!(
            crate::crash_guard::startup_resume_decision(0),
            crate::crash_guard::StartupResume::Resume
        );

        // First unclean launch: the marker survived, so the count is 1 and the
        // job is still resumed (one strike).
        set_resume_marker(&connection, "job-breaker", "2026-11-16T10:01:00Z").unwrap();
        let counts = reconcile_resume_markers(&connection, "2026-11-16T10:02:00Z").unwrap();
        assert_eq!(counts, vec![("job-breaker".to_string(), 1)]);
        assert!(resume_markers(&connection).unwrap().is_empty());
        assert_eq!(
            crate::crash_guard::startup_resume_decision(1),
            crate::crash_guard::StartupResume::Resume
        );

        // Second consecutive unclean launch: the breaker trips.
        set_resume_marker(&connection, "job-breaker", "2026-11-16T10:03:00Z").unwrap();
        let counts = reconcile_resume_markers(&connection, "2026-11-16T10:04:00Z").unwrap();
        assert_eq!(counts, vec![("job-breaker".to_string(), 2)]);
        assert_eq!(
            crate::crash_guard::startup_resume_decision(2),
            crate::crash_guard::StartupResume::Park
        );

        // A manual Resume clears the streak, so the user is never locked out.
        reset_resume_failures(&connection, "job-breaker").unwrap();
        assert_eq!(resume_failures(&connection, "job-breaker").unwrap(), 0);
        assert_eq!(
            crate::crash_guard::startup_resume_decision(0),
            crate::crash_guard::StartupResume::Resume
        );
    }

    #[test]
    fn a_marker_of_a_deleted_job_is_dropped_without_counting() {
        let connection = memory_database();
        set_resume_marker(&connection, "job-gone", "2026-11-16T10:00:00Z").unwrap();
        let counts = reconcile_resume_markers(&connection, "2026-11-16T10:01:00Z").unwrap();
        assert!(counts.is_empty());
        assert!(resume_markers(&connection).unwrap().is_empty());
        assert_eq!(resume_failures(&connection, "job-gone").unwrap(), 0);
    }

    #[test]
    fn parking_a_job_keeps_the_partial_file_and_stops_auto_resume() {
        let connection = memory_database();
        let part = std::env::temp_dir().join(format!("ludex-234-{}.part", std::process::id()));
        std::fs::write(&part, b"partial archive").unwrap();
        let part_path = part.to_string_lossy().to_string();
        store_job(
            &connection,
            "job-park",
            "error",
            crate::install::INTERRUPTED_MESSAGE,
            Some(&part_path),
        );

        assert!(park_install_job(
            &connection,
            "job-park",
            crate::install::RESUME_DISABLED_MESSAGE,
            "2026-11-16T10:00:00Z"
        )
        .unwrap());

        let record = get_install_job(&connection, "job-park").unwrap().unwrap();
        assert_eq!(record.phase, "error");
        assert_eq!(
            record.error.as_deref(),
            Some(crate::install::RESUME_DISABLED_MESSAGE)
        );
        assert_eq!(record.part_path.as_deref(), Some(part_path.as_str()));
        assert_eq!(record.queue_position, None);
        // A parked row is invisible to the startup auto-resume, even though the
        // `.part` is still on disk and a manual Resume can use it.
        assert!(interrupted_install_jobs(
            &connection,
            crate::install::INTERRUPTED_MESSAGE,
            crate::install::LEGACY_INTERRUPTED_MESSAGE
        )
        .unwrap()
        .is_empty());
        assert!(part.is_file(), "parking must never delete the partial file");
        // A manual Resume still finds the row and continues the same job.
        let resumable = latest_resumable_job_for_version(&connection, 7, 3, false)
            .unwrap()
            .expect("a parked job must stay resumable by hand");
        assert_eq!(resumable.job_id, "job-park");
        assert_eq!(resumable.part_path.as_deref(), Some(part_path.as_str()));
        let _ = std::fs::remove_file(&part);
    }

    #[test]
    fn auto_resume_never_selects_paused_or_cancelled_jobs() {
        let connection = memory_database();
        store_job(
            &connection,
            "job-interrupted",
            "error",
            crate::install::INTERRUPTED_MESSAGE,
            None,
        );
        store_job(
            &connection,
            "job-failed",
            "error",
            "The server returned 404",
            None,
        );
        store_job(&connection, "job-paused", "paused", "Paused", None);
        store_job(
            &connection,
            "job-cancelled",
            "cancelled",
            "Install cancelled",
            None,
        );
        let selected = interrupted_install_jobs(
            &connection,
            crate::install::INTERRUPTED_MESSAGE,
            crate::install::LEGACY_INTERRUPTED_MESSAGE,
        )
        .unwrap();
        let ids: Vec<&str> = selected.iter().map(|row| row.job_id.as_str()).collect();
        assert_eq!(ids, vec!["job-interrupted"]);
    }

    /* ------------------------------------------- migration safety (Wave 2) */

    fn temp_database_dir(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-db-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn backup_files(directory: &Path) -> Vec<std::path::PathBuf> {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().contains("-backup-"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn the_migration_table_covers_every_version_once() {
        let versions: Vec<i64> = MIGRATIONS
            .iter()
            .map(|migration| migration.version)
            .collect();
        assert!(
            versions.windows(2).all(|pair| pair[0] < pair[1]),
            "migrations must be appended in strictly increasing order"
        );
        // Parallel work may leave a gap until the lane that owns the missing
        // number merges (#296 owns 27 while 26 lands elsewhere); the runner is
        // gap-tolerant, so the invariant is "last entry == SCHEMA_VERSION".
        assert_eq!(
            versions.last().copied(),
            Some(SCHEMA_VERSION),
            "bump SCHEMA_VERSION and append the highest migration entry"
        );
    }

    #[test]
    fn opening_a_newer_database_is_refused_with_a_recovery_path() {
        let root = temp_database_dir("newer");
        let path = root.join("ludex-desktop.db");
        let newer = SCHEMA_VERSION + 1;
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .pragma_update(None, "user_version", newer)
                .unwrap();
        }

        let error = open(&path).unwrap_err();
        assert!(error.contains("newer Ludex"), "{error}");
        assert!(error.contains(&format!("version {newer}")), "{error}");
        assert!(
            error.contains(&format!("supports {SCHEMA_VERSION}")),
            "{error}"
        );
        // The message names the backup pattern, so a user can recover without
        // reading source.
        assert!(error.contains("-backup-"), "{error}");

        // Nothing was migrated or downgraded behind the error.
        let connection = Connection::open(&path).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, newer);
        assert!(backup_files(&root).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failing_migration_rolls_back_the_schema_and_the_version() {
        let connection = Connection::open_in_memory().unwrap();
        let migrations = [
            Migration {
                version: 1,
                sql: "CREATE TABLE kept (id INTEGER PRIMARY KEY);",
                context: "create the kept table",
            },
            Migration {
                version: 2,
                sql: "CREATE TABLE later (id INTEGER PRIMARY KEY);
                      CREATE TABLE later (id INTEGER PRIMARY KEY);",
                context: "create the later table",
            },
        ];

        let error = migrate_with(&connection, &migrations, 2).unwrap_err();
        assert!(
            error.contains("Could not create the later table"),
            "{error}"
        );
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 0, "a failed migration must not record a version");
        let tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type = 'table' AND name IN ('kept', 'later')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "no statement of the failed batch may survive");

        // The same connection still migrates with the real list: a rollback
        // leaves a retryable database, not a brick.
        migrate(&connection).unwrap();
    }

    #[test]
    fn an_old_database_is_backed_up_then_migrated() {
        let root = temp_database_dir("fixture");
        let path = root.join("ludex-desktop.db");
        const FIXTURE_VERSION: i64 = 18;
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
                .unwrap();
            migrate_with(&connection, MIGRATIONS, FIXTURE_VERSION).unwrap();
            connection
                .execute(
                    "INSERT INTO installed_games
                        (game_id, version_id, title, install_path, installed_at)
                     VALUES (7, 1, 'Hades', '/games/hades', '2026-01-01T00:00:00Z')",
                    [],
                )
                .unwrap();
            let version: i64 = connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version, FIXTURE_VERSION);
        }

        let connection = open(&path).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let title: String = connection
            .query_row(
                "SELECT title FROM installed_games WHERE game_id = 7",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "Hades");
        // The v23 outbox columns exist on the migrated fixture.
        let outbox: Option<String> = connection
            .query_row(
                "SELECT sync_error FROM sessions WHERE id = 'missing'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert!(outbox.is_none());

        let backups = backup_files(&root);
        assert_eq!(backups.len(), 1, "{backups:?}");
        assert!(
            backups[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-v0018.db"),
            "{:?}",
            backups[0]
        );
        // The snapshot is the pre-migration file: old version, old schema.
        let snapshot = Connection::open(&backups[0]).unwrap();
        let snapshot_version: i64 = snapshot
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(snapshot_version, FIXTURE_VERSION);

        // Re-opening a current database never snapshots again.
        drop(connection);
        open(&path).unwrap();
        assert_eq!(backup_files(&root).len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pre_migration_backups_are_pruned_to_the_retention_limit() {
        let root = temp_database_dir("prune");
        let path = root.join("ludex-desktop.db");
        let connection = Connection::open(&path).unwrap();
        migrate(&connection).unwrap();

        for stamp in [
            "20261101T000000000Z",
            "20261102T000000000Z",
            "20261103T000000000Z",
            "20261104T000000000Z",
            "20261105T000000000Z",
        ] {
            backup_with_stamp(&connection, &path, SCHEMA_VERSION - 1, stamp).unwrap();
        }

        let backups = backup_files(&root);
        assert_eq!(backups.len(), BACKUP_RETENTION, "{backups:?}");
        assert!(backups[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("20261103T"));
        assert!(backups[2]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("20261105T"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /* ---------------------------------------------- playtime outbox (Wave 2) */

    fn finished_session(connection: &Connection, id: &str, game_id: i64, minutes: i64) {
        insert_session(connection, id, game_id, "2026-11-16T10:00:00Z").unwrap();
        finish_session(
            connection,
            id,
            "2026-11-16T10:30:00Z",
            minutes,
            minutes * 60,
        )
        .unwrap();
    }

    #[test]
    fn playtime_outbox_respects_backoff_and_its_budget() {
        let connection = memory_database();
        finished_session(&connection, "session-a", 5, 30);

        // A fresh session is due immediately.
        let due = pending_sessions_due(&connection, "2026-11-16T11:00:00Z").unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].attempts, 0);

        // One failure schedules the next attempt; the worker sees nothing
        // before it and the row keeps its minutes.
        record_session_sync_failure(
            &connection,
            "session-a",
            1,
            Some("2026-11-16T11:00:30Z"),
            "server unreachable",
        )
        .unwrap();
        assert!(pending_sessions_due(&connection, "2026-11-16T11:00:10Z")
            .unwrap()
            .is_empty());
        let due = pending_sessions_due(&connection, "2026-11-16T11:00:30Z").unwrap();
        assert_eq!(due[0].attempts, 1);

        // The budget is spent: no further automatic attempt, but the message
        // is still local (unsynced) with its error.
        record_session_sync_failure(
            &connection,
            "session-a",
            PLAYTIME_SYNC_MAX_ATTEMPTS,
            None,
            "server unreachable (automatic retries exhausted)",
        )
        .unwrap();
        assert!(pending_sessions_due(&connection, "2027-01-01T00:00:00Z")
            .unwrap()
            .is_empty());
        let (synced, minutes, error): (i64, i64, Option<String>) = connection
            .query_row(
                "SELECT synced, minutes, sync_error FROM sessions WHERE id = 'session-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(synced, 0);
        assert_eq!(minutes, 30);
        assert!(error.unwrap().contains("exhausted"));
    }

    #[test]
    fn marking_a_session_synced_clears_its_retry_state() {
        let connection = memory_database();
        finished_session(&connection, "session-b", 6, 12);
        record_session_sync_failure(
            &connection,
            "session-b",
            2,
            Some("2026-11-16T11:02:00Z"),
            "timeout",
        )
        .unwrap();

        mark_session_synced(&connection, "session-b").unwrap();

        let (synced, next, error): (i64, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT synced, sync_next_at, sync_error FROM sessions WHERE id = 'session-b'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(synced, 1);
        assert!(next.is_none());
        assert!(error.is_none());
        assert!(pending_sessions_due(&connection, "2027-01-01T00:00:00Z")
            .unwrap()
            .is_empty());

        // A late duplicate ack is a no-op, not an un-sync.
        mark_session_synced(&connection, "session-b").unwrap();
        let synced: i64 = connection
            .query_row(
                "SELECT synced FROM sessions WHERE id = 'session-b'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(synced, 1);
    }
}
