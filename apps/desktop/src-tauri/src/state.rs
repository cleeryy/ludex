//! Application state managed by Tauri.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::install::{InstallJob, InstallParams};
use crate::settings::{self, LocalSettings};

/// Install jobs run one at a time by default to avoid disk thrash on large
/// archives. Additional requests wait in the queued phase.
const MAX_CONCURRENT_INSTALLS: usize = 1;

/// A launched game process, watched until its whole process tree exits.
pub struct ProcessEntry {
    pub child: Mutex<std::process::Child>,
    /// Root PID of the launched game (the direct child of Ludex).
    pub pid: u32,
    pub game_id: i64,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub started: std::time::Instant,
}

/// An install job: immutable request parameters, cancellation, live status and
/// the paths needed to persist/resume it.
pub struct JobEntry {
    pub params: InstallParams,
    pub cancel: CancellationToken,
    /// Cancelled by `pause_install`, replaced by `resume_install`. A paused job
    /// keeps its `.part` file and its place in the queue.
    pub pause: Mutex<CancellationToken>,
    /// True while a worker is running for this entry, so resuming twice cannot
    /// start two downloads for the same job.
    pub running: AtomicBool,
    pub job: Mutex<InstallJob>,
    pub created_at: String,
    pub part_path: Mutex<Option<String>>,
    pub install_path: Mutex<Option<String>>,
}

impl JobEntry {
    pub fn snapshot(&self) -> InstallJob {
        match self.job.lock() {
            Ok(job) => job.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn update(&self, change: impl FnOnce(&mut InstallJob)) {
        match self.job.lock() {
            Ok(mut job) => change(&mut job),
            Err(poisoned) => change(&mut poisoned.into_inner()),
        }
    }

    pub fn set_part_path(&self, path: Option<String>) {
        match self.part_path.lock() {
            Ok(mut slot) => *slot = path,
            Err(poisoned) => *poisoned.into_inner() = path,
        }
    }

    pub fn set_install_path(&self, path: Option<String>) {
        match self.install_path.lock() {
            Ok(mut slot) => *slot = path,
            Err(poisoned) => *poisoned.into_inner() = path,
        }
    }

    /// The pause token captured by the current worker.
    pub fn pause_token(&self) -> CancellationToken {
        match self.pause.lock() {
            Ok(token) => token.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn request_pause(&self) {
        self.pause_token().cancel();
    }

    /// Give the job a fresh pause token when it resumes.
    pub fn clear_pause(&self) {
        match self.pause.lock() {
            Ok(mut token) => *token = CancellationToken::new(),
            Err(poisoned) => *poisoned.into_inner() = CancellationToken::new(),
        }
    }

    pub fn stored_paths(&self) -> (Option<String>, Option<String>) {
        let part = self
            .part_path
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        let install = self
            .install_path
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default();
        (part, install)
    }
}

pub struct AppState {
    pub app_data: PathBuf,
    pub settings_path: PathBuf,
    pub settings: Mutex<LocalSettings>,
    pub db: Mutex<Connection>,
    pub server: AsyncMutex<Option<ludex_server::ServerHandle>>,
    pub jobs: Mutex<HashMap<String, Arc<JobEntry>>>,
    pub processes: Mutex<HashMap<String, Arc<ProcessEntry>>>,
    pub http: reqwest::Client,
    /// Install jobs run one at a time by default to avoid disk thrash on large
    /// archives. Additional requests wait in the queued phase.
    pub install_slots: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(app: &AppHandle) -> Result<Self, String> {
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("Could not resolve the application data folder: {e}"))?;
        std::fs::create_dir_all(&app_data).map_err(|e| {
            format!(
                "Could not create the application data folder {}: {e}",
                app_data.display()
            )
        })?;

        let settings_path = app_data.join("settings.json");
        let mut loaded = settings::load(&settings_path, &app_data);
        loaded.normalize(&app_data);
        settings::ensure_dirs(&loaded)?;
        settings::save(&settings_path, &loaded)?;

        let database = db::open(&app_data.join("ludex-desktop.db"))?;

        match db::mark_interrupted_jobs(
            &database,
            "Ludex closed before this install finished. Install again to resume.",
            &crate::util::now_rfc3339(),
        ) {
            Ok(0) => {}
            Ok(count) => tracing::warn!(count, "marked interrupted install jobs"),
            Err(error) => tracing::warn!(error = %error, "could not mark interrupted install jobs"),
        }

        let http = reqwest::Client::builder()
            .user_agent(format!("ludex-desktop/{}", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| format!("Could not create the HTTP client: {e}"))?;

        Ok(Self {
            app_data,
            settings_path,
            settings: Mutex::new(loaded),
            db: Mutex::new(database),
            server: AsyncMutex::new(None),
            jobs: Mutex::new(HashMap::new()),
            processes: Mutex::new(HashMap::new()),
            http,
            install_slots: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_INSTALLS)),
        })
    }

    pub fn settings_snapshot(&self) -> LocalSettings {
        match self.settings.lock() {
            Ok(settings) => settings.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Persist and apply a change to settings. The in-memory value is only
    /// updated once the file write succeeded.
    pub fn update_settings(
        &self,
        change: impl FnOnce(&mut LocalSettings),
    ) -> Result<LocalSettings, String> {
        let mut guard = self
            .settings
            .lock()
            .map_err(|_| "The settings lock is poisoned; restart Ludex.".to_string())?;
        let mut next = guard.clone();
        change(&mut next);
        next.normalize(&self.app_data);
        settings::validate(&next)?;
        settings::ensure_dirs(&next)?;
        settings::save(&self.settings_path, &next)?;
        *guard = next.clone();
        Ok(next)
    }

    /// Replace settings after an external validation/save (see `save_settings`).
    pub fn set_settings(&self, next: LocalSettings) -> Result<(), String> {
        let mut guard = self
            .settings
            .lock()
            .map_err(|_| "The settings lock is poisoned; restart Ludex.".to_string())?;
        *guard = next;
        Ok(())
    }

    pub fn prune_finished_jobs(&self) {
        let Ok(mut jobs) = self.jobs.lock() else {
            return;
        };
        if jobs.len() <= 64 {
            return;
        }
        let finished: Vec<String> = jobs
            .iter()
            .filter(|(_, entry)| {
                matches!(
                    entry.snapshot().phase,
                    crate::install::InstallPhase::Done
                        | crate::install::InstallPhase::Cancelled
                        | crate::install::InstallPhase::Error
                )
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in finished.into_iter().take(jobs.len().saturating_sub(32)) {
            jobs.remove(&id);
        }
    }
}
