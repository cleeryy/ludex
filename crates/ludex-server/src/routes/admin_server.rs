//! Administrator server settings stored in the database (#188).
//!
//! `default_locale` is the language a new visitor or a new account starts with
//! before it has a preference of its own. It is validated against the same
//! locale manifest as the account preference and exposed in the public
//! `/api/config`, so the login and setup screens can honour it before an
//! account exists.
//!
//! The display name is read here for the admin screen but has been owned by the
//! first-run setup since v0.1; this route does not change it.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::extract::JsonBody;
use crate::locale;
use crate::state::AppState;

/// Administrator settings routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/admin/server", get(get_settings).put(put_settings))
}

/// `GET /api/admin/server` response.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
    /// Display name shown to clients.
    pub server_name: String,
    /// Default interface language; empty means System/English.
    pub default_locale: String,
}

/// `PUT /api/admin/server` request.
///
/// Only `defaultLocale` is writable. `null` and `""` both clear it, which
/// returns the server to the System/English default.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateServerSettingsRequest {
    /// Default interface language from the locale manifest.
    #[serde(default, deserialize_with = "crate::models::deserialize_some")]
    pub default_locale: Option<Option<String>>,
}

async fn get_settings(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<ServerSettings>, ApiError> {
    user.require_admin()?;
    Ok(Json(read_settings(&state).await?))
}

async fn put_settings(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<UpdateServerSettingsRequest>,
) -> Result<Json<ServerSettings>, ApiError> {
    user.require_admin()?;

    if let Some(locale) = request.default_locale {
        let locale = locale::validate(locale.as_deref().unwrap_or_default())?;
        db::set_setting(
            &state.pool,
            "default_locale",
            locale.as_deref().unwrap_or_default(),
        )
        .await
        .map_err(ApiError::from)?;
        state.events.emit(
            Event::new(actions::SETTINGS_UPDATED, Actor::User { id: user.id })
                .target("settings", "server")
                .metadata(serde_json::json!({ "defaultLocale": locale })),
        );
    }

    Ok(Json(read_settings(&state).await?))
}

async fn read_settings(state: &AppState) -> Result<ServerSettings, ApiError> {
    let server_name = db::server_name(&state.pool, &state.config.server_name).await;
    let default_locale = db::default_locale(&state.pool).await;
    Ok(ServerSettings {
        server_name,
        default_locale,
    })
}
