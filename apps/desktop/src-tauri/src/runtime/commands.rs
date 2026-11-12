//! Tauri commands for "Make it run" (#178).
//!
//! Every command is explicit about consent: `make_it_run_apply` refuses to run
//! without `consent: true`, and the web view never passes a URL, size or
//! checksum. Those come from the shipped catalog, so a compromised web view
//! cannot redirect a download.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tokio_util::sync::CancellationToken;

use super::detect::{self, HostInfo};
use super::install::{self, InstallOutcome, InstallPhase};
use super::plan::{
    self, GameRuntimeContext, RuntimeDiagnosis, RuntimeStep, StepAction, StepStatus,
};
use super::{
    ProviderKind, RuntimeAuditEvent, RuntimeCatalog, RuntimeGamePrefs, RuntimeInstallRow,
    RuntimeProvider, RuntimeRun,
};
use crate::db;
use crate::linux_gaming::{self, LaunchMode};
use crate::state::AppState;
use crate::util;

/// Progress emitted on `runtime://progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProgress {
    pub game_id: i64,
    pub step_id: String,
    pub provider_id: Option<String>,
    /// `downloading`, `verifying`, `extracting`, `installing`, `done`,
    /// `failed`, `cancelled`.
    pub phase: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub percent: Option<f64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedStep {
    pub step_id: String,
    pub provider_id: Option<String>,
    pub status: StepStatus,
    pub message: String,
    pub log: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeApplyReport {
    pub game_id: i64,
    pub consent: bool,
    pub cancelled: bool,
    pub failed: bool,
    pub needs_elevation: bool,
    pub steps: Vec<AppliedStep>,
    /// Final plan, rebuilt after the last step. This is what the UI shows.
    pub diagnosis: RuntimeDiagnosis,
    pub wizard: plan::WizardPhase,
    pub log_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRemovalReport {
    pub provider_id: String,
    pub freed_bytes: u64,
    pub forgot_import: bool,
    pub diagnosis: RuntimeDiagnosis,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeImportReport {
    pub provider_id: String,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub diagnosis: RuntimeDiagnosis,
}

fn installed_game(state: &AppState, game_id: i64) -> Result<db::InstalledGame, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::get_installed(&connection, game_id)?
        .ok_or_else(|| format!("Game {game_id} is not installed on this device."))
}

fn game_context(state: &AppState, game_id: i64) -> Result<GameRuntimeContext, String> {
    let installed = installed_game(state, game_id)?;
    let (options, prefs) = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        (
            db::get_launch_options(&connection, game_id)?,
            db::get_runtime_prefs(&connection, game_id)?,
        )
    };
    let install_path = PathBuf::from(&installed.install_path);
    let exe_machine = installed.executable.as_deref().and_then(|executable| {
        let path = install_path.join(executable);
        path.starts_with(&install_path)
            .then(|| plan::pe_machine(&path))
            .flatten()
    });
    let mut prerequisites: BTreeSet<String> = prefs
        .prerequisites
        .iter()
        .filter(|value| plan::PREREQUISITE_IDS.contains(&value.as_str()))
        .cloned()
        .collect();
    prerequisites.extend(plan::declared_prerequisites(&install_path));
    let compat = crate::compat::scan(&state.settings_snapshot());
    Ok(GameRuntimeContext {
        game_id,
        title: installed.title,
        install_path,
        executable: installed.executable,
        exe_machine,
        launch_options: options,
        prerequisites: prerequisites.into_iter().collect(),
        compat_umu: compat.umu,
        compat_proton: compat.proton,
        compat_wine: compat.wine,
    })
}

fn build_diagnosis(app: &AppHandle, game_id: i64) -> Result<RuntimeDiagnosis, String> {
    let state = app.state::<AppState>();
    let context = game_context(&state, game_id)?;
    let (records, audit) = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        (
            db::list_runtime_installs(&connection)?,
            db::list_runtime_audit(&connection, game_id, 20)?,
        )
    };
    let host = HostInfo::current(state.app_data.clone());
    let running = state
        .runtimes
        .lock()
        .map(|runs| runs.contains_key(&game_id))
        .unwrap_or(false);
    let mut diagnosis = plan::diagnose(
        &super::catalog(),
        &host,
        &detect::RealProbe,
        &context,
        &records,
        &audit,
        &util::now_rfc3339(),
    );
    diagnosis.running = running;
    Ok(diagnosis)
}

fn audit(
    state: &AppState,
    game_id: i64,
    provider_id: &str,
    version: &str,
    action: &str,
    outcome: &str,
    detail: &str,
) {
    let event = RuntimeAuditEvent {
        id: uuid::Uuid::new_v4().to_string(),
        game_id,
        provider_id: provider_id.to_string(),
        version: version.to_string(),
        action: action.to_string(),
        outcome: outcome.to_string(),
        detail: detail.to_string(),
        created_at: util::now_rfc3339(),
    };
    if let Ok(connection) = state.db.lock() {
        if let Err(error) = db::record_runtime_audit(&connection, &event) {
            tracing::warn!(%error, "could not record the runtime audit entry");
        }
    }
}

fn emit(
    app: &AppHandle,
    game_id: i64,
    step: &RuntimeStep,
    phase: InstallPhase,
    done: u64,
    total: u64,
    message: &str,
) {
    let percent = (total > 0).then(|| (done as f64 / total as f64 * 100.0).clamp(0.0, 100.0));
    let _ = app.emit(
        "runtime://progress",
        RuntimeProgress {
            game_id,
            step_id: step.id.clone(),
            provider_id: step.provider_id.clone(),
            phase: phase.as_str().to_string(),
            bytes_done: done,
            bytes_total: total,
            percent,
            message: message.to_string(),
        },
    );
}

fn provider<'a>(catalog: &'a RuntimeCatalog, id: &str) -> Option<&'a RuntimeProvider> {
    catalog.providers.iter().find(|provider| provider.id == id)
}

fn applyable(step: &RuntimeStep) -> bool {
    matches!(
        step.action,
        StepAction::Install | StepAction::PreparePrefix | StepAction::Configure
    )
}

/// Clears the in-flight marker on every exit path, including an early `?`.
struct RunCleanup {
    app: AppHandle,
    game_id: i64,
}

impl Drop for RunCleanup {
    fn drop(&mut self) {
        let Some(state) = self.app.try_state::<AppState>() else {
            return;
        };
        let locked = state.runtimes.lock();
        if let Ok(mut runs) = locked {
            runs.remove(&self.game_id);
        }
    }
}

/// The shipped, version-pinned runtime manifest.
#[tauri::command]
pub fn runtime_catalog() -> RuntimeCatalog {
    super::catalog()
}

/// Diagnose why a game may not start, without downloading anything.
#[tauri::command]
pub async fn make_it_run_diagnose(
    app: AppHandle,
    game_id: i64,
) -> Result<RuntimeDiagnosis, String> {
    tauri::async_runtime::spawn_blocking(move || build_diagnosis(&app, game_id))
        .await
        .map_err(|error| format!("The runtime diagnosis failed: {error}"))?
}

/// Apply the selected steps after the user accepted the licences.
///
/// `step_ids` only selects steps the current diagnosis already offers; URLs,
/// sizes and checksums always come from the catalog.
#[tauri::command]
pub async fn make_it_run_apply(
    app: AppHandle,
    game_id: i64,
    step_ids: Vec<String>,
    consent: bool,
) -> Result<RuntimeApplyReport, String> {
    if !consent {
        let state = app.state::<AppState>();
        audit(
            &state,
            game_id,
            "run",
            "",
            "declined",
            "manual",
            "the consent click was not given",
        );
        return Err(
            "No runtime is installed without an explicit consent click. Review the plan and accept the licences first."
                .to_string(),
        );
    }
    let state = app.state::<AppState>();
    // One shared cache and one runtime folder: a second apply would race the
    // first on the same files, so serialize runs machine-wide.
    {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|entry| entry.game_id == game_id) {
            return Err("Stop the game before changing its runtime.".to_string());
        }
    }
    let diagnosis = build_diagnosis(&app, game_id)?;
    let requested: BTreeSet<&str> = step_ids.iter().map(String::as_str).collect();
    if requested.is_empty() {
        return Err("No runtime step was selected.".to_string());
    }
    let mut steps: Vec<RuntimeStep> = Vec::new();
    for id in &requested {
        let step = diagnosis
            .steps
            .iter()
            .find(|step| step.id == *id)
            .ok_or_else(|| format!("Unknown runtime step: {id}"))?
            .clone();
        if !applyable(&step) {
            return Err(format!(
                "“{}” is a manual step; Ludex cannot apply it for you.",
                step.title
            ));
        }
        if step.status == StepStatus::Blocked {
            return Err(format!(
                "“{}” is blocked: {}",
                step.title,
                step.blocked_reason
                    .unwrap_or_else(|| "missing prerequisite".to_string())
            ));
        }
        steps.push(step);
    }
    steps.sort_by_key(|step| {
        diagnosis
            .steps
            .iter()
            .position(|candidate| candidate.id == step.id)
            .unwrap_or(usize::MAX)
    });

    let cancel = CancellationToken::new();
    {
        let mut runs = state.runtimes.lock().map_err(|_| {
            "The runtime installer state is unavailable; restart Ludex.".to_string()
        })?;
        if !runs.is_empty() {
            return Err(
                "Another Make it run task is running. Finish or cancel it first.".to_string(),
            );
        }
        runs.insert(
            game_id,
            Arc::new(RuntimeRun {
                cancel: cancel.clone(),
            }),
        );
    }
    let _run_cleanup = RunCleanup {
        app: app.clone(),
        game_id,
    };

    let now = util::now_rfc3339();
    let host = HostInfo::current(state.app_data.clone());
    let log_path = install::log_path(&host, game_id, &now);
    install::append_log(
        &log_path,
        &format!(
            "[{now}] make_it_run_apply game={game_id} steps={:?} platform={} arch={}",
            steps
                .iter()
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>(),
            host.platform.as_str(),
            host.arch
        ),
    );

    let mut wizard = plan::WizardState::new();
    let _ = wizard.apply(plan::WizardEvent::Scan);
    let _ = wizard.apply(plan::WizardEvent::Consent);

    let mut report_steps: Vec<AppliedStep> = Vec::new();
    let mut cancelled = false;
    let mut failed = false;
    let mut needs_elevation = false;

    for step in &steps {
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }
        wizard.set_step(Some(step.id.clone()));
        let _ = wizard.apply(plan::WizardEvent::StepStarted);
        emit(
            &app,
            game_id,
            step,
            InstallPhase::Installing,
            0,
            0,
            "Starting",
        );
        install::append_log(&log_path, &format!("[step:{}] starting", step.id));

        let outcome = apply_step(&app, &state, &host, game_id, step, &cancel, &log_path).await;
        match outcome {
            Ok(StepOutcome::Done(message)) => {
                wizard.complete_step(&step.id);
                let _ = wizard.apply(plan::WizardEvent::StepDone);
                emit(&app, game_id, step, InstallPhase::Done, 0, 0, &message);
                report_steps.push(AppliedStep {
                    step_id: step.id.clone(),
                    provider_id: step.provider_id.clone(),
                    status: StepStatus::Done,
                    message: message.clone(),
                    log: Some(log_path.to_string_lossy().to_string()),
                });
                install::append_log(&log_path, &format!("[step:{}] done: {message}", step.id));
                // #178: the next step is decided against a fresh diagnosis, so
                // a step that a previous install made unnecessary is dropped.
                let refreshed = build_diagnosis(&app, game_id)?;
                install::append_log(
                    &log_path,
                    &format!("[step:{}] re-diagnosed: {}", step.id, refreshed.summary),
                );
                if refreshed.ready {
                    break;
                }
            }
            Ok(StepOutcome::NeedsElevation {
                program,
                args,
                message,
            }) => {
                needs_elevation = true;
                wizard.set_step(None);
                emit(
                    &app,
                    game_id,
                    step,
                    InstallPhase::Installing,
                    0,
                    0,
                    &message,
                );
                report_steps.push(AppliedStep {
                    step_id: step.id.clone(),
                    provider_id: step.provider_id.clone(),
                    status: StepStatus::NeedsElevation,
                    message: message.clone(),
                    log: Some(log_path.to_string_lossy().to_string()),
                });
                install::append_log(
                    &log_path,
                    &format!(
                        "[step:{}] needs elevation: {} {}",
                        step.id,
                        program,
                        args.join(" ")
                    ),
                );
                break;
            }
            Ok(StepOutcome::Cancelled) => {
                cancelled = true;
                break;
            }
            Err(error) => {
                failed = true;
                let _ = wizard.apply(plan::WizardEvent::Fail);
                audit(
                    &state,
                    game_id,
                    step.provider_id.as_deref().unwrap_or("run"),
                    "",
                    "install",
                    "failed",
                    &error,
                );
                emit(&app, game_id, step, InstallPhase::Failed, 0, 0, &error);
                report_steps.push(AppliedStep {
                    step_id: step.id.clone(),
                    provider_id: step.provider_id.clone(),
                    status: StepStatus::Failed,
                    message: error.clone(),
                    log: Some(log_path.to_string_lossy().to_string()),
                });
                install::append_log(&log_path, &format!("[step:{}] failed: {error}", step.id));
                break;
            }
        }
    }

    if cancelled {
        let _ = wizard.apply(plan::WizardEvent::Cancel);
        if let Some(step) = steps.first() {
            emit(
                &app,
                game_id,
                step,
                InstallPhase::Cancelled,
                0,
                0,
                "Cancelled",
            );
        }
        install::append_log(&log_path, "[run] cancelled");
        audit(
            &state,
            game_id,
            "run",
            "",
            "cancel",
            "cancelled",
            "the user cancelled the Make it run task",
        );
    } else if failed {
        install::append_log(&log_path, "[run] failed");
    } else if !needs_elevation {
        let _ = wizard.apply(plan::WizardEvent::Verify);
        let final_diagnosis = build_diagnosis(&app, game_id)?;
        if final_diagnosis.ready {
            let _ = wizard.apply(plan::WizardEvent::Succeed);
        } else {
            let _ = wizard.apply(plan::WizardEvent::Fail);
        }
        install::append_log(&log_path, "[run] verified");
    }

    {
        let mut runs = state.runtimes.lock().map_err(|_| {
            "The runtime installer state is unavailable; restart Ludex.".to_string()
        })?;
        runs.remove(&game_id);
    }

    let final_diagnosis = build_diagnosis(&app, game_id)?;
    Ok(RuntimeApplyReport {
        game_id,
        consent,
        cancelled,
        failed,
        needs_elevation,
        steps: report_steps,
        diagnosis: final_diagnosis,
        wizard: wizard.phase,
        log_path: log_path.to_string_lossy().to_string(),
    })
}

enum StepOutcome {
    Done(String),
    NeedsElevation {
        program: String,
        args: Vec<String>,
        message: String,
    },
    Cancelled,
}

async fn apply_step(
    app: &AppHandle,
    state: &AppState,
    host: &HostInfo,
    game_id: i64,
    step: &RuntimeStep,
    cancel: &CancellationToken,
    log_path: &std::path::Path,
) -> Result<StepOutcome, String> {
    match step.action {
        StepAction::Install => {
            let catalog = super::catalog();
            let provider_id = step
                .provider_id
                .as_deref()
                .ok_or_else(|| "The install step has no provider.".to_string())?;
            let provider = provider(&catalog, provider_id)
                .ok_or_else(|| format!("Unknown runtime provider: {provider_id}"))?;
            let client = install::runtime_client()?;
            let progress_app = app.clone();
            let step_clone = step.clone();
            let outcome = install::install_provider(
                &client,
                host,
                provider,
                cancel,
                move |phase, done, total, message| {
                    emit(
                        &progress_app,
                        game_id,
                        &step_clone,
                        phase,
                        done,
                        total,
                        message,
                    );
                },
            )
            .await;
            match outcome {
                Ok(InstallOutcome::Installed(installed)) => {
                    let row = RuntimeInstallRow {
                        provider_id: provider.id.clone(),
                        platform: host.platform.as_str().to_string(),
                        version: provider.version.clone(),
                        path: installed.path.clone(),
                        sha256: installed.sha256.clone(),
                        size_bytes: installed.size_bytes,
                        installed_at: util::now_rfc3339(),
                    };
                    {
                        let connection = state.db.lock().map_err(|_| {
                            "The local database is unavailable; restart Ludex.".to_string()
                        })?;
                        db::save_runtime_install(&connection, &row)?;
                    }
                    audit(
                        state,
                        game_id,
                        &provider.id,
                        &provider.version,
                        "install",
                        "ok",
                        &format!(
                            "{}; {} bytes; sha256 {}",
                            installed.detail, installed.size_bytes, installed.sha256
                        ),
                    );
                    if provider.platform == super::Platform::Linux {
                        remember_managed_path(state, provider, &installed.path)?;
                    }
                    if matches!(
                        provider.kind,
                        ProviderKind::Launcher | ProviderKind::Proton | ProviderKind::Wine
                    ) {
                        remember_choice(state, game_id, provider)?;
                    }
                    install::append_log(
                        log_path,
                        &format!(
                            "[install:{}] {} {} -> {} ({})",
                            provider.id,
                            provider.name,
                            provider.version,
                            installed.path,
                            installed.sha256
                        ),
                    );
                    Ok(StepOutcome::Done(installed.detail))
                }
                Ok(InstallOutcome::NeedsElevation { program, args, log }) => {
                    audit(
                        state,
                        game_id,
                        &provider.id,
                        &provider.version,
                        "needs-elevation",
                        "manual",
                        &log,
                    );
                    Ok(StepOutcome::NeedsElevation {
                        message: format!(
                            "Administrator rights are required. Run this command in an elevated terminal: {program} {}",
                            args.join(" ")
                        ),
                        program,
                        args,
                    })
                }
                Err(error) if error == "cancelled" => Ok(StepOutcome::Cancelled),
                Err(error) => Err(error),
            }
        }
        StepAction::PreparePrefix => {
            let installed = installed_game(state, game_id)?;
            let install_path = PathBuf::from(&installed.install_path);
            let options = {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::get_launch_options(&connection, game_id)?
            };
            let prefix = linux_gaming::prefix_for(&install_path, &options.prefix_path)?;
            if prefix == install_path {
                return Err("The Wine prefix cannot be the install folder.".to_string());
            }
            let prefs = {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::get_runtime_prefs(&connection, game_id)?
            };
            let provider_id = if prefs.provider_id.is_empty() {
                "umu-launcher".to_string()
            } else {
                prefs.provider_id.clone()
            };
            let version = if prefs.provider_version.is_empty() {
                "managed".to_string()
            } else {
                prefs.provider_version.clone()
            };
            let size =
                install::prepare_prefix(&prefix, &provider_id, &version, &util::now_rfc3339())?;
            audit(
                state,
                game_id,
                &provider_id,
                &version,
                "prepare-prefix",
                "ok",
                &format!("{} ({size} bytes)", prefix.display()),
            );
            install::append_log(
                log_path,
                &format!("[prefix] {} ({size} bytes)", prefix.display()),
            );
            Ok(StepOutcome::Done(format!(
                "Prefix ready at {}.",
                prefix.display()
            )))
        }
        StepAction::Configure => {
            let installed = installed_game(state, game_id)?;
            let install_path = PathBuf::from(&installed.install_path);
            let mut options = {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::get_launch_options(&connection, game_id)?
            };
            if step.id == "linux-prefix-reset" {
                options.prefix_path.clear();
                options.normalize();
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::save_launch_options(&connection, game_id, &options, &util::now_rfc3339())?;
                audit(
                    state,
                    game_id,
                    "umu-launcher",
                    "",
                    "configure",
                    "ok",
                    "reset the unsafe Wine prefix path",
                );
                return Ok(StepOutcome::Done(
                    "The Wine prefix path was reset to the per-game default.".to_string(),
                ));
            }

            options.mode = LaunchMode::Umu;
            options.tool_id.clear();
            options.normalize();
            let prefix = linux_gaming::prefix_for(&install_path, &options.prefix_path)?;
            {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::save_launch_options(&connection, game_id, &options, &util::now_rfc3339())?;
            }
            let catalog = super::catalog();
            let provider = provider(&catalog, "umu-launcher");
            if let Some(provider) = provider {
                remember_managed_path(state, provider, "")?;
                remember_choice(state, game_id, provider)?;
            }
            let prefs = RuntimeGamePrefs {
                game_id,
                provider_id: "umu-launcher".to_string(),
                provider_version: provider
                    .map(|provider| provider.version.clone())
                    .unwrap_or_default(),
                prefix_path: prefix.to_string_lossy().to_string(),
                prerequisites: plan::declared_prerequisites(&install_path),
                updated_at: util::now_rfc3339(),
            };
            {
                let connection = state
                    .db
                    .lock()
                    .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
                db::save_runtime_prefs(&connection, &prefs)?;
            }
            audit(
                state,
                game_id,
                "umu-launcher",
                &prefs.provider_version,
                "configure",
                "ok",
                &format!("launch mode umu, prefix {}", prefix.display()),
            );
            install::append_log(
                log_path,
                &format!("[configure] umu, prefix {}", prefix.display()),
            );
            Ok(StepOutcome::Done(
                "This game now launches through UMU with the pinned Proton build.".to_string(),
            ))
        }
        StepAction::Import | StepAction::Manual => Err(format!(
            "“{}” is a manual step; Ludex cannot apply it for you.",
            step.title
        )),
    }
}

/// Point the compatibility scan at a managed tool by filling an empty override.
/// A path the user set is never replaced.
fn remember_managed_path(
    state: &AppState,
    provider: &RuntimeProvider,
    installed_path: &str,
) -> Result<(), String> {
    if provider.platform != super::Platform::Linux {
        return Ok(());
    }
    let program = match provider.id.as_str() {
        "umu-launcher" => "umu",
        "ge-proton" => "proton",
        _ => return Ok(()),
    };
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let record = db::get_runtime_install(&connection, &provider.id)?;
    drop(connection);
    let Some(record) = record else {
        return Ok(());
    };
    let mut path = if installed_path.is_empty() {
        PathBuf::from(&record.path)
    } else {
        PathBuf::from(installed_path)
    };
    if program == "umu" {
        if let Some(entry) = provider
            .download
            .as_ref()
            .and_then(|download| download.entry_point.as_deref())
        {
            path = path.join(entry);
        }
    }
    let value = path.to_string_lossy().to_string();
    state.update_settings(|settings| match program {
        "umu"
            if settings.compat_umu_path.trim().is_empty()
                || !Path::new(settings.compat_umu_path.trim()).exists() =>
        {
            settings.compat_umu_path = value.clone();
        }
        "proton"
            if settings.compat_proton_path.trim().is_empty()
                || !Path::new(settings.compat_proton_path.trim()).exists() =>
        {
            settings.compat_proton_path = value.clone();
        }
        _ => {}
    })?;
    Ok(())
}

fn remember_choice(
    state: &AppState,
    game_id: i64,
    provider: &RuntimeProvider,
) -> Result<(), String> {
    let current = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_runtime_prefs(&connection, game_id)?
    };
    // Keep an explicit UMU choice: installing the pinned Proton next to it
    // must not rewrite what the game launches with.
    if !current.provider_id.is_empty()
        && current.provider_id != provider.id
        && provider.id == "ge-proton"
    {
        return Ok(());
    }
    let prefs = RuntimeGamePrefs {
        game_id,
        provider_id: provider.id.clone(),
        provider_version: provider.version.clone(),
        prefix_path: current.prefix_path,
        prerequisites: current.prerequisites,
        updated_at: util::now_rfc3339(),
    };
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::save_runtime_prefs(&connection, &prefs)
}

/// Cancel a running apply. Downloads stop at the next chunk.
#[tauri::command]
pub fn make_it_run_cancel(app: AppHandle, game_id: i64) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let runs = state
        .runtimes
        .lock()
        .map_err(|_| "The runtime installer state is unavailable; restart Ludex.".to_string())?;
    match runs.get(&game_id) {
        Some(run) => {
            run.cancel.cancel();
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Verify a user-supplied GPTK DMG and record it without copying it.
#[tauri::command]
pub async fn make_it_run_import(
    app: AppHandle,
    game_id: i64,
    provider_id: String,
    path: String,
) -> Result<RuntimeImportReport, String> {
    let state = app.state::<AppState>();
    let catalog = super::catalog();
    let provider = provider(&catalog, &provider_id)
        .ok_or_else(|| format!("Unknown runtime provider: {provider_id}"))?
        .clone();
    if provider.platform != super::Platform::Macos {
        return Err("Only the Game Porting Toolkit import is supported.".to_string());
    }
    let host = HostInfo::current(state.app_data.clone());
    if host.platform != super::Platform::Macos {
        return Err("The Game Porting Toolkit import is only available on macOS.".to_string());
    }
    let source = PathBuf::from(&path);
    let step = RuntimeStep {
        id: "macos-gptk".to_string(),
        provider_id: Some(provider.id.clone()),
        action: StepAction::Import,
        title: "Import Game Porting Toolkit".to_string(),
        detail: String::new(),
        requires_admin: false,
        requires_consent: true,
        download: None,
        manual: None,
        status: StepStatus::Pending,
        blocked_reason: None,
        optional: true,
    };
    emit(
        &app,
        game_id,
        &step,
        InstallPhase::Verifying,
        0,
        0,
        "Hashing the imported DMG",
    );
    let provider_clone = provider.clone();
    let imported =
        tokio::task::spawn_blocking(move || install::import_gptk(&provider_clone, &source))
            .await
            .map_err(|error| format!("The import task failed: {error}"))??;

    let row = RuntimeInstallRow {
        provider_id: provider.id.clone(),
        platform: "macos".to_string(),
        version: provider.version.clone(),
        path: imported.path.clone(),
        sha256: imported.sha256.clone(),
        size_bytes: imported.size_bytes,
        installed_at: util::now_rfc3339(),
    };
    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::save_runtime_install(&connection, &row)?;
    }
    audit(
        &state,
        game_id,
        &provider.id,
        &provider.version,
        "import",
        "ok",
        &format!("{}; sha256 {}", imported.path, imported.sha256),
    );
    emit(
        &app,
        game_id,
        &step,
        InstallPhase::Done,
        0,
        0,
        "Game Porting Toolkit verified and recorded",
    );
    let diagnosis = build_diagnosis(&app, game_id)?;
    Ok(RuntimeImportReport {
        provider_id: provider.id,
        path: imported.path,
        sha256: imported.sha256,
        size_bytes: imported.size_bytes,
        diagnosis,
    })
}

/// Native file picker for the GPTK DMG.
#[tauri::command]
pub async fn make_it_run_pick_import(
    app: AppHandle,
    game_id: i64,
) -> Result<Option<String>, String> {
    let title = installed_game(&app.state::<AppState>(), game_id)
        .map(|game| game.title)
        .unwrap_or_else(|_| "Game Porting Toolkit".to_string());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(format!("Choose the Game Porting Toolkit DMG for {title}"))
        .add_filter("Apple disk image", &["dmg"])
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    let picked = receiver
        .await
        .map_err(|error| format!("The file picker did not answer: {error}"))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|error| format!("Invalid file path: {error}"))?;
    Ok(Some(path.to_string_lossy().to_string()))
}

/// Remove a runtime Ludex installed. Imported files are only forgotten.
#[tauri::command]
pub async fn make_it_run_remove(
    app: AppHandle,
    game_id: i64,
    provider_id: String,
) -> Result<RuntimeRemovalReport, String> {
    let state = app.state::<AppState>();
    let catalog = super::catalog();
    let provider = provider(&catalog, &provider_id)
        .ok_or_else(|| format!("Unknown runtime provider: {provider_id}"))?
        .clone();
    {
        let processes = state
            .processes
            .lock()
            .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
        if processes.values().any(|entry| entry.game_id == game_id) {
            return Err("Stop the game before removing its runtime.".to_string());
        }
    }
    let record = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_runtime_install(&connection, &provider_id)?
    }
    .ok_or_else(|| format!("Ludex has no install record for {provider_id}."))?;
    if !provider.managed
        && provider
            .install
            .as_ref()
            .map(|install| install.kind != super::InstallKind::Import)
            .unwrap_or(true)
    {
        return Err(format!("{} is not managed by Ludex.", provider.name));
    }
    let host = HostInfo::current(state.app_data.clone());
    let provider_clone = provider.clone();
    let record_clone = record.clone();
    let imported = provider
        .install
        .as_ref()
        .map(|install| install.kind == super::InstallKind::Import)
        .unwrap_or(false);
    let forget_only = imported || record.path.trim().is_empty();
    let freed = tokio::task::spawn_blocking(move || {
        install::remove_provider(&host, &provider_clone, &record_clone)
    })
    .await
    .map_err(|error| format!("The removal task failed: {error}"))??;
    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::delete_runtime_install(&connection, &provider_id)?;
    }
    // Do not leave a compatibility override pointing at a removed folder.
    let removed = record.path.clone();
    state.update_settings(|settings| {
        if !removed.is_empty() && settings.compat_umu_path.starts_with(&removed) {
            settings.compat_umu_path.clear();
        }
        if !removed.is_empty() && settings.compat_proton_path.starts_with(&removed) {
            settings.compat_proton_path.clear();
        }
    })?;
    audit(
        &state,
        game_id,
        &provider.id,
        &provider.version,
        "remove",
        "ok",
        &if forget_only {
            "forgot the record; no user file was deleted".to_string()
        } else {
            format!("deleted {freed} bytes from {}", record.path)
        },
    );
    let diagnosis = build_diagnosis(&app, game_id)?;
    Ok(RuntimeRemovalReport {
        provider_id,
        freed_bytes: freed,
        forgot_import: forget_only,
        diagnosis,
    })
}

/// Persist the wrapper/tool the user picked (macOS wrappers are installed by
/// the user, so selection is separate from installation).
#[tauri::command]
pub async fn make_it_run_select(
    app: AppHandle,
    game_id: i64,
    provider_id: String,
) -> Result<RuntimeDiagnosis, String> {
    let state = app.state::<AppState>();
    let catalog = super::catalog();
    let provider = provider(&catalog, &provider_id)
        .ok_or_else(|| format!("Unknown runtime provider: {provider_id}"))?
        .clone();
    let diagnosis = build_diagnosis(&app, game_id)?;
    let installed = diagnosis
        .detections
        .iter()
        .find(|detection| detection.provider_id == provider_id)
        .map(|detection| detection.installed)
        .unwrap_or(false);
    if !installed {
        return Err(format!(
            "{} was not detected on this machine yet.",
            provider.name
        ));
    }
    remember_choice(&state, game_id, &provider)?;
    audit(
        &state,
        game_id,
        &provider.id,
        &provider.version,
        "select",
        "ok",
        "selected as this game's runtime",
    );
    build_diagnosis(&app, game_id)
}

/// Record the prerequisites a game declares, in addition to the installers
/// Ludex finds in its folder.
#[tauri::command]
pub async fn set_runtime_prerequisites(
    app: AppHandle,
    game_id: i64,
    prerequisites: Vec<String>,
) -> Result<RuntimeDiagnosis, String> {
    let state = app.state::<AppState>();
    let mut cleaned: Vec<String> = Vec::new();
    for value in prerequisites {
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() {
            continue;
        }
        if !plan::PREREQUISITE_IDS.contains(&value.as_str()) {
            return Err(format!("Unknown prerequisite: {value}"));
        }
        if !cleaned.contains(&value) {
            cleaned.push(value);
        }
    }
    let current = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_runtime_prefs(&connection, game_id)?
    };
    let prefs = RuntimeGamePrefs {
        game_id,
        provider_id: current.provider_id,
        provider_version: current.provider_version,
        prefix_path: current.prefix_path,
        prerequisites: cleaned,
        updated_at: util::now_rfc3339(),
    };
    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::save_runtime_prefs(&connection, &prefs)?;
    }
    build_diagnosis(&app, game_id)
}
