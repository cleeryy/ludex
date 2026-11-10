//! Public media cache route and administrator garbage collection (#18).

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery};
use crate::media_gc::{self, MediaGcResult};
use crate::models::MediaGcQuery;
use crate::state::AppState;

/// Media routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/media/gc", post(gc))
        .route("/media/{name}", get(media))
}

async fn media(State(state): State<AppState>, ApiPath(name): ApiPath<String>) -> Response {
    crate::media::media_response(&state.config.media_dir, &name).await
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
        state.events.emit(
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
