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

use crate::db::Database;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::config::Config;
use crate::db;
use crate::error::ApiError;
use crate::mounts::RootMountStatus;
use ludex_core::mount::DEFAULT_MIN_FREE_BYTES;

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
    /// `game` for an archive library, `rom` for a ROM/emulator library.
    pub kind: String,
    /// True when the configured mount is absent (#290): the root is offline,
    /// not deleted.
    pub offline: i64,
    /// Free-space floor kept free on the root, in bytes (#290).
    pub min_free_bytes: i64,
    /// Device id of the mount that served the root at the last scan.
    pub mount_device_id: Option<String>,
    /// Mount-point marker of that mount (mtime, Unix seconds).
    pub mount_marker: Option<String>,
}

/// Public root shape returned by `/api/library/roots`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
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
    /// `game` for an archive library, `rom` for a ROM/emulator library.
    pub kind: String,
    /// True when the configured mount is absent (#290). The root is kept with
    /// its rows; nothing is marked missing while it is offline.
    pub offline: bool,
    /// Free space kept free on this library, in bytes (#290).
    pub min_free_bytes: u64,
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
    kind: String,
    offline: i64,
    min_free_bytes: i64,
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
            kind: self.kind,
            offline: self.offline != 0,
            min_free_bytes: u64::try_from(self.min_free_bytes).unwrap_or(0),
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
            kind: self.kind.clone(),
            offline: self.offline != 0,
            min_free_bytes: u64::try_from(self.min_free_bytes).unwrap_or(0),
        }
    }
}

/// Lists every root in scan order with its version count.
pub async fn list(pool: &Database) -> Result<Vec<LibraryRootView>> {
    let rows = db::query_as::<RootRow>(
        "SELECT r.id, r.label, r.path, r.enabled, r.available, r.position,
                r.created_at, r.last_scan_at, r.last_error,
                (SELECT COUNT(*) FROM game_versions v WHERE v.root_id = r.id) AS version_count,
                r.kind, r.offline, r.min_free_bytes
         FROM library_roots r
         ORDER BY r.position, r.id",
    )
    .fetch_all(pool)
    .await
    .context("list library roots")?;
    Ok(rows.into_iter().map(RootRow::into_view).collect())
}

/// Enabled roots in scan order.
pub async fn enabled(pool: &Database) -> Result<Vec<LibraryRoot>> {
    db::query_as::<LibraryRoot>(
        "SELECT id, label, path, enabled, available, position, created_at,
                last_scan_at, last_error, kind, offline, min_free_bytes,
                mount_device_id, mount_marker
         FROM library_roots WHERE enabled = 1 ORDER BY position, id",
    )
    .fetch_all(pool)
    .await
    .context("list enabled library roots")
}

/// One root by id.
pub async fn get(pool: &Database, id: i64) -> Result<Option<LibraryRoot>> {
    db::query_as::<LibraryRoot>(
        "SELECT id, label, path, enabled, available, position, created_at,
                last_scan_at, last_error, kind, offline, min_free_bytes,
                mount_device_id, mount_marker
         FROM library_roots WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("read library root")
}

/// Number of versions stored under a root.
pub async fn version_count(pool: &Database, id: i64) -> Result<i64> {
    db::query_scalar::<i64>("SELECT COUNT(*) FROM game_versions WHERE root_id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .context("count root versions")
}

/// Creates a root at the end of the list.
pub async fn create(
    pool: &Database,
    label: &str,
    path: &str,
    enabled: bool,
    kind: &str,
) -> Result<i64> {
    create_with_floor(pool, label, path, enabled, kind, DEFAULT_MIN_FREE_BYTES).await
}

/// Creates a root with an explicit free-space floor (#290).
pub async fn create_with_floor(
    pool: &Database,
    label: &str,
    path: &str,
    enabled: bool,
    kind: &str,
    min_free_bytes: u64,
) -> Result<i64> {
    let position =
        db::query_scalar::<i64>("SELECT COALESCE(MAX(position), -1) + 1 FROM library_roots")
            .fetch_one(pool)
            .await
            .context("next root position")?;
    let now = db::db_time(db::now());
    db::query(
        "INSERT INTO library_roots
            (label, path, enabled, available, position, created_at, updated_at, kind,
             min_free_bytes)
         VALUES (?, ?, ?, 1, ?, ?, ?, ?, ?)",
    )
    .bind(label)
    .bind(path)
    .bind(i64::from(enabled))
    .bind(position)
    .bind(&now)
    .bind(&now)
    .bind(kind)
    .bind(i64::try_from(min_free_bytes).unwrap_or(i64::MAX))
    .returning_id(pool)
    .await
    .context("insert library root")
}

/// Records the outcome of a scan for one root.
pub async fn record_scan(
    pool: &Database,
    id: i64,
    available: bool,
    error: Option<&str>,
) -> Result<()> {
    db::query(
        "UPDATE library_roots SET available = ?, offline = 0, last_error = ?, last_scan_at = ?,
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

/// Records a mount check (#290): online stores the identity fingerprint,
/// offline keeps the rows and stores the human message.
pub async fn record_mount(pool: &Database, id: i64, status: &RootMountStatus) -> Result<()> {
    let now = db::db_time(db::now());
    match status {
        RootMountStatus::Online {
            mount_point,
            identity,
        } => {
            db::query(
                "UPDATE library_roots SET available = 1, offline = 0, last_error = NULL,
                    mount_device_id = ?, mount_marker = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(identity.device_id.to_string())
            .bind(identity.marker.map(|value| value.to_string()))
            .bind(&now)
            .bind(id)
            .execute(pool)
            .await
            .context("update library root mount state")?;
            tracing::debug!(root_id = id, mount = %mount_point, "library root mount is online");
        }
        RootMountStatus::Offline { message, .. } => {
            db::query(
                "UPDATE library_roots SET available = 0, offline = 1, last_error = ?,
                    updated_at = ? WHERE id = ?",
            )
            .bind(message)
            .bind(&now)
            .bind(id)
            .execute(pool)
            .await
            .context("update library root offline state")?;
        }
    }
    Ok(())
}

/// Records a successful walk and the mount identity that served it.
pub async fn record_scanned(
    pool: &Database,
    id: i64,
    identity: &ludex_core::mount::MountIdentity,
) -> Result<()> {
    let now = db::db_time(db::now());
    db::query(
        "UPDATE library_roots SET available = 1, offline = 0, last_error = NULL,
            last_scan_at = ?, mount_device_id = ?, mount_marker = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&now)
    .bind(identity.device_id.to_string())
    .bind(identity.marker.map(|value| value.to_string()))
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await
    .context("update library root scan state")?;
    Ok(())
}

/// Records a reconnect that reused the stored index (#290): the root is online
/// again but `last_scan_at` is left untouched, because nothing was walked.
pub async fn record_resumed(
    pool: &Database,
    id: i64,
    identity: &ludex_core::mount::MountIdentity,
) -> Result<()> {
    db::query(
        "UPDATE library_roots SET available = 1, offline = 0, last_error = NULL,
            mount_device_id = ?, mount_marker = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(identity.device_id.to_string())
    .bind(identity.marker.map(|value| value.to_string()))
    .bind(db::db_time(db::now()))
    .bind(id)
    .execute(pool)
    .await
    .context("update resumed library root")?;
    Ok(())
}

/// Free-space floor of one root, in bytes. A missing root falls back to the
/// 5 GiB default; `LUDEX_MIN_FREE_BYTES` overrides every root (#290).
pub async fn min_free_bytes(pool: &Database, config: &Config, root_id: Option<i64>) -> u64 {
    if let Some(floor) = config.disk_floor_bytes {
        return floor;
    }
    let Some(root_id) = root_id else {
        return default_min_free_bytes(pool, config).await;
    };
    db::query_scalar::<i64>("SELECT min_free_bytes FROM library_roots WHERE id = ?")
        .bind(root_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(DEFAULT_MIN_FREE_BYTES)
}

/// Floor of the ingest root (first configured root), used by uploads.
pub async fn default_min_free_bytes(pool: &Database, config: &Config) -> u64 {
    if let Some(floor) = config.disk_floor_bytes {
        return floor;
    }
    db::query_scalar::<i64>(
        "SELECT min_free_bytes FROM library_roots ORDER BY position, id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .and_then(|value| u64::try_from(value).ok())
    .unwrap_or(DEFAULT_MIN_FREE_BYTES)
}

/// Updates the floor of one root (#290).
pub async fn set_min_free_bytes(pool: &Database, id: i64, min_free_bytes: u64) -> Result<()> {
    db::query("UPDATE library_roots SET min_free_bytes = ?, updated_at = ? WHERE id = ?")
        .bind(i64::try_from(min_free_bytes).unwrap_or(i64::MAX))
        .bind(db::db_time(db::now()))
        .bind(id)
        .execute(pool)
        .await
        .context("update library root floor")?;
    Ok(())
}

/// Fills the seeded root, inserts configured roots and assigns legacy rows.
///
/// Idempotent. Called once per server start before the first scan.
pub async fn bootstrap(pool: &Database, config: &Config) -> Result<()> {
    let first = config
        .library_roots
        .first()
        .cloned()
        .unwrap_or_else(|| config.games_dir.clone());
    let placeholder = db::query_as::<(i64, String)>(
        "SELECT id, path FROM library_roots WHERE TRIM(path) = '' ORDER BY position, id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("find seeded library root")?;
    if let Some((id, _)) = placeholder {
        db::query("UPDATE library_roots SET path = ?, label = ?, updated_at = ? WHERE id = ?")
            .bind(first.display().to_string())
            .bind(DEFAULT_ROOT_LABEL)
            .bind(db::db_time(db::now()))
            .bind(id)
            .execute(pool)
            .await
            .context("backfill seeded library root")?;
        tracing::info!(root = %first.display(), "backfilled library root #1");
    }

    let existing = db::query_scalar::<String>("SELECT path FROM library_roots")
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
        db::query(
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
        db::query_scalar::<i64>("SELECT id FROM library_roots ORDER BY position, id LIMIT 1")
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
        let updated = db::query("UPDATE game_versions SET root_id = ? WHERE root_id IS NULL")
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
        db::query("UPDATE games SET root_id = ? WHERE root_id IS NULL")
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
pub async fn base_dir(pool: &Database, config: &Config, root_id: Option<i64>) -> Result<PathBuf> {
    if let Some(id) = root_id {
        if let Some(path) =
            db::query_scalar::<String>("SELECT path FROM library_roots WHERE id = ?")
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
    pool: &Database,
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

    #[tokio::test]
    async fn an_offline_root_keeps_its_rows_and_comes_back_online() {
        let pool = crate::db::connect_for_tests("sqlite::memory:")
            .await
            .expect("database");
        let id = create_with_floor(&pool, "NAS", "/mnt/nas", true, "game", 7 << 30)
            .await
            .expect("create root");
        // A version indexed under the root must survive the offline transition.
        let now = db::db_time(db::now());
        let game_id =
            db::query("INSERT INTO games (title, sort_title, created_at, updated_at) VALUES ('Doom', 'doom', ?, ?)")
                .bind(&now)
                .bind(&now)
                .returning_id(&pool)
                .await
                .expect("game");
        db::query(
            "INSERT INTO game_versions
                (game_id, root_id, file_path, file_name, size, missing, indexed_at)
             VALUES (?, ?, 'Doom (1993).zip', 'Doom (1993).zip', 10, 0, ?)",
        )
        .bind(game_id)
        .bind(id)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("version");
        let created = get(&pool, id).await.expect("read root").expect("root");
        assert_eq!(created.min_free_bytes, 7 << 30);
        assert_eq!(created.offline, 0);

        // The mount is absent: offline, with the human message, never deleted.
        record_mount(
            &pool,
            id,
            &RootMountStatus::Offline {
                mount_point: Some("/mnt/nas".to_string()),
                message: "The library at /mnt/nas is offline: its mount /mnt/nas is not mounted."
                    .to_string(),
                absent_mount: true,
            },
        )
        .await
        .expect("mark offline");
        let offline = get(&pool, id).await.expect("read root").expect("root");
        assert_eq!(offline.offline, 1);
        assert_eq!(offline.available, 0);
        assert!(offline
            .last_error
            .as_deref()
            .is_some_and(|message| message.contains("offline")));
        assert_eq!(version_count(&pool, id).await.expect("versions"), 1);
        let (missing, unavailable) = db::query_as::<(i64, i64)>(
            "SELECT missing, root_unavailable FROM game_versions WHERE root_id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("version row");
        // Marking a root offline never deletes or hides its versions.
        assert_eq!((missing, unavailable), (0, 0));

        // Reconnected with the same identity: online again, index reused.
        let identity = ludex_core::mount::MountIdentity {
            device_id: 259,
            marker: Some(1_700_000_000),
        };
        record_resumed(&pool, id, &identity).await.expect("resume");
        let online = get(&pool, id).await.expect("read root").expect("root");
        assert_eq!(online.offline, 0);
        assert_eq!(online.available, 1);
        assert!(online.last_error.is_none());
        assert!(online.last_scan_at.is_none(), "a resume does not rescan");
        assert_eq!(online.mount_device_id.as_deref(), Some("259"));
        assert_eq!(online.mount_marker.as_deref(), Some("1700000000"));

        // The migration seeds a placeholder root; ours is the one we created.
        let listed = list(&pool)
            .await
            .expect("list roots")
            .into_iter()
            .find(|root| root.id == id)
            .expect("created root");
        assert!(!listed.offline);
        assert_eq!(listed.min_free_bytes, 7 << 30);
        pool.close().await;
    }

    #[tokio::test]
    async fn a_scan_records_the_mount_identity() {
        let pool = crate::db::connect_for_tests("sqlite::memory:")
            .await
            .expect("database");
        let id = create(&pool, "Games", "/srv/games", true, "game")
            .await
            .expect("create root");
        let identity = ludex_core::mount::MountIdentity {
            device_id: 42,
            marker: None,
        };
        record_scanned(&pool, id, &identity).await.expect("scan");
        let root = get(&pool, id).await.expect("read root").expect("root");
        assert_eq!(root.mount_device_id.as_deref(), Some("42"));
        assert!(root.mount_marker.is_none());
        assert!(root.last_scan_at.is_some());
        assert_eq!(
            root.min_free_bytes,
            i64::try_from(DEFAULT_MIN_FREE_BYTES).unwrap()
        );
        pool.close().await;
    }

    #[test]
    fn labels_come_from_the_last_path_component() {
        assert_eq!(default_label(Path::new("/mnt/retro")), "retro");
        assert_eq!(default_label(Path::new("/")), DEFAULT_ROOT_LABEL);
    }
}
