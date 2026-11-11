//! Per-user controller profiles (#51).
//!
//! Profiles are stored server-side so a player finds the same deadzone,
//! remapping and vibration preference on every device. `game_id = 0` is the
//! account-wide profile; any other game id is a per-game override. The JSON
//! document is validated by [`ludex_core::gamepad`] before it reaches SQLite,
//! and the same parser is used on import.

use chrono::{DateTime, Utc};
use ludex_core::gamepad::ControllerProfile;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db;
use crate::error::ApiError;

/// One stored profile, with the game it applies to.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ControllerProfileEntry {
    /// Game id, or `null` for the account-wide profile.
    pub game_id: Option<i64>,
    /// Title of the game, when this is a per-game override.
    pub game_title: Option<String>,
    /// The profile document.
    pub profile: ControllerProfile,
    /// Last save.
    pub updated_at: DateTime<Utc>,
}

/// All profiles of one account plus the built-in presets.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ControllerProfilesResponse {
    /// Account-wide profile, when saved.
    pub global: Option<ControllerProfileEntry>,
    /// Per-game overrides, newest first.
    pub games: Vec<ControllerProfileEntry>,
    /// Built-in presets the editor can start from.
    pub presets: Vec<PresetEntry>,
}

/// One built-in preset.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresetEntry {
    /// Stable preset id (`xbox`, `dualsense`, `steamdeck`, `generic`).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Full profile document.
    pub profile: ControllerProfile,
}

/// Body of `PUT /api/controller-profiles`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SaveControllerProfileRequest {
    /// `null`/absent writes the account profile, a game id writes its override.
    #[serde(default)]
    pub game_id: Option<i64>,
    /// Profile document.
    pub profile: ControllerProfile,
}

struct ProfileRow {
    game_id: i64,
    game_title: Option<String>,
    profile_json: String,
    updated_at: String,
}

/// Lists every profile of an account.
pub async fn list(pool: &SqlitePool, user_id: i64) -> Result<ControllerProfilesResponse, ApiError> {
    let rows = sqlx::query_as::<_, (i64, Option<String>, String, String)>(
        "SELECT p.game_id, g.title, p.profile_json, p.updated_at
         FROM controller_profiles p
         LEFT JOIN games g ON g.id = p.game_id
         WHERE p.user_id = ?
         ORDER BY p.game_id = 0 DESC, p.updated_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;

    let mut global = None;
    let mut games = Vec::new();
    for (game_id, game_title, profile_json, updated_at) in rows {
        let row = ProfileRow {
            game_id,
            game_title,
            profile_json,
            updated_at,
        };
        let entry = row.into_entry()?;
        if entry.game_id.is_none() {
            global = Some(entry);
        } else {
            games.push(entry);
        }
    }

    Ok(ControllerProfilesResponse {
        global,
        games,
        presets: ludex_core::gamepad::PRESETS
            .iter()
            .map(|id| PresetEntry {
                id: (*id).to_string(),
                label: ludex_core::gamepad::preset(id).name,
                profile: ludex_core::gamepad::preset(id),
            })
            .collect(),
    })
}

impl ProfileRow {
    fn into_entry(self) -> Result<ControllerProfileEntry, ApiError> {
        let profile: ControllerProfile =
            serde_json::from_str(&self.profile_json).map_err(|error| {
                tracing::error!(%error, "stored controller profile is invalid");
                ApiError::internal()
            })?;
        Ok(ControllerProfileEntry {
            game_id: if self.game_id == 0 {
                None
            } else {
                Some(self.game_id)
            },
            game_title: self.game_title,
            profile,
            updated_at: db::parse_time(&self.updated_at).unwrap_or_else(Utc::now),
        })
    }
}

/// Validates and stores one profile, replacing the previous one for the scope.
pub async fn save(
    pool: &SqlitePool,
    user_id: i64,
    request: &SaveControllerProfileRequest,
) -> Result<ControllerProfileEntry, ApiError> {
    request
        .profile
        .validate()
        .map_err(|message| ApiError::bad_request("invalid_controller_profile", message))?;
    let game_id = request.game_id.unwrap_or(0);
    if game_id < 0 {
        return Err(ApiError::bad_request(
            "invalid_game_id",
            "gameId must be a positive game id or null",
        ));
    }
    if game_id > 0 {
        let exists = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games WHERE id = ?")
            .bind(game_id)
            .fetch_one(pool)
            .await
            .map_err(ApiError::from)?;
        if exists == 0 {
            return Err(ApiError::not_found("game_not_found", "Game not found"));
        }
    }

    let document = serde_json::to_string(&request.profile).map_err(|error| {
        tracing::error!(%error, "could not serialise the controller profile");
        ApiError::internal()
    })?;
    let now = db::db_time(db::now());
    sqlx::query(
        "INSERT INTO controller_profiles (user_id, game_id, name, profile_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(user_id, game_id) DO UPDATE SET
            name = excluded.name,
            profile_json = excluded.profile_json,
            updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(game_id)
    .bind(&request.profile.name)
    .bind(&document)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(ApiError::from)?;

    let game_title = if game_id == 0 {
        None
    } else {
        sqlx::query_scalar::<_, String>("SELECT title FROM games WHERE id = ?")
            .bind(game_id)
            .fetch_optional(pool)
            .await
            .map_err(ApiError::from)?
    };

    Ok(ControllerProfileEntry {
        game_id: if game_id == 0 { None } else { Some(game_id) },
        game_title,
        profile: request.profile.clone(),
        updated_at: db::parse_time(&now).unwrap_or_else(Utc::now),
    })
}

/// Deletes one profile scope. Returns whether a row existed.
pub async fn delete(pool: &SqlitePool, user_id: i64, game_id: i64) -> Result<bool, ApiError> {
    let result = sqlx::query("DELETE FROM controller_profiles WHERE user_id = ? AND game_id = ?")
        .bind(user_id)
        .bind(game_id)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use ludex_core::gamepad::preset;

    async fn create_user(state: &AppState, username: &str) -> i64 {
        sqlx::query(
            "INSERT INTO users (username, password_hash, role, activated, created_at, updated_at)
             VALUES (?, 'x', 'admin', 1, '2026-01-01', '2026-01-01')",
        )
        .bind(username)
        .execute(&state.pool)
        .await
        .expect("insert user")
        .last_insert_rowid()
    }

    async fn state() -> AppState {
        let dir = tempfile::tempdir().expect("temp dir");
        // Keep the temp directory alive for the duration of the process by
        // leaking it: SQLite keeps the file open and tests are short.
        let path = dir.keep();
        let config =
            crate::config::Config::for_local(path.join("games"), path.clone(), path.join("media"));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        AppState::for_tests(config, tx).await
    }

    #[tokio::test]
    async fn saves_lists_and_deletes_global_and_game_profiles() {
        let state = state().await;
        let user_id = create_user(&state, "pilot").await;
        let game_id = sqlx::query(
            "INSERT INTO games (title, sort_title, group_key, created_at, updated_at)
             VALUES ('Chrono Trigger', 'Chrono Trigger', 'rom::snes::chrono trigger', '2026-01-01', '2026-01-01')",
        )
        .execute(&state.pool)
        .await
        .expect("insert game")
        .last_insert_rowid();

        let global = SaveControllerProfileRequest {
            game_id: None,
            profile: preset("dualsense"),
        };
        save(&state.pool, user_id, &global)
            .await
            .expect("save global");
        let per_game = SaveControllerProfileRequest {
            game_id: Some(game_id),
            profile: preset("steamdeck"),
        };
        save(&state.pool, user_id, &per_game)
            .await
            .expect("save game");

        let listed = list(&state.pool, user_id).await.expect("list");
        assert_eq!(listed.global.as_ref().unwrap().profile.name, "DualSense");
        assert_eq!(listed.games.len(), 1);
        assert_eq!(
            listed.games[0].game_title.as_deref(),
            Some("Chrono Trigger")
        );
        assert_eq!(listed.presets.len(), 4);

        // Saving again replaces instead of duplicating.
        save(&state.pool, user_id, &per_game)
            .await
            .expect("replace game");
        let listed = list(&state.pool, user_id).await.expect("list");
        assert_eq!(listed.games.len(), 1);

        assert!(delete(&state.pool, user_id, game_id).await.expect("delete"));
        assert!(!delete(&state.pool, user_id, game_id).await.expect("delete"));
    }

    #[tokio::test]
    async fn rejects_invalid_profiles_and_unknown_games() {
        let state = state().await;
        let user_id = create_user(&state, "pilot").await;
        let mut profile = preset("xbox");
        profile.deadzone = 2.0;
        let request = SaveControllerProfileRequest {
            game_id: None,
            profile,
        };
        assert!(save(&state.pool, user_id, &request).await.is_err());
        let request = SaveControllerProfileRequest {
            game_id: Some(9999),
            profile: preset("xbox"),
        };
        assert!(save(&state.pool, user_id, &request).await.is_err());
    }
}
