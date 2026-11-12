//! Account self-service routes (#183).
//!
//! The account holder uploads a logo/avatar here. The file is sniffed and
//! stored in the media cache, and every user payload immediately carries the
//! new `avatarUrl`. Removing it clears the column and deletes the managed file.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::avatar::{self, AvatarRejection, MAX_AVATAR_BYTES};
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::MultipartBody;
use crate::models::User;
use crate::state::AppState;

/// Largest multipart body accepted by the avatar route (file + framing).
const AVATAR_BODY_LIMIT: usize = MAX_AVATAR_BYTES + 1024 * 1024;

/// Account routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/account/avatar", post(upload_avatar).delete(delete_avatar))
        .layer(DefaultBodyLimit::max(AVATAR_BODY_LIMIT))
}

/// `POST /api/account/avatar` — multipart field `file` (or `avatar`).
async fn upload_avatar(
    State(state): State<AppState>,
    user: AuthUser,
    MultipartBody(mut multipart): MultipartBody,
) -> Result<Json<User>, ApiError> {
    user.require_scope(Scope::Write)?;

    let mut bytes: Vec<u8> = Vec::new();
    let mut file_name = String::new();
    while let Some(field) = multipart.next_field().await.map_err(map_multipart_error)? {
        match field.name().unwrap_or_default() {
            "file" | "avatar" => {
                if !bytes.is_empty() {
                    return Err(ApiError::bad_request(
                        "invalid_avatar",
                        "Only one image field is allowed",
                    ));
                }
                file_name = field.file_name().unwrap_or_default().to_string();
                let batch = field.bytes().await.map_err(map_multipart_error)?;
                if batch.len() > MAX_AVATAR_BYTES {
                    return Err(rejection_error(AvatarRejection::TooLarge));
                }
                bytes = batch.to_vec();
            }
            _ => {
                let mut field = field;
                while field.chunk().await.map_err(map_multipart_error)?.is_some() {}
            }
        }
    }

    let image = avatar::inspect(&bytes, &file_name).map_err(rejection_error)?;
    let stored = avatar::store(&state.config.media_dir, user.id, &image, &bytes)
        .await
        .map_err(store_error)?;

    let previous: Option<String> =
        db::query_scalar::<Option<String>>("SELECT avatar_path FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?
            .flatten()
            .filter(|name| !name.is_empty() && name != &stored.name);

    if let Err(error) = db::query("UPDATE users SET avatar_path = ?, updated_at = ? WHERE id = ?")
        .bind(&stored.name)
        .bind(db::db_time(db::now()))
        .bind(user.id)
        .execute(&state.pool)
        .await
    {
        // The database row still points at the old file: remove the new one.
        avatar::remove(&state.config.media_dir, user.id, &stored.name).await;
        return Err(ApiError::from(error));
    }

    if let Some(previous) = previous {
        avatar::remove(&state.config.media_dir, user.id, &previous).await;
    }

    state.events.emit(
        Event::new(actions::USER_AVATAR_UPDATED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({
                "format": stored.format.as_str(),
                "width": stored.width,
                "height": stored.height,
                "bytes": stored.size,
                "sha256": stored.sha256,
            })),
    );

    Ok(Json(
        super::users::fetch_user(&state, user.id).await?.to_public(),
    ))
}

/// `DELETE /api/account/avatar` — clears the avatar and deletes the file.
async fn delete_avatar(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<User>, ApiError> {
    user.require_scope(Scope::Write)?;
    let previous: Option<String> =
        db::query_scalar::<Option<String>>("SELECT avatar_path FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?
            .flatten()
            .filter(|name| !name.is_empty());

    if let Some(name) = previous.as_deref() {
        db::query("UPDATE users SET avatar_path = NULL, updated_at = ? WHERE id = ?")
            .bind(db::db_time(db::now()))
            .bind(user.id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
        avatar::remove(&state.config.media_dir, user.id, name).await;
        state.events.emit(
            Event::new(actions::USER_AVATAR_UPDATED, user.actor())
                .target("user", user.id.to_string())
                .metadata(json!({ "removed": true })),
        );
    }

    Ok(Json(
        super::users::fetch_user(&state, user.id).await?.to_public(),
    ))
}

fn rejection_error(rejection: AvatarRejection) -> ApiError {
    let status = match rejection {
        AvatarRejection::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        _ => StatusCode::BAD_REQUEST,
    };
    ApiError::new(status, rejection.code(), rejection.message())
}

fn store_error(error: std::io::Error) -> ApiError {
    if error.kind() == std::io::ErrorKind::InvalidInput {
        return ApiError::bad_request("invalid_avatar_type", error.to_string());
    }
    tracing::error!(error = %error, "could not store avatar");
    ApiError::internal()
}

fn map_multipart_error(rejection: axum::extract::multipart::MultipartError) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        rejection_error(AvatarRejection::TooLarge)
    } else {
        ApiError::bad_request("invalid_multipart", rejection.body_text())
    }
}
