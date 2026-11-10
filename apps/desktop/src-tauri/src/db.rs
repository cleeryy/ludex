//! Local SQLite index (`{appDataDir}/ludex-desktop.db`).
//!
//! This database is a rebuildable index of what the desktop app installed on
//! this machine, plus local play sessions waiting to be synced to the server.
//! It is not the server database.

use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

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
    /// Absolute install root used for this game (not exposed to the web UI).
    #[serde(skip)]
    pub install_root: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingSession {
    pub id: String,
    pub game_id: i64,
    pub minutes: i64,
}

const SCHEMA_VERSION: i64 = 3;

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
                 size_bytes, installed_at, last_launched_at, install_root)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(game_id) DO UPDATE SET
                version_id       = excluded.version_id,
                title            = excluded.title,
                version          = excluded.version,
                install_path     = excluded.install_path,
                executable       = excluded.executable,
                size_bytes       = excluded.size_bytes,
                installed_at     = excluded.installed_at,
                last_launched_at = excluded.last_launched_at,
                install_root     = excluded.install_root
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
                game.install_root,
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
        install_root: row.get(9)?,
    })
}

pub fn list_installed(connection: &Connection) -> Result<Vec<InstalledGame>, String> {
    let mut statement = connection
        .prepare(
            r#"
            SELECT game_id, version_id, title, version, install_path, executable,
                   size_bytes, installed_at, last_launched_at, install_root
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
                   size_bytes, installed_at, last_launched_at, install_root
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
    let affected = connection
        .execute(
            "DELETE FROM installed_games WHERE game_id = ?1",
            params![game_id],
        )
        .map_err(|e| format!("Could not remove the installed game: {e}"))?;
    Ok(affected > 0)
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
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE sessions SET ended_at = ?2, minutes = ?3 WHERE id = ?1",
            params![id, ended_at, minutes.max(0)],
        )
        .map_err(|e| format!("Could not record the play session: {e}"))?;
    Ok(())
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
    pub created_at: String,
    pub updated_at: String,
}

pub fn upsert_install_job(connection: &Connection, job: &InstallJobRecord) -> Result<(), String> {
    connection
        .execute(
            r#"
            INSERT INTO install_jobs
                (job_id, game_id, version_id, phase, bytes_done, bytes_total,
                 speed_bps, part_path, install_path, error, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(job_id) DO UPDATE SET
                phase        = excluded.phase,
                bytes_done   = excluded.bytes_done,
                bytes_total  = excluded.bytes_total,
                speed_bps    = excluded.speed_bps,
                part_path    = excluded.part_path,
                install_path = excluded.install_path,
                error        = excluded.error,
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
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

const INSTALL_JOB_COLUMNS: &str = "job_id, game_id, version_id, phase, bytes_done, bytes_total, speed_bps, part_path, install_path, error, created_at, updated_at";

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

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(SCHEMA_V2).unwrap();
        connection.execute_batch(SCHEMA_V3).unwrap();
        connection
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
            install_root: Some("/tmp".into()),
        };
        upsert_installed(&connection, &game).unwrap();
        let loaded = get_installed(&connection, 7).unwrap().unwrap();
        assert_eq!(loaded.title, "Test");
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
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        upsert_install_job(&connection, &job).unwrap();
        let loaded = get_install_job(&connection, "job-1").unwrap().unwrap();
        assert_eq!(loaded.bytes_done, 1024);
        assert_eq!(loaded.part_path.as_deref(), Some("/tmp/game.part"));

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

    #[test]
    fn sessions_are_pending_until_synced() {
        let connection = memory_database();
        insert_session(&connection, "s1", 1, "2026-01-01T00:00:00Z").unwrap();
        assert!(pending_sessions(&connection).unwrap().is_empty());
        finish_session(&connection, "s1", "2026-01-01T00:30:00Z", 30).unwrap();
        let pending = pending_sessions(&connection).unwrap();
        assert_eq!(pending.len(), 1);
        mark_session_synced(&connection, "s1").unwrap();
        assert!(pending_sessions(&connection).unwrap().is_empty());
    }
}
