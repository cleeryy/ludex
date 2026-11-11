//! Adopting an existing local game folder (#129).
//!
//! Ludex can link a folder that already exists on disk — typically a GameVault
//! download — as a first-class install: it launches the detected executable in
//! place, tracks playtime and can remove the entry or, after an explicit
//! confirmation, the folder. Nothing is copied, moved or written during
//! adoption, and folders owned by a launcher import (#116) are never taken over.
//!
//! The filesystem work is kept in plain functions so the commands and the tests
//! share the same code; Tauri types only appear in the thin command wrappers.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::{self, InstalledGame};
use crate::manifest;
use crate::paths;
use crate::state::AppState;
use crate::util::now_rfc3339;

/// Optional GameVault metadata files read (never written) when present.
const GAMEVAULT_METADATA_FILES: [&str; 2] = ["gamevault.json", ".gamevault.json"];

/// A folder already indexed for this game, shown before replacing it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingInstall {
    pub game_id: i64,
    pub title: String,
    pub source: String,
    pub install_path: String,
}

/// Read-only preview of adopting a folder (#129). Nothing is written.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptPreview {
    /// Canonical folder that would be stored.
    pub folder: String,
    pub executable: Option<String>,
    /// Ranked executables found in the folder (#134): the first one is the
    /// current choice, the rest can be selected with a manual executable.
    pub executable_candidates: Vec<String>,
    pub size_bytes: u64,
    pub file_count: u64,
    /// `0` means "unknown"; versions are never guessed from the title.
    pub version_id: i64,
    pub version: Option<String>,
    pub warnings: Vec<String>,
    /// Set when the game already has an index row. Replacing a `ludex` or
    /// launcher row needs an explicit confirmation.
    pub existing: Option<ExistingInstall>,
}

/// Result of `delete_adopted_installation`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAdoptedPreview {
    pub folder: String,
    pub size_bytes: u64,
    pub file_count: u64,
    /// True once the folder was deleted (a preview returns `false`).
    pub deleted: bool,
    /// True when the folder was already gone; only the index row was removed.
    pub missing: bool,
    pub warnings: Vec<String>,
}

/// Result of hashing an adopted folder into a manifest (#129).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestReport {
    pub game_id: i64,
    pub files: u64,
    pub bytes: u64,
    pub built_at: String,
}

/// Progress for a background manifest build.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestProgress {
    pub game_id: i64,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub message: String,
}

/* ------------------------------- matching -------------------------------- */

/// How confidently a GameVault folder was matched to a library title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchStatus {
    /// The normalised folder/title and the library title are identical.
    Exact,
    /// One normalised title contains the other on word boundaries.
    Containment,
    /// Several library games match; the user must choose.
    Ambiguous,
    /// Nothing matched.
    None,
}

/// Word-boundary containment on normalised titles: `doom` matches
/// `doom eternal` but not `doometernal`.
fn contains_words(longer: &str, shorter: &str) -> bool {
    if shorter.is_empty() || longer.len() < shorter.len() {
        return false;
    }
    if longer == shorter {
        return true;
    }
    longer.starts_with(&format!("{shorter} "))
        || longer.ends_with(&format!(" {shorter}"))
        || longer.contains(&format!(" {shorter} "))
}

/// Match one candidate name against the server library.
///
/// Names are the folder name and the GameVault metadata title when present.
/// Exact matches win; a single word-boundary containment is accepted; anything
/// with several candidates is reported as ambiguous and never guessed.
pub fn match_library_title(
    names: &[String],
    server: &[(i64, String)],
) -> (Option<i64>, MatchStatus) {
    let candidates: Vec<String> = names
        .iter()
        .map(|name| ludex_core::naming::normalize_title(name))
        .filter(|name| !name.is_empty())
        .collect();
    if candidates.is_empty() || server.is_empty() {
        return (None, MatchStatus::None);
    }

    let mut exact: Vec<i64> = Vec::new();
    for (game_id, title) in server {
        let normalized = ludex_core::naming::normalize_title(title);
        if !normalized.is_empty() && candidates.contains(&normalized) {
            exact.push(*game_id);
        }
    }
    exact.sort_unstable();
    exact.dedup();
    match exact.len() {
        1 => return (Some(exact[0]), MatchStatus::Exact),
        0 => {}
        _ => return (None, MatchStatus::Ambiguous),
    }

    // Containment: at least four characters so "dlc" or "vr" never match alone.
    let mut contained: Vec<i64> = Vec::new();
    for (game_id, title) in server {
        let normalized = ludex_core::naming::normalize_title(title);
        if normalized.len() < 4 {
            continue;
        }
        if candidates
            .iter()
            .any(|name| contains_words(name, &normalized) || contains_words(&normalized, name))
        {
            contained.push(*game_id);
        }
    }
    contained.sort_unstable();
    contained.dedup();
    match contained.len() {
        1 => (Some(contained[0]), MatchStatus::Containment),
        0 => (None, MatchStatus::None),
        _ => (None, MatchStatus::Ambiguous),
    }
}

/* ------------------------- path safety helpers --------------------------- */

/// Canonicalize a user-chosen folder, refusing relative paths, parent traversal
/// and links.
///
/// The canonical path is what gets stored in the index, so a later delete can
/// never be redirected by swapping a folder for a symlink.
pub fn canonical_folder(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Choose the game folder to link.".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("The folder path contains control characters.".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("The folder path must be absolute.".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("The folder path cannot contain “..”.".to_string());
    }

    // Refuse a link in the chosen path: the stored canonical path would point at
    // a folder the user did not actually pick.
    let metadata = fs::symlink_metadata(&path)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is a link. Choose the real folder it points to.",
            path.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!("{} is not a folder.", path.display()));
    }

    fs::canonicalize(&path).map_err(|e| format!("Could not resolve {}: {e}", path.display()))
}

fn home_dir() -> Option<PathBuf> {
    let raw = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let path = PathBuf::from(raw);
    Some(fs::canonicalize(&path).unwrap_or(path))
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Refuse a drive root, the home folder or the Ludex install folder itself.
///
/// A game folder is always a child of a real parent; adopting a whole disk or
/// the user's home would make every later action dangerous.
pub fn validate_adoptable(folder: &Path, install_root: Option<&Path>) -> Result<(), String> {
    if !folder.is_dir() {
        return Err(format!("{} is not a folder.", folder.display()));
    }
    if folder.parent().is_none() || folder.parent() == Some(folder) {
        return Err("Choose the game folder, not a drive root.".to_string());
    }
    if folder.file_name().is_none() {
        return Err("Choose the game folder, not a drive root.".to_string());
    }
    if let Some(home) = home_dir() {
        if same_path(&home, folder) {
            return Err(
                "Choose a folder inside your home folder, not the home folder itself.".to_string(),
            );
        }
    }
    if let Some(root) = install_root {
        if same_path(root, folder) {
            return Err(
                "That is the Ludex install folder. Choose the game's own folder inside it."
                    .to_string(),
            );
        }
    }
    Ok(())
}

/// Refuse a folder whose immediate parent is a drive root, the home folder or
/// the Ludex install root. Deleting inside one of those is never a game folder.
pub fn validate_deletable(folder: &Path, install_root: Option<&Path>) -> Result<(), String> {
    validate_adoptable(folder, install_root)?;
    let parent = folder
        .parent()
        .ok_or_else(|| "Refusing to delete a drive root.".to_string())?;
    if parent.parent().is_none() {
        return Err(format!(
            "Refusing to delete {}: it sits directly on a drive root.",
            folder.display()
        ));
    }
    if let Some(home) = home_dir() {
        if same_path(&home, parent) {
            return Err(format!(
                "Refusing to delete {}: it sits directly in your home folder. Move it into a games folder first.",
                folder.display()
            ));
        }
    }
    if let Some(root) = install_root {
        if same_path(root, parent) {
            return Err(format!(
                "Refusing to delete {}: it is directly inside the Ludex install folder.",
                folder.display()
            ));
        }
    }
    Ok(())
}

/// Refuse a tree where a symlink points outside the adopted folder.
///
/// `remove_dir_all` never follows links, but a link can still make the folder
/// look bigger/different than what is being deleted. Links that stay inside the
/// folder and broken links are fine.
pub fn ensure_no_symlink_escape(root: &Path) -> Result<(), String> {
    let canonical =
        fs::canonicalize(root).map_err(|e| format!("Could not resolve {}: {e}", root.display()))?;
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                if let Ok(target) = fs::canonicalize(&path) {
                    if target != canonical && !target.starts_with(&canonical) {
                        return Err(format!(
                            "{} is a link that points outside the folder ({}). Adopt the real folder instead.",
                            path.display(),
                            target.display()
                        ));
                    }
                }
            } else if metadata.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(())
}

/// True when `stored` (an indexed install path) resolves to `canonical`.
fn path_matches(stored: &str, canonical: &Path) -> bool {
    if stored == canonical.to_string_lossy() {
        return true;
    }
    fs::canonicalize(stored)
        .map(|path| path == canonical)
        .unwrap_or(false)
}

/// The canonical form of an indexed path, or the stored string when it is gone.
fn indexed_path(stored: &str) -> PathBuf {
    fs::canonicalize(stored).unwrap_or_else(|_| PathBuf::from(stored))
}

/// Canonical comparison key for a folder that may be missing right now (#134).
pub fn canonical_path_key(raw: &str) -> String {
    fs::canonicalize(raw)
        .unwrap_or_else(|_| PathBuf::from(raw))
        .to_string_lossy()
        .to_string()
}

/* ------------------------------ inspection ------------------------------- */

/// Inspect a folder and build the preview + index row for it.
///
/// Pure filesystem work plus the already-loaded index rows, so the commands and
/// the tests share one implementation.
#[allow(clippy::too_many_arguments)]
pub fn prepare_adoption(
    installed: &[InstalledGame],
    install_root: &Path,
    game_id: i64,
    title: &str,
    version_id: Option<i64>,
    version_label: Option<&str>,
    folder: &str,
    executable: Option<&str>,
) -> Result<(AdoptPreview, InstalledGame), String> {
    let canonical = canonical_folder(folder)?;
    validate_adoptable(&canonical, Some(install_root))?;
    ensure_no_symlink_escape(&canonical)?;

    let existing = installed.iter().find(|row| row.game_id == game_id).cloned();
    let mut warnings = Vec::new();

    // The folder must not belong to another entry, and it must not contain or
    // sit inside one: deleting an adopted parent would remove another install.
    for row in installed.iter().filter(|row| row.game_id != game_id) {
        let other = indexed_path(&row.install_path);
        if path_matches(&row.install_path, &canonical) {
            if db::is_launcher_source(&row.source) {
                return Err(format!(
                    "That folder belongs to the {} import of “{}” (#116). Imported games stay managed by their launcher.",
                    row.source, row.title
                ));
            }
            return Err(format!(
                "That folder is already linked to “{}” ({}). Remove that entry first.",
                row.title, row.source
            ));
        }
        if other.as_os_str().is_empty() {
            continue;
        }
        if canonical.starts_with(&other) {
            return Err(format!(
                "That folder is inside “{}”, which Ludex already manages ({}). Choose the game's own folder.",
                row.title, row.source
            ));
        }
        if other.starts_with(&canonical) {
            return Err(format!(
                "That folder contains “{}”, which Ludex already manages ({}). Choose a folder that holds only this game.",
                row.title, row.source
            ));
        }
    }

    if let Some(row) = &existing {
        if row.source != db::SOURCE_ADOPTED {
            warnings.push(format!(
                "“{}” is already {}. Confirming replaces that entry; its files are left alone.",
                row.title,
                db::describe_source(&row.source)
            ));
        } else if !path_matches(&row.install_path, &canonical) {
            warnings.push(format!(
                "This game is currently linked to {}. Confirming points it at the new folder; the old files are left alone.",
                row.install_path
            ));
        }
    }

    let discovery =
        crate::bridge::discover_executables(&canonical, title, crate::bridge::DEFAULT_SCAN_DEPTH);
    let executable_candidates: Vec<String> = discovery
        .candidates
        .iter()
        .map(|candidate| candidate.path.to_string_lossy().to_string())
        .collect();
    let executable_path = match executable.map(str::trim).filter(|value| !value.is_empty()) {
        Some(requested) => Some(resolve_executable(&canonical, requested)?),
        None => discovery
            .chosen
            .map(|path| path.to_string_lossy().to_string()),
    };
    warnings.extend(discovery.notes);
    if executable_path.is_none() {
        warnings.push(
            "No game executable was found in this folder. You can still link it and choose the executable from the game settings."
                .to_string(),
        );
    }

    let (size_bytes, file_count) = paths::dir_stats(&canonical);
    let recorded_version = version_id.filter(|value| *value > 0).unwrap_or(0);
    let version = if recorded_version == 0 {
        None
    } else {
        version_label.map(str::to_string)
    };

    let folder_string = canonical.to_string_lossy().to_string();
    let preview = AdoptPreview {
        folder: folder_string.clone(),
        executable: executable_path.clone(),
        executable_candidates,
        size_bytes,
        file_count,
        version_id: recorded_version,
        version: version.clone(),
        warnings,
        existing: existing.as_ref().map(|row| ExistingInstall {
            game_id: row.game_id,
            title: row.title.clone(),
            source: row.source.clone(),
            install_path: row.install_path.clone(),
        }),
    };

    let record = InstalledGame {
        game_id,
        version_id: recorded_version,
        title: title.to_string(),
        version,
        install_path: folder_string,
        executable: executable_path,
        size_bytes: i64::try_from(size_bytes).unwrap_or(i64::MAX),
        installed_at: now_rfc3339(),
        // Adopting an existing folder is not a launch: keep the old timestamp
        // when the game was already adopted, so "last played" stays honest.
        last_launched_at: existing
            .as_ref()
            .and_then(|row| row.last_launched_at.clone()),
        archive_hash: None,
        install_root: canonical
            .parent()
            .map(|parent| parent.to_string_lossy().to_string()),
        source: db::SOURCE_ADOPTED.to_string(),
        launch_uri: None,
    };

    Ok((preview, record))
}

/// Resolve an executable chosen by the user, relative to the adopted folder.
fn resolve_executable(folder: &Path, requested: &str) -> Result<String, String> {
    let candidate = PathBuf::from(requested.trim());
    let full = if candidate.is_absolute() {
        candidate
    } else {
        folder.join(candidate)
    };
    if !full.exists() {
        return Err(format!("The executable does not exist: {}", full.display()));
    }
    let canonical = fs::canonicalize(&full)
        .map_err(|e| format!("Could not resolve {}: {e}", full.display()))?;
    if canonical != folder && !canonical.starts_with(folder) {
        return Err(format!(
            "Refusing to record {} because it is outside the adopted folder.",
            canonical.display()
        ));
    }
    Ok(canonical.to_string_lossy().to_string())
}

/* ------------------------------ unlink/delete ---------------------------- */

/// Remove the index row and manifest, keeping every file on disk.
pub fn unlink(state: &AppState, game_id: i64) -> Result<(), String> {
    ensure_idle(state, game_id, "removing it from Ludex")?;
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    let removed = db::remove_installed(&connection, game_id)?;
    db::remove_manifest(&connection, game_id)?;
    db::delete_save_paths(&connection, game_id)?;
    db::delete_save_detection(&connection, game_id)?;
    if !removed {
        return Err(format!("Game {game_id} is not installed."));
    }
    Ok(())
}

fn ensure_idle(state: &AppState, game_id: i64, action: &str) -> Result<(), String> {
    let processes = state
        .processes
        .lock()
        .map_err(|_| "The process list is unavailable; restart Ludex.".to_string())?;
    if processes.values().any(|entry| entry.game_id == game_id) {
        return Err(format!("Stop the game before {action}."));
    }
    Ok(())
}

/// What deleting an adopted folder should do (#129).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletePlan {
    /// The folder disappeared after adoption; only the index row is left.
    Missing,
    /// The stored canonical folder, which may be deleted after confirmation.
    Delete(PathBuf),
}

/// Resolve the stored path of an adopted folder, refusing a folder that became
/// a link since adoption.
pub fn plan_delete(stored: &Path) -> Result<DeletePlan, String> {
    match fs::canonicalize(stored) {
        Ok(canonical) => {
            if canonical != stored {
                return Err(format!(
                    "{} is now a link to {}. Refusing to delete through a link; remove the entry instead.",
                    stored.display(),
                    canonical.display()
                ));
            }
            Ok(DeletePlan::Delete(canonical))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DeletePlan::Missing),
        Err(error) => Err(format!("Could not read {}: {error}", stored.display())),
    }
}

/// Delete the folder of an adopted install, or preview the deletion.
///
/// `confirm_path` must be exactly the canonical stored path. Without it this is
/// a read-only preview that returns the size and file count for the dialog.
pub async fn delete_adopted(
    app: &AppHandle,
    game_id: i64,
    confirm_path: Option<String>,
) -> Result<DeleteAdoptedPreview, String> {
    let state = app.state::<AppState>();
    let installed = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_installed(&connection, game_id)?
    }
    .ok_or_else(|| format!("Game {game_id} is not installed."))?;
    if installed.source != db::SOURCE_ADOPTED {
        return Err(format!(
            "“{}” is {}. Only an adopted folder can be deleted with this action: remove the entry instead and delete the files yourself.",
            installed.title,
            db::describe_source(&installed.source)
        ));
    }
    ensure_idle(&state, game_id, "deleting its files")?;

    let stored = PathBuf::from(&installed.install_path);
    let install_root = state.settings_snapshot().install_path();

    let folder = match plan_delete(&stored)? {
        DeletePlan::Delete(canonical) => canonical,
        DeletePlan::Missing => {
            // The folder disappeared after adoption: offer to clean the index.
            if let Some(confirm) = confirm_path {
                let expected = stored.to_string_lossy().to_string();
                if confirm.trim() != expected {
                    return Err(format!(
                        "Type the exact folder path to confirm, or remove the entry instead: {expected}"
                    ));
                }
                remove_install_rows(&state, game_id)?;
                return Ok(DeleteAdoptedPreview {
                    folder: expected,
                    size_bytes: 0,
                    file_count: 0,
                    deleted: false,
                    missing: true,
                    warnings: vec![
                        "The folder no longer exists; only the Ludex entry was removed."
                            .to_string(),
                    ],
                });
            }
            return Ok(DeleteAdoptedPreview {
                folder: stored.to_string_lossy().to_string(),
                size_bytes: 0,
                file_count: 0,
                deleted: false,
                missing: true,
                warnings: vec![
                    "The folder no longer exists. Confirming removes the stale Ludex entry."
                        .to_string(),
                ],
            });
        }
    };

    validate_deletable(&folder, Some(&install_root))?;
    ensure_no_symlink_escape(&folder)?;
    let (size_bytes, file_count) = paths::dir_stats(&folder);
    let folder_string = folder.to_string_lossy().to_string();

    let Some(confirm) = confirm_path else {
        return Ok(DeleteAdoptedPreview {
            folder: folder_string,
            size_bytes,
            file_count,
            deleted: false,
            missing: false,
            warnings: Vec::new(),
        });
    };

    let typed = confirm.trim();
    if !confirmation_matches(typed, &folder) {
        return Err(format!(
            "The confirmation does not match. Type the exact folder path to delete it: {folder_string}"
        ));
    }

    let target = folder.clone();
    tokio::task::spawn_blocking(move || {
        fs::remove_dir_all(&target)
            .map_err(|e| format!("Could not delete {}: {e}", target.display()))
    })
    .await
    .map_err(|e| format!("The delete task failed: {e}"))??;

    remove_install_rows(&state, game_id)?;
    crate::tray::refresh_menu(app);
    tracing::info!(game_id, folder = %folder_string, "deleted an adopted install");
    Ok(DeleteAdoptedPreview {
        folder: folder_string,
        size_bytes,
        file_count,
        deleted: true,
        missing: false,
        warnings: Vec::new(),
    })
}

fn remove_install_rows(state: &AppState, game_id: i64) -> Result<(), String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::remove_installed(&connection, game_id)?;
    db::remove_manifest(&connection, game_id)?;
    db::delete_save_paths(&connection, game_id)?;
    db::delete_save_detection(&connection, game_id)?;
    Ok(())
}

/* ------------------------------- manifest -------------------------------- */

/// Hash an adopted folder so Verify/Repair work (#129).
///
/// The manifest is stored in the desktop index exactly like an installed one;
/// nothing is written inside the game folder.
pub async fn build_manifest(app: &AppHandle, game_id: i64) -> Result<ManifestReport, String> {
    let state = app.state::<AppState>();
    let installed = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_installed(&connection, game_id)?
    }
    .ok_or_else(|| format!("Game {game_id} is not installed."))?;
    ensure_idle(&state, game_id, "building its manifest")?;

    let root = PathBuf::from(&installed.install_path);
    if !root.is_dir() {
        return Err(format!(
            "The install folder is missing: {}. Re-link the folder or remove the entry.",
            root.display()
        ));
    }

    let handle = app.clone();
    let manifest = tokio::task::spawn_blocking(move || {
        manifest::build_manifest(&root, &mut |progress| {
            let _ = handle.emit(
                "install://manifest-progress",
                ManifestProgress {
                    game_id,
                    files_done: progress.files_done,
                    files_total: progress.files_total,
                    bytes_done: progress.bytes_done,
                    bytes_total: progress.bytes_total,
                    message: format!(
                        "Hashing {} of {} files",
                        progress.files_done, progress.files_total
                    ),
                },
            );
        })
    })
    .await
    .map_err(|e| format!("The manifest task failed: {e}"))??;

    let built_at = now_rfc3339();
    let record = db::InstallManifestRecord {
        game_id,
        version_id: installed.version_id,
        install_path: installed.install_path.clone(),
        manifest: serde_json::to_string(&manifest)
            .map_err(|e| format!("Could not encode the file manifest: {e}"))?,
        files: i64::try_from(manifest.entries.len()).unwrap_or(i64::MAX),
        bytes: i64::try_from(manifest.total_bytes).unwrap_or(i64::MAX),
        created_at: built_at.clone(),
        updated_at: built_at.clone(),
    };
    {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::upsert_manifest(&connection, &record)?;
    }

    Ok(ManifestReport {
        game_id,
        files: manifest.entries.len() as u64,
        bytes: manifest.total_bytes,
        built_at,
    })
}

/// True when the typed confirmation is exactly the canonical folder.
///
/// The user can type the path with a trailing separator; a bare folder name or a
/// parent/sibling path never matches.
pub fn confirmation_matches(typed: &str, folder: &Path) -> bool {
    let typed = typed.trim();
    if typed.is_empty() {
        return false;
    }
    let confirmed = fs::canonicalize(typed).unwrap_or_else(|_| PathBuf::from(typed));
    confirmed == folder
}

/* ------------------------------- GameVault ------------------------------- */

/// One subfolder of a GameVault-style install root (#129, #134).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVaultCandidate {
    pub folder: String,
    pub name: String,
    /// Root this candidate was found under (#134).
    pub source_root: String,
    /// Executable Ludex would launch now; `None` means "link anyway and choose".
    pub executable: Option<String>,
    /// Every plausible executable found, best first (#134). Empty when none.
    pub executable_candidates: Vec<String>,
    pub size_bytes: u64,
    pub file_count: u64,
    /// Title read from an optional `gamevault.json`/`.gamevault.json`.
    pub metadata_title: Option<String>,
    pub matched_game_id: Option<i64>,
    pub matched_title: Option<String>,
    pub match_status: MatchStatus,
    /// `adopted`, `ludex` or a launcher id when the folder is already indexed.
    pub installed: Option<String>,
    /// Informational diagnostics: skipped subfolders, depth truncation, the
    /// chosen candidate among several, Store-style layouts…
    pub notes: Vec<String>,
    /// Problems that need attention: unreadable folders, permission errors,
    /// broken links, long-path failures. Always carry the affected path.
    pub warnings: Vec<String>,
}

/// Status of one root in a multi-root scan (#134).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVaultRootScan {
    pub root: String,
    /// Immediate game folders found under this root.
    pub candidates: usize,
    pub warnings: Vec<String>,
    /// Set when the root itself could not be read or prepared.
    pub error: Option<String>,
}

/// Result of scanning one or more GameVault roots in a single pass (#134).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVaultScan {
    /// One entry per configured root, in the order the user configured.
    pub roots: Vec<GameVaultRootScan>,
    /// Deduplicated candidates from every root, grouped by root order.
    pub candidates: Vec<GameVaultCandidate>,
    /// Scan-level diagnostics (duplicate roots, folders found twice…).
    pub warnings: Vec<String>,
    /// False when the server could not be listed, so nothing was matched.
    pub matched: bool,
    /// Folder depth actually used for this scan.
    pub max_depth: usize,
}

/// A GameVault root proposed by the read-only auto-detection (#134).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVaultRootProposal {
    pub path: String,
    /// Why this folder is proposed: already exists, read from a config file…
    pub reason: String,
    pub exists: bool,
    /// True when the root is already configured and needs no action.
    pub configured: bool,
    /// Immediate subfolders that look like game folders, for confirmation.
    pub game_folders: u64,
}

/// Read a GameVault metadata title without ever writing the file back.
fn read_metadata_title(folder: &Path) -> Option<String> {
    for name in GAMEVAULT_METADATA_FILES {
        let path = folder.join(name);
        if !path.is_file() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        for key in ["title", "name"] {
            if let Some(title) = value.get(key).and_then(|value| value.as_str()) {
                let title = title.trim();
                if !title.is_empty() {
                    return Some(title.to_string());
                }
            }
        }
    }
    None
}

/// True when a folder carries GameVault metadata, read-only.
fn has_metadata(folder: &Path) -> bool {
    GAMEVAULT_METADATA_FILES
        .iter()
        .any(|name| folder.join(name).is_file())
}

/// Match a parsed folder name to server titles. Returns the normalized name and
/// the raw name so containment works on both forms.
fn folder_names(folder: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = folder
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
    {
        let parsed = ludex_core::naming::parse_filename(&name);
        if !parsed.title.trim().is_empty() {
            names.push(parsed.title);
        }
        names.push(name);
    }
    names
}

/// True for a Microsoft Store layout, which never has a classic `.exe`.
fn is_store_folder(folder: &Path) -> bool {
    folder.join("AppxManifest.xml").is_file() || folder.join("AppxBlockMap.xml").is_file()
}

/// Best-effort executable hunt for a folder, with the user's scan depth.
fn scan_folder_executables(
    folder: &Path,
    title: &str,
    max_depth: usize,
) -> crate::bridge::Discovery {
    crate::bridge::discover_executables(folder, title, max_depth)
}

/// Split discovery notes into informational notes and actionable warnings.
fn split_discovery_notes(discovery: &crate::bridge::Discovery) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    let mut warnings = Vec::new();
    for note in &discovery.notes {
        if note.starts_with("Could not read")
            || note.starts_with("Could not inspect")
            || note.contains("broken link")
        {
            warnings.push(note.clone());
        } else {
            notes.push(note.clone());
        }
    }
    (notes, warnings)
}

/// Scan several GameVault roots in one pass (#134).
///
/// Every immediate subfolder that contains an executable, matches a library
/// game or carries GameVault metadata is returned. A matched or metadata-bearing
/// folder is never dropped: when no executable is found it stays with
/// `executable: None`, a note and the `.exe` files that were found. Candidate
/// folders are deduplicated by canonical path across roots, and each root keeps
/// its own status so an unreadable drive never hides the others.
///
/// Nothing is written, and GameVault metadata is only read.
pub fn scan_gamevault_roots(
    roots: &[PathBuf],
    installed: &[(i64, String, String)],
    server: &[(i64, String)],
    max_depth: usize,
) -> (Vec<GameVaultCandidate>, Vec<GameVaultRootScan>, Vec<String>) {
    let max_depth = crate::bridge::clamp_scan_depth(max_depth);
    let mut candidates: Vec<GameVaultCandidate> = Vec::new();
    let mut root_scans: Vec<GameVaultRootScan> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut seen_roots: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_folders: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for raw_root in roots {
        let canonical_root = fs::canonicalize(raw_root).unwrap_or_else(|_| raw_root.clone());
        let root_key = canonical_root.to_string_lossy().to_string();
        if !seen_roots.insert(root_key.clone()) {
            warnings.push(format!(
                "{root_key} is listed twice as a GameVault root; it is scanned once."
            ));
            continue;
        }

        let mut root_scan = GameVaultRootScan {
            root: root_key.clone(),
            candidates: 0,
            warnings: Vec::new(),
            error: None,
        };

        let entries = match fs::read_dir(&canonical_root) {
            Ok(entries) => entries,
            Err(error) => {
                let message = format!("Could not read {root_key}: {error}");
                root_scan.error = Some(message.clone());
                warnings.push(message);
                root_scans.push(root_scan);
                continue;
            }
        };

        let mut folders: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                root_scan
                    .warnings
                    .push(format!("Could not read an entry in {root_key}."));
                continue;
            };
            let path = entry.path();
            // `metadata` follows a junction/symlink so a linked game folder is
            // still discovered; `canonical_folder` refuses linked folders later
            // and the note below explains it.
            let Ok(metadata) = fs::metadata(&path) else {
                root_scan
                    .warnings
                    .push(format!("Could not inspect {}.", path.display()));
                continue;
            };
            if metadata.is_dir() {
                let link = fs::symlink_metadata(&path)
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false);
                if link {
                    root_scan.warnings.push(format!(
                        "{} is a link. Linking it will ask for the real folder.",
                        path.display()
                    ));
                }
                folders.push(path);
            }
        }
        folders.sort();

        for folder in folders {
            let name = folder
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }

            let canonical = fs::canonicalize(&folder).unwrap_or_else(|_| folder.clone());
            let folder_key = canonical.to_string_lossy().to_string();

            let metadata_title = read_metadata_title(&folder);
            let store_folder = is_store_folder(&folder);
            let mut names = folder_names(&folder);
            if let Some(title) = &metadata_title {
                names.push(title.clone());
            }
            let (matched_game_id, match_status) = match_library_title(&names, server);
            let discovery = scan_folder_executables(
                &folder,
                metadata_title.as_deref().unwrap_or(&name),
                max_depth,
            );
            let executable = discovery
                .chosen
                .as_ref()
                .map(|path| path.to_string_lossy().to_string());
            let executable_candidates: Vec<String> = discovery
                .candidates
                .iter()
                .map(|candidate| candidate.path.to_string_lossy().to_string())
                .collect();

            let matched = matched_game_id.is_some() || match_status == MatchStatus::Ambiguous;
            if !matched
                && executable_candidates.is_empty()
                && metadata_title.is_none()
                && !store_folder
            {
                warnings.push(format!(
                    "{name}: no game executable found and no library game matches; skipped."
                ));
                continue;
            }

            if let Some(first_root) = seen_folders.get(&folder_key) {
                let message = format!(
                    "{} is listed under both {first_root} and {root_key}; it is listed once.",
                    canonical.display()
                );
                warnings.push(message.clone());
                root_scan.warnings.push(message);
                continue;
            }
            seen_folders.insert(folder_key.clone(), root_key.clone());

            let (size_bytes, file_count) = paths::dir_stats(&canonical);
            let installed_source = installed
                .iter()
                .find(|(_, path, _)| path_matches(path, &canonical))
                .map(|(_, _, source)| source.clone());
            let matched_title = matched_game_id.and_then(|id| {
                server
                    .iter()
                    .find(|(entry, _)| *entry == id)
                    .map(|(_, title)| title.clone())
            });

            let (mut notes, candidate_warnings) = split_discovery_notes(&discovery);
            if match_status == MatchStatus::Ambiguous {
                notes.push(
                    "Several library games match this folder; choose the right one before adopting."
                        .to_string(),
                );
            }
            if let Some(source) = &installed_source {
                notes.push(format!("Already indexed as {source}."));
            }
            if executable.is_none() {
                if store_folder {
                    notes.push(
                        "This looks like a Microsoft Store package: there is no classic .exe to launch. Link it anyway and choose the executable in the game settings if the game ships one."
                            .to_string(),
                    );
                } else if matched {
                    notes.push(format!(
                        "No game executable was found (searched {max_depth} folder levels). Link it anyway and choose the executable in the game settings."
                    ));
                }
            } else if executable_candidates.len() > 1 {
                notes.push(format!(
                    "{} executables found; the first one is used. You can choose another in the game settings.",
                    executable_candidates.len()
                ));
            }
            if executable.is_none() && !executable_candidates.is_empty() {
                notes.push(format!(
                    "Found {} executable(s) but none matches the title: {}",
                    executable_candidates.len(),
                    executable_candidates
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<String>>()
                        .join(", ")
                ));
            }

            root_scan.candidates += 1;
            candidates.push(GameVaultCandidate {
                folder: folder_key,
                name,
                source_root: root_key.clone(),
                executable,
                executable_candidates,
                size_bytes,
                file_count,
                metadata_title,
                matched_game_id,
                matched_title,
                match_status,
                installed: installed_source,
                notes,
                warnings: candidate_warnings,
            });
        }

        root_scans.push(root_scan);
    }

    (candidates, root_scans, warnings)
}

/// Scan a single root (kept for callers and tests that only need one).
#[cfg(test)]
pub fn scan_gamevault_root(
    root: &Path,
    installed: &[(i64, String, String)],
    server: &[(i64, String)],
) -> (Vec<GameVaultCandidate>, Vec<String>) {
    let (candidates, roots, warnings) = scan_gamevault_roots(
        &[root.to_path_buf()],
        installed,
        server,
        crate::bridge::DEFAULT_SCAN_DEPTH,
    );
    let mut errors: Vec<String> = roots.into_iter().filter_map(|scan| scan.error).collect();
    errors.extend(warnings);
    (candidates, errors)
}

/* ---------------------- GameVault root auto-detection -------------------- */

/// Absolute directories worth probing for a GameVault install (#134).
fn gamevault_probe_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home_dir() {
        for name in ["Games/GameVault", "GameVault", "games/GameVault"] {
            dirs.push(home.join(name));
        }
    }
    #[cfg(windows)]
    {
        // A: and B: are floppy drives on old machines and can block for seconds.
        for letter in b'C'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            dirs.push(PathBuf::from(&drive).join("Games").join("GameVault"));
            dirs.push(PathBuf::from(&drive).join("GameVault"));
        }
    }
    #[cfg(not(windows))]
    {
        for base in ["/mnt", "/media", "/run/media"] {
            let base = PathBuf::from(base);
            if !base.is_dir() {
                continue;
            }
            if let Ok(entries) = fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let path = entry.path().join("Games").join("GameVault");
                    if path.is_dir() {
                        dirs.push(path);
                    }
                }
            }
        }
    }
    dirs
}

/// GameVault client config files probed read-only (#134).
fn gamevault_config_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = home_dir() {
        for name in [
            ".config/gamevault/config.json",
            ".config/GameVault/config.json",
            ".config/gamevault/settings.json",
            ".gamevault/config.json",
        ] {
            files.push(home.join(name));
        }
    }
    for variable in ["APPDATA", "LOCALAPPDATA", "PROGRAMDATA"] {
        if let Some(raw) = std::env::var_os(variable) {
            let base = PathBuf::from(raw);
            for name in ["GameVault", "gamevault"] {
                files.push(base.join(name).join("config.json"));
                files.push(base.join(name).join("settings.json"));
            }
        }
    }
    files
}

/// Collect string values whose key looks like an install folder, recursively.
fn collect_config_paths(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, entry) in map {
                let lower = key.to_ascii_lowercase();
                let key_looks_like_a_folder = [
                    "install",
                    "path",
                    "folder",
                    "directory",
                    "library",
                    "root",
                    "download",
                    "games",
                ]
                .iter()
                .any(|needle| lower.contains(needle));
                if key_looks_like_a_folder {
                    if let Some(raw) = entry.as_str() {
                        found.push(raw.to_string());
                    }
                }
                collect_config_paths(entry, found);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_config_paths(item, found);
            }
        }
        _ => {}
    }
}

/// Immediate subfolders that look like game folders, for confirmation only.
fn count_game_folders(root: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return false;
            }
            if has_metadata(&path) || is_store_folder(&path) {
                return true;
            }
            // A hint is enough here: this only labels a proposal, the real scan
            // runs when the user confirms.
            crate::bridge::discover_executables(&path, "", 3)
                .candidates
                .iter()
                .any(|candidate| {
                    candidate
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                })
        })
        .count() as u64
}

/// Read-only proposal of GameVault roots (#134).
///
/// Probes the usual locations (one GameVault install location per drive) and,
/// when they exist, GameVault's own client config files. It never writes to
/// GameVault and never adopts anything: the UI shows each proposal with its
/// reason and the user confirms before it becomes a configured root.
pub fn detect_gamevault_roots(configured: &[String]) -> Vec<GameVaultRootProposal> {
    let mut proposals: Vec<GameVaultRootProposal> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let configured_keys: std::collections::HashSet<String> = configured
        .iter()
        .map(|raw| {
            fs::canonicalize(raw)
                .unwrap_or_else(|_| PathBuf::from(raw))
                .to_string_lossy()
                .to_string()
        })
        .collect();

    let mut push = |path: PathBuf, reason: String| {
        let Some(canonical) = fs::canonicalize(&path).ok() else {
            return;
        };
        let key = canonical.to_string_lossy().to_string();
        if !seen.insert(key.clone()) {
            return;
        }
        proposals.push(GameVaultRootProposal {
            path: key.clone(),
            reason,
            exists: canonical.is_dir(),
            configured: configured_keys.contains(&key),
            game_folders: if canonical.is_dir() {
                count_game_folders(&canonical)
            } else {
                0
            },
        });
    };

    for dir in gamevault_probe_dirs() {
        if dir.is_dir() {
            push(dir, "Found at a usual GameVault location.".to_string());
        }
    }

    for file in gamevault_config_files() {
        if !file.is_file() {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&file) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let mut found = Vec::new();
        collect_config_paths(&value, &mut found);
        for entry in found {
            let candidate = PathBuf::from(entry.trim());
            if !candidate.is_absolute() || !candidate.is_dir() {
                continue;
            }
            push(candidate, format!("Configured in {}.", file.display()));
        }
    }

    proposals
}

/// Canonicalize and validate a GameVault scan root without listing it (#134).
///
/// Unlike [`canonical_folder`], a root may be a junction/symlink: scanning is
/// read-only and the canonical target is what gets stored, so a linked
/// `C:\Games\GameVault` still works. Candidates are validated separately when
/// they are adopted.
pub fn prepare_scan_root(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Choose the GameVault folder to scan.".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("The folder path contains control characters.".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("The folder path must be absolute.".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("The folder path cannot contain “..”.".to_string());
    }
    let metadata =
        fs::metadata(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a folder.", path.display()));
    }
    let canonical = fs::canonicalize(&path)
        .map_err(|e| format!("Could not resolve {}: {e}", path.display()))?;
    if canonical.parent().is_none() {
        return Err("Choose the games folder, not a drive root.".to_string());
    }
    if let Some(home) = home_dir() {
        if same_path(&home, &canonical) {
            return Err(
                "Choose a folder inside your home folder, not the home folder itself.".to_string(),
            );
        }
    }
    Ok(canonical)
}

/// One selected GameVault folder sent to the bulk adopt command.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVaultAdoptEntry {
    pub folder: String,
    pub game_id: i64,
    #[serde(default)]
    pub version_id: Option<i64>,
    #[serde(default)]
    pub version: Option<String>,
    /// Executable chosen among the ranked candidates (#134). Empty = detect.
    #[serde(default)]
    pub executable: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVaultAdoptSkip {
    pub folder: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVaultAdoptReport {
    pub adopted: Vec<InstalledGame>,
    pub skipped: Vec<GameVaultAdoptSkip>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::FileManifest;

    fn temp_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-adopt-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn adopted_row(path: &str) -> InstalledGame {
        InstalledGame {
            game_id: 1,
            version_id: 0,
            title: "Game".into(),
            version: None,
            install_path: path.into(),
            executable: None,
            size_bytes: 0,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: None,
            source: db::SOURCE_ADOPTED.into(),
            launch_uri: None,
        }
    }

    /// The command path minus Tauri: inspect, then write the row.
    fn adopt_for_test(
        connection: &rusqlite::Connection,
        install_root: &Path,
        game_id: i64,
        title: &str,
        folder: &str,
        replace: bool,
    ) -> Result<InstalledGame, String> {
        let installed = db::list_installed(connection)?;
        let (_preview, record) = prepare_adoption(
            &installed,
            install_root,
            game_id,
            title,
            None,
            None,
            folder,
            None,
        )?;
        db::record_adoption(connection, &record, replace)?;
        Ok(record)
    }

    #[test]
    fn canonical_folder_refuses_relative_and_traversal_paths() {
        assert!(canonical_folder("games/doom").is_err());
        assert!(canonical_folder("../games").is_err());
        assert!(canonical_folder("").is_err());

        let root = temp_dir("canonical");
        let folder = root.join("doom");
        fs::create_dir_all(&folder).unwrap();

        let canonical = canonical_folder(&folder.to_string_lossy()).unwrap();
        assert!(canonical.is_absolute());
        assert!(canonical.ends_with("doom"));

        // A file is not a folder.
        let file = root.join("game.exe");
        fs::write(&file, b"MZ").unwrap();
        assert!(canonical_folder(&file.to_string_lossy()).is_err());

        // A link is refused: the stored path must be the real folder.
        #[cfg(unix)]
        {
            let link = root.join("link");
            std::os::unix::fs::symlink(&folder, &link).unwrap();
            assert!(canonical_folder(&link.to_string_lossy()).is_err());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adoptable_folders_are_not_roots_or_the_install_folder() {
        let root = temp_dir("roots");
        let folder = root.join("doom");
        fs::create_dir_all(&folder).unwrap();

        assert!(validate_adoptable(&folder, None).is_ok());
        // The install folder itself is refused, a child of it is not.
        assert!(validate_adoptable(&root, Some(&root)).is_err());
        assert!(validate_adoptable(&folder, Some(&root)).is_ok());
        // Deleting straight into the install root is refused, one level deeper is fine.
        assert!(validate_deletable(&folder, Some(&root)).is_err());
        let nested = folder.join("install");
        fs::create_dir_all(&nested).unwrap();
        assert!(validate_deletable(&nested, Some(&root)).is_ok());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn symlinks_that_escape_the_folder_are_refused() {
        #[cfg(unix)]
        {
            let root = temp_dir("symlink");
            let folder = root.join("game");
            fs::create_dir_all(&folder).unwrap();
            let outside = root.join("outside");
            fs::create_dir_all(&outside).unwrap();

            // A link inside the folder is harmless.
            std::os::unix::fs::symlink(&outside, folder.join("inner")).unwrap();
            assert!(ensure_no_symlink_escape(&folder).is_err());

            let inside = folder.join("sub");
            fs::create_dir_all(&inside).unwrap();
            fs::remove_file(folder.join("inner")).unwrap();
            std::os::unix::fs::symlink(&inside, folder.join("inner")).unwrap();
            assert!(ensure_no_symlink_escape(&folder).is_ok());

            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn preview_detects_the_executable_size_and_count() {
        let root = temp_dir("preview");
        let folder = root.join("Hades");
        fs::create_dir_all(folder.join("data")).unwrap();
        fs::write(folder.join("Hades.exe"), vec![0u8; 1024]).unwrap();
        fs::write(folder.join("data/level.bin"), vec![0u8; 2048]).unwrap();

        let (preview, record) = prepare_adoption(
            &[],
            &root,
            7,
            "Hades",
            None,
            None,
            &folder.to_string_lossy(),
            None,
        )
        .unwrap();
        assert_eq!(preview.size_bytes, 3 * 1024);
        assert_eq!(preview.file_count, 2);
        assert!(preview
            .executable
            .as_deref()
            .is_some_and(|path| path.ends_with("Hades.exe")));
        assert_eq!(preview.version_id, 0);
        assert_eq!(preview.version, None);
        assert!(preview.warnings.is_empty());
        assert_eq!(record.source, db::SOURCE_ADOPTED);
        assert_eq!(record.version_id, 0);
        assert_eq!(record.archive_hash, None);
        assert_eq!(
            record.install_root.as_deref(),
            Some(root.to_string_lossy().as_ref())
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn preview_without_an_executable_warns_but_works() {
        let root = temp_dir("noexe");
        let folder = root.join("data-only");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("readme.txt"), b"hello").unwrap();

        let (preview, record) = prepare_adoption(
            &[],
            &root,
            3,
            "Data Only",
            Some(9),
            Some("1.2"),
            &folder.to_string_lossy(),
            None,
        )
        .unwrap();
        assert!(preview.executable.is_none());
        assert_eq!(preview.warnings.len(), 1);
        assert_eq!(preview.version_id, 9);
        assert_eq!(preview.version.as_deref(), Some("1.2"));
        assert_eq!(record.executable, None);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopting_over_a_ludex_install_warns_and_requires_replace() {
        let root = temp_dir("replace");
        let folder = root.join("doom");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("doom.exe"), b"MZ").unwrap();

        let mut existing = adopted_row("/games/doom/1.0");
        existing.source = db::SOURCE_LUDEX.into();

        let (preview, _) = prepare_adoption(
            &[existing],
            &root,
            1,
            "Doom",
            None,
            None,
            &folder.to_string_lossy(),
            None,
        )
        .unwrap();
        assert!(preview.existing.is_some());
        assert_eq!(preview.warnings.len(), 1);
        assert!(preview.warnings[0].contains("installed through Ludex"));

        let connection = crate::db::open_in_memory_for_tests();
        crate::db::upsert_installed(&connection, &adopted_row("/games/doom/1.0")).unwrap();
        // Source is adopted there, so no replace is needed; a ludex/launcher row
        // is covered by the db test.
        let record = adopt_for_test(
            &connection,
            &root,
            1,
            "Doom",
            &folder.to_string_lossy(),
            false,
        )
        .unwrap();
        assert_eq!(record.source, db::SOURCE_ADOPTED);
        assert_eq!(record.game_id, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopting_a_folder_owned_by_a_launcher_entry_is_refused() {
        let root = temp_dir("launcher");
        let folder = root.join("Hades");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("Hades.exe"), b"MZ").unwrap();

        let mut steam_row = adopted_row(&folder.to_string_lossy());
        steam_row.game_id = 99;
        steam_row.title = "Hades".into();
        steam_row.source = db::SOURCE_STEAM.into();

        let error = prepare_adoption(
            &[steam_row],
            &root,
            7,
            "Hades",
            None,
            None,
            &folder.to_string_lossy(),
            None,
        )
        .unwrap_err();
        assert!(error.contains("launcher") || error.contains("steam"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopting_a_folder_that_contains_or_sits_inside_another_install_is_refused() {
        let root = temp_dir("nested");
        let install_root = root.join("ludex");
        fs::create_dir_all(&install_root).unwrap();
        let inner = root.join("hades");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("Hades.exe"), b"MZ").unwrap();

        let mut other = adopted_row(&inner.to_string_lossy());
        other.game_id = 99;
        other.title = "Hades".into();

        // The parent of an existing install cannot be adopted: deleting it would
        // remove the nested game too.
        let error = prepare_adoption(
            &[other.clone()],
            &install_root,
            7,
            "Hades Collection",
            None,
            None,
            &root.to_string_lossy(),
            None,
        )
        .unwrap_err();
        assert!(error.contains("contains"), "{error}");

        // A subfolder of an existing install is refused for the same reason.
        let nested = inner.join("data");
        fs::create_dir_all(&nested).unwrap();
        let error = prepare_adoption(
            &[other],
            &install_root,
            7,
            "Hades Data",
            None,
            None,
            &nested.to_string_lossy(),
            None,
        )
        .unwrap_err();
        assert!(error.contains("inside"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopting_the_same_folder_twice_is_refused() {
        let root = temp_dir("dupe");
        let folder = root.join("doom");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("doom.exe"), b"MZ").unwrap();

        let connection = crate::db::open_in_memory_for_tests();
        adopt_for_test(
            &connection,
            &root,
            1,
            "Doom",
            &folder.to_string_lossy(),
            false,
        )
        .unwrap();

        // Another game pointing at the same folder is refused by db::record_adoption.
        let error = adopt_for_test(
            &connection,
            &root,
            2,
            "Doom II",
            &folder.to_string_lossy(),
            false,
        )
        .unwrap_err();
        assert!(error.contains("already"), "{error}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unlink_keeps_the_files() {
        let root = temp_dir("unlink");
        let folder = root.join("doom");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("doom.exe"), b"MZ").unwrap();

        let connection = crate::db::open_in_memory_for_tests();
        adopt_for_test(
            &connection,
            &root,
            1,
            "Doom",
            &folder.to_string_lossy(),
            false,
        )
        .unwrap();
        assert!(crate::db::get_installed(&connection, 1).unwrap().is_some());
        crate::db::remove_installed(&connection, 1).unwrap();
        assert!(folder.join("doom.exe").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_confirmation_must_be_the_exact_canonical_path() {
        let root = temp_dir("confirm");
        let folder = root.join("doom");
        fs::create_dir_all(&folder).unwrap();
        let canonical = fs::canonicalize(&folder).unwrap();
        let path = canonical.to_string_lossy().to_string();

        assert!(confirmation_matches(&path, &canonical));
        assert!(confirmation_matches(
            &format!("{path}{}", std::path::MAIN_SEPARATOR),
            &canonical
        ));
        assert!(!confirmation_matches("doom", &canonical));
        assert!(!confirmation_matches("", &canonical));
        assert!(!confirmation_matches(&root.to_string_lossy(), &canonical));
        assert!(!confirmation_matches(
            &root.join("other").to_string_lossy(),
            &canonical
        ));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_plan_handles_a_folder_that_disappeared_or_became_a_link() {
        let root = temp_dir("plan");
        let folder = root.join("doom");
        fs::create_dir_all(&folder).unwrap();

        assert_eq!(
            plan_delete(&folder).unwrap(),
            DeletePlan::Delete(fs::canonicalize(&folder).unwrap())
        );

        fs::remove_dir_all(&folder).unwrap();
        assert_eq!(plan_delete(&folder).unwrap(), DeletePlan::Missing);

        // A folder replaced by a link to somewhere else is refused.
        #[cfg(unix)]
        {
            fs::create_dir_all(&folder).unwrap();
            let outside = root.join("outside");
            fs::create_dir_all(&outside).unwrap();
            fs::remove_dir_all(&folder).unwrap();
            std::os::unix::fs::symlink(&outside, &folder).unwrap();
            assert!(plan_delete(&folder).is_err());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopted_manifest_can_be_built_and_verified() {
        let root = temp_dir("manifest");
        let folder = root.join("doom");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("doom.exe"), b"MZ-game").unwrap();

        let manifest: FileManifest = manifest::build_manifest(&folder, &mut |_| {}).unwrap();
        assert_eq!(manifest.entries.len(), 1);
        let outcome = manifest::verify_tree(&folder, &manifest, &mut |_| {}).unwrap();
        assert!(!outcome.has_damage());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_roots_are_canonical_folders_not_drive_roots() {
        let root = temp_dir("scanroot");
        assert_eq!(
            prepare_scan_root(&root.to_string_lossy()).unwrap(),
            fs::canonicalize(&root).unwrap()
        );
        assert!(prepare_scan_root("gamevault").is_err());
        assert!(prepare_scan_root("").is_err());
        #[cfg(unix)]
        assert!(prepare_scan_root("/").is_err());
        // A linked root is resolved: scanning it is read-only and the canonical
        // target is what gets remembered (#134).
        #[cfg(unix)]
        {
            let link = root.with_file_name(format!(
                "{}-link",
                root.file_name().unwrap().to_string_lossy()
            ));
            std::os::unix::fs::symlink(&root, &link).unwrap();
            assert_eq!(
                prepare_scan_root(&link.to_string_lossy()).unwrap(),
                fs::canonicalize(&root).unwrap()
            );
            let _ = fs::remove_file(&link);
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn title_matching_is_exact_containment_or_ambiguous() {
        let server = vec![
            (1, "Hades".to_string()),
            (2, "Hades II".to_string()),
            (3, "DOOM Eternal".to_string()),
        ];

        assert_eq!(
            match_library_title(&["Hades".to_string()], &server),
            (Some(1), MatchStatus::Exact)
        );
        // Folder names go through `parse_filename` first (see `folder_names`), so
        // the version flag never reaches the matcher.
        let folder = Path::new("/gamevault/Hades II (v1.0.2)");
        let names = folder_names(folder);
        assert!(names.iter().any(|name| name == "Hades II"));
        assert_eq!(
            match_library_title(&names, &server),
            (Some(2), MatchStatus::Exact)
        );
        assert_eq!(
            match_library_title(&["DOOM Eternal Deluxe Edition".to_string()], &server),
            (Some(3), MatchStatus::Containment)
        );
        // A single containment is accepted; two overlapping library titles are
        // reported as ambiguous and never guessed.
        assert_eq!(
            match_library_title(&["Hades Game Folder".to_string()], &server),
            (Some(1), MatchStatus::Containment)
        );
        let overlapping = vec![(1, "Doom".to_string()), (2, "Doom Eternal".to_string())];
        assert_eq!(
            match_library_title(&["Doom Eternal Deluxe Edition".to_string()], &overlapping),
            (None, MatchStatus::Ambiguous)
        );
        assert_eq!(
            match_library_title(&["Celeste".to_string()], &server),
            (None, MatchStatus::None)
        );
        // Word boundaries: "doom" alone must not match "DOOM Eternal" if the
        // candidate is an unrelated compound word.
        assert_eq!(
            match_library_title(&["Doometernal Soundtrack".to_string()], &server),
            (None, MatchStatus::None)
        );
    }

    #[test]
    fn gamevault_scan_lists_folders_with_an_executable() {
        let root = temp_dir("gamevault");
        let hades = root.join("Hades (v1.0)");
        fs::create_dir_all(&hades).unwrap();
        fs::write(hades.join("Hades.exe"), b"MZ").unwrap();
        fs::write(hades.join("gamevault.json"), br#"{"title":"Hades"}"#).unwrap();

        let data_only = root.join("Soundtracks");
        fs::create_dir_all(&data_only).unwrap();
        fs::write(data_only.join("track.mp3"), b"id3").unwrap();

        let server = vec![(1, "Hades".to_string()), (2, "Celeste".to_string())];
        let (candidates, warnings) = scan_gamevault_root(&root, &[], &server);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "Hades (v1.0)");
        assert_eq!(candidates[0].metadata_title.as_deref(), Some("Hades"));
        assert_eq!(candidates[0].matched_game_id, Some(1));
        assert_eq!(candidates[0].match_status, MatchStatus::Exact);
        assert!(candidates[0].executable.is_some());
        assert_eq!(
            candidates[0].source_root,
            fs::canonicalize(&root).unwrap().to_string_lossy()
        );
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("Soundtracks")));

        // The metadata file is only read, never rewritten.
        assert_eq!(
            fs::read_to_string(hades.join("gamevault.json")).unwrap(),
            r#"{"title":"Hades"}"#
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gamevault_scan_finds_a_nested_executable_at_depth_five_and_beyond() {
        let root = temp_dir("gamevault-deep");
        let game = root.join("Forza Horizon 6");
        fs::create_dir_all(game.join("a/b/c/d")).unwrap();
        fs::write(game.join("a/b/c/d/ForzaHorizon6.exe"), b"MZ").unwrap();

        let server = vec![(10, "Forza Horizon 6".to_string())];
        let (candidates, _) = scan_gamevault_root(&root, &[], &server);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].executable.as_deref(),
            Some(
                game.join("a/b/c/d/ForzaHorizon6.exe")
                    .to_string_lossy()
                    .as_ref()
            )
        );
        // The ranked list is exposed so the UI can offer another file.
        assert!(candidates[0].executable_candidates.len() >= 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gamevault_scan_keeps_a_matched_folder_without_any_executable() {
        let root = temp_dir("gamevault-noexe");
        let game = root.join("It Takes Two");
        fs::create_dir_all(game.join("data")).unwrap();
        fs::write(game.join("data/level.bin"), b"data").unwrap();
        fs::write(game.join("readme.txt"), b"hello").unwrap();

        let server = vec![(20, "It Takes Two".to_string())];
        let (candidates, warnings) = scan_gamevault_root(&root, &[], &server);

        // Never dropped: the user can link it and choose the executable later.
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].matched_game_id, Some(20));
        assert_eq!(candidates[0].executable, None);
        assert!(candidates[0].executable_candidates.is_empty());
        assert!(candidates[0]
            .notes
            .iter()
            .any(|note| note.contains("Link it anyway")));
        assert!(candidates[0].notes.iter().any(|note| note.contains("6")));
        // Not a generic "no executable": the folder itself was readable.
        assert!(
            candidates[0].warnings.is_empty(),
            "{:?}",
            candidates[0].warnings
        );
        assert!(warnings.is_empty(), "{warnings:?}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gamevault_scan_keeps_a_store_style_folder_without_a_classic_exe() {
        let root = temp_dir("gamevault-store");
        let game = root.join("Minecraft Dungeons");
        fs::create_dir_all(game.join("Content")).unwrap();
        fs::write(game.join("AppxManifest.xml"), b"<Package/>").unwrap();
        fs::write(game.join("Content/level.pak"), b"pak").unwrap();

        let server = vec![(30, "Minecraft Dungeons".to_string())];
        let (candidates, _) = scan_gamevault_root(&root, &[], &server);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].matched_game_id, Some(30));
        assert_eq!(candidates[0].executable, None);
        assert!(candidates[0]
            .notes
            .iter()
            .any(|note| note.contains("Microsoft Store")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gamevault_scan_ranks_multiple_executables_and_never_guesses_unrelated_ones() {
        let root = temp_dir("gamevault-multi");
        let hades = root.join("Hades");
        fs::create_dir_all(&hades).unwrap();
        fs::write(hades.join("Hades.exe"), b"MZ").unwrap();
        fs::write(hades.join("Unrelated.exe"), b"MZ").unwrap();
        let server = vec![(1, "Hades".to_string())];
        let (candidates, _) = scan_gamevault_root(&root, &[], &server);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0]
            .executable
            .as_deref()
            .is_some_and(|path| path.ends_with("Hades.exe")));
        assert_eq!(candidates[0].executable_candidates.len(), 2);
        assert!(candidates[0]
            .executable_candidates
            .first()
            .is_some_and(|path| path.ends_with("Hades.exe")));
        assert!(candidates[0]
            .notes
            .iter()
            .any(|note| note.contains("2 executables found")));

        // Unrelated files only: the folder is kept, but no executable is chosen.
        let forza = root.join("Forza Horizon 6");
        fs::create_dir_all(&forza).unwrap();
        fs::write(forza.join("Helper.exe"), b"MZ").unwrap();
        fs::write(forza.join("Other.exe"), b"MZ").unwrap();
        let server = vec![(2, "Forza Horizon 6".to_string())];
        let (candidates, _) = scan_gamevault_root(&root, &[], &server);
        let forza = candidates
            .iter()
            .find(|candidate| candidate.name == "Forza Horizon 6")
            .expect("candidate");
        assert_eq!(forza.executable, None);
        assert_eq!(forza.executable_candidates.len(), 2);
        assert!(forza
            .notes
            .iter()
            .any(|note| note.contains("choose one manually")
                || note.contains("none matches the title")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gamevault_scan_handles_multiple_roots_and_duplicate_roots() {
        let root_a = temp_dir("gamevault-a");
        let root_b = temp_dir("gamevault-b");
        let alpha = root_a.join("Alpha");
        fs::create_dir_all(&alpha).unwrap();
        fs::write(alpha.join("Alpha.exe"), b"MZ").unwrap();
        let beta = root_b.join("Beta");
        fs::create_dir_all(&beta).unwrap();
        fs::write(beta.join("Beta.exe"), b"MZ").unwrap();

        let server = vec![(1, "Alpha".to_string()), (2, "Beta".to_string())];
        let (candidates, roots, _warnings) =
            scan_gamevault_roots(&[root_a.clone(), root_b.clone()], &[], &server, 6);

        assert_eq!(candidates.len(), 2);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].candidates, 1);
        assert_eq!(roots[1].candidates, 1);
        assert!(roots.iter().all(|root| root.error.is_none()));
        let alpha_source = fs::canonicalize(&root_a)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let beta_source = fs::canonicalize(&root_b)
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(candidates
            .iter()
            .any(|candidate| candidate.name == "Alpha" && candidate.source_root == alpha_source));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.name == "Beta" && candidate.source_root == beta_source));

        // The same root twice is scanned once and reported.
        let (candidates, roots, warnings) =
            scan_gamevault_roots(&[root_a.clone(), root_a.clone()], &[], &server, 6);
        assert_eq!(candidates.len(), 1);
        assert_eq!(roots.len(), 1);
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("listed twice")));

        // Two roots can expose the same game folder (a link): list it once.
        #[cfg(unix)]
        {
            let link = root_b.join("Alpha alias");
            let _ = std::os::unix::fs::symlink(&alpha, &link);
            let (candidates, _, warnings) =
                scan_gamevault_roots(&[root_a.clone(), root_b.clone()], &[], &server, 6);
            assert_eq!(candidates.len(), 2);
            assert_eq!(
                candidates
                    .iter()
                    .filter(|candidate| candidate.name == "Alpha")
                    .count(),
                1
            );
            assert!(warnings
                .iter()
                .any(|warning| warning.contains("listed under both")));
        }

        let _ = fs::remove_dir_all(&root_a);
        let _ = fs::remove_dir_all(&root_b);
    }

    #[cfg(unix)]
    #[test]
    fn gamevault_scan_reports_unreadable_folders_with_the_path() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("gamevault-locked");
        let locked = root.join("Locked Game");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        // Running as root bypasses permissions: skip instead of asserting a lie.
        if fs::read_dir(&locked).is_ok() {
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            let _ = fs::remove_dir_all(&root);
            return;
        }

        let server = vec![(40, "Locked Game".to_string())];
        let (candidates, _) = scan_gamevault_root(&root, &[], &server);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0]
            .warnings
            .iter()
            .any(|warning| warning.contains("Could not read") && warning.contains("Locked Game")));
        assert!(candidates[0]
            .notes
            .iter()
            .any(|note| note.contains("Link it anyway")));

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn gamevault_scan_survives_a_symlink_loop() {
        let root = temp_dir("gamevault-loop");
        let game = root.join("Sackboy");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("Sackboy.exe"), b"MZ").unwrap();
        std::os::unix::fs::symlink(&game, game.join("loop")).unwrap();

        let server = vec![(50, "Sackboy".to_string())];
        let (candidates, _) = scan_gamevault_root(&root, &[], &server);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0]
            .executable
            .as_deref()
            .is_some_and(|path| path.ends_with("Sackboy.exe")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gamevault_scan_skips_installer_redist_and_support_subfolders() {
        let root = temp_dir("gamevault-redist");
        let game = root.join("Uncharted");
        fs::create_dir_all(game.join("_CommonRedist")).unwrap();
        fs::create_dir_all(game.join("Support")).unwrap();
        fs::write(game.join("_CommonRedist/dxsetup.exe"), b"MZ").unwrap();
        fs::write(game.join("Support/helper.exe"), b"MZ").unwrap();
        fs::write(game.join("unins000.exe"), b"MZ").unwrap();
        fs::write(game.join("Uncharted.exe"), b"MZ").unwrap();

        let server = vec![(60, "Uncharted".to_string())];
        let (candidates, _) = scan_gamevault_root(&root, &[], &server);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].executable_candidates.len(), 1);
        assert!(candidates[0]
            .executable
            .as_deref()
            .is_some_and(|path| path.ends_with("Uncharted.exe")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gamevault_detection_is_read_only_and_only_proposes_existing_folders() {
        // No environment tricks: the probe list simply must never invent a path
        // and must never write anything.
        let proposals = detect_gamevault_roots(&[]);
        for proposal in &proposals {
            assert!(proposal.exists, "{}", proposal.path);
            assert!(Path::new(&proposal.path).is_absolute());
            assert!(!proposal.reason.is_empty());
        }
    }

    #[test]
    fn preview_lists_executable_candidates_and_accepts_a_manual_choice() {
        let root = temp_dir("preview-manual");
        let folder = root.join("Hades");
        fs::create_dir_all(folder.join("bin")).unwrap();
        fs::write(folder.join("bin/Hades.exe"), b"MZ").unwrap();
        fs::write(folder.join("Unrelated.exe"), b"MZ").unwrap();

        let (preview, _record) = prepare_adoption(
            &[],
            &root,
            7,
            "Hades",
            None,
            None,
            &folder.to_string_lossy(),
            None,
        )
        .unwrap();
        assert!(preview
            .executable
            .as_deref()
            .is_some_and(|path| path.ends_with("Hades.exe")));
        assert_eq!(preview.executable_candidates.len(), 2);

        // Linking anyway with an explicit executable works and is recorded.
        let (preview, record) = prepare_adoption(
            &[],
            &root,
            7,
            "Hades",
            None,
            None,
            &folder.to_string_lossy(),
            Some("Unrelated.exe"),
        )
        .unwrap();
        assert!(preview
            .executable
            .as_deref()
            .is_some_and(|path| path.ends_with("Unrelated.exe")));
        assert!(record
            .executable
            .as_deref()
            .is_some_and(|path| path.ends_with("Unrelated.exe")));

        let _ = fs::remove_dir_all(&root);
    }
}
