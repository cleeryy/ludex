//! The warm-start queue (#293).
//!
//! One background worker runs one game at a time. The queue is in memory: a
//! restart loses a queued job but never a cache, and
//! [`recover_interrupted`] turns a stale `queued`/`running` record into an
//! honest cancellation instead of a spinner that never moves.
//!
//! The worker never touches install state: a failure only writes the game's
//! `shaderPrep` record, so the game stays launchable exactly as before.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager};

use super::{
    cache_env, cache_layout, clamp_budget, clear_cache, enabled_for, record, record_for_outcome,
    supported, view_for, CommandSpawner, Outcome, Pending, PrepRecord, PrepState, PrepTick,
    ShaderPrepProgress, ShaderPrepView, Supervision,
};
use crate::inhibit::{self, Reason};
use crate::settings::LocalSettings;
use crate::state::AppState;
use crate::util::now_rfc3339;

/// Name of the event carrying every state change and progress sample.
pub const PROGRESS_EVENT: &str = "shader-prep://progress";

/// Name of the event fired once when a run ends, so the panel can refetch the
/// persisted view and the live cache size.
pub const CHANGED_EVENT: &str = "shader-prep://changed";

/// Queue handle stored in [`AppState`].
pub type ShaderPrepQueue = super::Queue;

/* ------------------------------- persistence ------------------------------ */

fn save_record(app: &AppHandle, game_id: i64, record: PrepRecord) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.update_settings(|settings| {
        settings
            .shader_prep
            .insert(game_id.to_string(), record.clone());
    })?;
    Ok(())
}

fn load_record(app: &AppHandle, game_id: i64) -> PrepRecord {
    let settings = app.state::<AppState>().settings_snapshot();
    record(&settings, game_id).cloned().unwrap_or_default()
}

fn budget_secs(settings: &LocalSettings) -> u64 {
    clamp_budget(settings.shader_prep_budget_secs)
}

/// Resolve an installed game or a readable error.
fn installed(state: &AppState, game_id: i64) -> Result<crate::db::InstalledGame, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    crate::db::get_installed(&connection, game_id)?
        .ok_or_else(|| format!("Game {game_id} is not installed on this device."))
}

fn view(app: &AppHandle, game_id: i64) -> ShaderPrepView {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let budget = budget_secs(&settings);
    let mut next = view_for(&settings, &state.app_data, game_id, budget);
    // The queue is authoritative while the app runs: a settings file written a
    // moment before the worker picked the job up can still say `queued`.
    if state.shader_prep.is_active(game_id) {
        let previous = record(&settings, game_id).cloned().unwrap_or_default();
        if !previous.state.is_active() {
            next.state = PrepState::Queued;
        }
    }
    next
}

fn emit_progress(
    app: &AppHandle,
    pending: &Pending,
    state: PrepState,
    cache_bytes: u64,
    elapsed_ms: u64,
    budget_secs: u64,
    message: Option<String>,
) {
    let _ = app.emit(
        PROGRESS_EVENT,
        ShaderPrepProgress {
            game_id: pending.game_id,
            title: pending.title.clone(),
            state,
            cache_bytes,
            elapsed_ms,
            budget_secs,
            message,
        },
    );
}

/* --------------------------------- worker --------------------------------- */

/// Queue one game and make sure the worker runs.
fn enqueue(app: &AppHandle, game_id: i64, title: &str) -> Result<bool, String> {
    if !supported() {
        return Err("Shader preparation is only available on Linux.".to_string());
    }
    let state = app.state::<AppState>();
    if state.shader_prep.is_active(game_id) {
        return Ok(false);
    }
    let previous = load_record(app, game_id);
    let queued = PrepRecord {
        state: PrepState::Queued,
        message: None,
        // A queued job must not inherit the reason of an older failure.
        ..previous
    };
    save_record(app, game_id, queued.clone())?;
    let pending = Pending {
        game_id,
        title: title.to_string(),
    };
    if !state.shader_prep.push(pending.clone()) {
        return Ok(false);
    }
    let settings = state.settings_snapshot();
    emit_progress(
        app,
        &pending,
        PrepState::Queued,
        queued.cache_bytes,
        0,
        budget_secs(&settings),
        None,
    );
    ensure_worker(app);
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(true)
}

/// Start the single worker thread if none runs.
fn ensure_worker(app: &AppHandle) {
    let state = app.state::<AppState>();
    let queue = Arc::clone(&state.shader_prep);
    if !queue.claim_worker() {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("ludex-shader-prep".to_string())
        .spawn(move || drain(app, queue));
    if let Err(error) = spawned {
        tracing::warn!(error = %error, "could not start the shader preparation worker");
        state.shader_prep.release_worker();
    }
}

/// Run queued games until the queue is empty, then give the worker slot back.
fn drain(app: AppHandle, queue: Arc<ShaderPrepQueue>) {
    loop {
        let Some((pending, cancel)) = queue.next() else {
            if queue.retire_worker() {
                return;
            }
            continue;
        };
        let game_id = pending.game_id;
        run_one(&app, &pending, cancel);
        queue.finish();
        let _ = app.emit(CHANGED_EVENT, ());
        tracing::info!(game_id, "shader preparation finished");
    }
}

/// One supervised warm start, from the running record to the persisted
/// outcome.
fn run_one(app: &AppHandle, pending: &Pending, cancel: Arc<AtomicBool>) {
    let game_id = pending.game_id;
    let started = Instant::now();
    let settings = app.state::<AppState>().settings_snapshot();
    let budget = budget_secs(&settings);

    let previous = load_record(app, game_id);
    let running = PrepRecord {
        state: PrepState::Running,
        message: None,
        ..previous
    };
    if let Err(error) = save_record(app, game_id, running.clone()) {
        tracing::warn!(error = %error, game_id, "could not record the shader preparation start");
    }
    emit_progress(
        app,
        pending,
        PrepState::Running,
        running.cache_bytes,
        0,
        budget,
        None,
    );
    let _ = app.emit(CHANGED_EVENT, ());

    let _hold = inhibit::hold(Reason::prepare(game_id, &pending.title));

    let outcome = warm_start(app, game_id, &cancel, budget, |tick| {
        emit_progress(
            app,
            pending,
            PrepState::Running,
            tick.bytes,
            u64::try_from(tick.elapsed.as_millis()).unwrap_or(u64::MAX),
            budget,
            None,
        );
    });

    let previous = load_record(app, game_id);
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let next = record_for_outcome(&previous, &outcome, &now_rfc3339(), duration_ms);
    if let Err(error) = save_record(app, game_id, next.clone()) {
        tracing::warn!(error = %error, game_id, "could not record the shader preparation result");
    }
    match &outcome {
        Outcome::Done { bytes, reason } => tracing::info!(
            game_id,
            bytes,
            reason = ?reason,
            duration_ms,
            "shader warm start done"
        ),
        Outcome::Cancelled { bytes } => {
            tracing::info!(game_id, bytes, "shader warm start cancelled")
        }
        Outcome::Failed { message, .. } => {
            tracing::warn!(game_id, reason = %message, "shader warm start failed (non-fatal)")
        }
    }
    tracing::debug!(
        game_id,
        state = next.state.as_str(),
        "shader preparation state"
    );
    emit_progress(
        app,
        pending,
        next.state,
        next.cache_bytes,
        duration_ms,
        budget,
        next.message.clone(),
    );
}

/// Resolve the game, build the launch command and supervise it.
fn warm_start(
    app: &AppHandle,
    game_id: i64,
    cancel: &Arc<AtomicBool>,
    budget: u64,
    report: impl FnMut(PrepTick),
) -> Outcome {
    let state = app.state::<AppState>();
    if crate::resume::running_game_session(&state, game_id).is_some() {
        return Outcome::Failed {
            message: "The game is running. Close it and prepare the shaders again.".to_string(),
            bytes: 0,
        };
    }
    let (installed, options) = {
        let Ok(connection) = state.db.lock() else {
            return Outcome::Failed {
                message: "The local database is unavailable; restart Ludex.".to_string(),
                bytes: 0,
            };
        };
        let Ok(installed) = crate::db::get_installed(&connection, game_id) else {
            return Outcome::Failed {
                message: format!("Game {game_id} is not installed on this device."),
                bytes: 0,
            };
        };
        let Some(installed) = installed else {
            return Outcome::Failed {
                message: format!("Game {game_id} is not installed on this device."),
                bytes: 0,
            };
        };
        let Ok(options) = crate::db::get_launch_options(&connection, game_id) else {
            return Outcome::Failed {
                message: "The launch options of this game could not be read.".to_string(),
                bytes: 0,
            };
        };
        (installed, options)
    };

    let layout = cache_layout(&state.app_data, game_id);
    let mut spec = match crate::launch::prepare_warm_start(app, &installed, &options) {
        Ok(spec) => spec,
        Err(message) => return Outcome::Failed { message, bytes: 0 },
    };
    // The controlled run always writes Ludex's cache folder: the size shown in
    // the UI is the size of what the next launch reads back. A per-game
    // environment override still wins at launch, which `docs/SHADERS.md`
    // documents as "no benefit".
    for (key, value) in cache_env(&layout) {
        spec.env.insert(key.to_string(), value);
    }

    let cfg = Supervision::new(std::time::Duration::from_secs(budget));
    super::run_with(
        &mut CommandSpawner,
        &spec,
        &layout,
        &cfg,
        || cancel.load(Ordering::SeqCst),
        report,
    )
}

/* -------------------------------- commands -------------------------------- */

/// One game's preparation state, cache size and budget.
#[tauri::command]
pub fn shader_prep_stats(app: AppHandle, game_id: i64) -> Result<ShaderPrepView, String> {
    installed(app.state::<AppState>().inner(), game_id)?;
    Ok(view(&app, game_id))
}

/// Every game with a preparation record, plus the queue's live jobs. Used by
/// the Downloads screen for the post-install section.
#[tauri::command]
pub fn list_shader_prep(app: AppHandle) -> Result<Vec<ShaderPrepView>, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let mut games: Vec<i64> = settings
        .shader_prep
        .keys()
        .filter_map(|key| key.parse::<i64>().ok())
        .collect();
    for game_id in state.shader_prep.active_games() {
        if !games.contains(&game_id) {
            games.push(game_id);
        }
    }
    games.sort_unstable();
    let mut views: Vec<ShaderPrepView> = games.into_iter().map(|id| view(&app, id)).collect();
    // Newest prepared first; never-prepared records fall to the bottom.
    views.sort_by(|left, right| {
        right
            .prepared_at
            .cmp(&left.prepared_at)
            .then_with(|| left.game_id.cmp(&right.game_id))
    });
    Ok(views)
}

/// Turn "prepare after install" on or off for one game.
#[tauri::command]
pub fn set_shader_prep_enabled(
    app: AppHandle,
    game_id: i64,
    enabled: bool,
) -> Result<ShaderPrepView, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let mut next = record(&settings, game_id)
        .cloned()
        .unwrap_or_else(|| PrepRecord::seeded(settings.shader_prep_after_install));
    next.enabled = enabled;
    save_record(&app, game_id, next)?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(view(&app, game_id))
}

/// Change the time budget every warm start uses, in seconds.
#[tauri::command]
pub fn set_shader_prep_budget(app: AppHandle, seconds: u64) -> Result<u64, String> {
    let state = app.state::<AppState>();
    let clamped = clamp_budget(seconds);
    state.update_settings(|settings| settings.shader_prep_budget_secs = clamped)?;
    Ok(clamped)
}

/// Start (or re-queue) a warm start for one installed game.
#[tauri::command]
pub fn prepare_game_shaders(app: AppHandle, game_id: i64) -> Result<ShaderPrepView, String> {
    let state = app.state::<AppState>();
    let installed = installed(state.inner(), game_id)?;
    enqueue(&app, game_id, &installed.title)?;
    Ok(view(&app, game_id))
}

/// Cancel a queued or running preparation. The partial cache is kept.
#[tauri::command]
pub fn cancel_shader_prep(app: AppHandle, game_id: i64) -> Result<ShaderPrepView, String> {
    let state = app.state::<AppState>();
    let (running, queued) = state.shader_prep.cancel(game_id);
    if !running && !queued {
        return Err("No shader preparation is queued for this game.".to_string());
    }
    if queued && !running {
        let previous = load_record(&app, game_id);
        let next = PrepRecord {
            state: PrepState::Cancelled,
            message: Some("Cancelled before it started.".to_string()),
            ..previous
        };
        save_record(&app, game_id, next)?;
        let _ = app.emit(CHANGED_EVENT, ());
    }
    Ok(view(&app, game_id))
}

/// Delete the prepared caches of one game. Refused while a run owns them.
#[tauri::command]
pub fn clear_shader_cache(app: AppHandle, game_id: i64) -> Result<ShaderPrepView, String> {
    let state = app.state::<AppState>();
    if state.shader_prep.is_active(game_id) {
        return Err("Cancel the shader preparation before clearing its cache.".to_string());
    }
    let layout = cache_layout(&state.app_data, game_id);
    clear_cache(&layout)?;
    let previous = load_record(&app, game_id);
    let next = PrepRecord {
        state: PrepState::Never,
        prepared_at: None,
        cache_bytes: 0,
        duration_ms: None,
        message: None,
        ..previous
    };
    save_record(&app, game_id, next.clone())?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(view(&app, game_id))
}

/// Turn a stale `queued`/`running` record into a cancellation at startup: the
/// in-memory queue did not survive the restart, so a spinner would lie.
pub fn recover_interrupted(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let stale: Vec<i64> = settings
        .shader_prep
        .iter()
        .filter(|(_, entry)| entry.state.is_active())
        .filter_map(|(key, _)| key.parse::<i64>().ok())
        .collect();
    if stale.is_empty() {
        return;
    }
    let message =
        "Ludex closed before the shader preparation finished. Run it again from the game page.";
    let mut touched = 0u32;
    for game_id in stale {
        let mut entry = load_record(app, game_id);
        entry.state = PrepState::Cancelled;
        entry.message = Some(message.to_string());
        if save_record(app, game_id, entry).is_ok() {
            touched += 1;
        }
    }
    if touched > 0 {
        tracing::info!(count = touched, "cancelled stale shader preparations");
        let _ = app.emit(CHANGED_EVENT, ());
    }
}

/// Entry point for the install scheduler: a finished install prepares itself
/// when the game (or the global default) asks for it. Never blocking, never
/// fatal.
pub fn on_install_complete(app: &AppHandle, game_id: i64, title: &str) {
    if !supported() {
        return;
    }
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    if !enabled_for(&settings, game_id) {
        return;
    }
    if let Err(error) = enqueue(app, game_id, title) {
        tracing::warn!(error = %error, game_id, "could not queue the shader preparation");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_read_through_the_clamp() {
        let settings = LocalSettings {
            shader_prep_budget_secs: 0,
            ..LocalSettings::default()
        };
        assert_eq!(budget_secs(&settings), super::super::DEFAULT_BUDGET_SECS);
        let settings = LocalSettings {
            shader_prep_budget_secs: 1,
            ..LocalSettings::default()
        };
        assert_eq!(budget_secs(&settings), super::super::MIN_BUDGET_SECS);
        let settings = LocalSettings {
            shader_prep_budget_secs: 120,
            ..LocalSettings::default()
        };
        assert_eq!(budget_secs(&settings), 120);
    }
}
