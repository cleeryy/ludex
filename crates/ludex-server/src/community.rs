//! Activity feed: recording and shared read queries.
//!
//! Only accounts with `public_profile = 1` appear in the feed and the community
//! directory. Rows are written from the routes that already own the event
//! (progress, playtime, install report, collection creation) and are deduped so
//! a chatty client cannot flood the feed.

use crate::db;
use serde_json::Value;

use crate::error::ApiError;
use crate::models::{ActivityPage, ActivityQuery, ActivityRow};
use crate::sql::{bind_query_as, Bind};
use crate::state::AppState;

/// Stable activity kinds.
pub mod kinds {
    /// A game entered the user's library (first progress or playtime).
    pub const GAME_ADDED: &str = "game_added";
    /// The desktop reported an install.
    pub const GAME_INSTALLED: &str = "game_installed";
    /// A session was played or the state switched to playing.
    pub const GAME_PLAYED: &str = "game_played";
    /// A game was marked completed.
    pub const GAME_COMPLETED: &str = "game_completed";
    /// A collection was created.
    pub const COLLECTION_CREATED: &str = "collection_created";
    /// An achievement was unlocked (public definitions only).
    pub const ACHIEVEMENT_UNLOCKED: &str = "achievement_unlocked";
}

/// Writes one activity row. Failures are logged, never propagated: a feed
/// entry must not turn a successful action into an HTTP error.
pub async fn record(
    state: &AppState,
    user_id: i64,
    kind: &str,
    game_id: Option<i64>,
    collection_id: Option<i64>,
    metadata: Value,
) {
    let result = db::query(
        "INSERT INTO activities (user_id, kind, game_id, collection_id, metadata, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(kind)
    .bind(game_id)
    .bind(collection_id)
    .bind(serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string()))
    .bind(crate::db::db_time(crate::db::now()))
    .execute(&state.pool)
    .await;
    if let Err(error) = result {
        tracing::warn!(error = %error, kind, user_id, "could not record activity");
    }
}

/// Writes an activity row unless the same user produced the same kind for the
/// same game within `window_minutes`. Returns whether a row was written.
pub async fn record_deduped(
    state: &AppState,
    user_id: i64,
    kind: &str,
    game_id: Option<i64>,
    metadata: Value,
    window_minutes: i64,
) -> bool {
    let since = crate::db::db_time(crate::db::now() - chrono::Duration::minutes(window_minutes));
    let recent = db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM activities
         WHERE user_id = ? AND kind = ? AND COALESCE(game_id, -1) = COALESCE(?, -1)
           AND created_at >= ?",
    )
    .bind(user_id)
    .bind(kind)
    .bind(game_id)
    .bind(&since)
    .fetch_one(&state.pool)
    .await;
    match recent {
        Ok(count) if count > 0 => return false,
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(error = %error, "could not check recent activity");
        }
    }
    record(state, user_id, kind, game_id, None, metadata).await;
    true
}

const FEED_COLUMNS: &str = "a.id, a.kind, a.created_at, a.metadata, \
u.id AS user_id, u.username, u.display_name, u.avatar_path, \
g.id AS game_id, g.title AS game_title, g.cover_path AS game_cover_path, \
g.release_date AS game_release_date, \
c.id AS collection_id, c.name AS collection_name";

const FEED_JOINS: &str = " FROM activities a \
JOIN users u ON u.id = a.user_id \
LEFT JOIN games g ON g.id = a.game_id \
LEFT JOIN collections c ON c.id = a.collection_id";

/// Reads one page of the public feed, optionally restricted to one account.
///
/// `viewer_id` is the caller. A profile's activity is only readable when the
/// profile is public or the caller is the owner; the route checks that before
/// calling this helper.
pub async fn feed(
    state: &AppState,
    user_id: Option<i64>,
    query: &ActivityQuery,
) -> Result<ActivityPage, ApiError> {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let mut clauses = vec!["u.public_profile = 1".to_string()];
    let mut binds = Vec::new();
    if let Some(user_id) = user_id {
        clauses.push("a.user_id = ?".to_string());
        binds.push(Bind::Int(user_id));
    }
    if let Some(before) = query.before {
        clauses.push("a.id < ?".to_string());
        binds.push(Bind::Int(before));
    }
    binds.push(Bind::Int(limit + 1));

    let sql = format!(
        "SELECT {FEED_COLUMNS}{FEED_JOINS} WHERE {} ORDER BY a.id DESC LIMIT ?",
        clauses.join(" AND ")
    );
    let mut rows = bind_query_as(db::query_as::<ActivityRow>(&sql), &binds)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if has_more {
        rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    let next_before = has_more.then(|| rows.last().map(|row| row.id)).flatten();
    Ok(ActivityPage {
        items: rows.iter().map(ActivityRow::to_public).collect(),
        next_before,
    })
}
