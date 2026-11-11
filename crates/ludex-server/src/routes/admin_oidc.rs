//! Admin OIDC settings: read, save (with hot reload), test discovery and
//! reset to environment defaults.
//!
//! The client secret is write-only. It is encrypted with a key derived from
//! the server secret before it touches the database and is never serialised
//! by any endpoint.

use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::JsonBody;
use crate::oidc::{
    self, changed_fields, non_empty, OidcClient, OidcDiscoveryInfo, OidcSettings, OidcStatus,
};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use ludex_core::models::Role;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Audit action recorded when the settings are saved or reset.
const ACTION_OIDC_UPDATED: &str = "settings.oidc_updated";

/// Admin OIDC routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/oidc", get(get_settings).put(put_settings))
        .route("/admin/oidc/test", post(test_settings))
        .route("/admin/oidc/reset", post(reset_settings))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OidcAdminResponse {
    /// Effective, non-secret settings.
    settings: OidcSettings,
    /// camelCase names of settings pinned by `LUDEX_OIDC_*`.
    env_locked_fields: Vec<String>,
    /// Last discovery check.
    status: OidcStatus,
    /// Accounts already linked to an SSO identity.
    linked_accounts: i64,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcSettingsRequest {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    issuer: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    redirect_url: Option<String>,
    #[serde(default)]
    public_base_url: Option<String>,
    #[serde(default)]
    scopes: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    auto_create: Option<bool>,
    #[serde(default)]
    default_role: Option<Role>,
    #[serde(default)]
    groups_claim: Option<String>,
    #[serde(default)]
    admin_groups: Option<Vec<String>>,
    /// Write-only replacement secret. Absent or blank keeps the stored one.
    #[serde(default)]
    replace_secret: Option<String>,
    /// Remove the stored secret (switch to a public PKCE client).
    #[serde(default)]
    clear_secret: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OidcTestResponse {
    ok: bool,
    discovery: OidcDiscoveryInfo,
}

async fn get_settings(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<OidcAdminResponse>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(admin_response(&state).await?))
}

async fn put_settings(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<OidcSettingsRequest>,
) -> Result<Json<OidcAdminResponse>, ApiError> {
    user.require_admin_access()?;

    let stored = oidc::load_settings(&state.pool, &state.jwt_secret)
        .await
        .map_err(ApiError::from)?;
    let resolved = oidc::resolve_settings(stored.as_ref(), &state.config);
    let previous = resolved.settings.clone();
    let mut next = previous.clone();
    apply_request(&request, &mut next, &resolved.env_locked_fields);
    next.admin_groups = normalize_groups(&next.admin_groups);

    let (encrypted_secret, secret_changed) =
        resolve_secret(&state, &request, &resolved, &mut next)?;

    if let Some(secret_error) = secret_error_message(&resolved, &request) {
        return Err(ApiError::bad_request(
            "oidc_secret_unreadable",
            secret_error,
        ));
    }

    oidc::validate_settings(
        &next,
        !resolved.env_locked_fields.iter().any(|f| f == "issuer"),
    )?;

    // Build and probe a candidate client before persisting or swapping.
    let candidate = build_candidate(&state, &next)?;
    let client = match &candidate {
        Some(client) => match client.probe().await {
            Ok(_) => Some(client.clone()),
            Err(error) => {
                record_status(&state, next.issuer.clone(), Err(&error)).await;
                return Err(discovery_failure(error));
            }
        },
        None => None,
    };

    oidc::save_settings(&state.pool, &next, encrypted_secret.as_deref())
        .await
        .map_err(ApiError::from)?;
    let enabled = client.is_some();
    *state.oidc.write().await = client;
    if enabled {
        record_status(&state, next.issuer.clone(), Ok(())).await;
    } else {
        *state.oidc_status.write().await = OidcStatus::initial(None);
    }

    let fields = changed_fields(&previous, &next, secret_changed);
    state.events.emit(
        Event::new(ACTION_OIDC_UPDATED, user.actor())
            .target("settings", "oidc")
            .metadata(json!({
                "issuer": next.issuer,
                "enabled": next.enabled,
                "fields": fields,
            })),
    );
    Ok(Json(admin_response(&state).await?))
}

async fn test_settings(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<OidcSettingsRequest>,
) -> Result<Json<OidcTestResponse>, ApiError> {
    user.require_admin_access()?;

    let stored = oidc::load_settings(&state.pool, &state.jwt_secret)
        .await
        .map_err(ApiError::from)?;
    let resolved = oidc::resolve_settings(stored.as_ref(), &state.config);
    let mut next = resolved.settings.clone();
    apply_request(&request, &mut next, &resolved.env_locked_fields);
    next.enabled = true;
    next.admin_groups = normalize_groups(&next.admin_groups);
    if !state.config.oidc_env_locked.client_secret {
        if request
            .replace_secret
            .as_deref()
            .map(str::trim)
            .is_some_and(|secret| !secret.is_empty())
        {
            next.client_secret = non_empty(request.replace_secret.as_deref());
        } else if request.clear_secret {
            next.client_secret = None;
        }
    }

    oidc::validate_settings(
        &next,
        !resolved.env_locked_fields.iter().any(|f| f == "issuer"),
    )?;
    let Some(client) = build_candidate(&state, &next)? else {
        return Err(ApiError::bad_request(
            "invalid_oidc_config",
            "An issuer and client ID are required to test discovery",
        ));
    };
    match client.probe().await {
        Ok(discovery) => {
            record_status(&state, next.issuer.clone(), Ok(())).await;
            Ok(Json(OidcTestResponse {
                ok: true,
                discovery,
            }))
        }
        Err(error) => {
            record_status(&state, next.issuer.clone(), Err(&error)).await;
            Err(discovery_failure(error))
        }
    }
}

async fn reset_settings(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<OidcAdminResponse>, ApiError> {
    user.require_admin_access()?;

    oidc::delete_settings(&state.pool)
        .await
        .map_err(ApiError::from)?;
    let resolved = oidc::resolve_settings(None, &state.config);
    let client =
        OidcClient::from_settings(&resolved.settings, state.http.clone()).map_err(|error| {
            tracing::error!(error = %error, "environment OIDC configuration is invalid");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "oidc_env_invalid",
                format!("The environment OIDC configuration is invalid: {error:#}"),
            )
        })?;
    *state.oidc.write().await = client;
    let initial = {
        let current = state.oidc.read().await;
        OidcStatus::initial(current.as_ref())
    };
    *state.oidc_status.write().await = initial;

    state.events.emit(
        Event::new(ACTION_OIDC_UPDATED, user.actor())
            .target("settings", "oidc")
            .metadata(json!({
                "reset": true,
                "issuer": resolved.settings.issuer,
                "enabled": resolved.settings.enabled,
                "fields": [],
            })),
    );

    Ok(Json(admin_response(&state).await?))
}

/// Applies the request over the current settings, skipping environment-locked
/// fields. Empty strings clear optional values.
fn apply_request(request: &OidcSettingsRequest, base: &mut OidcSettings, locked: &[String]) {
    let unlocked = |name: &str| !locked.iter().any(|field| field == name);
    if unlocked("enabled") {
        if let Some(enabled) = request.enabled {
            base.enabled = enabled;
        }
    }
    if unlocked("issuer") {
        if let Some(issuer) = &request.issuer {
            base.issuer = non_empty(Some(issuer));
        }
    }
    if unlocked("clientId") {
        if let Some(client_id) = &request.client_id {
            base.client_id = non_empty(Some(client_id));
        }
    }
    if unlocked("redirectUrl") {
        if let Some(redirect) = &request.redirect_url {
            base.redirect_url = non_empty(Some(redirect));
        }
    }
    if unlocked("publicBaseUrl") {
        if let Some(base_url) = &request.public_base_url {
            base.public_base_url = non_empty(Some(base_url));
        }
    }
    if unlocked("scopes") {
        if let Some(scopes) = &request.scopes {
            let trimmed = scopes.trim();
            base.scopes = if trimmed.is_empty() {
                "openid profile email".to_string()
            } else {
                trimmed.to_string()
            };
        }
    }
    if unlocked("displayName") {
        if let Some(display_name) = &request.display_name {
            let trimmed = display_name.trim();
            base.display_name = if trimmed.is_empty() {
                "SSO".to_string()
            } else {
                trimmed.to_string()
            };
        }
    }
    if unlocked("autoCreate") {
        if let Some(auto_create) = request.auto_create {
            base.auto_create = auto_create;
        }
    }
    if unlocked("defaultRole") {
        if let Some(role) = request.default_role {
            base.default_role = role;
        }
    }
    if unlocked("groupsClaim") {
        if let Some(groups_claim) = &request.groups_claim {
            let trimmed = groups_claim.trim();
            base.groups_claim = if trimmed.is_empty() {
                "groups".to_string()
            } else {
                trimmed.to_string()
            };
        }
    }
    if unlocked("adminGroups") {
        if let Some(groups) = &request.admin_groups {
            base.admin_groups = groups.clone();
        }
    }
}

/// Trimmed, de-duplicated (case-insensitive) admin group list.
fn normalize_groups(groups: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = Vec::new();
    for group in groups {
        let group = group.trim();
        if group.is_empty()
            || normalized
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(group))
        {
            continue;
        }
        normalized.push(group.to_string());
    }
    normalized
}

/// Decides which ciphertext to persist, if any. The plaintext stays in
/// `next.client_secret` for the candidate client only. Returns the ciphertext
/// and whether the secret changed.
fn resolve_secret(
    state: &AppState,
    request: &OidcSettingsRequest,
    resolved: &oidc::ResolvedOidcSettings,
    next: &mut OidcSettings,
) -> Result<(Option<String>, bool), ApiError> {
    if state.config.oidc_env_locked.client_secret {
        // The secret comes from the environment and wins: ignore the
        // write-only fields. Never copy it into the DB, but keep any previous
        // ciphertext so unsetting the variable later restores the saved secret.
        next.client_secret = resolved.settings.client_secret.clone();
        next.secret_configured = resolved.settings.client_secret.is_some();
        return Ok((resolved.encrypted_secret.clone(), false));
    }

    let replacement = request
        .replace_secret
        .as_deref()
        .map(str::trim)
        .filter(|secret| !secret.is_empty())
        .map(str::to_string);
    if let Some(secret) = replacement {
        if secret.chars().count() > 4096 {
            return Err(ApiError::bad_request(
                "invalid_oidc_secret",
                "The client secret must be at most 4096 characters",
            ));
        }
        let encrypted = oidc::encrypt_secret(&state.jwt_secret, &secret).map_err(ApiError::from)?;
        next.client_secret = Some(secret);
        next.secret_configured = true;
        return Ok((Some(encrypted), true));
    }
    if request.clear_secret {
        next.client_secret = None;
        next.secret_configured = false;
        return Ok((None, true));
    }
    Ok((resolved.encrypted_secret.clone(), false))
}

/// Explains an unreadable stored secret, unless the request replaces or clears
/// it, or the environment provides one.
fn secret_error_message(
    resolved: &oidc::ResolvedOidcSettings,
    request: &OidcSettingsRequest,
) -> Option<String> {
    if !resolved.secret_error
        || resolved
            .env_locked_fields
            .iter()
            .any(|f| f == "clientSecret")
        || resolved.encrypted_secret.is_none()
    {
        return None;
    }
    let replacing = request
        .replace_secret
        .as_deref()
        .map(str::trim)
        .is_some_and(|secret| !secret.is_empty());
    if replacing || request.clear_secret {
        return None;
    }
    Some(
        "The saved client secret cannot be decrypted because the server secret changed. \
         Enter the client secret again or clear it for a public client."
            .to_string(),
    )
}

fn build_candidate(
    state: &AppState,
    settings: &OidcSettings,
) -> Result<Option<OidcClient>, ApiError> {
    OidcClient::from_settings(settings, state.http.clone()).map_err(|error| {
        ApiError::bad_request(
            "invalid_oidc_config",
            format!("The OIDC configuration is invalid: {error:#}"),
        )
    })
}

fn discovery_failure(error: String) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        "oidc_discovery_failed",
        format!("OIDC discovery failed: {error}"),
    )
}

async fn record_status(state: &AppState, issuer: Option<String>, result: Result<(), &str>) {
    let mut status = state.oidc_status.write().await;
    status.issuer = issuer;
    status.checked_at = Some(db::db_time(db::now()));
    match result {
        Ok(()) => {
            status.ok = true;
            status.error = None;
        }
        Err(error) => {
            status.ok = false;
            status.error = Some(error.to_string());
        }
    }
}

async fn admin_response(state: &AppState) -> Result<OidcAdminResponse, ApiError> {
    let stored = oidc::load_settings(&state.pool, &state.jwt_secret)
        .await
        .map_err(ApiError::from)?;
    let resolved = oidc::resolve_settings(stored.as_ref(), &state.config);
    let status = state.oidc_status.read().await.clone();
    let linked_accounts =
        db::query_scalar::<i64>("SELECT COUNT(*) FROM users WHERE oidc_issuer IS NOT NULL")
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;

    Ok(OidcAdminResponse {
        settings: resolved.settings,
        env_locked_fields: resolved.env_locked_fields,
        status,
        linked_accounts,
    })
}
