//! Per-user cloud save snapshots.
//!
//! A snapshot is an opaque archive uploaded by one user for one game. Users
//! only ever see their own snapshots; administrators may download and delete
//! any snapshot (support/cleanup), but listing always stays per-user.

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get};
use axum::{Json, Router};
use tokio_util::io::ReaderStream;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, MultipartBody};
use crate::media::content_disposition;
use crate::models::{SaveSnapshot, SaveSnapshotRow};
use crate::saves::{self, PendingSave, SaveError};
use crate::state::AppState;

/// Longest accepted snapshot label, in characters.
const MAX_LABEL_CHARS: usize = 200;
/// Auto snapshots kept per user and game; manual snapshots are never pruned.
const AUTO_SNAPSHOT_RETENTION: i64 = 10;
/// Longest accepted device identifier, in characters.
const MAX_DEVICE_ID_CHARS: usize = 100;

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
        .route("/saves/{id}/download", get(download))
        .route("/saves/{id}", delete(delete_snapshot))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<Vec<SaveSnapshot>>, ApiError> {
    ensure_game(&state, game_id).await?;
    let rows = db::query_as::<SaveSnapshotRow>(
        "SELECT id, user_id, game_id, version_id, label, file_name, size, sha256, created_at,
                source, device_id
         FROM save_snapshots
         WHERE user_id = ? AND game_id = ?
         ORDER BY created_at DESC, id DESC",
    )
    .bind(user.id)
    .bind(game_id)
    .fetch_all(&state.pool)
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
                let dir = saves::game_dir(&state.config.data_dir, user.id, game_id);
                pending = Some(
                    saves::receive(&dir, file_name, field, state.config.saves_max_bytes)
                        .await
                        .map_err(|error| map_save_error(error, state.config.saves_max_bytes))?,
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
        let existing = db::query_as::<SaveSnapshotRow>(
            "SELECT id, user_id, game_id, version_id, label, file_name, size, sha256, created_at,
                    source, device_id
             FROM save_snapshots
             WHERE user_id = ? AND game_id = ? AND sha256 = ?
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(user.id)
        .bind(game_id)
        .bind(&sha256)
        .fetch_optional(&state.pool)
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
    .returning_id(&state.pool)
    .await;

    let snapshot_id = match inserted {
        Ok(id) => id,
        Err(error) => {
            pending.discard().await;
            return Err(ApiError::from(error));
        }
    };

    let dir = saves::game_dir(&state.config.data_dir, user.id, game_id);
    if let Err(error) = pending.commit(&dir, snapshot_id).await {
        tracing::error!(error = %error, "could not publish save archive");
        let _ = db::query("DELETE FROM save_snapshots WHERE id = ?")
            .bind(snapshot_id)
            .execute(&state.pool)
            .await;
        return Err(ApiError::internal());
    }

    state.events.emit(
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
        &state.config.data_dir,
        row.user_id,
        row.game_id,
        row.id,
        &row.file_name,
    );
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
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "save_not_found",
            "Save snapshot not found",
        ));
    }

    saves::delete_file(
        &state.config.data_dir,
        row.user_id,
        row.game_id,
        row.id,
        &row.file_name,
    )
    .await;
    state.events.emit(
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

/// Keeps only the newest `keep` automatic snapshots for one user and game.
///
/// Manual snapshots are never touched. Row deletions happen before the file
/// deletions so a crash leaves an orphaned file (removed by the media GC
/// rules) instead of a row pointing at a missing archive.
async fn prune_auto_snapshots(state: &AppState, user_id: i64, game_id: i64, keep: i64) {
    let stale = db::query_as::<(i64, String)>(
        "SELECT id, file_name FROM save_snapshots
         WHERE user_id = ? AND game_id = ? AND source = 'auto'
           AND id NOT IN (
               SELECT id FROM save_snapshots
               WHERE user_id = ? AND game_id = ? AND source = 'auto'
               ORDER BY created_at DESC, id DESC LIMIT ?
           )",
    )
    .bind(user_id)
    .bind(game_id)
    .bind(user_id)
    .bind(game_id)
    .bind(keep)
    .fetch_all(&state.pool)
    .await;

    let stale = match stale {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "could not list stale save snapshots");
            return;
        }
    };
    for (id, file_name) in stale {
        if let Err(error) = db::query("DELETE FROM save_snapshots WHERE id = ?")
            .bind(id)
            .execute(&state.pool)
            .await
        {
            tracing::warn!(error = %error, snapshot = id, "could not prune a save snapshot row");
            continue;
        }
        saves::delete_file(&state.config.data_dir, user_id, game_id, id, &file_name).await;
    }
}

async fn ensure_game(state: &AppState, game_id: i64) -> Result<(), ApiError> {
    let exists = db::query_scalar::<i64>("SELECT id FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}

async fn fetch_snapshot(state: &AppState, id: i64) -> Result<SaveSnapshotRow, ApiError> {
    db::query_as::<SaveSnapshotRow>(
        "SELECT id, user_id, game_id, version_id, label, file_name, size, sha256, created_at,
                source, device_id
         FROM save_snapshots WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
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
