//! Application state managed by Tauri.

use rusqlite::Connection;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::db;
use crate::game_executables::ExecutableCache;
use crate::install::{InstallJob, InstallParams};
use crate::profiles;
use crate::secrets::SecretStore;
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
    /// Staging folder of the current attempt. A resumable stream install may
    /// adopt the folder of a previous attempt, so cancel/pause must use the
    /// path the worker actually wrote to (#115).
    pub staging_path: Mutex<Option<String>>,
    /// Hex bitset of completed stream-install entries (#115). Kept in the job
    /// entry so the regular job persistence writes it with the rest of the row.
    pub stream_completed: Mutex<Option<String>>,
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

    pub fn set_staging_path(&self, path: Option<String>) {
        match self.staging_path.lock() {
            Ok(mut slot) => *slot = path,
            Err(poisoned) => *poisoned.into_inner() = path,
        }
    }

    pub fn staging_path(&self) -> Option<String> {
        self.staging_path.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn set_stream_completed(&self, value: Option<String>) {
        match self.stream_completed.lock() {
            Ok(mut slot) => *slot = value,
            Err(poisoned) => *poisoned.into_inner() = value,
        }
    }

    pub fn stream_completed(&self) -> Option<String> {
        self.stream_completed
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default()
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
    /// Cancellation flags for `run_installer` runs, per game (#7).
    pub installers: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    /// Short-lived executable listings for the Launch options picker (#143).
    pub executables: Mutex<ExecutableCache>,
    pub http: reqwest::Client,
    /// Install jobs run one at a time by default to avoid disk thrash on large
    /// archives. Additional requests wait in the queued phase.
    pub install_slots: Arc<tokio::sync::Semaphore>,
    /// Per-profile tokens (#24): OS keychain, or a local SQLite fallback.
    pub secrets: SecretStore,
    /// Pending desktop SSO sign-ins (#157), keyed by the generated state.
    pub oidc_logins: Arc<crate::oidc_login::LoginSessions>,
    /// In-game overlay runtime (#144): open state and native listener status.
    pub overlay: crate::overlay::OverlayRuntime,
    /// Local crash reports and opt-in diagnostics (#171).
    pub diagnostics: crate::diagnostics::DiagnosticsStore,
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
        // #24: the existing single-server configuration becomes the first
        // saved profile, with a stable id.
        profiles::sync_profiles(&mut loaded, &crate::util::now_rfc3339());
        settings::ensure_dirs(&loaded)?;
        settings::save(&settings_path, &loaded)?;

        let secrets = SecretStore::new(&app_data);
        if let Some(token) = crate::util::non_empty(loaded.token.as_deref()) {
            if !loaded.active_server_id.is_empty() {
                if let Err(error) =
                    secrets.set(&profiles::secret_key(&loaded.active_server_id), token)
                {
                    tracing::warn!(error = %error, "could not copy the current token into the secret store");
                }
            }
        }

        // Each server profile has its own index; the first profile adopts the
        // legacy database so nothing is lost on upgrade.
        let database_path = profiles::adopt_legacy_database(&app_data, &loaded.active_server_id)?;
        let database = db::open(&database_path)?;

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

        let mut server_urls: Vec<String> = loaded
            .servers
            .iter()
            .map(|profile| profile.url.clone())
            .collect();
        if !loaded.server_url.trim().is_empty() {
            server_urls.push(loaded.server_url.clone());
        }
        let diagnostics = crate::diagnostics::DiagnosticsStore::new(
            Some(crate::diagnostics::store_path(&app_data)),
            crate::diagnostics::local_redaction_context(server_urls),
        );

        Ok(Self {
            app_data,
            settings_path,
            settings: Mutex::new(loaded),
            db: Mutex::new(database),
            server: AsyncMutex::new(None),
            jobs: Mutex::new(HashMap::new()),
            processes: Mutex::new(HashMap::new()),
            installers: Mutex::new(HashMap::new()),
            executables: Mutex::new(ExecutableCache::default()),
            http,
            install_slots: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_INSTALLS)),
            secrets,
            oidc_logins: Arc::new(crate::oidc_login::LoginSessions::new()),
            overlay: crate::overlay::OverlayRuntime::default(),
            diagnostics,
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
        let mut next = self.settings_snapshot();
        change(&mut next);
        self.persist_settings(next)
    }

    /// Validate, save and install a complete settings value.
    ///
    /// Profile metadata is synchronised first (#24), and the active token is
    /// copied to the secret store; a secret store failure never blocks a
    /// settings save (the token stays in `settings.json` as before).
    pub fn persist_settings(&self, mut next: LocalSettings) -> Result<LocalSettings, String> {
        let mut guard = self
            .settings
            .lock()
            .map_err(|_| "The settings lock is poisoned; restart Ludex.".to_string())?;
        let previous_active = guard.active_server_id.clone();
        next.normalize(&self.app_data);
        profiles::sync_profiles(&mut next, &crate::util::now_rfc3339());
        settings::validate(&next)?;
        settings::ensure_dirs(&next)?;
        settings::save(&self.settings_path, &next)?;
        if let Some(token) = crate::util::non_empty(next.token.as_deref()) {
            if !next.active_server_id.is_empty() {
                if let Err(error) = self
                    .secrets
                    .set(&profiles::secret_key(&next.active_server_id), token)
                {
                    tracing::warn!(error = %error, "the token was saved in settings.json but not in the secret store");
                }
            }
        }
        *guard = next.clone();
        drop(guard);

        // A different server means a different index: adopt or open it now so
        // installs made right after the switch are written to the right place.
        // Done after releasing the settings lock to keep the lock order simple.
        if next.active_server_id != previous_active {
            if let Err(error) = self.open_server_database(&next.active_server_id) {
                tracing::warn!(error = %error, "the desktop index was not switched");
            }
        }
        Ok(next)
    }

    /// Swap the desktop index when the active server changes (#24). Refused
    /// while a job or a game is running by the command layer.
    pub fn open_server_database(&self, active_id: &str) -> Result<(), String> {
        let path = profiles::adopt_legacy_database(&self.app_data, active_id)?;
        let connection = db::open(&path)?;
        let mut guard = self
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        *guard = connection;
        tracing::info!(server = active_id, path = %path.display(), "desktop index switched");
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
