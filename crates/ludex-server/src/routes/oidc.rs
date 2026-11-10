//! Public OIDC routes: start, callback and one-time code exchange.

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::error::ApiError;
use crate::extract::{ApiQuery, JsonBody};
use crate::models::{AuthResponse, UserRow};
use crate::oidc;
use crate::state::AppState;

/// OIDC routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/oidc/start", get(start))
        .route("/auth/oidc/callback", get(callback))
        .route("/auth/oidc/exchange", post(exchange))
}

#[derive(Debug, Default, Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcExchangeRequest {
    code: String,
}

async fn start(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let client = state.oidc_client().await.ok_or_else(oidc_disabled)?;
    let url = client.begin(&headers).await?;
    let mut response = Redirect::to(&url).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn callback(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<CallbackQuery>,
) -> Result<Response, ApiError> {
    let client = state.oidc_client().await.ok_or_else(oidc_disabled)?;
    if let Some(error) = query.error {
        let message = query
            .error_description
            .filter(|description| !description.trim().is_empty())
            .unwrap_or_else(|| format!("The identity provider rejected the login: {error}"));
        return Err(ApiError::bad_request("oidc_provider_error", message));
    }
    let code = query
        .code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .ok_or_else(|| ApiError::bad_request("invalid_request", "Missing authorization code"))?;
    let state_param = query
        .state
        .as_deref()
        .map(str::trim)
        .filter(|state| !state.is_empty())
        .ok_or_else(|| ApiError::bad_request("invalid_state", "Missing SSO state"))?;

    let identity = client.complete(code, state_param).await?;
    let user_id = oidc::provision_user(&state, &client, &identity).await?;
    let one_time_code = client.issue_code(user_id)?;
    let location = client.callback_location(&one_time_code);

    let mut response = Redirect::to(&location).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn exchange(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<OidcExchangeRequest>,
) -> Result<Response, ApiError> {
    let client = state.oidc_client().await.ok_or_else(oidc_disabled)?;
    let code = request.code.trim();
    if code.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_request",
            "Missing SSO exchange code",
        ));
    }
    let user_id = client.take_code(code).ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_oidc_code",
            "The SSO code is invalid, expired or already used",
        )
    })?;

    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, email, role, activated, avatar_path,
                created_at, updated_at, oidc_issuer, oidc_subject
         FROM users WHERE id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_oidc_code",
            "The SSO account is no longer available",
        )
    })?;
    if row.activated == 0 {
        return Err(ApiError::forbidden(
            "account_disabled",
            "This account is disabled",
        ));
    }

    let user = row.to_public();
    let info = crate::auth::SessionInfo {
        user_agent: None,
        ip: None,
    };
    let (issued, _session_id) =
        crate::auth::start_session(&state, user.id, user.role, &info).await?;
    let mut response = Json(AuthResponse {
        token: issued.token,
        user,
        expires_at: Some(issued.expires_at),
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn oidc_disabled() -> ApiError {
    ApiError::not_found("oidc_disabled", "SSO is not configured on this server")
}
