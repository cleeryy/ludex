//! Version rows: listing, pinning, deletion and the cheap lookups.

use std::path::Path;

use axum::extract::State;
use axum::Json;
use ludex_core::archive::{format_from_extension, ArchiveFormat};
use ludex_core::models::LatestVersion;

use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::models::{
    DeleteVersionResponse, GameVersionInfo, GameVersionsResponse, SetDefaultVersionRequest,
    VersionRow,
};
use crate::roots;
use crate::state::AppState;
use crate::transfer::local::is_s3_version;
use crate::transfer::object::map_storage_error;

use super::ensure_game;

/// Version clients should update to: the game's pinned version (#168), else the
/// newest available one. The payload carries the metadata desktop clients need
/// to detect updates (version label, archive fingerprint, size, last update).
pub(super) async fn latest_version(
    State(state): State<AppState>,
    _user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<LatestVersion>, ApiError> {
    ensure_game(&state, id).await?;

    let version_id = preferred_version_id(&state, id).await.map_err(|error| {
        if error.code == "version_not_found" {
            ApiError::not_found("no_versions", "This game has no available version")
        } else {
            error
        }
    })?;
    let row = fetch_version(&state, id, version_id).await?;

    Ok(Json(LatestVersion {
        game_id: id,
        version_id: row.id,
        version_label: row.version,
        file_name: row.file_name,
        archive_hash: row.archive_hash,
        content_sha256: row.content_sha256,
        archive_size: row.size,
        updated_at: row.updated_at.as_deref().and_then(db::parse_time),
        early_access: row.early_access != 0,
        download_url: format!("/api/games/{id}/download?versionId={}", row.id),
    }))
}

/// Every version of a game with its archive state, for the editor version
/// manager (#168). Missing rows are listed so an editor can see what happened
/// to an archive instead of watching it disappear.
pub(super) async fn list_versions(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<GameVersionsResponse>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, id).await?;
    Ok(Json(fetch_versions(&state, id).await?))
}

/// Pins the version used by downloads, fresh installs and the update check.
/// Picking an older version is a rollback; newer archives stay available.
pub(super) async fn set_default_version(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<SetDefaultVersionRequest>,
) -> Result<Json<GameVersionsResponse>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, id).await?;

    let version = fetch_version(&state, id, request.version_id).await?;
    if version.missing != 0 {
        return Err(ApiError::conflict(
            "version_missing",
            "This archive is missing; restore it before making it the default version",
        ));
    }
    if version.root_unavailable != 0 {
        return Err(ApiError::conflict(
            "root_unavailable",
            "The library root holding this archive is offline",
        ));
    }

    db::query("UPDATE games SET default_version_id = ?, updated_at = ? WHERE id = ?")
        .bind(version.id)
        .bind(db::db_time(db::now()))
        .bind(id)
        .execute(&state.core.pool)
        .await
        .map_err(ApiError::from)?;

    state.core.events.emit(
        crate::events::Event::new(
            crate::audit::actions::GAME_VERSION_DEFAULT_CHANGED,
            user.actor(),
        )
        .target("game", id.to_string())
        .metadata(serde_json::json!({
            "versionId": version.id,
            "version": version.version,
            "fileName": version.file_name,
        })),
    );

    Ok(Json(fetch_versions(&state, id).await?))
}

/// Deletes one archive (file + row) after an explicit confirmation (#168).
///
/// The game, its metadata, saves and audit history are kept; only the archive
/// goes away. The last available version is never deleted, nor one that is
/// missing, on an offline root, or currently being downloaded/streamed.
pub(super) async fn delete_version(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((game_id, version_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<DeleteVersionQuery>,
) -> Result<Json<DeleteVersionResponse>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, game_id).await?;
    if !query.confirm.unwrap_or(false) {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Pass confirm=true to delete this archive; the game itself is kept",
        ));
    }

    let version = fetch_version(&state, game_id, version_id).await?;
    let remaining = db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM game_versions
         WHERE game_id = ? AND id != ? AND missing = 0 AND root_unavailable = 0",
    )
    .bind(game_id)
    .bind(version_id)
    .fetch_one(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    ensure_version_deletable(&state, &version, remaining)?;

    // Remove the archive bytes first: a row without a file would be re-marked
    // missing by the next scan, while a file without a row is re-indexed.
    if is_s3_version(&version) {
        if let Some(key) = version.object_key.as_deref() {
            state
                .transfer
                .storage
                .delete(key)
                .await
                .map_err(|error| map_storage_error(error, "delete"))?;
        }
    }
    if version.root_unavailable == 0 {
        if let Some(path) = roots::resolve_version_path(
            &state.core.pool,
            &state.core.config,
            version.root_id,
            &version.file_path,
        )
        .await
        {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::error!(%error, version = version.id, "could not delete the archive");
                    return Err(ApiError::internal());
                }
            }
        }
    }

    let default_changed = game_default_version_id(&state, game_id).await? == Some(version.id);
    let replacement = if default_changed {
        newest_other_version(&state, game_id, version.id).await?
    } else {
        None
    };

    let mut transaction = state.core.pool.begin().await.map_err(ApiError::from)?;
    db::query("DELETE FROM game_versions WHERE id = ? AND game_id = ?")
        .bind(version.id)
        .bind(game_id)
        .execute(&mut transaction)
        .await
        .map_err(ApiError::from)?;
    if default_changed {
        db::query("UPDATE games SET default_version_id = ?, updated_at = ? WHERE id = ?")
            .bind(replacement)
            .bind(db::db_time(db::now()))
            .bind(game_id)
            .execute(&mut transaction)
            .await
            .map_err(ApiError::from)?;
    }
    transaction.commit().await.map_err(ApiError::from)?;

    // The cache entry is keyed by archive content, so it is only dropped when
    // no other version references the same digest (#231).
    if let Some(sha256) = version.content_sha256.as_deref() {
        crate::routes::versions::forget_version_cache(
            &state,
            sha256,
            version.archive_hash.as_deref(),
        )
        .await;
    }

    state.core.events.emit(
        crate::events::Event::new(crate::audit::actions::GAME_VERSION_DELETED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "versionId": version.id,
                "version": version.version,
                "fileName": version.file_name,
                "size": version.size,
                "storageBackend": version.storage_backend,
            })),
    );

    Ok(Json(DeleteVersionResponse {
        version_id: version.id,
        freed_bytes: version.size,
        default_version_id: game_default_version_id(&state, game_id).await?,
    }))
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeleteVersionQuery {
    #[serde(default)]
    confirm: Option<bool>,
}

/// Guards shared by the delete route and its unit tests.
fn ensure_version_deletable(
    state: &AppState,
    version: &VersionRow,
    remaining: i64,
) -> Result<(), ApiError> {
    if version.missing != 0 {
        return Err(ApiError::conflict(
            "version_missing",
            "This archive is already missing; the row is kept for history",
        ));
    }
    if version.root_unavailable != 0 {
        return Err(ApiError::conflict(
            "root_unavailable",
            "The library root holding this archive is offline; restore it before deleting",
        ));
    }
    if state.transfer.transfers.is_active(version.id) {
        return Err(ApiError::conflict(
            "version_in_use",
            "A download or stream is using this archive; stop it first",
        ));
    }
    if remaining <= 0 {
        return Err(ApiError::conflict(
            "last_version",
            "This is the last available version; Ludex never deletes it",
        ));
    }
    Ok(())
}

/// Which half of a game `DELETE /api/games/{id}` removes (#209, #210).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteMode {
    /// Delete the database rows only; archives stay on disk and a scan can
    /// re-index them. Editor operation, audited.
    Library,
    /// Delete the archive files below the resolved library root, then the
    /// database rows. Administrator operation, audited.
    Archives,
}

impl DeleteMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Archives => "archives",
        }
    }

    pub(crate) fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw.map(str::trim) {
            Some("library") => Ok(Self::Library),
            Some("archives") => Ok(Self::Archives),
            Some(other) => Err(ApiError::bad_request(
                "invalid_mode",
                format!("Unknown delete mode: {other} (library or archives)"),
            )),
            None => Err(ApiError::bad_request(
                "mode_required",
                "Pass mode=library (keep the files) or mode=archives (delete them)",
            )),
        }
    }
}

/// Transport hints for a version, derived from its file name only (#226).
///
/// `stream_supported` mirrors the server-side stream gate: the per-entry
/// manifest exists for ZIP only. A 7z/RAR/unknown archive must be downloaded
/// whole and extracted; the payload still carries the format so clients can
/// say so before the first byte.
pub(super) fn version_transport(file_name: &str) -> (Option<String>, bool) {
    let format = format_from_extension(Path::new(file_name));
    (
        format.extension().map(str::to_string),
        format == ArchiveFormat::Zip,
    )
}

/// All versions (including missing rows), newest first, with the pinned id.
async fn fetch_versions(state: &AppState, game_id: i64) -> Result<GameVersionsResponse, ApiError> {
    let rows = db::query_as::<VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, content_sha256,
                indexed_at, updated_at, storage_backend, object_key, etag
         FROM game_versions WHERE game_id = ?
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC",
    )
    .bind(game_id)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(GameVersionsResponse {
        game_id,
        default_version_id: game_default_version_id(state, game_id).await?,
        versions: rows
            .into_iter()
            .map(|row| {
                let (archive_format, stream_supported) = version_transport(&row.file_name);
                GameVersionInfo {
                    id: row.id,
                    file_name: row.file_name,
                    size: row.size,
                    version: row.version,
                    release_date: row.release_date.as_deref().and_then(db::parse_time),
                    early_access: row.early_access != 0,
                    archive_hash: row.archive_hash,
                    content_sha256: row.content_sha256,
                    archive_format,
                    stream_supported,
                    indexed_at: row.indexed_at.as_deref().and_then(db::parse_time),
                    updated_at: row.updated_at.as_deref().and_then(db::parse_time),
                    missing: row.missing != 0,
                    root_unavailable: row.root_unavailable != 0,
                    download_url: format!("/api/games/{game_id}/download?versionId={}", row.id),
                }
            })
            .collect(),
    })
}

/// Pinned version id from `games.default_version_id`, if any.
pub(super) async fn game_default_version_id(
    state: &AppState,
    game_id: i64,
) -> Result<Option<i64>, ApiError> {
    Ok(
        db::query_scalar::<Option<i64>>("SELECT default_version_id FROM games WHERE id = ?")
            .bind(game_id)
            .fetch_optional(&state.core.pool)
            .await
            .map_err(ApiError::from)?
            .flatten(),
    )
}

/// Newest available version other than `exclude`, used as the fallback pin
/// after an editor deletes the pinned archive.
/// Newest available version of a game.
pub(super) async fn newest_version(state: &AppState, game_id: i64) -> Result<i64, ApiError> {
    db::query_scalar::<i64>(
        "SELECT id FROM game_versions WHERE game_id = ? AND missing = 0
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC LIMIT 1",
    )
    .bind(game_id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("version_not_found", "No downloadable version"))
}

/// Version used when the caller does not name one: the pinned version (#168)
/// while it still exists, otherwise the newest available one.
pub(super) async fn preferred_version_id(state: &AppState, game_id: i64) -> Result<i64, ApiError> {
    if let Some(pinned) = game_default_version_id(state, game_id).await? {
        let exists = db::query_scalar::<i64>(
            "SELECT id FROM game_versions WHERE id = ? AND game_id = ? AND missing = 0",
        )
        .bind(pinned)
        .bind(game_id)
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
        if let Some(id) = exists {
            return Ok(id);
        }
    }
    newest_version(state, game_id).await
}

async fn newest_other_version(
    state: &AppState,
    game_id: i64,
    exclude: i64,
) -> Result<Option<i64>, ApiError> {
    db::query_scalar::<i64>(
        "SELECT id FROM game_versions WHERE game_id = ? AND id != ? AND missing = 0
         ORDER BY COALESCE(release_date, indexed_at) DESC, id DESC LIMIT 1",
    )
    .bind(game_id)
    .bind(exclude)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)
}

pub(super) async fn fetch_version(
    state: &AppState,
    game_id: i64,
    version_id: i64,
) -> Result<VersionRow, ApiError> {
    db::query_as::<VersionRow>(
        "SELECT id, root_id, file_path, file_name, size, version, release_date,
                early_access, missing, root_unavailable, archive_hash, content_sha256,
                indexed_at, updated_at, storage_backend, object_key, etag
         FROM game_versions WHERE id = ? AND game_id = ?",
    )
    .bind(version_id)
    .bind(game_id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("version_not_found", "Game version not found"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::test_support::{state_with_memory_store, version_row};

    #[tokio::test]
    async fn version_delete_guards_cover_missing_offline_active_and_last() {
        let (_temp, state) = state_with_memory_store().await;
        let mut version = version_row(None, "local");
        assert!(ensure_version_deletable(&state, &version, 1).is_ok());

        version.missing = 1;
        assert_eq!(
            ensure_version_deletable(&state, &version, 1)
                .expect_err("missing")
                .code,
            "version_missing"
        );
        version.missing = 0;

        version.root_unavailable = 1;
        assert_eq!(
            ensure_version_deletable(&state, &version, 1)
                .expect_err("offline root")
                .code,
            "root_unavailable"
        );
        version.root_unavailable = 0;

        let lease = state.transfer.transfers.begin(version.id);
        assert!(state.transfer.transfers.is_active(version.id));
        assert_eq!(
            ensure_version_deletable(&state, &version, 1)
                .expect_err("in use")
                .code,
            "version_in_use"
        );
        drop(lease);
        assert!(!state.transfer.transfers.is_active(version.id));

        assert_eq!(
            ensure_version_deletable(&state, &version, 0)
                .expect_err("last version")
                .code,
            "last_version"
        );
    }

    #[test]
    fn version_payload_reports_archive_format_and_stream_support() {
        let build = |file_name: &str, id: i64| {
            let (archive_format, stream_supported) = version_transport(file_name);
            GameVersionInfo {
                id,
                file_name: file_name.to_string(),
                size: 16,
                version: None,
                release_date: None,
                early_access: false,
                archive_hash: None,
                content_sha256: None,
                archive_format,
                stream_supported,
                indexed_at: None,
                updated_at: None,
                missing: false,
                root_unavailable: false,
                download_url: format!("/api/games/1/download?versionId={id}"),
            }
        };

        let zip = serde_json::to_value(build("Cat Quest II.zip", 1)).expect("json");
        assert_eq!(zip["archiveFormat"], "zip");
        assert_eq!(zip["streamSupported"].as_bool(), Some(true));
        assert_eq!(zip["fileName"], "Cat Quest II.zip");
        assert_eq!(zip["size"], 16);

        let seven = serde_json::to_value(build("HITMAN 3 (v3.10.1).7z", 2)).expect("json");
        assert_eq!(seven["archiveFormat"], "7z");
        assert_eq!(seven["streamSupported"].as_bool(), Some(false));

        // Unknown extension or legacy row: no format claim, stream off.
        for name in ["repack", "portable.exe", "archive.tar.gz"] {
            let legacy = serde_json::to_value(build(name, 3)).expect("json");
            assert!(legacy.get("archiveFormat").is_none(), "{name}: {legacy}");
            assert_eq!(legacy["streamSupported"].as_bool(), Some(false), "{name}");
        }
    }
}
