//! HTTP routes, nested under `/api` by [`crate::build_router`].

pub mod auth;
pub mod games;
pub mod library;
pub mod media;
pub mod news;
pub mod system;
pub mod users;

use axum::Router;

use crate::error::ApiError;
use crate::state::AppState;

/// Builds the authenticated/public API router. Paths are relative to `/api`.
pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(system::router())
        .merge(auth::router())
        .merge(games::router())
        .merge(users::router())
        .merge(library::router())
        .merge(news::router())
        .merge(media::router())
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
