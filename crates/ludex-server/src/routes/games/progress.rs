//! Per-user progress, playtime, bookmarks and installed reports.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::Json;
use chrono::Duration;
use ludex_core::models::{ProgressState, ProgressUpdate, Stats};

use crate::api_keys::Scope;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::extract::{ApiPath, JsonBody};
use crate::models::{
    BookmarkRequest, BookmarkResponse, InstalledReportResponse, PlaytimeRequest, ProgressRequest,
};
use crate::state::AppState;

use super::ensure_game;

pub(super) async fn set_progress(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<ProgressRequest>,
) -> Result<Json<ProgressUpdate>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let existed = progress_exists(&state, user.id, game_id).await?;
    let last_played = if request.state == ProgressState::Playing {
        Some(db::db_time(db::now()))
    } else {
        None
    };
    db::query(
        "INSERT INTO progress (user_id, game_id, state, minutes_played, last_played_at)
         VALUES (?, ?, ?, 0, ?)
         ON CONFLICT(user_id, game_id) DO UPDATE SET
            state = excluded.state,
            last_played_at = COALESCE(excluded.last_played_at, progress.last_played_at)",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(request.state.as_str())
    .bind(last_played)
    .execute(&state.core.pool)
    .await
    .map_err(ApiError::from)?;

    if !existed {
        crate::community::record(
            &state,
            user.id,
            crate::community::kinds::GAME_ADDED,
            Some(game_id),
            None,
            serde_json::json!({}),
        )
        .await;
    }
    match request.state {
        ProgressState::Playing => {
            crate::community::record_deduped(
                &state,
                user.id,
                crate::community::kinds::GAME_PLAYED,
                Some(game_id),
                serde_json::json!({}),
                30,
            )
            .await;
            crate::social::touch_playing(&state, user.id, game_id).await;
        }
        ProgressState::Completed => {
            crate::community::record_deduped(
                &state,
                user.id,
                crate::community::kinds::GAME_COMPLETED,
                Some(game_id),
                serde_json::json!({}),
                60,
            )
            .await;
        }
        _ => {}
    }
    Ok(Json(fetch_progress(&state, user.id, game_id).await?))
}

pub(super) async fn report_playtime(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<PlaytimeRequest>,
) -> Result<Json<ProgressUpdate>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let minutes = request.minutes.clamp(1, 1440);
    let message_id = request
        .message_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    if message_id.is_some_and(|id| id.len() > 128) {
        return Err(ApiError::bad_request(
            "invalid_message_id",
            "`messageId` must be at most 128 characters",
        ));
    }
    let existed = progress_exists(&state, user.id, game_id).await?;
    let ended_at = db::now();
    let started_at = ended_at - Duration::minutes(minutes);
    let ended_text = db::db_time(ended_at);
    let started_text = db::db_time(started_at);

    // One unit: the receipt (when the client sent a message id), the progress
    // increment and the play-session row. A crash or error anywhere rolls all
    // three back, so the outbox retry can safely send the same message again.
    let applied = state.core.pool
        .with_tx(async |tx| {
            if let Some(message_id) = message_id {
                // Receipts only need to outlive the desktop's retry window; the
                // outbox stops retrying long before this.
                let cutoff = db::db_time(db::now() - Duration::days(90));
                db::query("DELETE FROM playtime_messages WHERE received_at < ?")
                    .bind(&cutoff)
                    .execute(&mut *tx)
                    .await?;
                let inserted = db::query(
                    "INSERT INTO playtime_messages (user_id, message_id, game_id, minutes, received_at)
                     VALUES (?, ?, ?, ?, ?)
                     ON CONFLICT(user_id, message_id) DO NOTHING",
                )
                .bind(user.id)
                .bind(message_id)
                .bind(game_id)
                .bind(minutes)
                .bind(&ended_text)
                .execute(&mut *tx)
                .await?;
                if inserted.rows_affected() == 0 {
                    // Replay: this message was already applied. Report the
                    // current totals without touching them.
                    return Ok::<bool, ApiError>(false);
                }
            }
            db::query(
                "INSERT INTO progress (user_id, game_id, state, minutes_played, last_played_at)
                 VALUES (?, ?, 'unplayed', ?, ?)
                 ON CONFLICT(user_id, game_id) DO UPDATE SET
                    minutes_played = progress.minutes_played + excluded.minutes_played,
                    last_played_at = excluded.last_played_at",
            )
            .bind(user.id)
            .bind(game_id)
            .bind(minutes)
            .bind(&ended_text)
            .execute(&mut *tx)
            .await?;
            db::query(
                "INSERT INTO play_sessions (user_id, game_id, minutes, started_at, ended_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(user.id)
            .bind(game_id)
            .bind(minutes)
            .bind(&started_text)
            .bind(&ended_text)
            .execute(&mut *tx)
            .await?;
            Ok(true)
        })
        .await?;

    if !applied {
        return Ok(Json(fetch_progress(&state, user.id, game_id).await?));
    }

    if !existed {
        crate::community::record(
            &state,
            user.id,
            crate::community::kinds::GAME_ADDED,
            Some(game_id),
            None,
            serde_json::json!({}),
        )
        .await;
    }
    crate::community::record_deduped(
        &state,
        user.id,
        crate::community::kinds::GAME_PLAYED,
        Some(game_id),
        serde_json::json!({ "minutes": minutes }),
        30,
    )
    .await;
    crate::social::touch_playing(&state, user.id, game_id).await;

    Ok(Json(fetch_progress(&state, user.id, game_id).await?))
}

pub(super) async fn set_bookmark(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    JsonBody(request): JsonBody<BookmarkRequest>,
) -> Result<Json<BookmarkResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    if request.bookmarked {
        db::query(
            "INSERT INTO bookmarks (user_id, game_id) VALUES (?, ?)
             ON CONFLICT(user_id, game_id) DO NOTHING",
        )
        .bind(user.id)
        .bind(game_id)
        .execute(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    } else {
        db::query("DELETE FROM bookmarks WHERE user_id = ? AND game_id = ?")
            .bind(user.id)
            .bind(game_id)
            .execute(&state.core.pool)
            .await
            .map_err(ApiError::from)?;
    }
    Ok(Json(BookmarkResponse {
        bookmarked: request.bookmarked,
    }))
}

pub(super) async fn stats(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Stats>, ApiError> {
    let games = db::query_scalar::<i64>("SELECT COUNT(*) FROM games")
        .fetch_one(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    let versions = db::query_scalar::<i64>("SELECT COUNT(*) FROM game_versions WHERE missing = 0")
        .fetch_one(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    let total_bytes = db::query_scalar::<i64>(
        "SELECT CAST(COALESCE(SUM(size), 0) AS BIGINT) FROM game_versions WHERE missing = 0",
    )
    .fetch_one(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    let users = db::query_scalar::<i64>("SELECT COUNT(*) FROM users")
        .fetch_one(&state.core.pool)
        .await
        .map_err(ApiError::from)?;

    let mut by_state: BTreeMap<ProgressState, i64> = ProgressState::ALL
        .iter()
        .copied()
        .map(|state| (state, 0))
        .collect();
    let rows = db::query_as::<(String, i64)>(
        "SELECT state, COUNT(*) FROM progress WHERE user_id = ? GROUP BY state",
    )
    .bind(user.id)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    let mut counted = 0;
    for (state, count) in rows {
        if let Ok(state) = state.parse::<ProgressState>() {
            if state != ProgressState::Unplayed {
                by_state.insert(state, count);
                counted += count;
            }
        }
    }
    by_state.insert(ProgressState::Unplayed, (games - counted).max(0));

    Ok(Json(Stats {
        games,
        versions,
        total_bytes,
        users,
        by_state,
    }))
}

async fn progress_exists(state: &AppState, user_id: i64, game_id: i64) -> Result<bool, ApiError> {
    let count =
        db::query_scalar::<i64>("SELECT COUNT(*) FROM progress WHERE user_id = ? AND game_id = ?")
            .bind(user_id)
            .bind(game_id)
            .fetch_one(&state.core.pool)
            .await
            .map_err(ApiError::from)?;
    Ok(count > 0)
}

pub(super) async fn report_installed(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<InstalledReportResponse>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let recorded = crate::community::record_deduped(
        &state,
        user.id,
        crate::community::kinds::GAME_INSTALLED,
        Some(game_id),
        serde_json::json!({ "source": "desktop" }),
        60,
    )
    .await;
    Ok(Json(InstalledReportResponse { recorded }))
}

async fn fetch_progress(
    state: &AppState,
    user_id: i64,
    game_id: i64,
) -> Result<ProgressUpdate, ApiError> {
    let row = db::query_as::<(String, i64, Option<String>)>(
        "SELECT state, minutes_played, last_played_at FROM progress
         WHERE user_id = ? AND game_id = ?",
    )
    .bind(user_id)
    .bind(game_id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(match row {
        Some((state, minutes, last_played)) => ProgressUpdate {
            state: state.parse().unwrap_or_default(),
            minutes_played: minutes,
            last_played_at: last_played.as_deref().and_then(db::parse_time),
        },
        None => ProgressUpdate {
            state: ProgressState::Unplayed,
            minutes_played: 0,
            last_played_at: None,
        },
    })
}
