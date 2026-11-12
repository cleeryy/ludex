//! Social routes (#161): friends, presence, notifications and preferences.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use crate::api_keys::Scope;
use crate::auth::AuthUser;
use crate::community;
use crate::error::ApiError;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::models::{ActivityPage, ActivityQuery};
use crate::social::{
    self, Friend, FriendRequest, FriendRequestInput, FriendsPresenceResponse, FriendsResponse,
    NotificationPage, NotificationPrefs, NotificationPrefsPatch, NotificationQuery,
    NotificationReadResponse, PresenceHeartbeat, PresenceStateResponse,
};
use crate::state::AppState;

/// Friend, presence and notification routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/friends", get(list_friends))
        .route("/friends/activity", get(friends_activity))
        .route("/friends/requests", post(send_request))
        .route("/friends/requests/{id}/accept", post(accept_request))
        .route("/friends/requests/{id}/decline", post(decline_request))
        .route("/friends/requests/{id}", delete(cancel_request))
        .route("/friends/{id}", delete(remove_friend))
        .route("/blocks", get(list_blocks))
        .route("/blocks/{id}", post(block_user).delete(unblock_user))
        .route("/presence", get(own_presence).post(heartbeat))
        .route("/presence/friends", get(friends_presence))
        .route("/notifications", get(list_notifications))
        .route("/notifications/read-all", post(mark_all_read))
        .route("/notifications/{id}/read", post(mark_read))
        .route("/me/notification-prefs", get(read_prefs).patch(write_prefs))
}

async fn list_friends(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<FriendsResponse>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(social::friends(&state, user.id).await?))
}

async fn send_request(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<FriendRequestInput>,
) -> Result<(StatusCode, Json<FriendRequest>), ApiError> {
    user.require_scope(Scope::Write)?;
    if let Err(seconds) = state
        .friend_request_limits
        .check(&format!("user:{}", user.id))
    {
        return Err(ApiError::too_many_requests(
            "friend_request_rate_limited",
            "Too many friend requests; try again later",
        )
        .with_retry_after(seconds));
    }
    let request = social::request_friend(&state, user.id, &request).await?;
    Ok((StatusCode::CREATED, Json(request)))
}

async fn accept_request(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<Friend>, ApiError> {
    user.require_scope(Scope::Write)?;
    Ok(Json(social::accept_request(&state, user.id, id).await?))
}

async fn decline_request(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    social::decline_request(&state, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn cancel_request(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    social::cancel_request(&state, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_friend(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    social::remove_friend(&state, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_blocks(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<social::SocialUser>>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(social::friends(&state, user.id).await?.blocked))
}

async fn block_user(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    social::block_user(&state, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unblock_user(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    social::unblock_user(&state, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn friends_activity(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ActivityQuery>,
) -> Result<Json<ActivityPage>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(
        community::friends_feed(&state, user.id, &query).await?,
    ))
}

async fn own_presence(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<PresenceStateResponse>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(social::own_presence(&state, user.id).await?))
}

async fn heartbeat(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<PresenceHeartbeat>,
) -> Result<Json<PresenceStateResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    Ok(Json(social::heartbeat(&state, user.id, &request).await?))
}

async fn friends_presence(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<FriendsPresenceResponse>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(social::friends_presence(&state, user.id).await?))
}

async fn list_notifications(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<NotificationQuery>,
) -> Result<Json<NotificationPage>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(social::notifications(&state, user.id, &query).await?))
}

async fn mark_read(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<NotificationReadResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    Ok(Json(social::mark_read(&state, user.id, id).await?))
}

async fn mark_all_read(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<NotificationReadResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    Ok(Json(social::mark_all_read(&state, user.id).await?))
}

async fn read_prefs(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<NotificationPrefs>, ApiError> {
    user.require_scope(Scope::Read)?;
    Ok(Json(social::prefs(&state.pool, user.id).await?))
}

async fn write_prefs(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(patch): JsonBody<NotificationPrefsPatch>,
) -> Result<Json<NotificationPrefs>, ApiError> {
    user.require_scope(Scope::Write)?;
    Ok(Json(
        social::save_prefs(&state.pool, user.id, &patch).await?,
    ))
}
