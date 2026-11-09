//! Public media cache route.

use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use crate::extract::ApiPath;
use crate::state::AppState;

/// Media route.
pub fn router() -> Router<AppState> {
    Router::new().route("/media/{name}", get(media))
}

async fn media(State(state): State<AppState>, ApiPath(name): ApiPath<String>) -> Response {
    crate::media::media_response(&state.config.media_dir, &name).await
}
