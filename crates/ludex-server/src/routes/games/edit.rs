//! Metadata edits and manual-match refreshes.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use ludex_core::models::GameDetail;

use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{RefreshResponse, UpdateGameRequest};
use crate::sql::{bind_query, Bind};
use crate::state::AppState;

use super::ensure_game;
use super::list::fetch_detail;

/// Trims, drops empties and de-duplicates a label list (case-insensitive).
fn clean_labels(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .collect()
}

pub(super) async fn update_game(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    JsonBody(request): JsonBody<UpdateGameRequest>,
) -> Result<Json<GameDetail>, ApiError> {
    user.require_editor()?;
    ensure_game(&state, id).await?;

    let mut sets = Vec::new();
    let mut binds = Vec::new();
    let mut manual_fields: Vec<&str> = Vec::new();

    if let Some(title) = request.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(ApiError::bad_request(
                "invalid_title",
                "Title cannot be empty",
            ));
        }
        sets.push("title = ?");
        binds.push(Bind::Text(title.to_string()));
        manual_fields.push("title");
        if request.sort_title.is_none() {
            let derived = ludex_core::naming::parse_filename(&format!("{title}.zip")).sort_title;
            sets.push("sort_title = ?");
            binds.push(Bind::Text(if derived.trim().is_empty() {
                title.to_string()
            } else {
                derived
            }));
        }
    }
    if let Some(sort_title) = request.sort_title {
        let sort_title = sort_title.trim();
        if sort_title.is_empty() {
            return Err(ApiError::bad_request(
                "invalid_sort_title",
                "Sort title cannot be empty",
            ));
        }
        sets.push("sort_title = ?");
        binds.push(Bind::Text(sort_title.to_string()));
    }
    if let Some(description) = request.description {
        manual_fields.push("description");
        sets.push("description = ?");
        binds.push(Bind::OptText(
            description
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        ));
    }
    if let Some(release_date) = request.release_date {
        manual_fields.push("releaseDate");
        let parsed = match release_date {
            Some(value) => Some(
                db::parse_date(&value)
                    .ok_or_else(|| ApiError::bad_request("invalid_date", "Invalid release date"))?,
            ),
            None => None,
        };
        sets.push("release_date = ?");
        binds.push(Bind::OptText(parsed.map(db::db_time)));
    }
    if let Some(genres) = request.genres {
        manual_fields.push("genres");
        sets.push("genres_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(genres))));
    }
    if let Some(developers) = request.developers {
        manual_fields.push("developers");
        sets.push("developers_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(developers))));
    }
    if let Some(publishers) = request.publishers {
        manual_fields.push("publishers");
        sets.push("publishers_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(publishers))));
    }
    if let Some(tags) = request.tags {
        sets.push("tags_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(tags))));
    }
    if let Some(rating) = request.rating {
        manual_fields.push("rating");
        sets.push("rating = ?");
        binds.push(Bind::OptFloat(rating.map(|value| value.clamp(0.0, 100.0))));
    }
    if let Some(age_rating) = request.age_rating {
        manual_fields.push("ageRating");
        sets.push("age_rating = ?");
        binds.push(Bind::OptInt(age_rating.map(i64::from)));
    }
    if let Some(early_access) = request.early_access {
        sets.push("early_access = ?");
        binds.push(Bind::Int(i64::from(early_access)));
    }
    if let Some(alt_titles) = request.alt_titles {
        manual_fields.push("altTitles");
        sets.push("alt_titles_json = ?");
        binds.push(Bind::Text(db::dump_json_list(&clean_labels(alt_titles))));
    }
    if let Some(players) = request.players {
        manual_fields.push("players");
        sets.push("max_players = ?");
        binds.push(Bind::OptInt(players.map(i64::from)));
    }
    if let Some(launch) = request.launch {
        sets.push("launch_executable = ?");
        binds.push(Bind::OptText(launch.executable));
        sets.push("launch_parameters = ?");
        binds.push(Bind::OptText(launch.parameters));
        sets.push("installer_executable = ?");
        binds.push(Bind::OptText(launch.installer_executable));
        sets.push("installer_parameters = ?");
        binds.push(Bind::OptText(launch.installer_parameters));
        sets.push("uninstaller_executable = ?");
        binds.push(Bind::OptText(launch.uninstaller_executable));
        sets.push("uninstaller_parameters = ?");
        binds.push(Bind::OptText(launch.uninstaller_parameters));
    }

    if !sets.is_empty() {
        sets.push("updated_at = ?");
        binds.push(Bind::Text(db::db_time(db::now())));
        binds.push(Bind::Int(id));
        let sql = format!("UPDATE games SET {} WHERE id = ?", sets.join(", "));
        bind_query(db::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    // Remember which metadata fields were edited by hand so a later provider
    // match can preserve them unless the editor asks to replace (#135).
    if !manual_fields.is_empty() {
        merge_manual_fields(&state, id, &manual_fields).await?;
    }

    Ok(Json(fetch_detail(&state, user.id, id).await?))
}

/// Adds `fields` to `games.manual_fields_json`, keeping existing markers.
pub(crate) async fn merge_manual_fields(
    state: &AppState,
    game_id: i64,
    fields: &[&str],
) -> Result<(), ApiError> {
    let existing = db::query_scalar::<String>("SELECT manual_fields_json FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .unwrap_or_default();
    let mut list = db::parse_json_list(&existing);
    for field in fields {
        if !list.iter().any(|marked| marked == field) {
            list.push((*field).to_string());
        }
    }
    db::query("UPDATE games SET manual_fields_json = ? WHERE id = ?")
        .bind(db::dump_json_list(&list))
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

pub(super) async fn refresh_metadata(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<(StatusCode, Json<RefreshResponse>), ApiError> {
    user.require_editor()?;
    ensure_game(&state, id).await?;
    let state = state.clone();
    tokio::spawn(async move {
        crate::metadata::enrich_game(&state, id).await;
    });
    Ok((StatusCode::ACCEPTED, Json(RefreshResponse { queued: true })))
}
