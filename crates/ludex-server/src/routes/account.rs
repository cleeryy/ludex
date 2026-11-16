//! Account self-service routes (#183, #188).
//!
//! The account holder uploads a logo/avatar here. The file is sniffed and
//! stored in the media cache, and every user payload immediately carries the
//! new `avatarUrl`. Removing it clears the column and deletes the managed file.
//!
//! The same module owns the account profile preferences: the interface
//! language is validated against the web locale manifest, so a locale the
//! picker hides is rejected before it reaches the database.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use utoipa::openapi::HttpMethod;
use utoipa::ToSchema;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::avatar::{self, AvatarRejection, MAX_AVATAR_BYTES};
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{JsonBody, MultipartBody};
use crate::locale;
use crate::models::User;
use crate::openapi::{body, ok, route, Auth, Payload, RouteDoc};
use crate::state::AppState;

/// Largest multipart body accepted by the avatar route (file + framing).
const AVATAR_BODY_LIMIT: usize = MAX_AVATAR_BYTES + 1024 * 1024;

/// Account routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/account/avatar", post(upload_avatar).delete(delete_avatar))
        .route("/account/profile", patch(update_profile))
        .layer(DefaultBodyLimit::max(AVATAR_BODY_LIMIT))
}

/// `PATCH /api/account/profile` request (#188).
///
/// The field is `Option<Option<String>>` so the three cases stay distinct:
/// absent leaves the preference untouched, `null` and `""` both clear it
/// ("inherit"), and a code is validated against the locale manifest.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfileRequest {
    /// Interface language from `apps/web/src/locales/manifest.json`.
    #[serde(default, deserialize_with = "crate::models::deserialize_some")]
    pub locale: Option<Option<String>>,
}

/// `PATCH /api/account/profile` — updates the caller's own preferences.
async fn update_profile(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<AccountProfileRequest>,
) -> Result<Json<User>, ApiError> {
    user.require_scope(Scope::Write)?;

    if let Some(locale) = request.locale {
        let locale = locale::validate(locale.as_deref().unwrap_or_default())?;
        db::query("UPDATE users SET locale = ?, updated_at = ? WHERE id = ?")
            .bind(locale.as_deref())
            .bind(db::db_time(db::now()))
            .bind(user.id)
            .execute(&state.core.pool)
            .await
            .map_err(ApiError::from)?;
        state.core.events.emit(
            Event::new(actions::USER_UPDATED, user.actor())
                .target("user", user.id.to_string())
                .metadata(json!({ "locale": locale })),
        );
    }

    Ok(Json(
        super::users::fetch_user(&state, user.id).await?.to_public(),
    ))
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
    let stored = avatar::store(&state.core.config.media_dir, user.id, &image, &bytes)
        .await
        .map_err(store_error)?;

    let previous: Option<String> =
        db::query_scalar::<Option<String>>("SELECT avatar_path FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_optional(&state.core.pool)
            .await
            .map_err(ApiError::from)?
            .flatten()
            .filter(|name| !name.is_empty() && name != &stored.name);

    if let Err(error) = db::query("UPDATE users SET avatar_path = ?, updated_at = ? WHERE id = ?")
        .bind(&stored.name)
        .bind(db::db_time(db::now()))
        .bind(user.id)
        .execute(&state.core.pool)
        .await
    {
        // The database row still points at the old file: remove the new one.
        avatar::remove(&state.core.config.media_dir, user.id, &stored.name).await;
        return Err(ApiError::from(error));
    }

    if let Some(previous) = previous {
        avatar::remove(&state.core.config.media_dir, user.id, &previous).await;
    }

    state.core.events.emit(
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
            .fetch_optional(&state.core.pool)
            .await
            .map_err(ApiError::from)?
            .flatten()
            .filter(|name| !name.is_empty());

    if let Some(name) = previous.as_deref() {
        db::query("UPDATE users SET avatar_path = NULL, updated_at = ? WHERE id = ?")
            .bind(db::db_time(db::now()))
            .bind(user.id)
            .execute(&state.core.pool)
            .await
            .map_err(ApiError::from)?;
        avatar::remove(&state.core.config.media_dir, user.id, name).await;
        state.core.events.emit(
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

/// OpenAPI operations served by this module (#237).
pub(crate) const DOCS: &[RouteDoc] = &[
    // Account avatar (#183)
    route(
        HttpMethod::Post,
        "/account/avatar",
        "Authentication",
        "Upload an avatar (multipart field `file`, PNG/JPEG/WebP, 2 MiB, 4096×4096)",
        Auth::Session,
        Payload::Schema("User"),
    ),
    route(
        HttpMethod::Delete,
        "/account/avatar",
        "Authentication",
        "Remove the avatar and delete the managed file",
        Auth::Session,
        Payload::Schema("User"),
    ),
    body(
        route(
            HttpMethod::Patch,
            "/account/profile",
            "Authentication",
            "Update the interface-language preference of the current account",
            Auth::Session,
            Payload::Schema("User"),
        ),
        "AccountProfileRequest",
    ),
    route(
        HttpMethod::Get,
        "/account/preferences",
        "Authentication",
        "Read the synced preference document and its revision",
        Auth::Session,
        Payload::Schema("PreferencesResponse"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/account/preferences",
            "Authentication",
            "Replace the preference document (If-Match revision, 409 on conflict)",
            Auth::Session,
            Payload::Schema("PreferencesResponse"),
        ),
        "ReplacePreferencesRequest",
    ),
    body(
        route(
            HttpMethod::Patch,
            "/account/preferences",
            "Authentication",
            "Deep-merge a preference fragment (If-Match revision, 409 on conflict)",
            Auth::Session,
            Payload::Schema("PreferencesResponse"),
        ),
        "PatchPreferencesRequest",
    ),
    route(
        HttpMethod::Delete,
        "/account/preferences",
        "Authentication",
        "Reset the synced preference document to defaults",
        Auth::Session,
        Payload::Schema("PreferencesResponse"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/auth/onboarding",
            "Authentication",
            "Update the onboarding preferences of the current account",
            Auth::Session,
            Payload::Schema("UserResponse"),
        ),
        "OnboardingRequest",
    ),
    ok(
        route(
            HttpMethod::Post,
            "/auth/logout",
            "Authentication",
            "Revoke the current token",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/change-password",
                "Authentication",
                "Change the account password",
                Auth::Session,
                Payload::None,
            ),
            204,
        ),
        "ChangePasswordRequest",
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/me/password",
                "Authentication",
                "Change the account password (alias)",
                Auth::Session,
                Payload::None,
            ),
            204,
        ),
        "ChangePasswordRequest",
    ),
    route(
        HttpMethod::Get,
        "/auth/sessions",
        "Authentication",
        "List login sessions",
        Auth::Session,
        Payload::Array("AuthSession"),
    ),
    route(
        HttpMethod::Delete,
        "/auth/sessions",
        "Authentication",
        "Revoke every other session of the account (#166)",
        Auth::Session,
        Payload::Schema("SessionRevocation"),
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/auth/sessions/{id}",
            "Authentication",
            "Revoke one session",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
    route(
        HttpMethod::Get,
        "/auth/mfa",
        "Authentication",
        "Second-factor status of the account (#166)",
        Auth::Session,
        Payload::Schema("MfaStatus"),
    ),
    ok(
        route(
            HttpMethod::Post,
            "/auth/mfa/totp/enroll",
            "Authentication",
            "Start a TOTP enrolment; returns the secret once (#166)",
            Auth::Session,
            Payload::Schema("TotpEnrollment"),
        ),
        200,
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/mfa/totp/activate",
                "Authentication",
                "Confirm a TOTP enrolment and receive recovery codes (#166)",
                Auth::Session,
                Payload::Schema("TotpActivated"),
            ),
            200,
        ),
        "TotpActivationRequest",
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/mfa/disable",
                "Authentication",
                "Disable two-factor authentication after re-checking the password (#166)",
                Auth::Session,
                Payload::Schema("MfaStatus"),
            ),
            200,
        ),
        "MfaDisableRequest",
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/auth/mfa/recovery-codes",
                "Authentication",
                "Replace the recovery codes (#166)",
                Auth::Session,
                Payload::Schema("RecoveryCodes"),
            ),
            200,
        ),
        "RecoveryCodesRequest",
    ),
];
