//! Per-user cloud save snapshots.
//!
//! A snapshot is an opaque archive uploaded by one user for one game. Users
//! only ever see their own snapshots; administrators may download and delete
//! any snapshot (support/cleanup), but listing always stays per-user.

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use tokio_util::io::ReaderStream;
use utoipa::openapi::HttpMethod;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, MultipartBody};
use crate::media::content_disposition;
use crate::models::{
    PruneSavesRequest, RestoreSnapshotRequest, RestoreSnapshotResponse, SaveHistory,
    SavePruneReport, SaveRestoreReceipt, SaveRestoreReceiptRow, SaveSnapshot, SaveSnapshotRow,
};
use crate::openapi::{body, ok, route, Auth, Payload, RouteDoc};
use crate::saves::{self, PendingSave, SaveError};
use crate::state::AppState;

/// Longest accepted snapshot label, in characters.
const MAX_LABEL_CHARS: usize = 200;
/// Auto snapshots kept per user and game; manual snapshots are never pruned.
const AUTO_SNAPSHOT_RETENTION: i64 = 10;
/// Longest accepted device identifier, in characters.
const MAX_DEVICE_ID_CHARS: usize = 100;
/// Bounds for an explicit retention request (#295).
const MIN_PRUNE_KEEP: i64 = 1;
const MAX_PRUNE_KEEP: i64 = 50;
/// History page sizes (#295); snapshots per game stay small in practice.
const HISTORY_SNAPSHOT_LIMIT: i64 = 100;
const HISTORY_RECEIPT_LIMIT: i64 = 50;

/// Snapshot columns shared by every read path; `pinned` is the restore-point
/// flag added in migration 0036.
const SNAPSHOT_COLUMNS: &str = "id, user_id, game_id, version_id, label, file_name, size, sha256,
     created_at, source, device_id, pinned";

/// Save routes, relative to `/api`.
///
/// The upload method gets its own body limit: the global limit is sized for
/// JSON (10 MiB) while save archives may be much larger.
pub fn router(max_upload_bytes: u64) -> Router<AppState> {
    let upload_limit = DefaultBodyLimit::max(saves::body_limit(max_upload_bytes));
    Router::new()
        .route(
            "/games/{id}/saves",
            get(list).post(upload).layer(upload_limit),
        )
        .route("/games/{id}/saves/history", get(history))
        .route("/games/{id}/saves/prune", post(prune))
        .route("/saves/{id}/download", get(download))
        .route("/saves/{id}/restore", post(restore))
        .route("/saves/{id}", delete(delete_snapshot))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<Vec<SaveSnapshot>>, ApiError> {
    ensure_game(&state, game_id).await?;
    let rows = db::query_as::<SaveSnapshotRow>(&format!(
        "SELECT {SNAPSHOT_COLUMNS}
         FROM save_snapshots
         WHERE user_id = ? AND game_id = ?
         ORDER BY created_at DESC, id DESC"
    ))
    .bind(user.id)
    .bind(game_id)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(
        rows.into_iter().map(SaveSnapshotRow::to_public).collect(),
    ))
}

async fn upload(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    MultipartBody(mut multipart): MultipartBody,
) -> Result<(StatusCode, Json<SaveSnapshot>), ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;

    let mut pending: Option<PendingSave> = None;
    let mut label = String::new();
    let mut source = "manual".to_string();
    let mut device_id: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(map_multipart_error)? {
        match field.name().unwrap_or_default() {
            "file" => {
                if pending.is_some() {
                    return Err(ApiError::bad_request(
                        "invalid_file",
                        "Only one file field is allowed",
                    ));
                }
                let raw_name = field.file_name().unwrap_or_default().to_string();
                if raw_name.trim().is_empty() {
                    return Err(ApiError::bad_request(
                        "invalid_file",
                        "The file field must include a file name",
                    ));
                }
                let file_name = saves::sanitize_file_name(&raw_name);
                let dir = saves::game_dir(&state.core.config.data_dir, user.id, game_id);
                pending = Some(
                    saves::receive(&dir, file_name, field, state.core.config.saves_max_bytes)
                        .await
                        .map_err(|error| {
                            map_save_error(error, state.core.config.saves_max_bytes)
                        })?,
                );
            }
            "label" => {
                let value = field.text().await.map_err(map_multipart_error)?;
                let value = value.trim().to_string();
                if value.chars().count() > MAX_LABEL_CHARS {
                    return Err(ApiError::bad_request(
                        "invalid_label",
                        format!("Label must be at most {MAX_LABEL_CHARS} characters"),
                    ));
                }
                label = value;
            }
            "source" => {
                let value = field.text().await.map_err(map_multipart_error)?;
                let value = value.trim().to_string();
                if value != "manual" && value != "auto" {
                    return Err(ApiError::bad_request(
                        "invalid_source",
                        "source must be 'manual' or 'auto'",
                    ));
                }
                source = value;
            }
            "deviceId" => {
                let value = field.text().await.map_err(map_multipart_error)?;
                let value = value.trim().to_string();
                if value.chars().count() > MAX_DEVICE_ID_CHARS
                    || value.chars().any(char::is_control)
                {
                    return Err(ApiError::bad_request(
                        "invalid_device_id",
                        "The device identifier is invalid",
                    ));
                }
                device_id = (!value.is_empty()).then_some(value);
            }
            _ => {
                // Unknown fields are drained and ignored for forward compatibility.
                let mut field = field;
                while field.chunk().await.map_err(map_multipart_error)?.is_some() {}
            }
        }
    }

    let pending = pending.ok_or_else(|| {
        ApiError::bad_request("invalid_file", "Multipart field 'file' is required")
    })?;

    let file_name = pending.file_name().to_string();
    let size = i64::try_from(pending.size()).unwrap_or(i64::MAX);
    let sha256 = pending.sha256().to_string();
    let now = db::db_time(db::now());

    // Automatic uploads are idempotent by content hash: an unchanged save
    // returns the snapshot that already holds those bytes (#108).
    if source == "auto" {
        let existing = db::query_as::<SaveSnapshotRow>(&format!(
            "SELECT {SNAPSHOT_COLUMNS}
             FROM save_snapshots
             WHERE user_id = ? AND game_id = ? AND sha256 = ?
             ORDER BY created_at DESC, id DESC LIMIT 1"
        ))
        .bind(user.id)
        .bind(game_id)
        .bind(&sha256)
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
        if let Some(existing) = existing {
            pending.discard().await;
            return Ok((StatusCode::OK, Json(existing.to_public())));
        }
    }

    let inserted = db::query(
        "INSERT INTO save_snapshots
            (user_id, game_id, version_id, label, file_name, size, sha256, created_at,
             source, device_id)
         VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(&label)
    .bind(&file_name)
    .bind(size)
    .bind(&sha256)
    .bind(&now)
    .bind(&source)
    .bind(device_id.as_deref())
    .returning_id(&state.core.pool)
    .await;

    let snapshot_id = match inserted {
        Ok(id) => id,
        Err(error) => {
            pending.discard().await;
            return Err(ApiError::from(error));
        }
    };

    let dir = saves::game_dir(&state.core.config.data_dir, user.id, game_id);
    if let Err(error) = pending.commit(&dir, snapshot_id).await {
        tracing::error!(error = %error, "could not publish save archive");
        let _ = db::query("DELETE FROM save_snapshots WHERE id = ?")
            .bind(snapshot_id)
            .execute(&state.core.pool)
            .await;
        return Err(ApiError::internal());
    }

    state.core.events.emit(
        Event::new(actions::SAVE_UPLOADED, user.actor())
            .target("save", snapshot_id.to_string())
            .metadata(serde_json::json!({
                "snapshotId": snapshot_id,
                "gameId": game_id,
                "userId": user.id,
                "size": size,
                "sha256": sha256,
                "label": label,
                "source": source,
                "deviceId": device_id,
            })),
    );

    if source == "auto" {
        prune_auto_snapshots(&state, user.id, game_id, AUTO_SNAPSHOT_RETENTION).await;
    }

    Ok((
        StatusCode::CREATED,
        Json(SaveSnapshot {
            id: snapshot_id,
            game_id,
            version_id: None,
            label,
            file_name,
            size,
            sha256,
            created_at: db::parse_time(&now).unwrap_or_else(chrono::Utc::now),
            source,
            device_id,
            pinned: false,
        }),
    ))
}

async fn download(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Response, ApiError> {
    let row = fetch_snapshot(&state, id).await?;
    ensure_access(&user, &row)?;

    let path = saves::snapshot_path(
        &state.core.config.data_dir,
        row.user_id,
        row.game_id,
        row.id,
        &row.file_name,
    )?;
    let file = tokio::fs::File::open(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found("file_not_found", "The save archive is missing from disk")
        } else {
            ApiError::from(error)
        }
    })?;
    let metadata = file.metadata().await.map_err(ApiError::from)?;
    let content_type = mime_guess::from_path(&row.file_name)
        .first_or_octet_stream()
        .to_string();
    let disposition = HeaderValue::from_str(&content_disposition(&row.file_name))
        .map_err(|_| ApiError::internal())?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|_| ApiError::internal())
}

async fn delete_snapshot(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let row = fetch_snapshot(&state, id).await?;
    ensure_access(&user, &row)?;

    let deleted = db::query("DELETE FROM save_snapshots WHERE id = ?")
        .bind(id)
        .execute(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "save_not_found",
            "Save snapshot not found",
        ));
    }

    saves::delete_file(
        &state.core.config.data_dir,
        row.user_id,
        row.game_id,
        row.id,
        &row.file_name,
    )
    .await;
    state.core.events.emit(
        Event::new(actions::SAVE_DELETED, user.actor())
            .target("save", row.id.to_string())
            .metadata(serde_json::json!({
                "snapshotId": row.id,
                "gameId": row.game_id,
                "ownerId": row.user_id,
                "fileName": row.file_name,
            })),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Retention pass shared by the post-upload cleanup and the explicit prune
/// endpoint (#295). Returns the report described by [`SavePruneReport`].
///
/// Rules, in order:
/// - pinned snapshots are restore points and are never removed;
/// - the newest snapshot of the game is the last known-good copy and stays
///   even when it is neither pinned nor inside the keep window;
/// - only automatic snapshots are candidates, so manual uploads are safe;
/// - row deletions happen before the file deletions so a crash leaves an
///   orphaned file (removed by the media GC rules) instead of a row pointing
///   at a missing archive.
async fn prune_snapshots(
    state: &AppState,
    user_id: i64,
    game_id: i64,
    keep: i64,
) -> SavePruneReport {
    let keep = keep.clamp(MIN_PRUNE_KEEP, MAX_PRUNE_KEEP);
    let rows = db::query_as::<(i64, String, String, i64)>(
        "SELECT id, file_name, source, pinned FROM save_snapshots
         WHERE user_id = ? AND game_id = ?
         ORDER BY created_at DESC, id DESC",
    )
    .bind(user_id)
    .bind(game_id)
    .fetch_all(&state.core.pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "could not list save snapshots for pruning");
            return SavePruneReport {
                removed: 0,
                kept: 0,
                protected: 0,
            };
        }
    };

    let total = i64::try_from(rows.len()).unwrap_or(i64::MAX);
    let protected = i64::try_from(rows.iter().filter(|(_, _, _, pinned)| *pinned != 0).count())
        .unwrap_or(i64::MAX);

    let mut auto_seen = 0i64;
    let mut stale = Vec::new();
    for (id, file_name, source, pinned) in rows {
        if source != "auto" || pinned != 0 {
            continue;
        }
        // The newest automatic snapshot counts toward the window first, so a
        // game whose latest session is also its only copy never loses it.
        if auto_seen < keep {
            auto_seen += 1;
            continue;
        }
        stale.push((id, file_name));
    }

    let mut removed = 0i64;
    for (id, file_name) in stale {
        match db::query("DELETE FROM save_snapshots WHERE id = ?")
            .bind(id)
            .execute(&state.core.pool)
            .await
        {
            Ok(_) => removed += 1,
            Err(error) => {
                tracing::warn!(error = %error, snapshot = id, "could not prune a save snapshot row");
                continue;
            }
        }
        saves::delete_file(
            &state.core.config.data_dir,
            user_id,
            game_id,
            id,
            &file_name,
        )
        .await;
    }

    SavePruneReport {
        removed,
        kept: total - removed,
        protected,
    }
}

/// Promotes a cloud snapshot to a restore point and records the restore in the
/// game history (#295). The device that writes the local files reports itself
/// here, so the panel can answer "which machine wrote this save last".
async fn restore(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    Json(request): Json<RestoreSnapshotRequest>,
) -> Result<(StatusCode, Json<RestoreSnapshotResponse>), ApiError> {
    user.require_scope(Scope::Write)?;
    let row = fetch_snapshot(&state, id).await?;
    ensure_access(&user, &row)?;

    let device_id = request
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(device_id) = device_id.as_deref() {
        if device_id.chars().count() > MAX_DEVICE_ID_CHARS
            || device_id.chars().any(char::is_control)
        {
            return Err(ApiError::bad_request(
                "invalid_device_id",
                "The device identifier is invalid",
            ));
        }
    }
    let kind = request.kind.as_deref().unwrap_or("server").trim();
    if kind != "server" && kind != "local" {
        return Err(ApiError::bad_request(
            "invalid_restore_kind",
            "kind must be 'server' or 'local'",
        ));
    }

    db::query("UPDATE save_snapshots SET pinned = 1 WHERE id = ?")
        .bind(id)
        .execute(&state.core.pool)
        .await
        .map_err(ApiError::from)?;

    let now = db::db_time(db::now());
    let receipt_id = db::query(
        "INSERT INTO save_restore_receipts
            (user_id, game_id, snapshot_id, device_id, kind, restored_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(row.game_id)
    .bind(id)
    .bind(device_id.as_deref())
    .bind(kind)
    .bind(&now)
    .returning_id(&state.core.pool)
    .await
    .map_err(ApiError::from)?;

    state.core.events.emit(
        Event::new(actions::SAVE_RESTORED, user.actor())
            .target("save", id.to_string())
            .metadata(serde_json::json!({
                "snapshotId": id,
                "gameId": row.game_id,
                "userId": user.id,
                "deviceId": device_id,
                "kind": kind,
            })),
    );

    Ok((
        StatusCode::CREATED,
        Json(RestoreSnapshotResponse {
            snapshot: fetch_snapshot(&state, id).await?.to_public(),
            receipt: SaveRestoreReceipt {
                id: receipt_id,
                game_id: row.game_id,
                snapshot_id: id,
                device_id,
                kind: kind.to_string(),
                restored_at: db::parse_time(&now).unwrap_or_else(chrono::Utc::now),
            },
        }),
    ))
}

/// Explicit retention pass (#295). `keep` defaults to the automatic retention;
/// pinned restore points and the newest snapshot are always kept.
async fn prune(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    Json(request): Json<PruneSavesRequest>,
) -> Result<Json<SavePruneReport>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let keep = request.keep.unwrap_or(AUTO_SNAPSHOT_RETENTION);
    let report = prune_snapshots(&state, user.id, game_id, keep).await;
    state.core.events.emit(
        Event::new(actions::SAVE_PRUNED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "gameId": game_id,
                "userId": user.id,
                "removed": report.removed,
                "kept": report.kept,
                "protected": report.protected,
            })),
    );
    Ok(Json(report))
}

/// Snapshot history and restore receipts for the game page (#295).
async fn history(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<SaveHistory>, ApiError> {
    ensure_game(&state, game_id).await?;
    let snapshots = db::query_as::<SaveSnapshotRow>(&format!(
        "SELECT {SNAPSHOT_COLUMNS}
         FROM save_snapshots
         WHERE user_id = ? AND game_id = ?
         ORDER BY created_at DESC, id DESC
         LIMIT ?"
    ))
    .bind(user.id)
    .bind(game_id)
    .bind(HISTORY_SNAPSHOT_LIMIT)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    let receipts = db::query_as::<SaveRestoreReceiptRow>(
        "SELECT id, game_id, snapshot_id, device_id, kind, restored_at
         FROM save_restore_receipts
         WHERE user_id = ? AND game_id = ?
         ORDER BY restored_at DESC, id DESC
         LIMIT ?",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(HISTORY_RECEIPT_LIMIT)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(SaveHistory {
        snapshots: snapshots
            .into_iter()
            .map(SaveSnapshotRow::to_public)
            .collect(),
        receipts: receipts
            .into_iter()
            .map(SaveRestoreReceiptRow::to_public)
            .collect(),
        auto_retention: AUTO_SNAPSHOT_RETENTION,
    }))
}

/// Keeps only the newest `keep` automatic snapshots for one user and game.
///
/// Same rules as [`prune_snapshots`]: pinned restore points, manual uploads and
/// the newest snapshot are never touched.
async fn prune_auto_snapshots(state: &AppState, user_id: i64, game_id: i64, keep: i64) {
    let _ = prune_snapshots(state, user_id, game_id, keep).await;
}

async fn ensure_game(state: &AppState, game_id: i64) -> Result<(), ApiError> {
    let exists = db::query_scalar::<i64>("SELECT id FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}

async fn fetch_snapshot(state: &AppState, id: i64) -> Result<SaveSnapshotRow, ApiError> {
    db::query_as::<SaveSnapshotRow>(&format!(
        "SELECT {SNAPSHOT_COLUMNS} FROM save_snapshots WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("save_not_found", "Save snapshot not found"))
}

fn ensure_access(user: &AuthUser, row: &SaveSnapshotRow) -> Result<(), ApiError> {
    if row.user_id == user.id || user.is_admin() {
        Ok(())
    } else {
        // 404 rather than 403 so another user's snapshot never leaks existence.
        Err(ApiError::not_found(
            "save_not_found",
            "Save snapshot not found",
        ))
    }
}

fn map_multipart_error(error: axum::extract::multipart::MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "save_too_large",
            "The save archive exceeds the server limit",
        )
    } else {
        ApiError::bad_request("invalid_multipart", error.body_text())
    }
}

fn map_save_error(error: SaveError, max_bytes: u64) -> ApiError {
    match error {
        SaveError::TooLarge => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "save_too_large",
            format!("Save archives are limited to {max_bytes} bytes"),
        ),
        SaveError::Empty => {
            ApiError::bad_request("invalid_file", "The uploaded save archive is empty")
        }
        SaveError::Stream(message) => ApiError::bad_request("invalid_multipart", message),
        SaveError::Io(error) => {
            tracing::error!(error = %error, "could not store save archive");
            ApiError::internal()
        }
    }
}

/// OpenAPI operations served by this module (#237).
pub(crate) const DOCS: &[RouteDoc] = &[
    // Cloud saves
    route(
        HttpMethod::Get,
        "/games/{id}/saves",
        "Cloud saves",
        "List your save snapshots",
        Auth::Session,
        Payload::Array("SaveSnapshot"),
    ),
    route(
        HttpMethod::Post,
        "/games/{id}/saves",
        "Cloud saves",
        "Upload a save archive (multipart)",
        Auth::Session,
        Payload::Schema("SaveSnapshot"),
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/saves/history",
        "Cloud saves",
        "List snapshot history and restore receipts",
        Auth::Session,
        Payload::Schema("SaveHistory"),
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/games/{id}/saves/prune",
                "Cloud saves",
                "Prune old automatic snapshots (pinned and newest are kept)",
                Auth::Session,
                Payload::Schema("SavePruneReport"),
            ),
            200,
        ),
        "PruneSavesRequest",
    ),
    body(
        route(
            HttpMethod::Post,
            "/saves/{id}/restore",
            "Cloud saves",
            "Promote a snapshot to a restore point and record the restore",
            Auth::Session,
            Payload::Schema("RestoreSnapshotResponse"),
        ),
        "RestoreSnapshotRequest",
    ),
    route(
        HttpMethod::Get,
        "/saves/{id}/download",
        "Cloud saves",
        "Download a save archive",
        Auth::Session,
        Payload::Binary,
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/saves/{id}",
            "Cloud saves",
            "Delete a save snapshot",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
];
