//! Custom artwork routes (#38).
//!
//! Editors upload a cover or background; the image is sniffed, stored in the
//! media cache and the game row points at it. Removing custom art clears the
//! column so provider artwork (or the placeholder) is used again.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::artwork::{self, ArtworkKind, MAX_ARTWORK_BYTES};
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, MultipartBody};
use crate::state::AppState;

/// Largest multipart body accepted by the artwork route (file + framing).
const ARTWORK_BODY_LIMIT: usize = MAX_ARTWORK_BYTES + 1024 * 1024;

/// Response of a custom artwork upload.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtworkResponse {
    /// `cover` or `background`.
    pub kind: String,
    /// Public media URL, cache-busted by the upload timestamp in the name.
    pub url: String,
    /// Stored image format (`png`, `jpg`, `webp`, `gif`).
    pub format: String,
    /// Stored file size in bytes.
    pub size: u64,
    /// SHA-256 of the uploaded bytes.
    pub sha256: String,
}

/// `DELETE /api/games/{id}/artwork` query.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtworkQuery {
    /// Slot to reset.
    pub kind: String,
}

/// Artwork routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/games/{id}/artwork",
            post(upload).layer(DefaultBodyLimit::max(ARTWORK_BODY_LIMIT)),
        )
        .route("/games/{id}/artwork", delete(reset))
}

async fn upload(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    MultipartBody(mut multipart): MultipartBody,
) -> Result<(StatusCode, Json<ArtworkResponse>), ApiError> {
    user.require_editor()?;
    ensure_game(&state, game_id).await?;

    let mut kind: Option<ArtworkKind> = None;
    let mut bytes: Vec<u8> = Vec::new();
    let mut file_name = String::new();

    while let Some(field) = multipart.next_field().await.map_err(map_multipart_error)? {
        match field.name().unwrap_or_default() {
            "kind" => {
                let value = field.text().await.map_err(map_multipart_error)?;
                kind = Some(ArtworkKind::parse(&value).ok_or_else(|| {
                    ApiError::bad_request("invalid_kind", "kind must be 'cover' or 'background'")
                })?);
            }
            "file" => {
                if !bytes.is_empty() {
                    return Err(ApiError::bad_request(
                        "invalid_file",
                        "Only one file field is allowed",
                    ));
                }
                file_name = field.file_name().unwrap_or_default().to_string();
                let batch = field.bytes().await.map_err(map_multipart_error)?;
                if batch.len() > MAX_ARTWORK_BYTES {
                    return Err(ApiError::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "artwork_too_large",
                        format!(
                            "Artwork must be at most {} MiB",
                            MAX_ARTWORK_BYTES / (1024 * 1024)
                        ),
                    ));
                }
                bytes = batch.to_vec();
            }
            _ => {
                let mut field = field;
                while field.chunk().await.map_err(map_multipart_error)?.is_some() {}
            }
        }
    }

    let kind = kind.ok_or_else(|| {
        ApiError::bad_request("invalid_kind", "Multipart field 'kind' is required")
    })?;
    if bytes.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_file",
            "Multipart field 'file' is required",
        ));
    }
    if !artwork::is_acceptable(&bytes) {
        return Err(ApiError::bad_request(
            "invalid_image",
            format!(
                "Unsupported image ({}). Use PNG, JPEG, WebP or GIF under {} MiB",
                if file_name.is_empty() {
                    "unknown file"
                } else {
                    file_name.as_str()
                },
                MAX_ARTWORK_BYTES / (1024 * 1024)
            ),
        ));
    }

    let stored = artwork::store(&state.config.media_dir, game_id, kind, &bytes)
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                ApiError::bad_request("invalid_image", error.to_string())
            } else {
                tracing::error!(error = %error, "could not store custom artwork");
                ApiError::internal()
            }
        })?;

    let previous: Option<String> =
        sqlx::query_scalar(&format!("SELECT {} FROM games WHERE id = ?", kind.column()))
            .bind(game_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;

    sqlx::query(&format!(
        "UPDATE games SET {} = ?, updated_at = ? WHERE id = ?",
        kind.column()
    ))
    .bind(&stored.name)
    .bind(crate::db::db_time(crate::db::now()))
    .bind(game_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    // Provider art replaced by this upload is left to the media GC: it may be
    // shared by other games.
    if let Some(previous) = previous.filter(|name| name != &stored.name) {
        tracing::debug!(previous, game = game_id, "custom artwork replaced");
    }

    state.events.emit(
        Event::new(actions::GAME_ARTWORK_UPDATED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "kind": kind.as_str(),
                "file": stored.name,
                "format": stored.format,
                "size": stored.size,
                "sha256": stored.sha256,
            })),
    );

    Ok((
        StatusCode::CREATED,
        Json(ArtworkResponse {
            kind: kind.as_str().to_string(),
            url: crate::media::media_url(&stored.name),
            format: stored.format.to_string(),
            size: stored.size as u64,
            sha256: stored.sha256,
        }),
    ))
}

async fn reset(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<ArtworkQuery>,
) -> Result<StatusCode, ApiError> {
    user.require_editor()?;
    ensure_game(&state, game_id).await?;
    let kind = ArtworkKind::parse(&query.kind).ok_or_else(|| {
        ApiError::bad_request("invalid_kind", "kind must be 'cover' or 'background'")
    })?;

    let previous: Option<String> =
        sqlx::query_scalar(&format!("SELECT {} FROM games WHERE id = ?", kind.column()))
            .bind(game_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;

    sqlx::query(&format!(
        "UPDATE games SET {} = NULL, updated_at = ? WHERE id = ?",
        kind.column()
    ))
    .bind(crate::db::db_time(crate::db::now()))
    .bind(game_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    if let Some(name) = previous.as_deref() {
        if !name.is_empty() {
            artwork::remove(&state.config.media_dir, name).await;
        }
    }

    state.events.emit(
        Event::new(actions::GAME_ARTWORK_UPDATED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "kind": kind.as_str(),
                "reset": true,
            })),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_game(state: &AppState, game_id: i64) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar::<_, i64>("SELECT id FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}

fn map_multipart_error(rejection: axum::extract::multipart::MultipartError) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "artwork_too_large",
            "The artwork upload exceeds the server limit",
        )
    } else {
        ApiError::bad_request("invalid_multipart", rejection.body_text())
    }
}
