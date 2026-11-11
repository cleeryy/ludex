//! Library scan, roots and folder watcher routes.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::models::{
    CreateLibraryRootRequest, LibraryFolders, RootScanResponse, ScanResponse, ScanStatus,
    UpdateLibraryRootRequest,
};
use crate::roots::{self, LibraryRootView};
use crate::state::AppState;
use crate::watcher::WatcherStatus;

/// Library routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/library/scan", get(scan_status).post(scan))
        .route("/library/folders", get(folders))
        .route("/library/roots", get(list_roots).post(create_root))
        .route(
            "/library/roots/{id}",
            patch(update_root).delete(delete_root),
        )
        .route("/library/roots/{id}/scan", post(scan_root))
        .route("/library/watcher", get(watcher_status))
}

async fn scan_status(State(state): State<AppState>, _user: AuthUser) -> Json<ScanStatus> {
    Json(state.scan.read().await.clone())
}

async fn scan(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<(StatusCode, Json<ScanResponse>), ApiError> {
    user.require_editor()?;
    let started = crate::scanner::trigger(&state).await;
    Ok((StatusCode::ACCEPTED, Json(ScanResponse { started })))
}

async fn folders(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<LibraryFolders>, ApiError> {
    user.require_admin()?;
    Ok(Json(LibraryFolders {
        games_dir: state.config.games_dir.display().to_string(),
        media_dir: state.config.media_dir.display().to_string(),
        data_dir: state.config.data_dir.display().to_string(),
    }))
}

async fn list_roots(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<LibraryRootView>>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(roots::list(&state.pool).await?))
}

async fn create_root(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateLibraryRootRequest>,
) -> Result<(StatusCode, Json<LibraryRootView>), ApiError> {
    user.require_admin_access()?;
    let path = roots::validate_path(&request.path)?;
    let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM library_roots WHERE path = ?")
        .bind(&path)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if exists > 0 {
        return Err(ApiError::conflict(
            "root_exists",
            "A library root with this path already exists",
        ));
    }
    let label = match request.label.as_deref().map(str::trim) {
        Some(label) if !label.is_empty() => validate_label(label)?,
        _ => default_label(&path),
    };
    let enabled = request.enabled.unwrap_or(true);
    let kind = validate_root_kind(request.kind.as_deref())?;
    let id = roots::create(&state.pool, &label, &path, enabled, &kind).await?;
    state.events.emit(
        Event::new(actions::LIBRARY_ROOT_CREATED, user.actor())
            .target("library_root", id.to_string())
            .metadata(serde_json::json!({ "label": label, "path": path, "enabled": enabled })),
    );
    reload_watcher(&state).await;
    Ok((StatusCode::CREATED, Json(root_view(&state, id).await?)))
}

async fn update_root(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateLibraryRootRequest>,
) -> Result<Json<LibraryRootView>, ApiError> {
    user.require_admin_access()?;
    let current = roots::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("root_not_found", "Library root not found"))?;

    let label = match request.label {
        Some(label) => validate_label(label.trim())?,
        None => current.label,
    };
    let path = match request.path {
        Some(path) => roots::validate_path(&path)?,
        None => current.path.clone(),
    };
    if path != current.path {
        let duplicate = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM library_roots WHERE path = ? AND id != ?",
        )
        .bind(&path)
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
        if duplicate > 0 {
            return Err(ApiError::conflict(
                "root_exists",
                "A library root with this path already exists",
            ));
        }
    }
    let enabled = request.enabled.unwrap_or(current.enabled != 0);
    let position = request.position.unwrap_or(current.position);
    let kind = match request.kind.as_deref() {
        Some(kind) => validate_root_kind(Some(kind))?,
        None => current.kind.clone(),
    };
    if kind != current.kind {
        let versions = roots::version_count(&state.pool, id).await?;
        if versions > 0 {
            return Err(ApiError::conflict(
                "root_kind_locked",
                "Move or remove the indexed files before changing the root kind",
            ));
        }
    }

    sqlx::query(
        "UPDATE library_roots SET label = ?, path = ?, enabled = ?, position = ?, kind = ?,
            available = 1, last_error = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(&label)
    .bind(&path)
    .bind(i64::from(enabled))
    .bind(position)
    .bind(&kind)
    .bind(db::db_time(db::now()))
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    state.events.emit(
        Event::new(actions::LIBRARY_ROOT_UPDATED, user.actor())
            .target("library_root", id.to_string())
            .metadata(serde_json::json!({
                "label": label,
                "path": path,
                "enabled": enabled,
                "position": position,
            })),
    );
    reload_watcher(&state).await;
    Ok(Json(root_view(&state, id).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteRootQuery {
    #[serde(default)]
    force: Option<bool>,
}

async fn delete_root(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<DeleteRootQuery>,
) -> Result<StatusCode, ApiError> {
    user.require_admin_access()?;
    let root = roots::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("root_not_found", "Library root not found"))?;
    let versions = roots::version_count(&state.pool, id).await?;
    if versions > 0 && !query.force.unwrap_or(false) {
        return Err(ApiError::conflict(
            "root_not_empty",
            format!(
                "This root still indexes {versions} version(s); disable it or pass force=true to remove it"
            ),
        ));
    }

    // Versions stay (progress and saves are preserved) but become unreachable:
    // their files went away with the root. `root_id` becomes NULL through the
    // foreign key, so a later scan never reattributes them to another root.
    sqlx::query("UPDATE game_versions SET missing = 1, root_unavailable = 0 WHERE root_id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    sqlx::query("DELETE FROM library_roots WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;

    state.events.emit(
        Event::new(actions::LIBRARY_ROOT_DELETED, user.actor())
            .target("library_root", id.to_string())
            .metadata(serde_json::json!({
                "label": root.label,
                "path": root.path,
                "versions": versions,
            })),
    );
    reload_watcher(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn scan_root(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<(StatusCode, Json<RootScanResponse>), ApiError> {
    user.require_editor()?;
    roots::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("root_not_found", "Library root not found"))?;
    let started = crate::scanner::trigger_root(&state, id).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(RootScanResponse {
            started,
            root_id: id,
        }),
    ))
}

async fn watcher_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<WatcherStatus>, ApiError> {
    user.require_admin_access()?;
    Ok(Json(state.watcher.read().await.clone()))
}

fn validate_root_kind(kind: Option<&str>) -> Result<String, ApiError> {
    match kind.unwrap_or("game").trim().to_ascii_lowercase().as_str() {
        "game" | "archive" => Ok("game".to_string()),
        "rom" | "emulator" => Ok("rom".to_string()),
        other => Err(ApiError::bad_request(
            "invalid_root_kind",
            format!("Unknown root kind `{other}`; expected `game` or `rom`"),
        )),
    }
}

async fn reload_watcher(state: &AppState) {
    if let Some(handle) = state.watcher_handle.read().await.as_ref() {
        if !handle.reload() {
            tracing::warn!("library watcher is not accepting reloads");
        }
    }
}

async fn root_view(state: &AppState, id: i64) -> Result<LibraryRootView, ApiError> {
    let root = roots::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("root_not_found", "Library root not found"))?;
    let versions = roots::version_count(&state.pool, id).await?;
    Ok(root.to_view(versions))
}

fn validate_label(label: &str) -> Result<String, ApiError> {
    if label.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_root_label",
            "A label is required",
        ));
    }
    if label.chars().count() > 64 || label.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_root_label",
            "The label must be at most 64 characters",
        ));
    }
    Ok(label.to_string())
}

fn default_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map_or_else(|| "Library".to_string(), str::to_string)
}
