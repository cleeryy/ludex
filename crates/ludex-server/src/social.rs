//! Community phase 2 (#161): friendships, presence and notifications.
//!
//! Privacy invariants enforced here, not in the UI:
//!
//! * requests target an exact username or id — there is no member search;
//! * presence leaves the server only for accepted friends of an account that
//!   opted in (`users.presence_enabled`), and a row older than
//!   [`PRESENCE_TTL_SECONDS`] reads as offline with no game attached;
//! * a block removes the friendship, stops requests in both directions and
//!   hides the pair's notifications;
//! * notifications are per recipient, honour a per-kind preference, are
//!   pruned on write, and are never written for the actor's own action.
//!
//! An account only appears in the friends activity feed when it opts in with
//! `users.friends_activity`; that is independent from the public profile.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::audit::actions;
use crate::db::{self, Database};
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::sql::{bind_query_as, Bind};
use crate::state::AppState;

/// A presence row older than this reads as offline.
pub const PRESENCE_TTL_SECONDS: i64 = 120;
/// Suggested client polling interval for the friends presence strip.
pub const PRESENCE_POLL_SECONDS: i64 = 30;
/// Notifications older than this are dropped on the next write.
pub const NOTIFICATION_RETENTION_DAYS: i64 = 90;
/// Newest notifications kept per account.
pub const NOTIFICATION_KEEP_PER_USER: i64 = 200;
/// Friend requests one account may send per hour.
pub const FRIEND_REQUEST_LIMIT_PER_HOUR: u32 = 20;
/// Upper bound for friend lists and notification fan-out.
const MAX_FRIENDS: i64 = 500;
/// Incoming/outgoing request lists stay small; the page shows them all.
const MAX_REQUESTS: i64 = 100;
/// Notifications returned per page.
const MAX_NOTIFICATIONS: i64 = 50;

/// Stable notification kinds.
pub mod notification_kinds {
    /// Someone sent the recipient a friend request.
    pub const FRIEND_REQUEST: &str = "friend_request";
    /// The recipient's friend request was accepted.
    pub const FRIEND_ACCEPTED: &str = "friend_accepted";
    /// A friend unlocked an achievement (double opt-in).
    pub const FRIEND_ACHIEVEMENT: &str = "friend_achievement";
    /// A friend published a collection.
    pub const COLLECTION_SHARE: &str = "collection_share";
}

/* --------------------------------- models -------------------------------- */

/// Minimal account shape used by social responses. Never carries an email.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SocialUser {
    /// Account id.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Public display name, when set.
    pub display_name: Option<String>,
    /// Avatar URL, when set.
    pub avatar_url: Option<String>,
}

impl SocialUser {
    fn from_row(row: &SocialUserRow) -> Self {
        Self {
            id: row.id,
            username: row.username.clone(),
            display_name: row.display_name.clone(),
            avatar_url: avatar_url(row.avatar_path.as_deref()),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct SocialUserRow {
    id: i64,
    username: String,
    display_name: Option<String>,
    avatar_path: Option<String>,
}

/// Game attached to a presence row; friends only ever see id and title.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FriendGame {
    /// Game id.
    pub id: i64,
    /// Game title.
    pub title: String,
}

/// Presence of one friend, already filtered for visibility.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FriendPresence {
    /// `online`, `in_game` or `offline`.
    pub state: String,
    /// Game being played, only while `state` is `in_game`.
    pub game: Option<FriendGame>,
    /// When the account was last seen. `null` when it never checked in.
    pub last_seen: Option<DateTime<Utc>>,
}

/// One accepted friendship.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Friend {
    /// The other account.
    pub user: SocialUser,
    /// When the friendship was accepted.
    pub since: DateTime<Utc>,
    /// Presence, only when the friend opted in.
    pub presence: Option<FriendPresence>,
}

/// One pending friend request.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FriendRequest {
    /// Request id, used by accept/decline/cancel.
    pub id: i64,
    /// The other account.
    pub user: SocialUser,
    /// When the request was sent.
    pub created_at: DateTime<Utc>,
}

/// `GET /api/friends` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FriendsResponse {
    /// Accepted friends, alphabetical.
    pub friends: Vec<Friend>,
    /// Requests waiting for this account.
    pub incoming: Vec<FriendRequest>,
    /// Requests this account sent.
    pub outgoing: Vec<FriendRequest>,
    /// Accounts this account blocked.
    pub blocked: Vec<SocialUser>,
}

/// `POST /api/friends/requests` body.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FriendRequestInput {
    /// Exact login name (case-insensitive).
    #[serde(default)]
    pub username: Option<String>,
    /// Account id, an alternative to `username`.
    #[serde(default)]
    pub user_id: Option<i64>,
}

/// Presence entry of the lightweight polling endpoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FriendPresenceEntry {
    /// Friend account id.
    pub user_id: i64,
    /// `online`, `in_game` or `offline`.
    pub state: String,
    /// Game being played, only while `state` is `in_game`.
    pub game: Option<FriendGame>,
    /// Last presence update.
    pub last_seen: Option<DateTime<Utc>>,
}

/// `GET /api/presence/friends` response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FriendsPresenceResponse {
    /// Online friends of opted-in accounts, at most 500.
    pub friends: Vec<FriendPresenceEntry>,
    /// Suggested polling interval, in seconds.
    pub poll_after_seconds: i64,
}

/// Own presence and opt-in state.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresenceStateResponse {
    /// Whether the account opted into presence.
    pub enabled: bool,
    /// Current state; `null` when presence is disabled.
    pub state: Option<String>,
    /// Game being played, when `state` is `in_game`.
    pub game: Option<FriendGame>,
    /// Last presence update.
    pub last_seen: Option<DateTime<Utc>>,
    /// Rows older than this read as offline.
    pub stale_after_seconds: i64,
}

/// `POST /api/presence` body.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresenceHeartbeat {
    /// `online` or `in_game`; defaults to `online`.
    #[serde(default)]
    pub state: Option<String>,
    /// Game being played; required for `in_game`.
    #[serde(default)]
    pub game_id: Option<i64>,
}

/// Achievement summary attached to a notification.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationAchievement {
    /// Achievement row id.
    pub id: i64,
    /// Stable key.
    pub key: String,
    /// Display name.
    pub name: String,
    /// Points.
    pub points: i64,
}

/// One in-app notification.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    /// Row id, also the pagination cursor.
    pub id: i64,
    /// `friend_request`, `friend_accepted`, `friend_achievement` or
    /// `collection_share`.
    pub kind: String,
    /// When it happened.
    pub created_at: DateTime<Utc>,
    /// When the recipient read it; `null` while unread.
    pub read_at: Option<DateTime<Utc>>,
    /// The account that caused it, when it still exists.
    pub actor: Option<SocialUser>,
    /// Related game, when it still exists.
    pub game: Option<FriendGame>,
    /// Related achievement, for `friend_achievement`.
    pub achievement: Option<NotificationAchievement>,
    /// Extra structured detail (achievement name, collection id and name).
    pub payload: Value,
}

/// One page of notifications.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPage {
    /// Newest-first notifications.
    pub items: Vec<Notification>,
    /// Pass as `before` for the next page; `null` on the last page.
    pub next_before: Option<i64>,
    /// Unread notifications for the account, independent of the page.
    pub unread_count: i64,
}

/// Per-kind notification preferences.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPrefs {
    /// Friend requests (default on).
    pub friend_requests: bool,
    /// Accepted requests (default on).
    pub friend_accepted: bool,
    /// Friends' achievement unlocks (opt-in, default off).
    pub friend_achievements: bool,
    /// Friends publishing collections (default on).
    pub collection_shares: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        Self {
            friend_requests: true,
            friend_accepted: true,
            friend_achievements: false,
            collection_shares: true,
        }
    }
}

impl NotificationPrefs {
    fn allows(self, kind: &str) -> bool {
        match kind {
            notification_kinds::FRIEND_REQUEST => self.friend_requests,
            notification_kinds::FRIEND_ACCEPTED => self.friend_accepted,
            notification_kinds::FRIEND_ACHIEVEMENT => self.friend_achievements,
            notification_kinds::COLLECTION_SHARE => self.collection_shares,
            _ => false,
        }
    }
}

/// `PATCH /api/me/notification-prefs` body.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPrefsPatch {
    /// Friend requests.
    #[serde(default)]
    pub friend_requests: Option<bool>,
    /// Accepted requests.
    #[serde(default)]
    pub friend_accepted: Option<bool>,
    /// Friends' achievement unlocks.
    #[serde(default)]
    pub friend_achievements: Option<bool>,
    /// Friends publishing collections.
    #[serde(default)]
    pub collection_shares: Option<bool>,
}

/// Result of marking one or all notifications read.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationReadResponse {
    /// Rows actually changed.
    pub updated: i64,
    /// Unread notifications left.
    pub unread_count: i64,
}

/// Query string for the notification list.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationQuery {
    /// Page size, 1..=50 (default 20).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Return rows with `id < before`.
    #[serde(default)]
    pub before: Option<i64>,
}

/* --------------------------------- helpers -------------------------------- */

fn avatar_url(path: Option<&str>) -> Option<String> {
    path.filter(|value| !value.is_empty())
        .map(crate::media::media_url)
}

/// Canonical unordered account pair, used by the unique friendship index.
#[must_use]
pub fn pair_key(first: i64, second: i64) -> String {
    let (low, high) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    format!("{low}:{high}")
}

fn parse_or_now(value: &str) -> DateTime<Utc> {
    db::parse_time(value).unwrap_or_else(Utc::now)
}

/* -------------------------------- friendships ----------------------------- */

async fn load_user(pool: &Database, id: i64) -> Result<SocialUserRow, ApiError> {
    db::query_as::<SocialUserRow>(
        "SELECT id, username, display_name, avatar_path FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("user_not_found", "User not found"))
}

async fn find_user(pool: &Database, input: &FriendRequestInput) -> Result<SocialUserRow, ApiError> {
    if let Some(id) = input.user_id {
        return load_user(pool, id).await;
    }
    let username = input
        .username
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_friend_request",
                "Provide an exact username or a user id",
            )
        })?;
    db::query_as::<SocialUserRow>(
        "SELECT id, username, display_name, avatar_path FROM users
         WHERE lower(username) = lower(?) LIMIT 1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| {
        ApiError::not_found(
            "user_not_found",
            "No account with that exact username exists",
        )
    })
}

async fn is_blocked(pool: &Database, blocker: i64, blocked: i64) -> Result<bool, ApiError> {
    let count = db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM user_blocks WHERE blocker_id = ? AND blocked_id = ?",
    )
    .bind(blocker)
    .bind(blocked)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(count > 0)
}

async fn friendship_id(pool: &Database, first: i64, second: i64) -> Result<Option<i64>, ApiError> {
    db::query_scalar::<i64>("SELECT id FROM friendships WHERE pair_key = ?")
        .bind(pair_key(first, second))
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)
}

/// Loads the friend list, requests and blocks of one account.
///
/// # Errors
///
/// Propagates database failures.
pub async fn friends(state: &AppState, viewer_id: i64) -> Result<FriendsResponse, ApiError> {
    let now = db::now();
    let friend_sql = "SELECT f.updated_at AS since,
                u.id AS user_id, u.username, u.display_name, u.avatar_path,
                u.presence_enabled,
                p.state AS presence_state, p.game_id AS presence_game_id,
                g.title AS presence_game_title, p.updated_at AS presence_updated_at
         FROM friendships f
         JOIN users u ON u.id = CASE WHEN f.requester_id = ? THEN f.addressee_id ELSE f.requester_id END
         LEFT JOIN presence p ON p.user_id = u.id
         LEFT JOIN games g ON g.id = p.game_id
         WHERE f.status = 'accepted' AND (f.requester_id = ? OR f.addressee_id = ?)
         ORDER BY lower(u.username) ASC, u.id ASC
         LIMIT ?";
    let rows = bind_query_as(
        db::query_as::<FriendRow>(friend_sql),
        &[
            Bind::Int(viewer_id),
            Bind::Int(viewer_id),
            Bind::Int(viewer_id),
            Bind::Int(MAX_FRIENDS),
        ],
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let friends = rows
        .iter()
        .map(|row| Friend {
            user: SocialUser {
                id: row.user_id,
                username: row.username.clone(),
                display_name: row.display_name.clone(),
                avatar_url: avatar_url(row.avatar_path.as_deref()),
            },
            since: parse_or_now(&row.since),
            presence: presence_view(
                row.presence_enabled != 0,
                row.presence_state.as_deref(),
                row.presence_game_id,
                row.presence_game_title.clone(),
                row.presence_updated_at.as_deref(),
                now,
            ),
        })
        .collect();

    let incoming = request_rows(state, viewer_id, false).await?;
    let outgoing = request_rows(state, viewer_id, true).await?;

    let blocked_rows = db::query_as::<SocialUserRow>(
        "SELECT u.id, u.username, u.display_name, u.avatar_path
         FROM user_blocks b JOIN users u ON u.id = b.blocked_id
         WHERE b.blocker_id = ? ORDER BY lower(u.username) ASC",
    )
    .bind(viewer_id)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(FriendsResponse {
        friends,
        incoming,
        outgoing,
        blocked: blocked_rows.iter().map(SocialUser::from_row).collect(),
    })
}

async fn request_rows(
    state: &AppState,
    viewer_id: i64,
    outgoing: bool,
) -> Result<Vec<FriendRequest>, ApiError> {
    let sql = if outgoing {
        "SELECT f.id, f.created_at,
                u.id AS user_id, u.username, u.display_name, u.avatar_path
         FROM friendships f JOIN users u ON u.id = f.addressee_id
         WHERE f.requester_id = ? AND f.status = 'pending'
         ORDER BY f.id DESC LIMIT ?"
    } else {
        "SELECT f.id, f.created_at,
                u.id AS user_id, u.username, u.display_name, u.avatar_path
         FROM friendships f JOIN users u ON u.id = f.requester_id
         WHERE f.addressee_id = ? AND f.status = 'pending'
         ORDER BY f.id DESC LIMIT ?"
    };
    let rows = bind_query_as(
        db::query_as::<RequestRow>(sql),
        &[Bind::Int(viewer_id), Bind::Int(MAX_REQUESTS)],
    )
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows
        .iter()
        .map(|row| FriendRequest {
            id: row.id,
            user: SocialUser {
                id: row.user_id,
                username: row.username.clone(),
                display_name: row.display_name.clone(),
                avatar_url: avatar_url(row.avatar_path.as_deref()),
            },
            created_at: parse_or_now(&row.created_at),
        })
        .collect())
}

#[derive(Debug, sqlx::FromRow)]
struct FriendRow {
    since: String,
    user_id: i64,
    username: String,
    display_name: Option<String>,
    avatar_path: Option<String>,
    presence_enabled: i64,
    presence_state: Option<String>,
    presence_game_id: Option<i64>,
    presence_game_title: Option<String>,
    presence_updated_at: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct RequestRow {
    id: i64,
    created_at: String,
    user_id: i64,
    username: String,
    display_name: Option<String>,
    avatar_path: Option<String>,
}

/// Sends a friend request by exact username or id.
///
/// # Errors
///
/// 400 `invalid_friend_request` for a self-request, 404 when the account does
/// not exist (or blocked the caller), 403 when the caller blocked the target,
/// 409 when a relationship already exists.
pub async fn request_friend(
    state: &AppState,
    viewer_id: i64,
    input: &FriendRequestInput,
) -> Result<FriendRequest, ApiError> {
    let target = find_user(&state.pool, input).await?;
    if target.id == viewer_id {
        return Err(ApiError::bad_request(
            "invalid_friend_request",
            "You cannot send a friend request to yourself",
        ));
    }
    // A block by the target is indistinguishable from a missing account; a
    // block by the caller is explicit so the UI can offer to unblock.
    if is_blocked(&state.pool, target.id, viewer_id).await? {
        return Err(ApiError::not_found("user_not_found", "User not found"));
    }
    if is_blocked(&state.pool, viewer_id, target.id).await? {
        return Err(ApiError::forbidden(
            "friendship_blocked",
            "Unblock this account before sending a friend request",
        ));
    }
    if friendship_id(&state.pool, viewer_id, target.id)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict(
            "friend_request_exists",
            "A friendship or pending request already exists with this account",
        ));
    }

    let now = db::now();
    let stamp = db::db_time(now);
    let inserted = db::query(
        "INSERT INTO friendships
            (requester_id, addressee_id, pair_key, status, created_at, updated_at)
         VALUES (?, ?, ?, 'pending', ?, ?)
         ON CONFLICT(pair_key) DO NOTHING",
    )
    .bind(viewer_id)
    .bind(target.id)
    .bind(pair_key(viewer_id, target.id))
    .bind(&stamp)
    .bind(&stamp)
    .returning_id_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let Some(id) = inserted else {
        // The pair was created concurrently; the caller retries the GET.
        return Err(ApiError::conflict(
            "friend_request_exists",
            "A friendship or pending request already exists with this account",
        ));
    };

    notify(
        state,
        target.id,
        notification_kinds::FRIEND_REQUEST,
        Some(viewer_id),
        None,
        None,
        json!({}),
    )
    .await;
    state.events.emit(
        Event::new(actions::FRIEND_REQUESTED, Actor::User { id: viewer_id })
            .target("user", target.id.to_string())
            .metadata(json!({ "requesterId": viewer_id, "addresseeId": target.id })),
    );

    Ok(FriendRequest {
        id,
        user: SocialUser::from_row(&target),
        created_at: now,
    })
}

/// Accepts one incoming request.
///
/// # Errors
///
/// 404 `friend_request_not_found` when the request is unknown or not addressed
/// to the caller.
pub async fn accept_request(
    state: &AppState,
    viewer_id: i64,
    request_id: i64,
) -> Result<Friend, ApiError> {
    let requester_id = pending_requester(&state.pool, request_id, viewer_id, false).await?;
    let now = db::now();
    let result = db::query(
        "UPDATE friendships SET status = 'accepted', updated_at = ?
         WHERE id = ? AND addressee_id = ? AND status = 'pending'",
    )
    .bind(db::db_time(now))
    .bind(request_id)
    .bind(viewer_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "friend_request_not_found",
            "Friend request not found",
        ));
    }

    notify(
        state,
        requester_id,
        notification_kinds::FRIEND_ACCEPTED,
        Some(viewer_id),
        None,
        None,
        json!({}),
    )
    .await;
    clear_pair_notification(
        &state.pool,
        viewer_id,
        requester_id,
        notification_kinds::FRIEND_REQUEST,
    )
    .await;
    state.events.emit(
        Event::new(actions::FRIEND_ACCEPTED, Actor::User { id: viewer_id })
            .target("user", requester_id.to_string())
            .metadata(json!({ "requesterId": requester_id, "addresseeId": viewer_id })),
    );

    let row = load_user(&state.pool, requester_id).await?;
    Ok(Friend {
        user: SocialUser::from_row(&row),
        since: now,
        presence: None,
    })
}

/// Declines one incoming request.
///
/// # Errors
///
/// 404 when the request is unknown or not addressed to the caller.
pub async fn decline_request(
    state: &AppState,
    viewer_id: i64,
    request_id: i64,
) -> Result<(), ApiError> {
    let requester_id = pending_requester(&state.pool, request_id, viewer_id, false).await?;
    let result = db::query(
        "DELETE FROM friendships WHERE id = ? AND addressee_id = ? AND status = 'pending'",
    )
    .bind(request_id)
    .bind(viewer_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "friend_request_not_found",
            "Friend request not found",
        ));
    }
    // A declined request must not linger in the recipient's notification list.
    clear_pair_notification(
        &state.pool,
        viewer_id,
        requester_id,
        notification_kinds::FRIEND_REQUEST,
    )
    .await;
    Ok(())
}

/// Cancels one outgoing request.
///
/// # Errors
///
/// 404 when the request is unknown or not sent by the caller.
pub async fn cancel_request(
    state: &AppState,
    viewer_id: i64,
    request_id: i64,
) -> Result<(), ApiError> {
    let addressee_id = pending_requester(&state.pool, request_id, viewer_id, true).await?;
    let result = db::query(
        "DELETE FROM friendships WHERE id = ? AND requester_id = ? AND status = 'pending'",
    )
    .bind(request_id)
    .bind(viewer_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "friend_request_not_found",
            "Friend request not found",
        ));
    }
    clear_pair_notification(
        &state.pool,
        addressee_id,
        viewer_id,
        notification_kinds::FRIEND_REQUEST,
    )
    .await;
    Ok(())
}

/// Returns the other side of a pending request, or 404.
async fn pending_requester(
    pool: &Database,
    request_id: i64,
    viewer_id: i64,
    outgoing: bool,
) -> Result<i64, ApiError> {
    let (requester, addressee) = db::query_as::<(i64, i64)>(
        "SELECT requester_id, addressee_id FROM friendships
         WHERE id = ? AND status = 'pending'",
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("friend_request_not_found", "Friend request not found"))?;
    if outgoing {
        if requester != viewer_id {
            return Err(ApiError::not_found(
                "friend_request_not_found",
                "Friend request not found",
            ));
        }
        Ok(addressee)
    } else {
        if addressee != viewer_id {
            return Err(ApiError::not_found(
                "friend_request_not_found",
                "Friend request not found",
            ));
        }
        Ok(requester)
    }
}

/// Removes an accepted friendship. Either side may call this.
///
/// # Errors
///
/// 404 when the accounts are not friends.
pub async fn remove_friend(
    state: &AppState,
    viewer_id: i64,
    friend_id: i64,
) -> Result<(), ApiError> {
    let result = db::query(
        "DELETE FROM friendships
         WHERE status = 'accepted'
           AND ((requester_id = ? AND addressee_id = ?)
                OR (requester_id = ? AND addressee_id = ?))",
    )
    .bind(viewer_id)
    .bind(friend_id)
    .bind(friend_id)
    .bind(viewer_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("friend_not_found", "Friend not found"));
    }
    state.events.emit(
        Event::new(actions::FRIEND_REMOVED, Actor::User { id: viewer_id })
            .target("user", friend_id.to_string())
            .metadata(json!({ "userId": viewer_id, "friendId": friend_id })),
    );
    Ok(())
}

/// Blocks an account: ends the friendship, removes pending requests and drops
/// notifications that mention the blocked account.
///
/// # Errors
///
/// 400 for a self-block, 404 when the account does not exist.
pub async fn block_user(state: &AppState, viewer_id: i64, target_id: i64) -> Result<(), ApiError> {
    if viewer_id == target_id {
        return Err(ApiError::bad_request(
            "invalid_block",
            "You cannot block yourself",
        ));
    }
    load_user(&state.pool, target_id).await?;
    let now = db::db_time(db::now());
    db::query(
        "INSERT INTO user_blocks (blocker_id, blocked_id, created_at) VALUES (?, ?, ?)
         ON CONFLICT(blocker_id, blocked_id) DO NOTHING",
    )
    .bind(viewer_id)
    .bind(target_id)
    .bind(&now)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    db::query(
        "DELETE FROM friendships
         WHERE (requester_id = ? AND addressee_id = ?)
            OR (requester_id = ? AND addressee_id = ?)",
    )
    .bind(viewer_id)
    .bind(target_id)
    .bind(target_id)
    .bind(viewer_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    // Notifications caused by the blocked account must not resurface.
    db::query("DELETE FROM notifications WHERE user_id = ? AND actor_id = ?")
        .bind(viewer_id)
        .bind(target_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    state.events.emit(
        Event::new(actions::FRIEND_BLOCKED, Actor::User { id: viewer_id })
            .target("user", target_id.to_string())
            .metadata(json!({ "blockerId": viewer_id, "blockedId": target_id })),
    );
    Ok(())
}

/// Removes a block. Idempotent.
///
/// # Errors
///
/// Propagates database failures.
pub async fn unblock_user(
    state: &AppState,
    viewer_id: i64,
    target_id: i64,
) -> Result<(), ApiError> {
    db::query("DELETE FROM user_blocks WHERE blocker_id = ? AND blocked_id = ?")
        .bind(viewer_id)
        .bind(target_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    state.events.emit(
        Event::new(actions::FRIEND_UNBLOCKED, Actor::User { id: viewer_id })
            .target("user", target_id.to_string())
            .metadata(json!({ "blockerId": viewer_id, "blockedId": target_id })),
    );
    Ok(())
}

/// Whether the two accounts are accepted friends.
///
/// # Errors
///
/// Propagates database failures.
pub async fn are_friends(state: &AppState, first: i64, second: i64) -> Result<bool, ApiError> {
    if first == second {
        return Ok(false);
    }
    let count = db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM friendships
         WHERE status = 'accepted' AND pair_key = ?",
    )
    .bind(pair_key(first, second))
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(count > 0)
}

/// Asks whether an account opted into the friends-only activity feed.
///
/// # Errors
///
/// Propagates database failures.
pub async fn shares_with_friends(pool: &Database, user_id: i64) -> Result<bool, ApiError> {
    let flag = db::query_scalar::<i64>("SELECT friends_activity FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .unwrap_or(0);
    Ok(flag != 0)
}

/* --------------------------------- presence ------------------------------- */

fn presence_view(
    enabled: bool,
    state: Option<&str>,
    game_id: Option<i64>,
    game_title: Option<String>,
    updated_at: Option<&str>,
    now: DateTime<Utc>,
) -> Option<FriendPresence> {
    if !enabled {
        return None;
    }
    let Some(updated) = updated_at.and_then(db::parse_time) else {
        return Some(FriendPresence {
            state: "offline".to_string(),
            game: None,
            last_seen: None,
        });
    };
    let fresh = now.signed_duration_since(updated).num_seconds() <= PRESENCE_TTL_SECONDS;
    if !fresh {
        return Some(FriendPresence {
            state: "offline".to_string(),
            game: None,
            last_seen: Some(updated),
        });
    }
    Some(FriendPresence {
        state: state.unwrap_or("online").to_string(),
        game: game_id
            .zip(game_title)
            .map(|(id, title)| FriendGame { id, title }),
        last_seen: Some(updated),
    })
}

/// Whether the account opted into presence.
///
/// # Errors
///
/// Propagates database failures.
pub async fn presence_enabled(pool: &Database, user_id: i64) -> Result<bool, ApiError> {
    let flag = db::query_scalar::<i64>("SELECT presence_enabled FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .unwrap_or(0);
    Ok(flag != 0)
}

/// Records one heartbeat for the caller.
///
/// # Errors
///
/// 400 `invalid_presence` for an unknown state or a missing game, 404
/// `game_not_found` when the reference is unknown.
pub async fn heartbeat(
    state: &AppState,
    user_id: i64,
    heartbeat: &PresenceHeartbeat,
) -> Result<PresenceStateResponse, ApiError> {
    if !presence_enabled(&state.pool, user_id).await? {
        return Ok(PresenceStateResponse {
            enabled: false,
            state: None,
            game: None,
            last_seen: None,
            stale_after_seconds: PRESENCE_TTL_SECONDS,
        });
    }
    let next_state = heartbeat.state.as_deref().unwrap_or("online");
    if !matches!(next_state, "online" | "in_game") {
        return Err(ApiError::bad_request(
            "invalid_presence",
            "Presence state must be 'online' or 'in_game'",
        ));
    }
    let game = if next_state == "in_game" {
        let game_id = heartbeat.game_id.ok_or_else(|| {
            ApiError::bad_request("invalid_presence", "in_game requires a gameId")
        })?;
        let title = db::query_scalar::<String>("SELECT title FROM games WHERE id = ?")
            .bind(game_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;
        Some(FriendGame { id: game_id, title })
    } else {
        None
    };

    let now = db::now();
    let stamp = db::db_time(now);
    db::query(
        "INSERT INTO presence (user_id, state, game_id, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET
            state = excluded.state,
            game_id = excluded.game_id,
            updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(next_state)
    .bind(game.as_ref().map(|entry| entry.id))
    .bind(&stamp)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(PresenceStateResponse {
        enabled: true,
        state: Some(next_state.to_string()),
        game,
        last_seen: Some(now),
        stale_after_seconds: PRESENCE_TTL_SECONDS,
    })
}

/// Own presence, or the disabled state.
///
/// # Errors
///
/// Propagates database failures.
pub async fn own_presence(
    state: &AppState,
    user_id: i64,
) -> Result<PresenceStateResponse, ApiError> {
    let enabled = presence_enabled(&state.pool, user_id).await?;
    if !enabled {
        return Ok(PresenceStateResponse {
            enabled: false,
            state: None,
            game: None,
            last_seen: None,
            stale_after_seconds: PRESENCE_TTL_SECONDS,
        });
    }
    let row = db::query_as::<(String, Option<i64>, Option<String>, String)>(
        "SELECT p.state, p.game_id, g.title, p.updated_at
         FROM presence p LEFT JOIN games g ON g.id = p.game_id
         WHERE p.user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(ApiError::from)?;
    let view = row.as_ref().and_then(|row| {
        presence_view(
            true,
            Some(row.0.as_str()),
            row.1,
            row.2.clone(),
            Some(row.3.as_str()),
            db::now(),
        )
    });
    Ok(PresenceStateResponse {
        enabled: true,
        state: view.as_ref().map(|presence| presence.state.clone()),
        game: view.as_ref().and_then(|presence| presence.game.clone()),
        last_seen: view.and_then(|presence| presence.last_seen),
        stale_after_seconds: PRESENCE_TTL_SECONDS,
    })
}

/// Marks the caller as in-game from a play session.
///
/// Called from progress and playtime writes; failures are logged because a
/// presence update must never fail the play report.
pub async fn touch_playing(state: &AppState, user_id: i64, game_id: i64) {
    let enabled = match presence_enabled(&state.pool, user_id).await {
        Ok(enabled) => enabled,
        Err(error) => {
            tracing::warn!(code = %error.code, message = %error.message, user_id, "could not read presence opt-in");
            return;
        }
    };
    if !enabled {
        return;
    }
    let result = db::query(
        "INSERT INTO presence (user_id, state, game_id, updated_at) VALUES (?, 'in_game', ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET
            state = excluded.state,
            game_id = excluded.game_id,
            updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(game_id)
    .bind(db::db_time(db::now()))
    .execute(&state.pool)
    .await;
    if let Err(error) = result {
        tracing::warn!(error = %error, user_id, "could not update presence");
    }
}

/// Presence of every accepted friend that opted in, for polling.
///
/// # Errors
///
/// Propagates database failures.
pub async fn friends_presence(
    state: &AppState,
    viewer_id: i64,
) -> Result<FriendsPresenceResponse, ApiError> {
    let now = db::now();
    let rows = db::query_as::<PresenceRow>(
        "SELECT u.id AS user_id, p.state, p.game_id, g.title AS game_title,
                p.updated_at
         FROM friendships f
         JOIN users u ON u.id = CASE WHEN f.requester_id = ? THEN f.addressee_id ELSE f.requester_id END
         LEFT JOIN presence p ON p.user_id = u.id
         LEFT JOIN games g ON g.id = p.game_id
         WHERE f.status = 'accepted' AND (f.requester_id = ? OR f.addressee_id = ?)
           AND u.presence_enabled = 1
         ORDER BY lower(u.username) ASC, u.id ASC
         LIMIT ?",
    )
    .bind(viewer_id)
    .bind(viewer_id)
    .bind(viewer_id)
    .bind(MAX_FRIENDS)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let friends = rows
        .iter()
        .map(|row| {
            let view = presence_view(
                true,
                row.state.as_deref(),
                row.game_id,
                row.game_title.clone(),
                row.updated_at.as_deref(),
                now,
            );
            let state = view
                .as_ref()
                .map(|presence| presence.state.clone())
                .unwrap_or_else(|| "offline".to_string());
            FriendPresenceEntry {
                user_id: row.user_id,
                state,
                game: view.as_ref().and_then(|presence| presence.game.clone()),
                last_seen: view.and_then(|presence| presence.last_seen),
            }
        })
        .collect();

    Ok(FriendsPresenceResponse {
        friends,
        poll_after_seconds: PRESENCE_POLL_SECONDS,
    })
}

#[derive(Debug, sqlx::FromRow)]
struct PresenceRow {
    user_id: i64,
    state: Option<String>,
    game_id: Option<i64>,
    game_title: Option<String>,
    updated_at: Option<String>,
}

/* ------------------------------ notifications ----------------------------- */

/// Loads the per-kind notification preferences of one account.
///
/// # Errors
///
/// Propagates database failures.
pub async fn prefs(pool: &Database, user_id: i64) -> Result<NotificationPrefs, ApiError> {
    let row = db::query_as::<(i64, i64, i64, i64)>(
        "SELECT friend_requests, friend_accepted, friend_achievements, collection_shares
         FROM notification_prefs WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(row
        .map(
            |(requests, accepted, achievements, shares)| NotificationPrefs {
                friend_requests: requests != 0,
                friend_accepted: accepted != 0,
                friend_achievements: achievements != 0,
                collection_shares: shares != 0,
            },
        )
        .unwrap_or_default())
}

/// Applies a partial preference update.
///
/// # Errors
///
/// Propagates database failures.
pub async fn save_prefs(
    pool: &Database,
    user_id: i64,
    patch: &NotificationPrefsPatch,
) -> Result<NotificationPrefs, ApiError> {
    let mut next = prefs(pool, user_id).await?;
    if let Some(value) = patch.friend_requests {
        next.friend_requests = value;
    }
    if let Some(value) = patch.friend_accepted {
        next.friend_accepted = value;
    }
    if let Some(value) = patch.friend_achievements {
        next.friend_achievements = value;
    }
    if let Some(value) = patch.collection_shares {
        next.collection_shares = value;
    }
    db::query(
        "INSERT INTO notification_prefs
            (user_id, friend_requests, friend_accepted, friend_achievements,
             collection_shares, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET
            friend_requests = excluded.friend_requests,
            friend_accepted = excluded.friend_accepted,
            friend_achievements = excluded.friend_achievements,
            collection_shares = excluded.collection_shares,
            updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(i64::from(next.friend_requests))
    .bind(i64::from(next.friend_accepted))
    .bind(i64::from(next.friend_achievements))
    .bind(i64::from(next.collection_shares))
    .bind(db::db_time(db::now()))
    .execute(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(next)
}

/// Writes one notification, honouring the recipient's preferences.
///
/// Never fails the caller's action: preference and insert errors are logged.
pub async fn notify(
    state: &AppState,
    recipient_id: i64,
    kind: &str,
    actor_id: Option<i64>,
    game_id: Option<i64>,
    achievement_id: Option<i64>,
    payload: Value,
) {
    if actor_id == Some(recipient_id) {
        return;
    }
    let prefs = match prefs(&state.pool, recipient_id).await {
        Ok(prefs) => prefs,
        Err(error) => {
            tracing::warn!(code = %error.code, message = %error.message, recipient_id, kind, "could not read notification prefs");
            return;
        }
    };
    if !prefs.allows(kind) {
        return;
    }
    if let Err(error) = insert_notification(
        &state.pool,
        recipient_id,
        kind,
        actor_id,
        game_id,
        achievement_id,
        &payload,
    )
    .await
    {
        tracing::warn!(code = %error.code, message = %error.message, recipient_id, kind, "could not write notification");
        return;
    }
    prune_notifications(&state.pool, recipient_id).await;
}

async fn insert_notification(
    pool: &Database,
    recipient_id: i64,
    kind: &str,
    actor_id: Option<i64>,
    game_id: Option<i64>,
    achievement_id: Option<i64>,
    payload: &Value,
) -> Result<(), ApiError> {
    db::query(
        "INSERT INTO notifications
            (user_id, kind, actor_id, game_id, achievement_id, payload, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(recipient_id)
    .bind(kind)
    .bind(actor_id)
    .bind(game_id)
    .bind(achievement_id)
    .bind(serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string()))
    .bind(db::db_time(db::now()))
    .execute(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(())
}

/// Drops old and surplus notifications for one account.
pub async fn prune_notifications(pool: &Database, user_id: i64) {
    let cutoff = db::db_time(db::now() - Duration::days(NOTIFICATION_RETENTION_DAYS));
    if let Err(error) = db::query("DELETE FROM notifications WHERE user_id = ? AND created_at < ?")
        .bind(user_id)
        .bind(cutoff)
        .execute(pool)
        .await
    {
        tracing::warn!(error = %error, user_id, "could not prune old notifications");
        return;
    }
    if let Err(error) = db::query(
        "DELETE FROM notifications
         WHERE user_id = ?
           AND id NOT IN (
                SELECT id FROM notifications WHERE user_id = ?
                ORDER BY id DESC LIMIT ?
           )",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(NOTIFICATION_KEEP_PER_USER)
    .execute(pool)
    .await
    {
        tracing::warn!(error = %error, user_id, "could not cap notifications");
    }
}

async fn clear_pair_notification(pool: &Database, recipient_id: i64, actor_id: i64, kind: &str) {
    let result = db::query(
        "DELETE FROM notifications
         WHERE user_id = ? AND actor_id = ? AND kind = ? AND read_at IS NULL",
    )
    .bind(recipient_id)
    .bind(actor_id)
    .bind(kind)
    .execute(pool)
    .await;
    if let Err(error) = result {
        tracing::warn!(error = %error, "could not clear friend request notification");
    }
}

/// Notifies every friend of an achievement unlock.
///
/// Double opt-in: the unlocker must share friends activity and each recipient
/// must enable the `friendAchievements` preference.
pub async fn notify_achievement(
    state: &AppState,
    owner_id: i64,
    game_id: i64,
    achievement_id: i64,
    key: &str,
    name: &str,
    points: i64,
) {
    match shares_with_friends(&state.pool, owner_id).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            tracing::warn!(code = %error.code, message = %error.message, owner_id, "could not read friends activity opt-in");
            return;
        }
    }
    for friend_id in friend_ids(state, owner_id).await {
        notify(
            state,
            friend_id,
            notification_kinds::FRIEND_ACHIEVEMENT,
            Some(owner_id),
            Some(game_id),
            Some(achievement_id),
            json!({
                "achievementKey": key,
                "achievementName": name,
                "points": points,
            }),
        )
        .await;
    }
}

/// Notifies friends when an account publishes a collection.
pub async fn notify_collection_shared(
    state: &AppState,
    owner_id: i64,
    collection_id: i64,
    name: &str,
) {
    for friend_id in friend_ids(state, owner_id).await {
        notify(
            state,
            friend_id,
            notification_kinds::COLLECTION_SHARE,
            Some(owner_id),
            None,
            None,
            json!({ "collectionId": collection_id, "collectionName": name }),
        )
        .await;
    }
}

async fn friend_ids(state: &AppState, user_id: i64) -> Vec<i64> {
    match db::query_scalar::<i64>(
        "SELECT CASE WHEN requester_id = ? THEN addressee_id ELSE requester_id END
         FROM friendships
         WHERE status = 'accepted' AND (requester_id = ? OR addressee_id = ?)
         ORDER BY id ASC LIMIT ?",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(MAX_FRIENDS)
    .fetch_all(&state.pool)
    .await
    {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(error = %error, user_id, "could not load friend ids");
            Vec::new()
        }
    }
}

/// One page of the caller's notifications, newest first.
///
/// # Errors
///
/// Propagates database failures.
pub async fn notifications(
    state: &AppState,
    user_id: i64,
    query: &NotificationQuery,
) -> Result<NotificationPage, ApiError> {
    let limit = query.limit.unwrap_or(20).clamp(1, MAX_NOTIFICATIONS);
    let mut clauses = vec!["n.user_id = ?".to_string()];
    let mut binds = vec![Bind::Int(user_id)];
    if let Some(before) = query.before {
        clauses.push("n.id < ?".to_string());
        binds.push(Bind::Int(before));
    }
    binds.push(Bind::Int(limit + 1));

    let sql = format!(
        "SELECT n.id, n.kind, n.created_at, n.read_at, n.payload,
                u.id AS actor_id, u.username AS actor_username,
                u.display_name AS actor_display_name, u.avatar_path AS actor_avatar_path,
                g.id AS game_id, g.title AS game_title,
                a.id AS achievement_id, a.key AS achievement_key,
                a.name AS achievement_name, a.points AS achievement_points
         FROM notifications n
         LEFT JOIN users u ON u.id = n.actor_id
         LEFT JOIN games g ON g.id = n.game_id
         LEFT JOIN achievements a ON a.id = n.achievement_id
         WHERE {}
         ORDER BY n.id DESC LIMIT ?",
        clauses.join(" AND ")
    );
    let mut rows = bind_query_as(db::query_as::<NotificationRow>(&sql), &binds)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;
    let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if has_more {
        rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    let next_before = has_more.then(|| rows.last().map(|row| row.id)).flatten();

    Ok(NotificationPage {
        items: rows.iter().map(NotificationRow::to_public).collect(),
        next_before,
        unread_count: unread_count(&state.pool, user_id).await?,
    })
}

#[derive(Debug, sqlx::FromRow)]
struct NotificationRow {
    id: i64,
    kind: String,
    created_at: String,
    read_at: Option<String>,
    payload: String,
    actor_id: Option<i64>,
    actor_username: Option<String>,
    actor_display_name: Option<String>,
    actor_avatar_path: Option<String>,
    game_id: Option<i64>,
    game_title: Option<String>,
    achievement_id: Option<i64>,
    achievement_key: Option<String>,
    achievement_name: Option<String>,
    achievement_points: Option<i64>,
}

impl NotificationRow {
    fn to_public(&self) -> Notification {
        Notification {
            id: self.id,
            kind: self.kind.clone(),
            created_at: parse_or_now(&self.created_at),
            read_at: self.read_at.as_deref().and_then(db::parse_time),
            actor: self
                .actor_id
                .zip(self.actor_username.clone())
                .map(|(id, username)| SocialUser {
                    id,
                    username,
                    display_name: self.actor_display_name.clone(),
                    avatar_url: avatar_url(self.actor_avatar_path.as_deref()),
                }),
            game: self
                .game_id
                .zip(self.game_title.clone())
                .map(|(id, title)| FriendGame { id, title }),
            achievement: self
                .achievement_id
                .zip(self.achievement_key.clone())
                .map(|(id, key)| NotificationAchievement {
                    id,
                    key,
                    name: self.achievement_name.clone().unwrap_or_default(),
                    points: self.achievement_points.unwrap_or(0),
                }),
            payload: serde_json::from_str(&self.payload).unwrap_or_else(|_| json!({})),
        }
    }
}

async fn unread_count(pool: &Database, user_id: i64) -> Result<i64, ApiError> {
    db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM notifications WHERE user_id = ? AND read_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)
}

/// Marks one notification read.
///
/// # Errors
///
/// 404 when the row is unknown or belongs to another account.
pub async fn mark_read(
    state: &AppState,
    user_id: i64,
    notification_id: i64,
) -> Result<NotificationReadResponse, ApiError> {
    let result = db::query(
        "UPDATE notifications SET read_at = ?
         WHERE id = ? AND user_id = ? AND read_at IS NULL",
    )
    .bind(db::db_time(db::now()))
    .bind(notification_id)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        let exists = db::query_scalar::<i64>(
            "SELECT COUNT(*) FROM notifications WHERE id = ? AND user_id = ?",
        )
        .bind(notification_id)
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
        if exists == 0 {
            return Err(ApiError::not_found(
                "notification_not_found",
                "Notification not found",
            ));
        }
    }
    Ok(NotificationReadResponse {
        updated: i64::try_from(result.rows_affected()).unwrap_or(i64::MAX),
        unread_count: unread_count(&state.pool, user_id).await?,
    })
}

/// Marks every notification of the caller read.
///
/// # Errors
///
/// Propagates database failures.
pub async fn mark_all_read(
    state: &AppState,
    user_id: i64,
) -> Result<NotificationReadResponse, ApiError> {
    let result =
        db::query("UPDATE notifications SET read_at = ? WHERE user_id = ? AND read_at IS NULL")
            .bind(db::db_time(db::now()))
            .bind(user_id)
            .execute(&state.pool)
            .await
            .map_err(ApiError::from)?;
    Ok(NotificationReadResponse {
        updated: i64::try_from(result.rows_affected()).unwrap_or(i64::MAX),
        unread_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Database {
        crate::db::connect_for_tests("sqlite::memory:")
            .await
            .expect("test database")
    }

    async fn add_user(pool: &Database, username: &str) -> i64 {
        let now = db::db_time(db::now());
        db::query(
            "INSERT INTO users (username, password_hash, role, activated, created_at, updated_at)
             VALUES (?, 'x', 'user', 1, ?, ?)",
        )
        .bind(username)
        .bind(&now)
        .bind(&now)
        .returning_id(pool)
        .await
        .expect("insert user")
    }

    #[test]
    fn pair_key_is_order_independent() {
        assert_eq!(pair_key(2, 9), "2:9");
        assert_eq!(pair_key(9, 2), "2:9");
        assert_eq!(pair_key(4, 4), "4:4");
    }

    #[test]
    fn presence_requires_opt_in_and_goes_stale() {
        let now = Utc::now();
        let fresh = db::db_time(now - Duration::seconds(10));
        let stale = db::db_time(now - Duration::seconds(PRESENCE_TTL_SECONDS + 1));

        assert!(presence_view(
            false,
            Some("in_game"),
            Some(3),
            Some("Hades".into()),
            Some(&fresh),
            now
        )
        .is_none());

        let online = presence_view(
            true,
            Some("in_game"),
            Some(3),
            Some("Hades".into()),
            Some(&fresh),
            now,
        )
        .expect("opted-in friend");
        assert_eq!(online.state, "in_game");
        assert_eq!(online.game.expect("game").title, "Hades");

        let offline = presence_view(
            true,
            Some("in_game"),
            Some(3),
            Some("Hades".into()),
            Some(&stale),
            now,
        )
        .expect("opted-in friend");
        assert_eq!(offline.state, "offline");
        assert!(offline.game.is_none(), "stale rows never expose a game");
        assert!(offline.last_seen.is_some());

        let never = presence_view(true, None, None, None, None, now).expect("opted-in friend");
        assert_eq!(never.state, "offline");
        assert!(never.last_seen.is_none());
    }

    #[tokio::test]
    async fn notifications_are_pruned_to_the_newest_rows() {
        let pool = test_pool().await;
        let user = add_user(&pool, "mara").await;
        let actor = add_user(&pool, "jonas").await;
        for index in 0..(NOTIFICATION_KEEP_PER_USER + 5) {
            insert_notification(
                &pool,
                user,
                notification_kinds::FRIEND_REQUEST,
                Some(actor),
                None,
                None,
                &json!({ "index": index }),
            )
            .await
            .expect("insert notification");
        }
        prune_notifications(&pool, user).await;
        let remaining = unread_count(&pool, user).await.expect("count");
        assert_eq!(remaining, NOTIFICATION_KEEP_PER_USER);
    }

    #[tokio::test]
    async fn notification_prefs_default_to_opt_in_achievements() {
        let pool = test_pool().await;
        let user = add_user(&pool, "ines").await;
        let defaults = prefs(&pool, user).await.expect("prefs");
        assert!(defaults.friend_requests);
        assert!(defaults.friend_accepted);
        assert!(!defaults.friend_achievements);
        assert!(defaults.collection_shares);

        let saved = save_prefs(
            &pool,
            user,
            &NotificationPrefsPatch {
                friend_achievements: Some(true),
                friend_requests: Some(false),
                ..NotificationPrefsPatch::default()
            },
        )
        .await
        .expect("save prefs");
        assert!(saved.friend_achievements);
        assert!(!saved.friend_requests);
        assert!(saved.friend_accepted, "untouched fields keep their value");
    }
}
