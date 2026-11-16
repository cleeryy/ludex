//! Game deletion, including file and row cleanup.

use axum::extract::State;
use axum::Json;

use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery};
use crate::models::VersionRow;
use crate::roots;
use crate::state::AppState;
use crate::storage::StorageError;
use crate::transfer::local::is_s3_version;
use crate::transfer::object::map_storage_error;

use super::versions::DeleteMode;

/// `DELETE /api/games/{id}` query.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeleteGameQuery {
    #[serde(default)]
    pub(crate) mode: Option<String>,
    /// Legacy spelling of `mode=library` (#209).
    #[serde(default)]
    pub(crate) keep_files: Option<bool>,
    #[serde(default)]
    pub(crate) confirm: Option<bool>,
}

impl DeleteGameQuery {
    pub(crate) fn mode(&self) -> Result<DeleteMode, ApiError> {
        if self.mode.is_none() {
            if let Some(keep) = self.keep_files {
                return Ok(if keep {
                    DeleteMode::Library
                } else {
                    DeleteMode::Archives
                });
            }
        }
        DeleteMode::parse(self.mode.as_deref())
    }
}

/// Result of removing one game from the library.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGameResponse {
    /// Game row that was removed.
    pub game_id: i64,
    /// `library` (rows only) or `archives` (files first).
    pub mode: String,
    /// Versions that existed on the row.
    pub versions: i64,
    /// Versions already marked missing or on an offline root.
    pub missing_versions: i64,
    /// Archive files actually unlinked by this call.
    pub deleted_files: i64,
    /// Bytes reclaimed by the unlinks.
    pub freed_bytes: i64,
    /// Archive files intentionally left on disk.
    pub kept_files: i64,
}

pub(super) async fn delete_game(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<DeleteGameQuery>,
) -> Result<Json<DeleteGameResponse>, ApiError> {
    let mode = query.mode()?;
    match mode {
        DeleteMode::Archives => user.require_admin_access()?,
        DeleteMode::Library => user.require_editor()?,
    }
    if !query.confirm.unwrap_or(false) {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Pass confirm=true to remove this game; the response says what was deleted and what was kept",
        ));
    }
    Ok(Json(remove_game(&state, &user, id, mode).await?))
}

/// Shared by the single-game route and the admin bulk endpoint.
///
/// Files are removed before rows: a row without a file would be re-marked
/// missing by the next scan, while an orphan file is harmless and re-indexable.
pub(crate) async fn remove_game(
    state: &AppState,
    user: &AuthUser,
    game_id: i64,
    mode: DeleteMode,
) -> Result<DeleteGameResponse, ApiError> {
    let Some((title, _default_version)) = db::query_as::<(String, Option<i64>)>(
        "SELECT title, default_version_id FROM games WHERE id = ?",
    )
    .bind(game_id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)?
    else {
        return Err(ApiError::not_found("game_not_found", "Game not found"));
    };

    let versions = db::query_as::<VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, content_sha256,
                indexed_at, updated_at, storage_backend, object_key, etag
         FROM game_versions WHERE game_id = ? ORDER BY id",
    )
    .bind(game_id)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;

    let mut deleted_files = 0_i64;
    let mut freed_bytes = 0_i64;
    let missing_versions = versions
        .iter()
        .filter(|version| version.missing != 0 || version.root_unavailable != 0)
        .count() as i64;

    if mode == DeleteMode::Archives {
        // An offline root cannot be verified: deleting rows would leave files
        // behind with no way to find them again.
        if versions.iter().any(|version| version.root_unavailable != 0) {
            return Err(ApiError::conflict(
                "root_unavailable",
                "A library root holding this game is offline; restore it before deleting the archives",
            ));
        }
        if versions
            .iter()
            .any(|version| state.transfer.transfers.is_active(version.id))
        {
            return Err(ApiError::conflict(
                "version_in_use",
                "A download or stream is using an archive of this game; stop it first",
            ));
        }
        for version in &versions {
            if is_s3_version(version) {
                if let Some(key) = version.object_key.as_deref() {
                    match state.transfer.storage.delete(key).await {
                        Ok(()) => {
                            deleted_files += 1;
                            freed_bytes += version.size;
                        }
                        Err(StorageError::NotFound) => {}
                        Err(error) => return Err(map_storage_error(error, "delete")),
                    }
                }
                continue;
            }
            if delete_archive_file(state, version).await? {
                deleted_files += 1;
                freed_bytes += version.size;
            }
        }
    }

    delete_game_rows(state, game_id).await?;

    state.core.events.emit(
        crate::events::Event::new(crate::audit::actions::GAME_DELETED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "title": title,
                "mode": mode.as_str(),
                "versions": versions.len(),
                "deletedFiles": deleted_files,
                "freedBytes": freed_bytes,
                "missingVersions": missing_versions,
                "files": versions
                    .iter()
                    .filter(|version| version.missing == 0 && version.root_unavailable == 0)
                    .map(|version| version.file_path.as_str())
                    .collect::<Vec<_>>(),
            })),
    );

    Ok(DeleteGameResponse {
        game_id,
        mode: mode.as_str().to_string(),
        versions: versions.len() as i64,
        missing_versions,
        deleted_files,
        freed_bytes,
        kept_files: versions.len() as i64 - deleted_files,
    })
}

/// Unlinks one archive below its library root.
///
/// `Ok(false)` means the file was already gone (idempotent). A symlink, a path
/// that escapes the root, or a canonical target outside it is refused with
/// `unsafe_path` and nothing is deleted.
async fn delete_archive_file(state: &AppState, version: &VersionRow) -> Result<bool, ApiError> {
    let base = roots::base_dir(&state.core.pool, &state.core.config, version.root_id)
        .await
        .map_err(|_| ApiError::internal())?;
    if !base.is_dir() {
        return Err(ApiError::conflict(
            "root_unavailable",
            "The library root holding this archive is offline; restore it before deleting",
        ));
    }
    let Some(path) = crate::scanner::resolve_relative(&base, &version.file_path) else {
        return Err(ApiError::conflict(
            "unsafe_path",
            "The archive path escapes the library root; refusing to delete it",
        ));
    };
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ApiError::conflict(
                "unsafe_path",
                "The archive path is a symlink; refusing to follow it",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            tracing::error!(%error, version = version.id, "could not stat the archive");
            return Err(ApiError::internal());
        }
    }
    let root = tokio::fs::canonicalize(&base)
        .await
        .map_err(|_| ApiError::internal())?;
    let file = tokio::fs::canonicalize(&path)
        .await
        .map_err(|_| ApiError::internal())?;
    if !file.starts_with(&root) {
        return Err(ApiError::conflict(
            "unsafe_path",
            "The archive resolves outside the library root; refusing to delete it",
        ));
    }
    match tokio::fs::remove_file(&file).await {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            tracing::error!(%error, version = version.id, "could not delete the archive");
            Err(ApiError::internal())
        }
    }
}

/// Removes the game and every row that belongs to it.
///
/// Cascade: versions, progress, bookmarks, play sessions, collection entries,
/// share links, achievements and their unlocks, achievement sources/scans,
/// cloud-save snapshots and remote-install requests. Kept: the activity feed
/// and notifications (their `game_id` becomes null), audit logs, health
/// findings, uploads and provider fetches (historical reports).
async fn delete_game_rows(state: &AppState, game_id: i64) -> Result<(), ApiError> {
    state
        .core
        .pool
        .with_tx(async |tx| {
            for sql in [
                "DELETE FROM game_versions WHERE game_id = ?",
                "DELETE FROM achievements WHERE game_id = ?",
                "DELETE FROM achievement_sources WHERE game_id = ?",
                "DELETE FROM achievement_scans WHERE game_id = ?",
                "DELETE FROM collection_games WHERE game_id = ?",
                "DELETE FROM progress WHERE game_id = ?",
                "DELETE FROM bookmarks WHERE game_id = ?",
                "DELETE FROM play_sessions WHERE game_id = ?",
                "DELETE FROM save_snapshots WHERE game_id = ?",
                "DELETE FROM share_links WHERE game_id = ?",
                "DELETE FROM remote_installs WHERE game_id = ?",
                "DELETE FROM games WHERE id = ?",
            ] {
                db::query(sql).bind(game_id).execute(&mut *tx).await?;
            }
            Ok::<(), ApiError>(())
        })
        .await
}
