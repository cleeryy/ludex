//! Community: opt-in public profiles, member directory and activity feed.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use ludex_core::models::GameSummary;

use crate::api_keys::Scope;
use crate::auth::AuthUser;
use crate::community;
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::models::{
    ActivityPage, ActivityQuery, PublicProfile, PublicProfileResponse, PublicStats,
    UpdateProfileRequest,
};
use crate::routes::games::{summary_from_list, FROM_JOINS, LIST_COLUMNS};
use crate::sql::{bind_query, bind_query_as, Bind};
use crate::state::AppState;

/// Row of the `users` table with the community columns.
#[derive(Debug, sqlx::FromRow)]
struct ProfileRow {
    id: i64,
    username: String,
    display_name: Option<String>,
    bio: Option<String>,
    avatar_path: Option<String>,
    public_profile: i64,
    created_at: String,
}

impl ProfileRow {
    fn to_public(&self) -> PublicProfile {
        PublicProfile {
            id: self.id,
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            bio: self.bio.clone(),
            avatar_url: self
                .avatar_path
                .as_deref()
                .filter(|path| !path.is_empty())
                .map(crate::media::media_url),
            public_profile: self.public_profile != 0,
            created_at: crate::db::parse_time(&self.created_at).unwrap_or_else(chrono::Utc::now),
        }
    }
}

const PROFILE_COLUMNS: &str =
    "id, username, display_name, bio, avatar_path, public_profile, created_at";

/// Community routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me/profile", get(own_profile).patch(update_profile))
        .route("/community/feed", get(feed))
        .route("/community/users", get(directory))
        .route("/community/users/{id}", get(profile))
        .route("/community/users/{id}/activity", get(profile_activity))
}

async fn own_profile(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<PublicProfile>, ApiError> {
    let row = load_profile(&state, user.id).await?;
    Ok(Json(row.to_public()))
}

async fn update_profile(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<UpdateProfileRequest>,
) -> Result<Json<PublicProfile>, ApiError> {
    user.require_scope(Scope::Write)?;
    let mut sets = Vec::new();
    let mut binds = Vec::new();

    if let Some(public_profile) = request.public_profile {
        sets.push("public_profile = ?");
        binds.push(Bind::Int(i64::from(public_profile)));
    }
    if let Some(display_name) = request.display_name {
        sets.push("display_name = ?");
        binds.push(Bind::OptText(clean_text(
            display_name,
            64,
            "invalid_display_name",
            "Display names are limited to 64 characters",
        )?));
    }
    if let Some(bio) = request.bio {
        sets.push("bio = ?");
        binds.push(Bind::OptText(clean_text(
            bio,
            500,
            "invalid_bio",
            "Bios are limited to 500 characters",
        )?));
    }

    if !sets.is_empty() {
        sets.push("updated_at = ?");
        binds.push(Bind::Text(db::db_time(db::now())));
        binds.push(Bind::Int(user.id));
        let sql = format!("UPDATE users SET {} WHERE id = ?", sets.join(", "));
        bind_query(db::query(&sql), &binds)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    }

    let row = load_profile(&state, user.id).await?;
    Ok(Json(row.to_public()))
}

async fn directory(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<PublicProfile>>, ApiError> {
    user.require_scope(Scope::Read)?;
    let rows = db::query_as::<ProfileRow>(&format!(
        "SELECT {PROFILE_COLUMNS} FROM users WHERE public_profile = 1 ORDER BY lower(username) ASC"
    ))
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(rows.iter().map(ProfileRow::to_public).collect()))
}

async fn feed(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ActivityQuery>,
) -> Result<Json<ActivityPage>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(community::feed(&state, None, &query).await?))
}

async fn profile(
    State(state): State<AppState>,
    viewer: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<PublicProfileResponse>, ApiError> {
    viewer.require_scope(Scope::Read)?;
    let row = load_profile(&state, id).await?;
    if row.public_profile == 0 && row.id != viewer.id {
        return Err(ApiError::not_found(
            "profile_not_found",
            "Profile not found",
        ));
    }

    let (games, completed, minutes_played) = db::query_as::<(i64, i64, i64)>(
        "SELECT COUNT(*),
                CAST(COALESCE(SUM(CASE WHEN state = 'completed' THEN 1 ELSE 0 END), 0) AS BIGINT),
                CAST(COALESCE(SUM(minutes_played), 0) AS BIGINT)
         FROM progress WHERE user_id = ?",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let bookmarked = db::query_scalar::<i64>("SELECT COUNT(*) FROM bookmarks WHERE user_id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let collections = super::collections::collections_for(&state, viewer.id, id).await?;

    // The library is the account's own shelf: progress or bookmark rows. The
    // summary fields carry that account's progress, not the viewer's.
    let sql = format!(
        "SELECT {LIST_COLUMNS}{FROM_JOINS}
         WHERE p.user_id IS NOT NULL OR b.user_id IS NOT NULL
         ORDER BY p.last_played_at DESC, g.sort_title ASC, g.id ASC
         LIMIT 60"
    );
    let library: Vec<GameSummary> = bind_query_as(
        db::query_as::<crate::models::GameListRow>(&sql),
        &[Bind::Int(id), Bind::Int(id)],
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?
    .into_iter()
    .map(summary_from_list)
    .collect();

    Ok(Json(PublicProfileResponse {
        user: row.to_public(),
        stats: PublicStats {
            games,
            completed,
            minutes_played,
            bookmarked,
            collections: i64::try_from(collections.len()).unwrap_or(i64::MAX),
        },
        library,
        collections,
    }))
}

async fn profile_activity(
    State(state): State<AppState>,
    viewer: AuthUser,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(query): ApiQuery<ActivityQuery>,
) -> Result<Json<ActivityPage>, ApiError> {
    viewer.require_scope(Scope::Read)?;
    let row = load_profile(&state, id).await?;
    if row.public_profile == 0 && row.id != viewer.id {
        return Err(ApiError::not_found(
            "profile_not_found",
            "Profile not found",
        ));
    }
    Ok(Json(community::feed(&state, Some(id), &query).await?))
}

async fn load_profile(state: &AppState, id: i64) -> Result<ProfileRow, ApiError> {
    db::query_as::<ProfileRow>(&format!("SELECT {PROFILE_COLUMNS} FROM users WHERE id = ?"))
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("profile_not_found", "Profile not found"))
}

fn clean_text(
    value: Option<String>,
    max: usize,
    code: &'static str,
    message: &'static str,
) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(code, message));
    }
    Ok(Some(value.to_string()))
}
