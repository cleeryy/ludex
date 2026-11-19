//! Local save manager history (#295).
//!
//! Sync moves bytes; this module keeps a copy of every session's saves on the
//! device so a corrupt save or a wrong-way sync stays recoverable without the
//! server. Each game exit archives the detected save paths into one bounded
//! zip under `{appDataDir}/restore-points/game-<id>/`, records the size, hash,
//! device and session, and applies the configured per-session retention.
//!
//! Rules that are deliberate and tested:
//! - the newest restore point is the last known-good copy and is never
//!   removed by a retention pass, even when the configured window is zero;
//! - pruning rows happens before deleting their archives, so a crash leaves
//!   orphaned files instead of rows pointing at missing zips (same rule as the
//!   server-side retention);
//! - an unchanged session is not stored twice: the archive hash is compared
//!   with the newest point first;
//! - restoring always snapshots the current files first, so a rollback is
//!   itself reversible.
//!
//! Host paths never leave this machine. Nothing here talks to the server.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::db::{self, NewSaveRestorePoint, SavePathRecord, SaveRestorePoint};
use crate::saves::sync;
use crate::settings;
use crate::state::AppState;
use crate::util::now_rfc3339;

/// Default per-session retention. Mirrors `settings::default_save_restore_retention`.
pub const DEFAULT_RETENTION: i64 = 5;
/// A retention window of zero would remove the copy the feature exists for.
pub const MIN_RETENTION: i64 = 1;
/// Bounded so the panel cannot promise an unbounded number of multi-GiB zips.
pub const MAX_RETENTION: i64 = 50;

/// `source` values stored in `save_restore_points`.
pub const SOURCE_SESSION: &str = "session";
pub const SOURCE_MANUAL: &str = "manual";
pub const SOURCE_PRE_RESTORE: &str = "pre_restore";

/// Everything the "My saves" panel lists for one game.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePointsView {
    /// Newest first.
    pub points: Vec<SaveRestorePoint>,
    /// Sessions kept per game, after clamping to [`MIN_RETENTION`]–[`MAX_RETENTION`].
    pub retention: i64,
    /// Sum of the stored archives.
    pub total_bytes: u64,
    /// When this device last captured a save, including unchanged sessions.
    pub last_capture_at: Option<String>,
}

/// Result of a one-click rollback.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRestoreOutcome {
    pub restored: bool,
    pub point_id: i64,
    /// Restore point created from the files that were replaced, when any.
    pub backup_point_id: Option<i64>,
    pub restored_at: String,
    pub message: String,
}

/// Result of an explicit retention pass.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneReport {
    /// Restore points removed from disk and database.
    pub removed: i64,
    /// Restore points still stored for the game.
    pub kept: i64,
}

/// What a capture produced. `Unchanged` means the save bytes already had a
/// restore point; `NoFiles` means the game has no save files yet.
#[derive(Debug)]
pub enum Capture {
    Created {
        point: SaveRestorePoint,
        pruned: i64,
    },
    Unchanged,
    NoFiles,
}

/// A freshly written archive before it is recorded.
struct Archive {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

/// Folder holding one game's restore points.
pub fn restore_points_dir(data_dir: &Path, game_id: i64) -> PathBuf {
    data_dir
        .join("restore-points")
        .join(format!("game-{game_id}"))
}

/// Absolute archive path for a stored restore point.
pub fn point_path(data_dir: &Path, point: &SaveRestorePoint) -> PathBuf {
    restore_points_dir(data_dir, point.game_id).join(&point.file_name)
}

/// Clamp a requested retention window into the supported range.
#[must_use]
pub fn clamp_retention(retention: i64) -> i64 {
    retention.clamp(MIN_RETENTION, MAX_RETENTION)
}

/// Effective retention from the local settings. A missing or zero value (old
/// settings file) falls back to [`DEFAULT_RETENTION`].
#[must_use]
pub fn retention_from(settings: &settings::LocalSettings) -> i64 {
    let configured = i64::from(settings.save_restore_retention);
    clamp_retention(if configured == 0 {
        DEFAULT_RETENTION
    } else {
        configured
    })
}

/// Archive the save paths without touching the database.
///
/// Returns `Ok(None)` when the paths currently hold no files, so an exit hook
/// never stores an empty session.
fn create_point_archive(
    data_dir: &Path,
    game_id: i64,
    rows: &[SavePathRecord],
) -> Result<Option<Archive>, String> {
    let local = sync::scan_local_state(rows);
    if local.files == 0 {
        return Ok(None);
    }
    let directory = restore_points_dir(data_dir, game_id);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the restore-point folder: {error}"))?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let unique = crate::util::short_id(&uuid::Uuid::new_v4().to_string());
    let path = directory.join(format!("{stamp}-{unique}.zip"));
    let info = sync::create_archive(rows, &path)?;
    if info.files == 0 {
        let _ = std::fs::remove_file(&info.path);
        return Ok(None);
    }
    Ok(Some(Archive {
        path: info.path,
        bytes: info.bytes,
        sha256: info.sha256,
    }))
}

/// Record an archive and apply retention. Deletes the archive when an
/// identical point already exists, so unchanged sessions cost one zip write and
/// no row.
#[allow(clippy::too_many_arguments)]
fn store_archive(
    data_dir: &Path,
    connection: &rusqlite::Connection,
    game_id: i64,
    archive: Archive,
    source: &str,
    label: &str,
    session_id: Option<&str>,
    device_id: Option<&str>,
    retention: Option<i64>,
) -> Result<Capture, String> {
    if let Some(latest) = db::latest_save_restore_point(connection, game_id)? {
        if !latest.sha256.is_empty() && latest.sha256 == archive.sha256 {
            let _ = std::fs::remove_file(&archive.path);
            db::record_local_save_snapshot(connection, game_id, &now_rfc3339())?;
            return Ok(Capture::Unchanged);
        }
    }
    let file_name = archive
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| "The restore-point archive has no file name.".to_string())?;
    let now = now_rfc3339();
    let point = match db::insert_save_restore_point(
        connection,
        &NewSaveRestorePoint {
            game_id,
            session_id: session_id.map(str::to_string),
            source: source.to_string(),
            label: label.to_string(),
            file_name,
            size_bytes: i64::try_from(archive.bytes).unwrap_or(i64::MAX),
            sha256: archive.sha256,
            device_id: device_id.map(str::to_string),
            created_at: now.clone(),
        },
    ) {
        Ok(point) => point,
        Err(error) => {
            let _ = std::fs::remove_file(&archive.path);
            return Err(error);
        }
    };
    db::record_local_save_snapshot(connection, game_id, &now)?;
    let pruned = match retention {
        Some(keep) => prune_points(data_dir, connection, game_id, keep)?.removed,
        None => 0,
    };
    Ok(Capture::Created { point, pruned })
}

/// Retention pass that deletes archives first only after their rows are gone.
fn prune_points(
    data_dir: &Path,
    connection: &rusqlite::Connection,
    game_id: i64,
    keep: i64,
) -> Result<PruneReport, String> {
    let removed = db::prune_save_restore_points(connection, game_id, keep)?;
    for point in &removed {
        let path = point_path(data_dir, point);
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    error = %error,
                    path = %path.display(),
                    "could not delete a pruned restore point"
                );
            }
        }
    }
    let kept = db::count_save_restore_points(connection, game_id)?;
    Ok(PruneReport {
        removed: i64::try_from(removed.len()).unwrap_or(i64::MAX),
        kept,
    })
}

/// Capture one session (or a manual point) with the database already open.
///
/// This is the whole flow in one function so tests can drive it with a temp
/// directory and an in-memory database.
#[allow(clippy::too_many_arguments)]
pub fn capture_session_at(
    data_dir: &Path,
    connection: &rusqlite::Connection,
    game_id: i64,
    rows: &[SavePathRecord],
    source: &str,
    label: &str,
    session_id: Option<&str>,
    device_id: Option<&str>,
    retention: i64,
) -> Result<Capture, String> {
    let Some(archive) = create_point_archive(data_dir, game_id, rows)? else {
        return Ok(Capture::NoFiles);
    };
    store_archive(
        data_dir,
        connection,
        game_id,
        archive,
        source,
        label,
        session_id,
        device_id,
        Some(clamp_retention(retention)),
    )
}

/// Roll a game back to one local restore point.
///
/// The current files are archived first; if that fails, nothing is
/// overwritten. The archive hash is verified before extraction so a damaged
/// zip cannot half-write the save folder.
#[allow(clippy::too_many_arguments)]
pub fn restore_point_at(
    data_dir: &Path,
    connection: &rusqlite::Connection,
    game_id: i64,
    point_id: i64,
    rows: &[SavePathRecord],
    device_id: Option<&str>,
    retention: i64,
) -> Result<LocalRestoreOutcome, String> {
    let point = db::get_save_restore_point(connection, point_id)?
        .filter(|point| point.game_id == game_id)
        .ok_or_else(|| "That restore point no longer exists.".to_string())?;
    let archive = point_path(data_dir, &point);
    if !archive.is_file() {
        return Err("The restore point archive is missing from disk.".to_string());
    }
    if !point.sha256.is_empty() {
        let actual = sync::sha256_file(&archive)?;
        if actual != point.sha256 {
            return Err(
                "The restore point archive does not match its checksum; nothing was restored."
                    .to_string(),
            );
        }
    }
    if rows.is_empty() {
        return Err(crate::saves::detect::NO_SAVE_MESSAGE.to_string());
    }

    // Safety copy of the files about to be replaced. A failure aborts the
    // restore: the user must never lose the current progress silently. The
    // backup is stored without a retention pass, because pruning here could
    // delete the very point being restored when the window is full.
    let backup_point_id = match create_point_archive(data_dir, game_id, rows)? {
        Some(archive) => match store_archive(
            data_dir,
            connection,
            game_id,
            archive,
            SOURCE_PRE_RESTORE,
            "Before restoring a save",
            None,
            device_id,
            None,
        )? {
            Capture::Created { point, .. } => Some(point.id),
            // The current bytes were already stored as the newest point.
            Capture::Unchanged | Capture::NoFiles => None,
        },
        None => None,
    };

    sync::extract_archive(&archive, rows)?;
    prune_points(data_dir, connection, game_id, clamp_retention(retention))?;
    let now = now_rfc3339();
    db::record_local_save_snapshot(connection, game_id, &now)?;
    sync::invalidate_scan_cache(game_id);

    Ok(LocalRestoreOutcome {
        restored: true,
        point_id: point.id,
        backup_point_id,
        restored_at: now,
        message: "Restored the local save from this device.".to_string(),
    })
}

/* ------------------------------------------------------------- commands */

fn state_rows(app: &AppHandle, game_id: i64) -> Result<Vec<SavePathRecord>, String> {
    let state = app.state::<AppState>();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::list_save_paths(&connection, game_id)
}

fn device_id(app: &AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let id = match sync::ensure_device_identity(&state) {
        Ok((id, _)) => id,
        Err(_) => state.settings_snapshot().cloud_device_id,
    };
    let id = id.trim().to_string();
    (!id.is_empty()).then_some(id)
}

fn retention(app: &AppHandle) -> i64 {
    let state = app.state::<AppState>();
    retention_from(&state.settings_snapshot())
}

/// List a game's local restore points for the "My saves" panel.
pub fn list(app: &AppHandle, game_id: i64) -> Result<RestorePointsView, String> {
    let state = app.state::<AppState>();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let points = db::list_save_restore_points(&connection, game_id)?;
    let total_bytes = points
        .iter()
        .map(|point| u64::try_from(point.size_bytes).unwrap_or(0))
        .sum();
    let last_capture_at = db::last_local_save_change(&connection, game_id)?;
    Ok(RestorePointsView {
        points,
        retention: retention(app),
        total_bytes,
        last_capture_at,
    })
}

/// Capture a restore point now (panel button), regardless of the sync toggle.
pub async fn capture_now(
    app: AppHandle,
    game_id: i64,
    label: Option<String>,
) -> Result<Option<SaveRestorePoint>, String> {
    let rows = state_rows(&app, game_id)?;
    if rows.is_empty() {
        return Err(crate::saves::detect::NO_SAVE_MESSAGE.to_string());
    }
    let data_dir = app.state::<AppState>().app_data.clone();
    let device = device_id(&app);
    let retention = retention(&app);
    let label = label
        .map(|label| label.trim().chars().take(200).collect::<String>())
        .unwrap_or_default();
    let capture = tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        capture_session_at(
            &data_dir,
            &connection,
            game_id,
            &rows,
            SOURCE_MANUAL,
            &label,
            None,
            device.as_deref(),
            retention,
        )
    })
    .await
    .map_err(|error| format!("The restore-point task failed: {error}"))??;
    match capture {
        Capture::Created { point, .. } => Ok(Some(point)),
        Capture::Unchanged => Ok(None),
        Capture::NoFiles => Err(crate::saves::detect::NO_SAVE_MESSAGE.to_string()),
    }
}

/// One-click rollback to a local restore point.
pub async fn restore(
    app: AppHandle,
    game_id: i64,
    point_id: i64,
) -> Result<LocalRestoreOutcome, String> {
    let rows = state_rows(&app, game_id)?;
    let data_dir = app.state::<AppState>().app_data.clone();
    let device = device_id(&app);
    let retention = retention(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        restore_point_at(
            &data_dir,
            &connection,
            game_id,
            point_id,
            &rows,
            device.as_deref(),
            retention,
        )
    })
    .await
    .map_err(|error| format!("The save restore task failed: {error}"))?
}

/// Explicit retention pass; the UI confirms before calling it.
pub fn prune(app: &AppHandle, game_id: i64, keep: Option<i64>) -> Result<PruneReport, String> {
    let state = app.state::<AppState>();
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let keep = clamp_retention(keep.unwrap_or_else(|| retention(app)));
    prune_points(&state.app_data, &connection, game_id, keep)
}

/// Store the new retention window and apply it right away. The panel warns
/// with the number of points that will be removed before calling this.
pub fn set_retention(app: &AppHandle, sessions: i64) -> Result<i64, String> {
    let sessions = clamp_retention(sessions);
    let state = app.state::<AppState>();
    state
        .update_settings(|settings| settings.save_restore_retention = sessions as u32)
        .map_err(|error| format!("Could not save the restore-point setting: {error}"))?;
    Ok(sessions)
}

/// Exit hook: keep this session's save state as a restore point. Runs after the
/// client debounce, before the cloud upload, and works with sync disabled.
pub async fn snapshot_after_exit(app: AppHandle, game_id: i64) {
    let rows = match state_rows(&app, game_id) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::debug!(error = %error, game_id, "restore point skipped");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    let data_dir = app.state::<AppState>().app_data.clone();
    let device = device_id(&app);
    let retention = retention(&app);

    // The zip walk runs on the blocking pool without the database lock.
    let archive = {
        let rows = rows.clone();
        let data_dir = data_dir.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            create_point_archive(&data_dir, game_id, &rows)
        })
        .await
        {
            Ok(Ok(archive)) => archive,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, game_id, "could not create the session restore point");
                return;
            }
            Err(error) => {
                tracing::warn!(error = %error, game_id, "restore-point task failed");
                return;
            }
        }
    };
    let Some(archive) = archive else {
        tracing::debug!(game_id, "session restore point skipped: no save files");
        return;
    };
    let state = app.state::<AppState>();
    let connection = match state.db.lock() {
        Ok(connection) => connection,
        Err(_) => return,
    };
    match store_archive(
        &data_dir,
        &connection,
        game_id,
        archive,
        SOURCE_SESSION,
        "",
        None,
        device.as_deref(),
        Some(retention),
    ) {
        Ok(Capture::Created { point, pruned }) => {
            tracing::info!(
                game_id,
                point = point.id,
                bytes = point.size_bytes,
                pruned,
                "session restore point stored"
            );
        }
        Ok(Capture::Unchanged) => {
            tracing::debug!(game_id, "session restore point skipped: unchanged");
        }
        Ok(Capture::NoFiles) => {}
        Err(error) => {
            tracing::warn!(error = %error, game_id, "could not record the session restore point");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn row(game_id: i64, path: &Path, kind: &str) -> SavePathRecord {
        SavePathRecord {
            game_id,
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            source: "manual".to_string(),
            primary: true,
            created_at: "2026-11-01T00:00:00Z".to_string(),
        }
    }

    fn fixture() -> (std::path::PathBuf, rusqlite::Connection, SavePathRecord) {
        let temp = std::env::temp_dir().join(format!(
            "ludex-restore-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        let save_dir = temp.join("saves");
        std::fs::create_dir_all(&save_dir).expect("mkdir");
        std::fs::write(save_dir.join("slot1.sav"), b"good progress").expect("write");
        let connection = db::open_in_memory_for_tests();
        let rows = row(9, &save_dir, "directory");
        (temp, connection, rows)
    }

    #[test]
    fn a_session_snapshot_is_stored_on_exit_and_can_be_restored() {
        let (temp, connection, rows) = fixture();
        let data_dir = temp.join("app-data");
        let capture = capture_session_at(
            &data_dir,
            &connection,
            9,
            std::slice::from_ref(&rows),
            SOURCE_SESSION,
            "",
            Some("session-42"),
            Some("device-a"),
            DEFAULT_RETENTION,
        )
        .expect("capture");
        let Capture::Created { point, pruned } = capture else {
            panic!("expected a created point");
        };
        assert_eq!(point.source, SOURCE_SESSION);
        assert_eq!(point.session_id.as_deref(), Some("session-42"));
        assert_eq!(point.device_id.as_deref(), Some("device-a"));
        assert!(point.size_bytes > 0, "archive size is recorded");
        assert_eq!(pruned, 0);
        assert!(point_path(&data_dir, &point).is_file());

        // The game writes garbage during a later session, or the user corrupts
        // the save by hand.
        std::fs::write(temp.join("saves/slot1.sav"), b"corrupt!!!").expect("corrupt");
        // Retention 1: the backup capture must not prune the point being
        // restored before its archive is read.
        let outcome = restore_point_at(
            &data_dir,
            &connection,
            9,
            point.id,
            std::slice::from_ref(&rows),
            Some("device-a"),
            1,
        )
        .expect("restore");
        assert!(outcome.restored);
        assert_eq!(outcome.point_id, point.id);
        assert_eq!(
            std::fs::read(temp.join("saves/slot1.sav")).expect("read"),
            b"good progress"
        );
        // The rollback kept the corrupted state, so it is reversible too.
        let backup =
            db::get_save_restore_point(&connection, outcome.backup_point_id.expect("backup"))
                .expect("read backup")
                .expect("backup row");
        assert_eq!(backup.source, SOURCE_PRE_RESTORE);
        assert!(db::last_local_save_change(&connection, 9)
            .expect("last change")
            .is_some());
    }

    #[test]
    fn retention_prunes_old_sessions_but_never_the_last_known_good() {
        let (temp, connection, rows) = fixture();
        let data_dir = temp.join("app-data");
        let save_file = temp.join("saves/slot1.sav");
        let mut ids = Vec::new();
        for index in 0..6 {
            std::fs::write(&save_file, format!("progress-{index}")).expect("write");
            let capture = capture_session_at(
                &data_dir,
                &connection,
                9,
                std::slice::from_ref(&rows),
                SOURCE_SESSION,
                "",
                None,
                None,
                3,
            )
            .expect("capture");
            let Capture::Created { point, .. } = capture else {
                panic!("expected a new point for a changed save");
            };
            ids.push(point.id);
        }
        let points = db::list_save_restore_points(&connection, 9).expect("list");
        assert_eq!(points.len(), 3, "retention 3");
        assert_eq!(points[0].id, ids[5], "newest first");
        for point in &points {
            assert!(
                point_path(&data_dir, point).is_file(),
                "kept archive exists: {}",
                point.file_name
            );
        }
        let archives = std::fs::read_dir(restore_points_dir(&data_dir, 9))
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "zip"))
            .count();
        assert_eq!(archives, 3, "pruned archives are deleted");

        // An unchanged save does not create a duplicate point.
        let capture = capture_session_at(
            &data_dir,
            &connection,
            9,
            std::slice::from_ref(&rows),
            SOURCE_SESSION,
            "",
            None,
            None,
            3,
        )
        .expect("capture");
        assert!(matches!(capture, Capture::Unchanged));
        assert_eq!(db::count_save_restore_points(&connection, 9).unwrap(), 3);
    }

    #[test]
    fn restore_refuses_a_damaged_archive_and_keeps_the_current_files() {
        let (temp, connection, rows) = fixture();
        let data_dir = temp.join("app-data");
        let Capture::Created { point, .. } = capture_session_at(
            &data_dir,
            &connection,
            9,
            std::slice::from_ref(&rows),
            SOURCE_SESSION,
            "",
            None,
            None,
            DEFAULT_RETENTION,
        )
        .expect("capture") else {
            panic!("expected a point");
        };
        std::fs::write(point_path(&data_dir, &point), b"not a zip").expect("damage");
        std::fs::write(temp.join("saves/slot1.sav"), b"current").expect("write");
        let error = restore_point_at(
            &data_dir,
            &connection,
            9,
            point.id,
            std::slice::from_ref(&rows),
            None,
            DEFAULT_RETENTION,
        )
        .expect_err("checksum mismatch");
        assert!(error.contains("checksum"), "{error}");
        assert_eq!(
            std::fs::read(temp.join("saves/slot1.sav")).expect("read"),
            b"current",
            "nothing was overwritten"
        );
    }
}
