//! Multiple library roots (#45).
//!
//! The library can span several volumes: `/games`, `/mnt/retro`, a NAS mount…
//! Roots live in the `library_roots` table (label, path, enabled, position) so
//! they can be added, renamed, disabled or removed at runtime without a
//! restart. The first configured root (`LUDEX_GAMES_DIR`) is the ingest target
//! and is always kept first: new downloads land there.
//!
//! A missing or unmounted root is marked `available = 0` and its versions are
//! flagged `root_unavailable` instead of `missing`, so an unplugged disk never
//! looks like a deleted game.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::config::Config;
use crate::db;
use crate::error::ApiError;

/// Label given to the root seeded from the previous single-directory config.
pub const DEFAULT_ROOT_LABEL: &str = "Library";

/// Row of the `library_roots` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LibraryRoot {
    /// Row id.
    pub id: i64,
    /// Human-readable label.
    pub label: String,
    /// Absolute path (or a config-relative path for the seeded root).
    pub path: String,
    /// Whether the scanner walks this root.
    pub enabled: i64,
    /// Whether the path existed and was readable at the last scan.
    pub available: i64,
    /// Ordering, ascending; ties fall back to id.
    pub position: i64,
    /// Creation timestamp.
    pub created_at: String,
    /// Last time a scan walked this root.
    pub last_scan_at: Option<String>,
    /// Why the root was marked unavailable.
    pub last_error: Option<String>,
}

/// Public root shape returned by `/api/library/roots`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRootView {
    /// Row id.
    pub id: i64,
    /// Human-readable label.
    pub label: String,
    /// Path on the server.
    pub path: String,
    /// Whether the scanner walks this root.
    pub enabled: bool,
    /// Whether the path was readable at the last scan.
    pub available: bool,
    /// Ordering, ascending.
    pub position: i64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last time a scan walked this root.
    pub last_scan_at: Option<DateTime<Utc>>,
    /// Why the root was marked unavailable.
    pub last_error: Option<String>,
    /// Indexed versions stored under this root.
    pub version_count: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct RootRow {
    id: i64,
    label: String,
    path: String,
    enabled: i64,
    available: i64,
    position: i64,
    created_at: String,
    last_scan_at: Option<String>,
    last_error: Option<String>,
    version_count: i64,
}

impl RootRow {
    fn into_view(self) -> LibraryRootView {
        LibraryRootView {
            id: self.id,
            label: self.label,
            path: self.path,
            enabled: self.enabled != 0,
            available: self.available != 0,
            position: self.position,
            created_at: db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            last_scan_at: self.last_scan_at.as_deref().and_then(db::parse_time),
            last_error: self.last_error,
            version_count: self.version_count,
        }
    }
}

impl LibraryRoot {
    /// Public representation.
    #[must_use]
    pub fn to_view(&self, version_count: i64) -> LibraryRootView {
        LibraryRootView {
            id: self.id,
            label: self.label.clone(),
            path: self.path.clone(),
            enabled: self.enabled != 0,
            available: self.available != 0,
            position: self.position,
            created_at: db::parse_time(&self.created_at).unwrap_or_else(Utc::now),
            last_scan_at: self.last_scan_at.as_deref().and_then(db::parse_time),
            last_error: self.last_error.clone(),
            version_count,
        }
    }
}

/// Lists every root in scan order with its version count.
pub async fn list(pool: &SqlitePool) -> Result<Vec<LibraryRootView>> {
    let rows = sqlx::query_as::<_, RootRow>(
        "SELECT r.id, r.label, r.path, r.enabled, r.available, r.position,
                r.created_at, r.last_scan_at, r.last_error,
                (SELECT COUNT(*) FROM game_versions v WHERE v.root_id = r.id) AS version_count
         FROM library_roots r
         ORDER BY r.position, r.id",
    )
    .fetch_all(pool)
    .await
    .context("list library roots")?;
    Ok(rows.into_iter().map(RootRow::into_view).collect())
}

/// Enabled roots in scan order.
pub async fn enabled(pool: &SqlitePool) -> Result<Vec<LibraryRoot>> {
    sqlx::query_as::<_, LibraryRoot>(
        "SELECT id, label, path, enabled, available, position, created_at,
                last_scan_at, last_error
         FROM library_roots WHERE enabled = 1 ORDER BY position, id",
    )
    .fetch_all(pool)
    .await
    .context("list enabled library roots")
}

/// One root by id.
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<LibraryRoot>> {
    sqlx::query_as::<_, LibraryRoot>(
        "SELECT id, label, path, enabled, available, position, created_at,
                last_scan_at, last_error
         FROM library_roots WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("read library root")
}

/// Number of versions stored under a root.
pub async fn version_count(pool: &SqlitePool, id: i64) -> Result<i64> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM game_versions WHERE root_id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .context("count root versions")
}

/// Creates a root at the end of the list.
pub async fn create(pool: &SqlitePool, label: &str, path: &str, enabled: bool) -> Result<i64> {
    let position =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(position), -1) + 1 FROM library_roots")
            .fetch_one(pool)
            .await
            .context("next root position")?;
    let now = db::db_time(db::now());
    let result = sqlx::query(
        "INSERT INTO library_roots
            (label, path, enabled, available, position, created_at, updated_at)
         VALUES (?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(label)
    .bind(path)
    .bind(i64::from(enabled))
    .bind(position)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .context("insert library root")?;
    Ok(result.last_insert_rowid())
}

/// Records the outcome of a scan for one root.
pub async fn record_scan(
    pool: &SqlitePool,
    id: i64,
    available: bool,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE library_roots SET available = ?, last_error = ?, last_scan_at = ?,
            updated_at = ? WHERE id = ?",
    )
    .bind(i64::from(available))
    .bind(error)
    .bind(db::db_time(db::now()))
    .bind(db::db_time(db::now()))
    .bind(id)
    .execute(pool)
    .await
    .context("update library root scan state")?;
    Ok(())
}

/// Fills the seeded root, inserts configured roots and assigns legacy rows.
///
/// Idempotent. Called once per server start before the first scan.
pub async fn bootstrap(pool: &SqlitePool, config: &Config) -> Result<()> {
    let first = config
        .library_roots
        .first()
        .cloned()
        .unwrap_or_else(|| config.games_dir.clone());
    let placeholder = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, path FROM library_roots WHERE TRIM(path) = '' ORDER BY position, id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("find seeded library root")?;
    if let Some((id, _)) = placeholder {
        sqlx::query("UPDATE library_roots SET path = ?, label = ?, updated_at = ? WHERE id = ?")
            .bind(first.display().to_string())
            .bind(DEFAULT_ROOT_LABEL)
            .bind(db::db_time(db::now()))
            .bind(id)
            .execute(pool)
            .await
            .context("backfill seeded library root")?;
        tracing::info!(root = %first.display(), "backfilled library root #1");
    }

    let existing = sqlx::query_scalar::<_, String>("SELECT path FROM library_roots")
        .fetch_all(pool)
        .await
        .context("list library root paths")?;
    for (index, path) in config.library_roots.iter().enumerate() {
        let value = path.display().to_string();
        if existing.iter().any(|known| known == &value) {
            continue;
        }
        let position = i64::try_from(index).unwrap_or(i64::MAX);
        let now = db::db_time(db::now());
        sqlx::query(
            "INSERT INTO library_roots
                (label, path, enabled, available, position, created_at, updated_at)
             VALUES (?, ?, 1, 1, ?, ?, ?)",
        )
        .bind(default_label(path))
        .bind(&value)
        .bind(position)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await
        .context("insert configured library root")?;
        tracing::info!(root = %value, "registered configured library root");
    }

    let first_id =
        sqlx::query_scalar::<_, i64>("SELECT id FROM library_roots ORDER BY position, id LIMIT 1")
            .fetch_optional(pool)
            .await
            .context("find primary library root")?;
    // Rows indexed before this migration were relative to `LUDEX_GAMES_DIR`.
    // Run the backfill once: versions orphaned by a deleted root must keep
    // `root_id = NULL` and never be reattributed to another root.
    let backfilled = db::get_setting(pool, "library_roots.backfilled")
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some("1");
    if let (Some(root_id), false) = (first_id, backfilled) {
        let updated = sqlx::query("UPDATE game_versions SET root_id = ? WHERE root_id IS NULL")
            .bind(root_id)
            .execute(pool)
            .await
            .context("assign legacy versions to root #1")?;
        if updated.rows_affected() > 0 {
            tracing::info!(
                rows = updated.rows_affected(),
                "assigned legacy versions to library root #1"
            );
        }
        sqlx::query("UPDATE games SET root_id = ? WHERE root_id IS NULL")
            .bind(root_id)
            .execute(pool)
            .await
            .context("assign legacy games to root #1")?;
        db::set_setting(pool, "library_roots.backfilled", "1").await?;
    }
    Ok(())
}

fn default_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map_or_else(|| DEFAULT_ROOT_LABEL.to_string(), str::to_string)
}

/// Validates a root path supplied through the API.
///
/// Only absolute paths are accepted: a relative root would resolve against the
/// server's working directory and silently change after a restart.
pub fn validate_path(raw: &str) -> Result<String, ApiError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_root_path",
            "A root path is required",
        ));
    }
    if value.chars().count() > 4096 || value.contains('\0') {
        return Err(ApiError::bad_request(
            "invalid_root_path",
            "The root path is invalid",
        ));
    }
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(ApiError::bad_request(
            "invalid_root_path",
            "The root path must be absolute",
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ApiError::bad_request(
            "invalid_root_path",
            "The root path must not contain '..'",
        ));
    }
    Ok(value.to_string())
}

/// Base directory for a version row.
///
/// `root_id = NULL` means the row predates multiple roots or its root was
/// deleted; it falls back to `LUDEX_GAMES_DIR`, which is what those rows were
/// relative to.
pub async fn base_dir(pool: &SqlitePool, config: &Config, root_id: Option<i64>) -> Result<PathBuf> {
    if let Some(id) = root_id {
        if let Some(path) =
            sqlx::query_scalar::<_, String>("SELECT path FROM library_roots WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await
                .context("read library root path")?
        {
            if !path.trim().is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }
    Ok(config.games_dir.clone())
}

/// Resolves a version's `file_path` to an absolute path, rejecting traversal.
pub async fn resolve_version_path(
    pool: &SqlitePool,
    config: &Config,
    root_id: Option<i64>,
    relative: &str,
) -> Option<PathBuf> {
    let base = base_dir(pool, config, root_id).await.ok()?;
    crate::scanner::resolve_relative(&base, relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_paths_must_be_absolute_and_traversal_free() {
        assert_eq!(
            validate_path(" /mnt/games ").expect("valid"),
            "/mnt/games".to_string()
        );
        assert!(validate_path("").is_err());
        assert!(validate_path("games").is_err());
        assert!(validate_path("/mnt/../etc").is_err());
        assert!(validate_path("/mnt/\0bad").is_err());
    }

    #[test]
    fn labels_come_from_the_last_path_component() {
        assert_eq!(default_label(Path::new("/mnt/retro")), "retro");
        assert_eq!(default_label(Path::new("/")), DEFAULT_ROOT_LABEL);
    }
}
