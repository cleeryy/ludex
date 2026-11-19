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
use utoipa::openapi::HttpMethod;

use crate::db;
use crate::error::ApiError;
use crate::openapi::{body, ok, query, route, Auth, Payload, RouteDoc};
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
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}

/// OpenAPI operations served by this module (#237).
pub(crate) const DOCS: &[RouteDoc] = &[
    // Games
    query(
        route(
            HttpMethod::Get,
            "/games",
            "Games",
            "List or search the library",
            Auth::Session,
            Payload::Schema("GameList"),
        ),
        &[
            ("q", "Title substring"),
            ("genre", "Exact genre"),
            ("state", "Progress state"),
            ("sort", "Sort column"),
            ("order", "asc or desc"),
            ("page", "1-based page"),
            ("limit", "Page size"),
        ],
    ),
    query(
        route(
            HttpMethod::Get,
            "/games/random",
            "Games",
            "Pick a random game",
            Auth::Session,
            Payload::Schema("GameSummary"),
        ),
        &[
            ("q", "Title substring"),
            ("genre", "Exact genre"),
            ("state", "Progress state"),
        ],
    ),
    route(
        HttpMethod::Get,
        "/games/{id}",
        "Games",
        "Game detail",
        Auth::Session,
        Payload::Schema("GameDetail"),
    ),
    body(
        route(
            HttpMethod::Patch,
            "/games/{id}",
            "Games",
            "Edit game metadata",
            Auth::Editor,
            Payload::Schema("GameDetail"),
        ),
        "UpdateGameRequest",
    ),
    query(
        route(
            HttpMethod::Delete,
            "/games/{id}",
            "Games",
            "Remove a game from the library (#209, #210)",
            Auth::Admin,
            Payload::Schema("DeleteGameResponse"),
        ),
        &[
            (
                "mode",
                "`library` deletes the rows and keeps the files (editor); `archives` deletes the files under the library root, then the rows (admin)",
            ),
            ("keepFiles", "Legacy spelling of mode=library"),
            ("confirm", "Must be true; deletion is never implicit"),
        ],
    ),
    route(
        HttpMethod::Post,
        "/games/{id}/refresh-metadata",
        "Games",
        "Queue a metadata refresh",
        Auth::Editor,
        Payload::Schema("RefreshResponse"),
    ),
    query(
        route(
            HttpMethod::Get,
            "/games/{id}/metadata/candidates",
            "Games",
            "Search metadata providers for a manual match",
            Auth::Editor,
            Payload::Schema("MetadataCandidatesResponse"),
        ),
        &[
            ("q", "Search text; defaults to the game title"),
            ("provider", "Restrict to one provider slug"),
            ("year", "Preferred release year"),
        ],
    ),
    ok(
        body(
            route(
                HttpMethod::Post,
                "/games/{id}/metadata/apply",
                "Games",
                "Apply one provider record to a game",
                Auth::Editor,
                Payload::Schema("GameDetail"),
            ),
            "ApplyMetadataRequest",
        ),
        200,
    ),
    body(
        route(
            HttpMethod::Post,
            "/games/{id}/download-ticket",
            "Games",
            "Issue a short-lived download ticket",
            Auth::Session,
            Payload::Schema("TicketResponse"),
        ),
        "TicketRequest",
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/download",
        "Games",
        "Download the archive (ticket or Bearer)",
        Auth::Public,
        Payload::Binary,
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions",
        "Games",
        "List every version with its archive state (editor)",
        Auth::Editor,
        Payload::Schema("GameVersionsResponse"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/games/{id}/versions/default",
            "Games",
            "Pin the version used by downloads and updates (editor)",
            Auth::Editor,
            Payload::Schema("GameVersionsResponse"),
        ),
        "SetDefaultVersionRequest",
    ),
    query(
        route(
            HttpMethod::Delete,
            "/games/{id}/versions/{version_id}",
            "Games",
            "Delete one archive while keeping the game (editor)",
            Auth::Editor,
            Payload::Schema("DeleteVersionResponse"),
        ),
        &[(
            "confirm",
            "Must be true; the game, its metadata and its saves are kept",
        )],
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/latest",
        "Games",
        "Preferred version for update checks",
        Auth::Session,
        Payload::Schema("LatestVersion"),
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/{version_id}/stream",
        "Games",
        "Stream the archive as a tar",
        Auth::Public,
        Payload::Binary,
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/{version_id}/stream/manifest",
        "Games",
        "Ordered ZIP manifest for resumable stream installs",
        Auth::Public,
        Payload::Schema("StreamManifest"),
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/{version_id}/stream/entries/{index}",
        "Games",
        "Raw content of one archive entry (single Range; ?restart=1 restarts a compressed entry)",
        Auth::Public,
        Payload::Binary,
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/versions/{version_id}/local-path",
        "Games",
        "Local install path (desktop local mode)",
        Auth::Session,
        Payload::Schema("LocalPathResponse"),
    ),
    body(
        route(
            HttpMethod::Put,
            "/games/{id}/progress",
            "Games",
            "Set progress state",
            Auth::Session,
            Payload::Schema("ProgressUpdate"),
        ),
        "ProgressRequest",
    ),
    body(
        route(
            HttpMethod::Post,
            "/games/{id}/playtime",
            "Games",
            "Add played minutes",
            Auth::Session,
            Payload::Schema("ProgressUpdate"),
        ),
        "PlaytimeRequest",
    ),
    body(
        route(
            HttpMethod::Put,
            "/games/{id}/bookmark",
            "Games",
            "Bookmark or unbookmark",
            Auth::Session,
            Payload::Schema("BookmarkResponse"),
        ),
        "BookmarkRequest",
    ),
    route(
        HttpMethod::Post,
        "/games/{id}/installed",
        "Games",
        "Report a desktop install",
        Auth::Session,
        Payload::Schema("InstalledReportResponse"),
    ),
    query(
        route(
            HttpMethod::Get,
            "/stats",
            "Games",
            "Library statistics",
            Auth::Session,
            Payload::Schema("Stats"),
        ),
        &[],
    ),
];
