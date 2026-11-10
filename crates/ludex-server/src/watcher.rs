//! Optional folder watcher (#44).
//!
//! When `LUDEX_WATCHER_ENABLED=true`, every enabled library root is watched
//! recursively with [`notify`]. Filesystem events are coalesced over an idle
//! window (`LUDEX_WATCHER_DEBOUNCE_MS`, default 5 s) and then trigger the
//! normal scan pipeline, so the watcher never writes to the database itself.
//!
//! Partial files written by the ingest manager (`.part`), the desktop client
//! and the scanner's own temporary names are ignored to avoid scan loops.
//!
//! Platform limits: Linux uses inotify, macOS FSEvents/kqueue, Windows
//! `ReadDirectoryChangesW`. Network filesystems (NFS, CIFS/SMB) and some FUSE
//! mounts do not emit change events; on those the periodic scan interval
//! remains the only refresh path. Watch descriptors are finite: very large
//! trees may exhaust `fs.inotify.max_user_watches` (the error is reported in
//! `lastError`).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use tokio::sync::{mpsc, watch};

use crate::state::AppState;

/// Commands accepted by the watcher manager.
#[derive(Debug, Clone, Copy)]
pub enum Command {
    /// Re-read the enabled roots from the database and re-watch them.
    Reload,
}

/// One watched root, as reported by `GET /api/library/watcher`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherRoot {
    /// Root id.
    pub id: i64,
    /// Root label.
    pub label: String,
    /// Root path.
    pub path: String,
}

/// Live watcher state.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherStatus {
    /// Whether the watcher task is running.
    pub enabled: bool,
    /// Idle window used to coalesce events.
    pub debounce_ms: u64,
    /// Roots currently watched.
    pub roots: Vec<WatcherRoot>,
    /// Last relevant filesystem event.
    pub last_event_at: Option<DateTime<Utc>>,
    /// Last scan triggered by the watcher.
    pub last_scan_at: Option<DateTime<Utc>>,
    /// Whether a scan trigger is waiting for the debounce window.
    pub pending: bool,
    /// Last watch error (missing path, inotify limit, ...).
    pub last_error: Option<String>,
}

/// Handle used by routes to ask the watcher to reload its roots.
///
/// Dropping every handle stops the watcher task.
#[derive(Debug, Clone)]
pub struct WatcherHandle {
    commands: mpsc::UnboundedSender<Command>,
    _cancel: watch::Sender<bool>,
}

impl WatcherHandle {
    /// Re-reads roots and re-watches them. Returns false once the task stopped.
    pub fn reload(&self) -> bool {
        self.commands.send(Command::Reload).is_ok()
    }
}

/// Starts the watcher unless it is disabled. Returns a handle for runtime root
/// changes.
pub async fn start(state: &AppState) -> Option<WatcherHandle> {
    if !state.config.watcher_enabled {
        return None;
    }
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let handle = WatcherHandle {
        commands: command_tx,
        _cancel: cancel_tx,
    };
    {
        let mut status = state.watcher.write().await;
        status.enabled = true;
        status.debounce_ms =
            u64::try_from(state.config.watcher_debounce.as_millis()).unwrap_or(u64::MAX);
    }
    tokio::spawn(manager(state.clone(), command_rx, cancel_rx));
    Some(handle)
}

struct Watched {
    id: i64,
    label: String,
    path: PathBuf,
}

async fn manager(
    state: AppState,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut cancel: watch::Receiver<bool>,
) {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(move |event| {
        let _ = event_tx.send(event);
    }) {
        Ok(watcher) => watcher,
        Err(error) => {
            tracing::error!(%error, "could not create the library folder watcher");
            state.watcher.write().await.last_error = Some(error.to_string());
            return;
        }
    };

    let mut watched: Vec<Watched> = Vec::new();
    reload(&state, &mut watcher, &mut watched).await;

    let debounce = state.config.watcher_debounce;
    let mut pending = false;
    let mut deadline: Option<tokio::time::Instant> = None;

    loop {
        let sleep = async {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
            }
            command = commands.recv() => {
                if command.is_none() {
                    break;
                }
                reload(&state, &mut watcher, &mut watched).await;
            }
            event = event_rx.recv() => {
                match event {
                    None => break,
                    Some(Err(error)) => {
                        tracing::warn!(%error, "library watcher error");
                        state.watcher.write().await.last_error = Some(error.to_string());
                    }
                    Some(Ok(event)) => {
                        if event
                            .paths
                            .iter()
                            .any(|path| is_relevant(&watched, path))
                        {
                            let now = Utc::now();
                            pending = true;
                            deadline = Some(tokio::time::Instant::now() + debounce);
                            let mut status = state.watcher.write().await;
                            status.last_event_at = Some(now);
                            status.pending = true;
                        }
                    }
                }
            }
            () = sleep => {
                deadline = None;
                if !pending {
                    continue;
                }
                if crate::scanner::trigger(&state).await {
                    pending = false;
                    let mut status = state.watcher.write().await;
                    status.pending = false;
                    status.last_scan_at = Some(Utc::now());
                } else {
                    // A scan is already running: retry once it can be queued.
                    deadline = Some(tokio::time::Instant::now() + debounce);
                }
            }
        }
    }
    tracing::debug!("library watcher stopped");
}

async fn reload(
    state: &AppState,
    watcher: &mut notify::RecommendedWatcher,
    watched: &mut Vec<Watched>,
) {
    for root in watched.iter() {
        if let Err(error) = watcher.unwatch(&root.path) {
            tracing::debug!(path = %root.path.display(), %error, "could not unwatch root");
        }
    }
    watched.clear();

    let roots = match crate::roots::enabled(&state.pool).await {
        Ok(roots) => roots,
        Err(error) => {
            tracing::warn!(%error, "could not list roots for the watcher");
            state.watcher.write().await.last_error = Some(error.to_string());
            return;
        }
    };

    let mut errors: Vec<String> = Vec::new();
    for root in roots {
        let path = PathBuf::from(&root.path);
        if !path.is_dir() {
            errors.push(format!("{}: not a directory", root.path));
            continue;
        }
        match watcher.watch(&path, RecursiveMode::Recursive) {
            Ok(()) => watched.push(Watched {
                id: root.id,
                label: root.label,
                path,
            }),
            Err(error) => {
                tracing::warn!(path = %root.path, %error, "could not watch library root");
                errors.push(format!("{}: {error}", root.path));
            }
        }
    }

    let mut status = state.watcher.write().await;
    status.enabled = true;
    status.roots = watched
        .iter()
        .map(|root| WatcherRoot {
            id: root.id,
            label: root.label.clone(),
            path: root.path.display().to_string(),
        })
        .collect();
    status.last_error = errors.first().cloned();
}

/// Whether an event path belongs to a watched root and is worth a scan.
fn is_relevant(watched: &[Watched], path: &Path) -> bool {
    let Some(root) = watched
        .iter()
        .filter(|root| path.starts_with(&root.path))
        .max_by_key(|root| root.path.components().count())
    else {
        return false;
    };
    let relative = path.strip_prefix(&root.path).unwrap_or(path);
    !is_ignored(relative)
}

/// Whether a path relative to a watched root is written by Ludex itself or is
/// a partial download.
///
/// Hidden files and directories are ignored by the scanner too, so watching
/// them would only trigger scans that find nothing. The check is relative to
/// the root: a hidden ancestor of the root (a mount point or a temp directory)
/// must not silence the whole root.
fn is_ignored(relative: &Path) -> bool {
    for component in relative.components() {
        let name = component.as_os_str().to_string_lossy();
        if name.starts_with('.') && name != "." && name != ".." {
            return true;
        }
        if name == "ludex-tmp" {
            return true;
        }
    }
    let Some(name) = relative.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let lowered = name.to_ascii_lowercase();
    lowered.ends_with(".part")
        || lowered.ends_with(".tmp")
        || lowered.ends_with(".download")
        || lowered.ends_with(".crdownload")
        || lowered.starts_with(".tmp-")
        || lowered.starts_with("~$")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::AppState;
    use std::fs;
    use std::time::Duration;

    async fn test_state(
        root: &Path,
        debounce_ms: u64,
        enabled: bool,
    ) -> (AppState, mpsc::Receiver<crate::scanner::ScanRequest>) {
        let data = root.join("data");
        let media = root.join("media");
        fs::create_dir_all(&data).expect("data");
        fs::create_dir_all(&media).expect("media");
        let mut config = Config::for_local(root.join("games"), data, media);
        config.database_url = format!(
            "sqlite://{}?mode=rwc",
            root.join("data").join("ludex.db").display()
        );
        config.watcher_enabled = enabled;
        config.watcher_debounce = Duration::from_millis(debounce_ms);
        let (scan_tx, scan_rx) = mpsc::channel(16);
        let state = AppState::for_tests(config, scan_tx).await;
        (state, scan_rx)
    }

    #[test]
    fn partial_and_hidden_paths_are_ignored() {
        assert!(is_ignored(Path::new("Game.zip.part")));
        assert!(is_ignored(Path::new("Game.zip.tmp")));
        assert!(is_ignored(Path::new(".ludex-tmp/Game.zip")));
        assert!(is_ignored(Path::new(".hidden/Game.zip")));
        assert!(is_ignored(Path::new("~$Game.zip")));
        assert!(!is_ignored(Path::new("Game (2020).zip")));
        assert!(!is_ignored(Path::new("Sub/Game (2020).zip")));
    }

    #[tokio::test]
    async fn disabled_watcher_does_not_start() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (state, _rx) = test_state(temp.path(), 50, false).await;
        assert!(start(&state).await.is_none());
    }

    #[tokio::test]
    async fn new_archive_triggers_a_scan_after_the_debounce() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("games");
        fs::create_dir_all(&root).expect("root");
        let (state, mut rx) = test_state(temp.path(), 120, true).await;
        let handle = start(&state).await.expect("watcher starts");
        assert!(handle.reload());
        tokio::time::sleep(Duration::from_millis(150)).await;

        fs::write(root.join("Hades (2020).zip"), vec![0_u8; 2048]).expect("write archive");
        let request = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("scan triggered")
            .expect("request");
        assert_eq!(request, crate::scanner::ScanRequest::all());
        assert!(state.watcher.read().await.last_event_at.is_some());
    }

    #[tokio::test]
    async fn debounce_coalesces_bursts_into_one_scan() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("games");
        fs::create_dir_all(&root).expect("root");
        let (state, mut rx) = test_state(temp.path(), 200, true).await;
        let handle = start(&state).await.expect("watcher starts");
        assert!(handle.reload());
        tokio::time::sleep(Duration::from_millis(150)).await;

        for index in 0..3 {
            fs::write(root.join(format!("Game {index}.zip")), vec![0_u8; 2048]).expect("write");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("scan triggered");
        assert!(first.is_some());
        // No second request within the debounce window.
        assert!(tokio::time::timeout(Duration::from_millis(400), rx.recv())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn partial_files_do_not_trigger_scans() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("games");
        fs::create_dir_all(&root).expect("root");
        let (state, mut rx) = test_state(temp.path(), 80, true).await;
        let handle = start(&state).await.expect("watcher starts");
        assert!(handle.reload());
        tokio::time::sleep(Duration::from_millis(150)).await;

        fs::write(root.join("Hades (2020).zip.part"), vec![0_u8; 2048]).expect("write");
        assert!(tokio::time::timeout(Duration::from_millis(400), rx.recv())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn status_reports_watched_roots_and_debounce() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("games");
        fs::create_dir_all(&root).expect("root");
        let (state, _rx) = test_state(temp.path(), 80, true).await;
        let handle = start(&state).await.expect("watcher starts");
        assert!(handle.reload());
        tokio::time::sleep(Duration::from_millis(150)).await;
        let status = state.watcher.read().await.clone();
        assert!(status.enabled);
        assert_eq!(status.debounce_ms, 80);
        assert_eq!(status.roots.len(), 1);
        assert_eq!(status.roots[0].path, root.display().to_string());
    }
}
