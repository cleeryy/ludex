//! Game library routes: browsing, metadata editing, progress, downloads.
//!
//! The module used to be one 3 300-line file. It is split by concern (#237):
//! [`list`] browses and reads detail, [`versions`] owns version rows and
//! deletion, [`delete`] deletes games, [`edit`] applies metadata edits,
//! [`download`] serves tickets, ranged downloads and the stream endpoints,
//! [`progress`] tracks per-user state and [`summary`] maps rows to DTOs (and
//! holds the shared SQL column lists). Archive transport lives in
//! [`crate::transfer`].

mod delete;
mod download;
mod edit;
mod list;
mod progress;
mod stream;
mod summary;
mod versions;

use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::db;
use crate::error::ApiError;
use crate::state::AppState;

pub(crate) use delete::remove_game;
pub use delete::DeleteGameResponse;
pub(crate) use download::{age_blocked, age_not_found, deny_age_restricted, game_age_rating};
pub(crate) use list::fetch_detail;
pub(crate) use summary::{summary_from_list, version_status, FROM_JOINS, LIST_COLUMNS};
pub(crate) use versions::DeleteMode;

/// Game routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/games", get(list::list))
        .route("/games/random", get(list::random))
        .route(
            "/games/{id}",
            get(list::detail)
                .patch(edit::update_game)
                .delete(delete::delete_game),
        )
        .route("/games/{id}/refresh-metadata", post(edit::refresh_metadata))
        .route(
            "/games/{id}/download-ticket",
            post(download::download_ticket),
        )
        .route("/games/{id}/download", get(download::download))
        .route("/games/{id}/versions", get(versions::list_versions))
        .route(
            "/games/{id}/versions/default",
            put(versions::set_default_version),
        )
        .route(
            "/games/{id}/versions/{version_id}",
            delete(versions::delete_version),
        )
        .route("/games/{id}/versions/latest", get(versions::latest_version))
        .route(
            "/games/{id}/versions/{version_id}/stream",
            get(download::stream_version),
        )
        .route(
            "/games/{id}/versions/{version_id}/stream/manifest",
            get(stream::stream_manifest),
        )
        .route(
            "/games/{id}/versions/{version_id}/stream/entries/{index}",
            get(stream::stream_entry),
        )
        .route(
            "/games/{id}/versions/{version_id}/local-path",
            get(download::local_path),
        )
        .route("/games/{id}/progress", put(progress::set_progress))
        .route("/games/{id}/playtime", post(progress::report_playtime))
        .route("/games/{id}/bookmark", put(progress::set_bookmark))
        .route("/games/{id}/installed", post(progress::report_installed))
        .route("/stats", get(progress::stats))
}

/// Fails with `game_not_found` unless the game exists.
async fn ensure_game(state: &AppState, id: i64) -> Result<(), ApiError> {
    let exists = db::query_scalar::<i64>("SELECT id FROM games WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}
