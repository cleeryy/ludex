//! HTTP routes, nested under `/api` by [`crate::build_router`].

pub mod achievements;
pub mod admin_oidc;
pub mod artwork;
pub mod audit;
pub mod auth;
pub mod collections;
pub mod community;
pub mod controllers;
pub mod desktop;
pub mod diagnostics;
pub mod emulators;
pub mod game_metadata;
pub mod games;
pub mod integrity;
pub mod keys;
pub mod library;
pub mod media;
pub mod metadata_providers;
pub mod metrics;
pub mod news;
pub mod oidc;
pub mod openapi;
pub mod saves;
pub mod search;
pub mod share;
pub mod system;
pub mod uploads;
pub mod users;
pub mod webhooks;

use axum::Router;

use crate::error::ApiError;
use crate::state::AppState;

/// Builds the authenticated/public API router. Paths are relative to `/api`.
pub fn api_router(saves_max_bytes: u64) -> Router<AppState> {
    Router::new()
        .merge(system::router())
        .merge(auth::router())
        .merge(oidc::router())
        .merge(admin_oidc::router())
        .merge(metadata_providers::router())
        .merge(games::router())
        .merge(game_metadata::router())
        .merge(artwork::router())
        .merge(integrity::router())
        .merge(collections::router())
        .merge(community::router())
        .merge(search::router())
        .merge(users::router())
        .merge(library::router())
        .merge(news::router())
        .merge(media::router())
        .merge(saves::router(saves_max_bytes))
        .merge(keys::router())
        .merge(audit::router())
        .merge(webhooks::router())
        .merge(share::router())
        .merge(uploads::router())
        .merge(openapi::router())
        .merge(achievements::router())
        .merge(controllers::router())
        .merge(desktop::router())
        .merge(diagnostics::router())
        .merge(emulators::router())
        .fallback(api_not_found)
}

async fn api_not_found() -> ApiError {
    ApiError::not_found("not_found", "Endpoint not found")
}

/// Validates and normalises a username.
pub(crate) fn validate_username(username: &str) -> Result<String, ApiError> {
    let username = username.trim();
    if username.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_username",
            "Username is required",
        ));
    }
    if username.chars().count() > 64 {
        return Err(ApiError::bad_request(
            "invalid_username",
            "Username must be at most 64 characters",
        ));
    }
    if username.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_username",
            "Username contains invalid characters",
        ));
    }
    Ok(username.to_string())
}

/// Validates a password. The API only enforces a minimum length.
pub(crate) fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < 8 {
        return Err(ApiError::bad_request(
            "invalid_password",
            "Password must be at least 8 characters",
        ));
    }
    if password.chars().count() > 1024 {
        return Err(ApiError::bad_request(
            "invalid_password",
            "Password must be at most 1024 characters",
        ));
    }
    Ok(())
}

/// Validates an optional e-mail address.
pub(crate) fn validate_email(email: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(email) = email else {
        return Ok(None);
    };
    let email = email.trim();
    if email.is_empty() {
        return Ok(None);
    }
    if email.chars().count() > 254 || !email.contains('@') || email.contains(char::is_whitespace) {
        return Err(ApiError::bad_request(
            "invalid_email",
            "Email address is invalid",
        ));
    }
    Ok(Some(email.to_string()))
}
