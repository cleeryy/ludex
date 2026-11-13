//! Account preference document sync (#201).
//!
//! One JSON document per account carries the settings that should follow the
//! user across browsers and machines: the theme selection, the Big Picture
//! layout and haptics, the default library view, the update prompt choice and
//! the update notification channels (#213).
//! Genuinely device-specific values (install and download roots, bandwidth,
//! Sunshine/Moonlight paths, overlay settings, server profiles and tokens)
//! never enter this document — the allowlist below is the enforcement point.
//!
//! The revision is an optimistic-concurrency token: clients read it with the
//! document, then send it back in `If-Match`. A stale revision is a 409 with
//! the current revision in the body, so the client can offer "keep this
//! device" or "use the server" instead of silently overwriting the other
//! machine. The audit trail records the revision, key names and size, never
//! the values.

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use utoipa::ToSchema;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::JsonBody;
use crate::state::AppState;

/// Largest accepted document, measured after canonicalisation.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
/// Version of the document shape this server validates and returns.
pub const PREFERENCES_SCHEMA_VERSION: i64 = 1;

const MAX_SELECTION_CHARS: usize = 64;
const ALLOWED_TOP_LEVEL: &[&str] = &["theme", "bigPicture", "library", "updates"];
const THEME_KEYS: &[&str] = &["selection", "accent", "tint"];
const BIG_PICTURE_KEYS: &[&str] = &["layout", "sound", "haptics"];
const LIBRARY_KEYS: &[&str] = &["view"];
const UPDATE_KEYS: &[&str] = &["prompt", "notifications"];
const LAYOUTS: &[&str] = &["shelf", "hero", "grid", "compact"];
const LIBRARY_VIEWS: &[&str] = &["grid", "list"];
const UPDATE_PROMPTS: &[&str] = &["always", "notify", "off"];
const UPDATE_NOTIFICATIONS: &[&str] = &["app-and-os", "app", "off"];

/// Account preference routes.
pub fn router() -> Router<AppState> {
    Router::new().route(
        "/account/preferences",
        get(read).put(replace).patch(merge).delete(reset),
    )
}

/// Largest accepted request body: the document cap plus JSON framing.
const MAX_BODY_BYTES: u64 = MAX_DOCUMENT_BYTES as u64 + 4 * 1024;

/// Stored document plus its concurrency metadata.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesResponse {
    /// `null` when the account has never synced (revision 0).
    pub document: Option<Value>,
    /// Optimistic-concurrency token, `0` when no document exists.
    pub revision: i64,
    /// Document shape version, for in-place client migrations.
    pub schema_version: i64,
    /// Last write, RFC 3339, or `null`.
    pub updated_at: Option<String>,
}

/// `PUT /api/account/preferences` body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplacePreferencesRequest {
    /// Complete document, validated against the allowlist.
    pub document: Value,
    /// Document shape the client wrote; absent means the current version.
    #[serde(default)]
    pub schema_version: Option<i64>,
}

/// `PATCH /api/account/preferences` body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PatchPreferencesRequest {
    /// Partial document, deep-merged into the stored one before validation.
    pub patch: Value,
    /// Document shape the client wrote; absent means the current version.
    #[serde(default)]
    pub schema_version: Option<i64>,
}

struct StoredPreferences {
    document: Value,
    schema_version: i64,
    revision: i64,
    updated_at: String,
}

impl StoredPreferences {
    fn response(&self) -> PreferencesResponse {
        PreferencesResponse {
            document: Some(self.document.clone()),
            revision: self.revision,
            schema_version: self.schema_version,
            updated_at: Some(self.updated_at.clone()),
        }
    }
}

/// `GET /api/account/preferences` — the account's document, or an empty one.
async fn read(State(state): State<AppState>, user: AuthUser) -> Result<Response, ApiError> {
    user.require_scope(Scope::Read)?;
    let response = match load(&state, user.id).await? {
        Some(stored) => stored.response(),
        None => PreferencesResponse {
            document: None,
            revision: 0,
            schema_version: PREFERENCES_SCHEMA_VERSION,
            updated_at: None,
        },
    };
    Ok(no_store(response.revision, Json(response)))
}

/// `PUT /api/account/preferences` — replaces the document.
async fn replace(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    JsonBody(request): JsonBody<ReplacePreferencesRequest>,
) -> Result<Response, ApiError> {
    user.require_scope(Scope::Write)?;
    let precondition = parse_if_match(&headers)?;
    check_body_size(&headers)?;
    let document = validate_document(&request.document)?;
    let schema_version = check_schema_version(request.schema_version)?;
    write(&state, &user, &document, schema_version, precondition).await
}

/// `PATCH /api/account/preferences` — deep-merges a fragment, then validates.
async fn merge(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    JsonBody(request): JsonBody<PatchPreferencesRequest>,
) -> Result<Response, ApiError> {
    user.require_scope(Scope::Write)?;
    let precondition = parse_if_match(&headers)?;
    check_body_size(&headers)?;
    let patch = request.patch.as_object().ok_or_else(|| {
        ApiError::bad_request(
            "invalid_preferences",
            "The preference patch must be a JSON object",
        )
    })?;

    let current = load(&state, user.id).await?;
    let mut merged = match &current {
        Some(stored) => stored.document.as_object().cloned().unwrap_or_default(),
        None => Map::new(),
    };
    deep_merge(&mut merged, patch);
    let document = validate_document(&Value::Object(merged))?;
    let schema_version = check_schema_version(request.schema_version)?;
    write(&state, &user, &document, schema_version, precondition).await
}

/// `DELETE /api/account/preferences` — resets the account document.
///
/// The reset is explicit and destructive by design: the row is deleted, every
/// client falls back to its defaults on the next read, and the next write
/// starts again at revision 1. Device files and local caches are untouched.
async fn reset(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    user.require_scope(Scope::Write)?;
    // An explicit revision still wins over the reset, so a stale device cannot
    // wipe a document it never saw. Without `If-Match` the reset is absolute.
    if let Some(Precondition::Revision(expected)) = parse_if_match_optional(&headers)? {
        let current = load(&state, user.id).await?;
        let revision = current.as_ref().map_or(0, |stored| stored.revision);
        if revision != expected {
            return Ok(conflict(revision));
        }
    }

    let deleted = db::query("DELETE FROM user_preferences WHERE user_id = ?")
        .bind(user.id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?
        .rows_affected();

    if deleted > 0 {
        state.events.emit(
            Event::new(actions::USER_PREFERENCES_UPDATED, user.actor())
                .target("user", user.id.to_string())
                .metadata(json!({ "reset": true, "revision": 0 })),
        );
    }

    Ok(no_store(
        0,
        Json(PreferencesResponse {
            document: None,
            revision: 0,
            schema_version: PREFERENCES_SCHEMA_VERSION,
            updated_at: None,
        }),
    ))
}

enum Precondition {
    /// `If-Match: *` — overwrite whatever is stored.
    Any,
    /// `If-Match: "3"` — only when the stored revision is still 3.
    Revision(i64),
}

async fn write(
    state: &AppState,
    user: &AuthUser,
    document: &Value,
    schema_version: i64,
    precondition: Precondition,
) -> Result<Response, ApiError> {
    let current = load(state, user.id).await?;
    let current_revision = current.as_ref().map_or(0, |stored| stored.revision);
    if let Precondition::Revision(expected) = precondition {
        if expected != current_revision {
            return Ok(conflict(current_revision));
        }
    }

    let serialized = serde_json::to_string(document).map_err(|error| {
        tracing::error!(error = %error, "could not serialise a preference document");
        ApiError::internal()
    })?;
    if serialized.len() > MAX_DOCUMENT_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "preferences_too_large",
            format!("The preference document exceeds {MAX_DOCUMENT_BYTES} bytes"),
        ));
    }

    let revision = current_revision + 1;
    let updated_at = db::db_time(db::now());
    let stored = match &current {
        Some(stored) => {
            let changed = db::query(
                "UPDATE user_preferences SET document_json = ?, schema_version = ?, \
                 revision = ?, updated_at = ? WHERE user_id = ? AND revision = ?",
            )
            .bind(&serialized)
            .bind(schema_version)
            .bind(revision)
            .bind(&updated_at)
            .bind(user.id)
            .bind(stored.revision)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?
            .rows_affected();
            if changed == 0 {
                // Another device wrote between our read and our update.
                let fresh = load(state, user.id).await?;
                return Ok(conflict(fresh.as_ref().map_or(0, |stored| stored.revision)));
            }
            StoredPreferences {
                document: document.clone(),
                schema_version,
                revision,
                updated_at,
            }
        }
        None => {
            let inserted = db::query(
                "INSERT INTO user_preferences (user_id, document_json, schema_version, revision, \
                 updated_at) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(user.id)
            .bind(&serialized)
            .bind(schema_version)
            .bind(revision)
            .bind(&updated_at)
            .execute(&state.pool)
            .await;
            match inserted {
                Ok(_) => StoredPreferences {
                    document: document.clone(),
                    schema_version,
                    revision,
                    updated_at,
                },
                // Two first writes raced; the loser retries with the winner's
                // revision instead of replacing it.
                Err(error) if is_unique_violation(&error) => {
                    let fresh = load(state, user.id).await?;
                    return Ok(conflict(fresh.as_ref().map_or(1, |stored| stored.revision)));
                }
                Err(error) => return Err(ApiError::from(error)),
            }
        }
    };

    // Audit the shape of the change: revision, key names and size. Values are
    // user preferences and may be personal; they never enter the trail.
    let keys: Vec<&str> = document
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    state.events.emit(
        Event::new(actions::USER_PREFERENCES_UPDATED, user.actor())
            .target("user", user.id.to_string())
            .metadata(json!({
                "revision": revision,
                "schemaVersion": schema_version,
                "keys": keys,
                "bytes": serialized.len(),
            })),
    );

    Ok(no_store(revision, Json(stored.response())))
}

async fn load(state: &AppState, user_id: i64) -> Result<Option<StoredPreferences>, ApiError> {
    let row = db::query(
        "SELECT document_json, schema_version, revision, updated_at \
         FROM user_preferences WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let Some(row) = row else { return Ok(None) };
    let raw: String = row.get("document_json");
    let document: Value = serde_json::from_str(&raw).map_err(|error| {
        tracing::error!(error = %error, user_id, "stored preference document is not JSON");
        ApiError::internal()
    })?;
    Ok(Some(StoredPreferences {
        document,
        schema_version: row.get("schema_version"),
        revision: row.get("revision"),
        updated_at: row.get("updated_at"),
    }))
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|database| database.is_unique_violation())
}

/// Rejects an oversized body before parsing it. The canonical document is
/// capped separately; this catches a client that sends a huge unknown blob.
fn check_body_size(headers: &HeaderMap) -> Result<(), ApiError> {
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|length| length > MAX_BODY_BYTES) {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "preferences_too_large",
            format!("The preference document exceeds {MAX_DOCUMENT_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// 409 with the current revision, so the client can resolve the conflict.
fn conflict(revision: i64) -> Response {
    let body = Json(json!({
        "error": {
            "code": "revision_conflict",
            "message": "The preference document changed on another device",
        },
        "revision": revision,
    }));
    let mut response = (StatusCode::CONFLICT, body).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

fn no_store(revision: i64, body: Json<PreferencesResponse>) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{revision}\"")) {
        headers.insert(header::ETAG, etag);
    }
    response
}

/// `If-Match` is mandatory on writes; `*` overwrites without a revision check.
fn parse_if_match(headers: &HeaderMap) -> Result<Precondition, ApiError> {
    match parse_if_match_optional(headers)? {
        Some(precondition) => Ok(precondition),
        None => Err(ApiError::new(
            StatusCode::PRECONDITION_REQUIRED,
            "precondition_required",
            "Send If-Match with the revision you read, or * to overwrite",
        )),
    }
}

fn parse_if_match_optional(headers: &HeaderMap) -> Result<Option<Precondition>, ApiError> {
    let Some(raw) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let text = raw.to_str().map_err(|_| {
        ApiError::bad_request("invalid_if_match", "If-Match must be an ASCII revision")
    })?;
    let text = text.trim();
    if text == "*" {
        return Ok(Some(Precondition::Any));
    }
    // Accept the ETag we emit (`"3"`), a bare number and a `rev-` prefix; the
    // weak prefix is tolerated because a proxy may rewrite the header.
    let text = text.strip_prefix("W/").unwrap_or(text);
    let text = text.trim_matches('"');
    let text = text.strip_prefix("rev-").unwrap_or(text);
    let revision = text.parse::<i64>().map_err(|_| {
        ApiError::bad_request(
            "invalid_if_match",
            "If-Match must be the revision returned by GET, or *",
        )
    })?;
    if revision < 0 {
        return Err(ApiError::bad_request(
            "invalid_if_match",
            "The revision cannot be negative",
        ));
    }
    Ok(Some(Precondition::Revision(revision)))
}

fn check_schema_version(version: Option<i64>) -> Result<i64, ApiError> {
    match version.unwrap_or(PREFERENCES_SCHEMA_VERSION) {
        PREFERENCES_SCHEMA_VERSION => Ok(PREFERENCES_SCHEMA_VERSION),
        other => Err(ApiError::bad_request(
            "unsupported_schema_version",
            format!("Preference schema version {other} is not supported"),
        )),
    }
}

/// Validates the allowlisted document and rebuilds it, so nothing outside the
/// schema can be stored even when the input carried extra whitespace or types.
fn validate_document(value: &Value) -> Result<Value, ApiError> {
    let object = value.as_object().ok_or_else(|| {
        ApiError::bad_request(
            "invalid_preferences",
            "The preference document must be a JSON object",
        )
    })?;
    reject_unknown_keys(object, ALLOWED_TOP_LEVEL, "")?;

    let mut canonical = Map::new();

    if let Some(theme) = object.get("theme") {
        let theme = child_object(theme, "theme")?;
        reject_unknown_keys(theme, THEME_KEYS, "theme")?;
        let mut next = Map::new();
        if let Some(selection) = theme.get("selection") {
            next.insert(
                "selection".into(),
                Value::String(selection_value(selection)?),
            );
        }
        for key in ["accent", "tint"] {
            if let Some(color) = theme.get(key) {
                next.insert(key.into(), color_value(color, &format!("theme.{key}"))?);
            }
        }
        canonical.insert("theme".into(), Value::Object(next));
    }

    if let Some(big_picture) = object.get("bigPicture") {
        let big_picture = child_object(big_picture, "bigPicture")?;
        reject_unknown_keys(big_picture, BIG_PICTURE_KEYS, "bigPicture")?;
        let mut next = Map::new();
        if let Some(layout) = big_picture.get("layout") {
            next.insert(
                "layout".into(),
                Value::String(enum_value(layout, "bigPicture.layout", LAYOUTS)?),
            );
        }
        for key in ["sound", "haptics"] {
            if let Some(flag) = big_picture.get(key) {
                let flag = flag.as_bool().ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_preference_value",
                        format!("bigPicture.{key} must be a boolean"),
                    )
                })?;
                next.insert(key.into(), Value::Bool(flag));
            }
        }
        canonical.insert("bigPicture".into(), Value::Object(next));
    }

    if let Some(library) = object.get("library") {
        let library = child_object(library, "library")?;
        reject_unknown_keys(library, LIBRARY_KEYS, "library")?;
        let mut next = Map::new();
        if let Some(view) = library.get("view") {
            next.insert(
                "view".into(),
                Value::String(enum_value(view, "library.view", LIBRARY_VIEWS)?),
            );
        }
        canonical.insert("library".into(), Value::Object(next));
    }

    if let Some(updates) = object.get("updates") {
        let updates = child_object(updates, "updates")?;
        reject_unknown_keys(updates, UPDATE_KEYS, "updates")?;
        let mut next = Map::new();
        if let Some(prompt) = updates.get("prompt") {
            next.insert(
                "prompt".into(),
                Value::String(enum_value(prompt, "updates.prompt", UPDATE_PROMPTS)?),
            );
        }
        if let Some(notifications) = updates.get("notifications") {
            next.insert(
                "notifications".into(),
                Value::String(enum_value(
                    notifications,
                    "updates.notifications",
                    UPDATE_NOTIFICATIONS,
                )?),
            );
        }
        canonical.insert("updates".into(), Value::Object(next));
    }

    let document = Value::Object(canonical);
    let bytes = serde_json::to_vec(&document).map_err(|error| {
        tracing::error!(error = %error, "could not measure a preference document");
        ApiError::internal()
    })?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "preferences_too_large",
            format!("The preference document exceeds {MAX_DOCUMENT_BYTES} bytes"),
        ));
    }
    Ok(document)
}

fn child_object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, ApiError> {
    value.as_object().ok_or_else(|| {
        ApiError::bad_request(
            "invalid_preference_value",
            format!("{path} must be a JSON object"),
        )
    })
}

fn reject_unknown_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
    path: &str,
) -> Result<(), ApiError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            let full = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            return Err(ApiError::bad_request(
                "unknown_preference_key",
                format!("Unknown preference key {full:?}"),
            ));
        }
    }
    Ok(())
}

/// Theme selections are built-in ids, `system` or `custom:<hash>`; the pattern
/// keeps URLs, paths and token-looking values out by construction.
fn selection_value(value: &Value) -> Result<String, ApiError> {
    let text = value.as_str().ok_or_else(|| {
        ApiError::bad_request(
            "invalid_preference_value",
            "theme.selection must be a string",
        )
    })?;
    let valid = !text.is_empty()
        && text.len() <= MAX_SELECTION_CHARS
        && text
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && text.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ':' | '-' | '_')
        });
    if !valid {
        return Err(ApiError::bad_request(
            "invalid_preference_value",
            "theme.selection is not a valid theme id",
        ));
    }
    Ok(text.to_string())
}

fn color_value(value: &Value, path: &str) -> Result<Value, ApiError> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let text = value.as_str().ok_or_else(|| {
        ApiError::bad_request(
            "invalid_preference_value",
            format!("{path} must be a hex colour or null"),
        )
    })?;
    let valid = text.len() >= 4
        && text.len() <= 9
        && text.starts_with('#')
        && text[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit());
    if !valid {
        return Err(ApiError::bad_request(
            "invalid_preference_value",
            format!("{path} must be a hex colour such as #1f6feb"),
        ));
    }
    Ok(Value::String(text.to_ascii_lowercase()))
}

fn enum_value(value: &Value, path: &str, allowed: &[&str]) -> Result<String, ApiError> {
    let text = value.as_str().ok_or_else(|| {
        ApiError::bad_request(
            "invalid_preference_value",
            format!("{path} must be a string"),
        )
    })?;
    if !allowed.contains(&text) {
        return Err(ApiError::bad_request(
            "invalid_preference_value",
            format!("{path} must be one of {}", allowed.join(", ")),
        ));
    }
    Ok(text.to_string())
}

fn deep_merge(base: &mut Map<String, Value>, patch: &Map<String, Value>) {
    for (key, value) in patch {
        match (base.get_mut(key), value) {
            (Some(Value::Object(current)), Value::Object(next)) => deep_merge(current, next),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}
