//! Public media cache route and administrator garbage collection (#18).

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use utoipa::openapi::HttpMethod;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery};
use crate::media_gc::{self, MediaGcResult};
use crate::models::MediaGcQuery;
use crate::openapi::{query, route, Auth, Payload, RouteDoc};
use crate::state::AppState;

/// Media routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/media/gc", post(gc))
        .route("/media/{name}", get(media))
}

async fn media(State(state): State<AppState>, ApiPath(name): ApiPath<String>) -> Response {
    crate::media::media_response(&state.core.config.media_dir, &name).await
}

async fn gc(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<MediaGcQuery>,
) -> Result<Json<MediaGcResult>, ApiError> {
    user.require_admin_access()?;
    let dry_run = query.dry_run.unwrap_or(true);
    let result = media_gc::run(&state, dry_run).await?;
    if !dry_run && result.deleted_files > 0 {
        state.core.events.emit(
            Event::new(actions::MEDIA_GC_RUN, user.actor())
                .target("media_cache", "gc")
                .metadata(serde_json::json!({
                    "deletedFiles": result.deleted_files,
                    "deletedBytes": result.deleted_bytes,
                    "orphanFiles": result.orphan_files,
                    "orphanBytes": result.orphan_bytes,
                })),
        );
    }
    Ok(Json(result))
}

/// OpenAPI operations served by this module (#237).
pub(crate) const DOCS: &[RouteDoc] = &[
    // Media
    query(
        route(
            HttpMethod::Post,
            "/media/gc",
            "Media",
            "Garbage-collect unused media",
            Auth::Admin,
            Payload::Object,
        ),
        &[("graceHours", "Minimum age before deletion")],
    ),
    route(
        HttpMethod::Get,
        "/media/{name}",
        "Media",
        "Cached cover or background",
        Auth::Public,
        Payload::Binary,
    ),
];
