//! Resumable archive uploads (#21).
//!
//! `POST /api/uploads` creates a session, `PUT .../parts/{n}` stores idempotent
//! chunks, `POST .../complete` verifies and indexes the archive, `DELETE`
//! abandons it and `GET` reports progress for resume-after-reload. Only
//! administrators and editors may upload library archives.

use std::path::Path;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use ludex_core::archive::is_archive_path;
use serde_json::json;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{
    part_len, total_parts, CompleteUploadResponse, CreateUploadRequest, UploadPartResponse,
    UploadRow, UploadSession,
};
use crate::state::AppState;
use crate::uploads;

/// Largest accepted chunk body (the configurable chunk size is capped at
/// 64 MiB, and axum needs a little headroom for the multipart framing of other
/// routes).
const MAX_CHUNK_BODY: usize = 65 * 1024 * 1024;

/// Upload routes, relative to `/api`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/uploads", post(create))
        .route("/uploads/{id}", get(progress).delete(cancel))
        .route(
            "/uploads/{id}/parts/{part}",
            put(receive_part).layer(DefaultBodyLimit::max(MAX_CHUNK_BODY)),
        )
        .route("/uploads/{id}/complete", post(complete))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateUploadRequest>,
) -> Result<(StatusCode, Json<UploadSession>), ApiError> {
    user.require_editor()?;
    // Best-effort housekeeping; never let a cleanup hiccup block a new upload.
    let _ = uploads::cleanup_stale(&state, state.config.upload_ttl).await;

    let file_name = uploads::sanitize_file_name(&request.file_name);
    if !is_archive_path(Path::new(&file_name)) {
        return Err(ApiError::bad_request(
            "invalid_file_name",
            "Uploads must be .zip, .rar or .7z archives",
        ));
    }
    if request.size <= 0 {
        return Err(ApiError::bad_request(
            "invalid_size",
            "size must be greater than zero",
        ));
    }
    let max = i64::try_from(state.config.upload_max_bytes).unwrap_or(i64::MAX);
    if request.size > max {
        return Err(ApiError::bad_request(
            "upload_too_large",
            format!("Uploads are limited to {max} bytes"),
        ));
    }
    let sha256 = match request.sha256.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(value) => {
            let value = value.to_ascii_lowercase();
            if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ApiError::bad_request(
                    "invalid_sha256",
                    "sha256 must be 64 hexadecimal characters",
                ));
            }
            Some(value)
        }
    };

    let chunk_size = i64::try_from(state.config.upload_chunk_bytes).unwrap_or(16 * 1024 * 1024);
    let id = uuid::Uuid::new_v4().to_string();
    tokio::fs::create_dir_all(uploads::upload_dir(&state.config.data_dir, &id))
        .await
        .map_err(ApiError::from)?;
    let now = db::db_time(db::now());
    db::query(
        "INSERT INTO uploads
            (id, user_id, file_name, size, sha256, chunk_size, status, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, 'open', ?, ?)",
    )
    .bind(&id)
    .bind(user.id)
    .bind(&file_name)
    .bind(request.size)
    .bind(&sha256)
    .bind(chunk_size)
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let row = fetch_row(&state, &id).await?;
    Ok((StatusCode::CREATED, Json(row.to_public(Vec::new()))))
}

async fn progress(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<UploadSession>, ApiError> {
    let row = fetch_owned(&state, &user, &id).await?;
    let parts = uploads::received_parts(
        &uploads::upload_dir(&state.config.data_dir, &id),
        total_parts(row.size, row.chunk_size),
    );
    Ok(Json(row.to_public(parts)))
}

async fn receive_part(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((id, part)): ApiPath<(String, i64)>,
    body: Bytes,
) -> Result<Json<UploadPartResponse>, ApiError> {
    let row = fetch_owned(&state, &user, &id).await?;
    if row.status != "open" {
        return Err(ApiError::conflict(
            "upload_not_open",
            "This upload is no longer accepting chunks",
        ));
    }
    let expected_parts = total_parts(row.size, row.chunk_size);
    if !(0..expected_parts).contains(&part) {
        return Err(ApiError::bad_request(
            "invalid_part",
            format!("Part must be between 0 and {}", expected_parts - 1),
        ));
    }
    let expected = part_len(row.size, row.chunk_size, part);
    let received = i64::try_from(body.len()).unwrap_or(i64::MAX);
    if received != expected {
        return Err(ApiError::bad_request(
            "invalid_part_size",
            format!("Part {part} must be exactly {expected} bytes"),
        ));
    }

    let path = uploads::part_path(&state.config.data_dir, &id, part);
    let temporary = path.with_extension("part.tmp");
    tokio::fs::write(&temporary, &body)
        .await
        .map_err(ApiError::from)?;
    tokio::fs::rename(&temporary, &path)
        .await
        .map_err(ApiError::from)?;
    db::query("UPDATE uploads SET updated_at = ? WHERE id = ?")
        .bind(db::db_time(db::now()))
        .bind(&id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let parts = uploads::received_parts(
        &uploads::upload_dir(&state.config.data_dir, &id),
        expected_parts,
    );
    Ok(Json(UploadPartResponse {
        id,
        part,
        received_bytes: uploads::received_bytes(row.size, row.chunk_size, &parts),
        received_parts: i64::try_from(parts.len()).unwrap_or(0),
        total_parts: expected_parts,
    }))
}

async fn complete(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<CompleteUploadResponse>, ApiError> {
    let row = fetch_owned(&state, &user, &id).await?;
    if row.status == "complete" {
        return completed_response(row);
    }
    if row.status != "open" {
        return Err(ApiError::conflict(
            "upload_not_open",
            "This upload cannot be completed",
        ));
    }

    let dir = uploads::upload_dir(&state.config.data_dir, &id);
    let expected_parts = total_parts(row.size, row.chunk_size);
    let parts = uploads::received_parts(&dir, expected_parts);
    if i64::try_from(parts.len()).unwrap_or(0) != expected_parts {
        return Err(ApiError::bad_request(
            "upload_incomplete",
            format!("{} of {expected_parts} chunks received", parts.len()),
        ));
    }
    for part in 0..expected_parts {
        let path = uploads::part_path(&state.config.data_dir, &id, part);
        let size = tokio::fs::metadata(&path)
            .await
            .map_err(|_| ApiError::bad_request("upload_incomplete", "A chunk is missing"))?
            .len();
        if i64::try_from(size).unwrap_or(i64::MAX) != part_len(row.size, row.chunk_size, part) {
            return Err(ApiError::bad_request(
                "invalid_part_size",
                format!("Chunk {part} has an unexpected size; upload it again"),
            ));
        }
    }

    let destination = uploads::unique_destination(&state.config.games_dir, &row.file_name);
    let temporary = state.config.games_dir.join(format!(".upload-{id}.tmp"));
    let assembled = uploads::assemble(&dir, row.size, row.chunk_size, &temporary).await;
    let (size, hash) = match assembled {
        Ok(value) => value,
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            mark_error(&state, &id, &format!("could not assemble chunks: {error}")).await;
            return Err(ApiError::internal());
        }
    };

    if size != row.size {
        let _ = tokio::fs::remove_file(&temporary).await;
        let message = format!("Assembled {} bytes, expected {}", size, row.size);
        mark_error(&state, &id, &message).await;
        return Err(ApiError::unprocessable("size_mismatch", message));
    }
    if let Some(expected) = row.sha256.as_deref() {
        if !hash.eq_ignore_ascii_case(expected) {
            let _ = tokio::fs::remove_file(&temporary).await;
            mark_error(&state, &id, "SHA-256 does not match the declared hash").await;
            return Err(ApiError::unprocessable(
                "hash_mismatch",
                "SHA-256 does not match the declared hash",
            ));
        }
    }

    tokio::fs::rename(&temporary, &destination)
        .await
        .map_err(ApiError::from)?;
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(&row.file_name)
        .to_string();
    let indexed = crate::scanner::index_uploaded_archive(&state, &destination).await;
    let (game_id, version_id) = match indexed {
        Ok(ids) => ids,
        Err(error) => {
            mark_error(&state, &id, &format!("indexing failed: {error}")).await;
            return Err(ApiError::internal());
        }
    };

    let now = db::db_time(db::now());
    db::query(
        "UPDATE uploads SET status = 'complete', game_id = ?, version_id = ?,
            error = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(game_id)
    .bind(version_id)
    .bind(&now)
    .bind(&id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    // The chunks are no longer needed; the row stays for progress queries.
    let _ = tokio::fs::remove_dir_all(&dir).await;
    state.events.emit(
        Event::new(actions::UPLOAD_COMPLETED, user.actor())
            .target("game", game_id.to_string())
            .metadata(json!({
                "uploadId": id,
                "versionId": version_id,
                "fileName": file_name,
                "size": size,
                "sha256": hash,
            })),
    );

    Ok(Json(CompleteUploadResponse {
        id,
        game_id,
        version_id,
        file_name,
        size,
        sha256: row.sha256,
    }))
}

async fn cancel(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<String>,
) -> Result<StatusCode, ApiError> {
    let row = fetch_owned(&state, &user, &id).await?;
    if row.status == "complete" {
        // Keep the indexed game; only drop the progress row.
        db::query("DELETE FROM uploads WHERE id = ?")
            .bind(&id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
        return Ok(StatusCode::NO_CONTENT);
    }
    uploads::remove(&state, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn completed_response(row: UploadRow) -> Result<Json<CompleteUploadResponse>, ApiError> {
    let (Some(game_id), Some(version_id)) = (row.game_id, row.version_id) else {
        return Err(ApiError::conflict(
            "upload_incomplete",
            "This upload was not indexed",
        ));
    };
    Ok(Json(CompleteUploadResponse {
        id: row.id,
        game_id,
        version_id,
        file_name: row.file_name,
        size: row.size,
        sha256: row.sha256,
    }))
}

async fn fetch_row(state: &AppState, id: &str) -> Result<UploadRow, ApiError> {
    db::query_as::<UploadRow>(
        "SELECT id, user_id, file_name, size, sha256, chunk_size, status,
                game_id, version_id, error, created_at, updated_at
         FROM uploads WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("upload_not_found", "Upload not found"))
}

async fn fetch_owned(state: &AppState, user: &AuthUser, id: &str) -> Result<UploadRow, ApiError> {
    user.require_scope(Scope::Write)?;
    let row = fetch_row(state, id).await?;
    if row.user_id != user.id {
        user.require_editor()?;
        user.require_admin_access()?;
    }
    Ok(row)
}

async fn mark_error(state: &AppState, id: &str, message: &str) {
    let _ = db::query("UPDATE uploads SET error = ?, updated_at = ? WHERE id = ?")
        .bind(message)
        .bind(db::db_time(db::now()))
        .bind(id)
        .execute(&state.pool)
        .await;
}
