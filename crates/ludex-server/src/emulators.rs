//! Emulator registry and ROM launch resolution (#52).
//!
//! Ludex ships no emulator binary. An administrator registers a command
//! template containing `%ROM%` (for example `retroarch -L snes9x_libretro.so
//! %ROM%`), lists the platforms it handles and can make it the default for a
//! platform. Commands are tokenised by [`ludex_core::rom::build_launch_command`]
//! and spawned without a shell; ROM paths never reach a command interpreter.

use std::collections::BTreeMap;

use crate::db::Database;
use chrono::{DateTime, Utc};
use ludex_core::rom::{build_launch_command, platform, LaunchCommand, PLATFORMS};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::error::ApiError;

/// A configured emulator.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Emulator {
    /// Row id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Command template containing `%ROM%`.
    pub command: String,
    /// Platform slugs this emulator handles.
    pub platforms: Vec<String>,
    /// Whether the emulator can be picked.
    pub enabled: bool,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last edit.
    pub updated_at: DateTime<Utc>,
}

/// Body of `POST /api/emulators`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateEmulatorRequest {
    /// Display name.
    pub name: String,
    /// Command template containing `%ROM%`.
    pub command: String,
    /// Platform slugs.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Enabled flag, defaults to true.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Body of `PATCH /api/emulators/{id}`; omitted fields are unchanged.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEmulatorRequest {
    /// New display name.
    #[serde(default)]
    pub name: Option<String>,
    /// New command template.
    #[serde(default)]
    pub command: Option<String>,
    /// New platform list.
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
    /// Enable or disable.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// One platform of the built-in catalog.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlatformView {
    /// Stable slug.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Recognised extensions.
    pub extensions: Vec<String>,
    /// Suggested RetroArch core, when one exists.
    pub retroarch_core: Option<String>,
    /// ROM games indexed for this platform.
    pub game_count: i64,
}

/// A platform default assignment.
#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlatformDefault {
    /// Platform slug.
    pub platform: String,
    /// Emulator id, or `null` to clear the default.
    pub emulator_id: Option<i64>,
}

/// Body of `PUT /api/emulators/defaults`.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDefaultsRequest {
    /// Assignments to apply.
    pub defaults: Vec<PlatformDefault>,
}

/// Body of `POST /api/emulators/validate`.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateCommandRequest {
    /// Command template to validate.
    pub command: String,
}

/// Result of a command validation.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateCommandResponse {
    /// Whether the template can be launched.
    pub valid: bool,
    /// Reason when invalid.
    pub error: Option<String>,
    /// Resolved program when valid.
    pub program: Option<String>,
    /// Resolved arguments, with a sample ROM substituted.
    pub args: Vec<String>,
}

/// Body of `PUT /api/games/{id}/emulator`.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetGameEmulatorRequest {
    /// Emulator id, or `null` to fall back to the platform default.
    pub emulator_id: Option<i64>,
}

/// Resolved launch command for one ROM game.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmulatorLaunch {
    /// Game id.
    pub game_id: i64,
    /// Platform slug.
    pub platform: String,
    /// Emulator id.
    pub emulator_id: i64,
    /// Emulator name.
    pub emulator_name: String,
    /// Executable to spawn.
    pub program: String,
    /// Arguments, with `%ROM%` replaced.
    pub args: Vec<String>,
    /// Absolute path of the ROM on the server. Remote desktop clients must
    /// resolve the same file locally before spawning.
    pub rom_path: String,
}

struct EmulatorRow {
    id: i64,
    name: String,
    command: String,
    platforms_json: String,
    enabled: i64,
    created_at: String,
    updated_at: String,
}

impl EmulatorRow {
    fn into_emulator(self) -> Emulator {
        Emulator {
            id: self.id,
            name: self.name,
            command: self.command,
            platforms: db::parse_json_list(&self.platforms_json),
            enabled: self.enabled != 0,
            created_at: db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            updated_at: db::parse_time(&self.updated_at).unwrap_or_else(Utc::now),
        }
    }
}

/// Validates an emulator command against a synthetic ROM path.
pub fn validate_command(command: &str) -> Result<LaunchCommand, String> {
    build_launch_command(command, std::path::Path::new("/roms/sample.sfc"))
        .map_err(|error| error.to_string())
}

fn validate_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::bad_request(
            "invalid_emulator",
            "Emulator name must be between 1 and 80 characters",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            "invalid_emulator",
            "Emulator name must not contain control characters",
        ));
    }
    Ok(name.to_string())
}

fn normalise_platforms(platforms: &[String]) -> Result<Vec<String>, ApiError> {
    let mut result = Vec::new();
    for slug in platforms {
        let slug = slug.trim().to_ascii_lowercase();
        if slug.is_empty() {
            continue;
        }
        if platform(&slug).is_none() {
            return Err(ApiError::bad_request(
                "unknown_platform",
                format!("Unknown platform `{slug}`"),
            ));
        }
        if !result.contains(&slug) {
            result.push(slug);
        }
    }
    Ok(result)
}

/// Lists every emulator, enabled or not.
pub async fn list(pool: &Database) -> Result<Vec<Emulator>, ApiError> {
    let rows = db::query_as::<(i64, String, String, String, i64, String, String)>(
        "SELECT id, name, command, platforms_json, enabled, created_at, updated_at
         FROM emulators ORDER BY lower(name)",
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            EmulatorRow {
                id: row.0,
                name: row.1,
                command: row.2,
                platforms_json: row.3,
                enabled: row.4,
                created_at: row.5,
                updated_at: row.6,
            }
            .into_emulator()
        })
        .collect())
}

/// Creates an emulator after validating its command and platforms.
pub async fn create(
    pool: &Database,
    request: &CreateEmulatorRequest,
) -> Result<Emulator, ApiError> {
    let name = validate_name(&request.name)?;
    validate_command(&request.command)
        .map_err(|message| ApiError::bad_request("invalid_emulator_command", message))?;
    let platforms = normalise_platforms(&request.platforms)?;
    let now = db::db_time(db::now());
    let platforms_json = db::dump_json_list(&platforms);
    let id = db::query(
        "INSERT INTO emulators (name, command, platforms_json, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&name)
    .bind(request.command.trim())
    .bind(platforms_json)
    .bind(i64::from(request.enabled.unwrap_or(true)))
    .bind(&now)
    .bind(&now)
    .returning_id(pool)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            ApiError::conflict(
                "emulator_exists",
                "An emulator with this name already exists",
            )
        } else {
            ApiError::from(error)
        }
    })?;
    get(pool, id).await?.ok_or_else(ApiError::internal)
}

/// Fetches one emulator.
pub async fn get(pool: &Database, id: i64) -> Result<Option<Emulator>, ApiError> {
    let row = db::query_as::<(i64, String, String, String, i64, String, String)>(
        "SELECT id, name, command, platforms_json, enabled, created_at, updated_at
         FROM emulators WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(row.map(|row| {
        EmulatorRow {
            id: row.0,
            name: row.1,
            command: row.2,
            platforms_json: row.3,
            enabled: row.4,
            created_at: row.5,
            updated_at: row.6,
        }
        .into_emulator()
    }))
}

/// Updates an emulator.
pub async fn update(
    pool: &Database,
    id: i64,
    request: &UpdateEmulatorRequest,
) -> Result<Emulator, ApiError> {
    let current = get(pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("emulator_not_found", "Emulator not found"))?;
    let name = match request.name.as_deref() {
        Some(name) => validate_name(name)?,
        None => current.name,
    };
    let command = match request.command.as_deref() {
        Some(command) => {
            validate_command(command)
                .map_err(|message| ApiError::bad_request("invalid_emulator_command", message))?;
            command.trim().to_string()
        }
        None => current.command,
    };
    let platforms = match request.platforms.as_deref() {
        Some(platforms) => normalise_platforms(platforms)?,
        None => current.platforms,
    };
    let enabled = request.enabled.unwrap_or(current.enabled);
    db::query(
        "UPDATE emulators SET name = ?, command = ?, platforms_json = ?, enabled = ?,
            updated_at = ? WHERE id = ?",
    )
    .bind(&name)
    .bind(&command)
    .bind(db::dump_json_list(&platforms))
    .bind(i64::from(enabled))
    .bind(db::db_time(db::now()))
    .bind(id)
    .execute(pool)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            ApiError::conflict(
                "emulator_exists",
                "An emulator with this name already exists",
            )
        } else {
            ApiError::from(error)
        }
    })?;
    get(pool, id).await?.ok_or_else(ApiError::internal)
}

/// Deletes an emulator. Platform defaults referencing it are removed by the
/// foreign key; per-game assignments fall back to the platform default.
pub async fn delete(pool: &Database, id: i64) -> Result<bool, ApiError> {
    let result = db::query("DELETE FROM emulators WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(result.rows_affected() > 0)
}

/// Platform catalog enriched with ROM game counts.
pub async fn platforms(pool: &Database) -> Result<Vec<PlatformView>, ApiError> {
    let counts = db::query_as::<(Option<String>, i64)>(
        "SELECT platform, COUNT(*) FROM games WHERE kind = 'rom' AND platform IS NOT NULL GROUP BY platform",
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    let counts: BTreeMap<String, i64> = counts
        .into_iter()
        .filter_map(|(platform, count)| platform.map(|platform| (platform, count)))
        .collect();
    Ok(PLATFORMS
        .iter()
        .map(|platform| PlatformView {
            id: platform.id.to_string(),
            label: platform.label.to_string(),
            extensions: platform
                .extensions
                .iter()
                .map(|extension| (*extension).to_string())
                .collect(),
            retroarch_core: platform.retroarch_core.map(str::to_string),
            game_count: counts.get(platform.id).copied().unwrap_or(0),
        })
        .collect())
}

/// Reads the configured default emulator per platform.
pub async fn defaults(pool: &Database) -> Result<Vec<PlatformDefault>, ApiError> {
    let rows = db::query_as::<(String, i64)>(
        "SELECT platform, emulator_id FROM emulator_defaults ORDER BY platform",
    )
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows
        .into_iter()
        .map(|(platform, emulator_id)| PlatformDefault {
            platform,
            emulator_id: Some(emulator_id),
        })
        .collect())
}

/// Applies platform defaults atomically.
pub async fn set_defaults(
    pool: &Database,
    request: &UpdateDefaultsRequest,
) -> Result<Vec<PlatformDefault>, ApiError> {
    let mut transaction = pool.begin().await.map_err(ApiError::from)?;
    for assignment in &request.defaults {
        let slug = assignment.platform.trim().to_ascii_lowercase();
        if platform(&slug).is_none() {
            return Err(ApiError::bad_request(
                "unknown_platform",
                format!("Unknown platform `{slug}`"),
            ));
        }
        match assignment.emulator_id {
            Some(id) => {
                let exists = db::query_scalar::<i64>(
                    "SELECT COUNT(*) FROM emulators WHERE id = ? AND enabled = 1",
                )
                .bind(id)
                .fetch_one(&mut transaction)
                .await
                .map_err(ApiError::from)?;
                if exists == 0 {
                    return Err(ApiError::not_found(
                        "emulator_not_found",
                        format!("Emulator {id} does not exist or is disabled"),
                    ));
                }
                db::query(
                    "INSERT INTO emulator_defaults (platform, emulator_id, created_at)
                     VALUES (?, ?, ?)
                     ON CONFLICT(platform) DO UPDATE SET emulator_id = excluded.emulator_id",
                )
                .bind(&slug)
                .bind(id)
                .bind(db::db_time(db::now()))
                .execute(&mut transaction)
                .await
                .map_err(ApiError::from)?;
            }
            None => {
                db::query("DELETE FROM emulator_defaults WHERE platform = ?")
                    .bind(&slug)
                    .execute(&mut transaction)
                    .await
                    .map_err(ApiError::from)?;
            }
        }
    }
    transaction.commit().await.map_err(ApiError::from)?;
    defaults(pool).await
}

/// Picks the emulator for a ROM game: per-game assignment, then platform
/// default, then the first enabled emulator that supports the platform.
pub async fn emulator_for(
    pool: &Database,
    platform_slug: &str,
    game_emulator_id: Option<i64>,
) -> Result<Option<Emulator>, ApiError> {
    if let Some(id) = game_emulator_id {
        if let Some(emulator) = get(pool, id).await? {
            if emulator.enabled {
                return Ok(Some(emulator));
            }
        }
    }
    let default_id =
        db::query_scalar::<i64>("SELECT emulator_id FROM emulator_defaults WHERE platform = ?")
            .bind(platform_slug)
            .fetch_optional(pool)
            .await
            .map_err(ApiError::from)?;
    if let Some(id) = default_id {
        if let Some(emulator) = get(pool, id).await? {
            if emulator.enabled {
                return Ok(Some(emulator));
            }
        }
    }
    let candidates = list(pool).await?;
    Ok(candidates
        .into_iter()
        .find(|emulator| emulator.enabled && emulator.platforms.iter().any(|p| p == platform_slug)))
}

/// Resolves the launch command of one ROM game.
pub async fn launch_for_game(pool: &Database, game_id: i64) -> Result<EmulatorLaunch, ApiError> {
    let row = db::query_as::<(
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
    )>(
        "SELECT g.title, g.platform, g.emulator_id, g.kind,
                (SELECT v.file_path FROM game_versions v
                 WHERE v.game_id = g.id AND v.missing = 0 ORDER BY v.id DESC LIMIT 1)
         FROM games g WHERE g.id = ?",
    )
    .bind(game_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))?;
    let (_title, platform_slug, emulator_id, kind, file_path) = row;
    let file_path = file_path
        .ok_or_else(|| ApiError::unprocessable("rom_missing", "The ROM file is not indexed"))?;
    if kind.as_deref() != Some("rom") {
        return Err(ApiError::unprocessable(
            "not_a_rom",
            "This game is not a ROM entry",
        ));
    }
    let platform_slug = platform_slug.ok_or_else(|| {
        ApiError::unprocessable("unknown_platform", "The ROM has no detected platform")
    })?;
    let emulator = emulator_for(pool, &platform_slug, emulator_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                "emulator_not_configured",
                format!("No enabled emulator is configured for `{platform_slug}`"),
            )
        })?;
    let root = db::query_scalar::<String>(
        "SELECT r.path FROM game_versions v JOIN library_roots r ON r.id = v.root_id
         WHERE v.game_id = ? AND v.missing = 0 ORDER BY v.id DESC LIMIT 1",
    )
    .bind(game_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .unwrap_or_default();
    let rom_path = std::path::Path::new(&root).join(&file_path);
    let command = build_launch_command(&emulator.command, &rom_path)
        .map_err(|error| ApiError::unprocessable("invalid_emulator_command", error.to_string()))?;
    Ok(EmulatorLaunch {
        game_id,
        platform: platform_slug,
        emulator_id: emulator.id,
        emulator_name: emulator.name,
        program: command.program,
        args: command.args,
        rom_path: rom_path.display().to_string(),
    })
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    // SQLite uses 2067/1555, PostgreSQL uses SQLSTATE 23505; the trait method
    // normalises both.
    match error {
        sqlx::Error::Database(database) => database.is_unique_violation(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    async fn state() -> AppState {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.keep();
        let config =
            crate::config::Config::for_local(path.join("games"), path.clone(), path.join("media"));
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        AppState::for_tests(config, tx).await
    }

    #[tokio::test]
    async fn creates_validates_and_resolves_emulators() {
        let state = state().await;
        let dolphin = create(
            &state.pool,
            &CreateEmulatorRequest {
                name: "Dolphin".to_string(),
                command: "dolphin-emu -b -e %ROM%".to_string(),
                platforms: vec!["gamecube".to_string(), "wii".to_string()],
                enabled: None,
            },
        )
        .await
        .expect("create dolphin");
        let retro = create(
            &state.pool,
            &CreateEmulatorRequest {
                name: "RetroArch SNES".to_string(),
                command: "retroarch -L snes9x_libretro.so %ROM%".to_string(),
                platforms: vec!["snes".to_string()],
                enabled: None,
            },
        )
        .await
        .expect("create retroarch");

        assert!(validate_command("retroarch %ROM%").is_ok());
        assert!(validate_command("retroarch").is_err());
        assert!(validate_command("retroarch %ROM%; rm -rf /").is_err());

        set_defaults(
            &state.pool,
            &UpdateDefaultsRequest {
                defaults: vec![PlatformDefault {
                    platform: "snes".to_string(),
                    emulator_id: Some(retro.id),
                }],
            },
        )
        .await
        .expect("set default");
        let resolved = emulator_for(&state.pool, "snes", None)
            .await
            .expect("resolve")
            .expect("default");
        assert_eq!(resolved.id, retro.id);
        let fallback = emulator_for(&state.pool, "wii", None)
            .await
            .expect("resolve")
            .expect("platform match");
        assert_eq!(fallback.id, dolphin.id);

        // An emulator that does not support the platform is ignored.
        assert!(emulator_for(&state.pool, "gba", None)
            .await
            .expect("resolve")
            .is_none());
    }

    #[tokio::test]
    async fn builds_a_launch_command_for_a_rom_game() {
        let state = state().await;
        let root = state.config.games_dir.display().to_string();
        let emulator = create(
            &state.pool,
            &CreateEmulatorRequest {
                name: "Snes9x".to_string(),
                command: "snes9x %ROM%".to_string(),
                platforms: vec!["snes".to_string()],
                enabled: None,
            },
        )
        .await
        .expect("create emulator");
        let game_id = db::query(
            "INSERT INTO games (title, sort_title, group_key, kind, platform, created_at, updated_at)
             VALUES ('Chrono Trigger', 'Chrono Trigger', 'rom::snes::chrono trigger', 'rom', 'snes', '2026-01-01', '2026-01-01')",
        )
        .returning_id(&state.pool)
        .await
        .expect("insert game");
        db::query(
            "INSERT INTO game_versions (game_id, root_id, file_path, file_name, size, missing, root_unavailable, indexed_at, updated_at)
             VALUES (?, (SELECT id FROM library_roots LIMIT 1), 'Chrono Trigger.sfc', 'Chrono Trigger.sfc', 4096, 0, 0, '2026-01-01', '2026-01-01')",
        )
        .bind(game_id)
        .execute(&state.pool)
        .await
        .expect("insert version");
        let launch = launch_for_game(&state.pool, game_id)
            .await
            .expect("launch command");
        assert_eq!(launch.emulator_id, emulator.id);
        assert_eq!(launch.program, "snes9x");
        assert_eq!(launch.args[0], format!("{root}/Chrono Trigger.sfc"));
    }

    #[tokio::test]
    async fn rejects_unknown_platforms_and_duplicate_names() {
        let state = state().await;
        let request = CreateEmulatorRequest {
            name: "Bad".to_string(),
            command: "emu %ROM%".to_string(),
            platforms: vec!["not-a-platform".to_string()],
            enabled: None,
        };
        assert!(create(&state.pool, &request).await.is_err());
        let request = CreateEmulatorRequest {
            name: "Dolphin".to_string(),
            command: "dolphin %ROM%".to_string(),
            platforms: vec!["gamecube".to_string()],
            enabled: None,
        };
        create(&state.pool, &request).await.expect("first");
        assert!(create(&state.pool, &request).await.is_err());
    }
}
