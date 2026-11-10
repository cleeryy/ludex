//! Consistent backup and restore for Ludex (#49).
//!
//! A backup is a `.tar.gz` containing:
//!
//! - `backup.json` — format version, counters, storage layout and a SHA-256
//!   checksum for every archived file;
//! - `config.json` — non-secret server configuration (no credentials);
//! - `ludex.db` — a consistent SQLite snapshot taken with `VACUUM INTO`;
//! - `media/` — cached artwork;
//! - `saves/` — cloud save snapshots.
//!
//! Game archives are **not** included: they are large and already live on the
//! library roots or in the S3 bucket. See `docs/BACKUP.md`.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::config::{Config, StorageBackend};
use crate::storage::ArchiveStorage;

/// Version of the archive layout understood by this build.
pub const BACKUP_FORMAT_VERSION: u32 = 1;

/// Options for [`create_backup`].
#[derive(Debug, Clone)]
pub struct BackupRequest {
    /// Server data directory (`ludex.db`, `saves/`, `jwt.secret`).
    pub data_dir: PathBuf,
    /// Media cache directory.
    pub media_dir: PathBuf,
    /// Games directory, only used for the informational config manifest.
    pub games_dir: PathBuf,
    /// SQLite URL.
    pub database_url: String,
    /// Destination archive. Defaults to `{data_dir}/backups/ludex-backup-<ts>.tar.gz`.
    pub output: Option<PathBuf>,
    /// Validate and measure without writing the archive.
    pub dry_run: bool,
    /// Include `media/` (default true; `--no-media` disables it).
    pub include_media: bool,
    /// Upload the finished archive with the configured S3 storage.
    pub upload_s3: bool,
    /// Storage used by `--upload-s3`.
    pub storage: Option<ArchiveStorage>,
    /// Original configuration, used to write `config.json`.
    pub config: Option<Config>,
}

/// Result of a backup run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupReport {
    /// Archive path (or planned path in dry-run).
    pub path: String,
    /// Archive size in bytes (0 in dry-run).
    pub bytes: u64,
    /// Files included in the snapshot.
    pub files: usize,
    /// Whether the archive was actually written.
    pub dry_run: bool,
    /// Object key when `--upload-s3` succeeded.
    pub uploaded_key: Option<String>,
    /// Creation timestamp written in `backup.json`.
    pub created_at: DateTime<Utc>,
}

/// Options for [`restore_backup`].
#[derive(Debug, Clone)]
pub struct RestoreRequest {
    /// Archive to restore.
    pub archive: PathBuf,
    /// Destination data directory.
    pub data_dir: PathBuf,
    /// Destination media directory.
    pub media_dir: PathBuf,
    /// Restore even when `ludex.db` or media/saves already contain data.
    pub force: bool,
    /// Verify everything and report what would change without writing.
    pub dry_run: bool,
    /// Restore `media/` (default true).
    pub include_media: bool,
}

/// Result of a restore or verify run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    /// Whether nothing was written.
    pub dry_run: bool,
    /// Whether the destination already had data.
    pub overwrote: bool,
    /// Files verified in the archive.
    pub files: usize,
    /// Bytes extracted (or that would be extracted in dry-run).
    pub bytes: u64,
    /// Counter from the backup manifest.
    pub games: i64,
    /// Counter from the backup manifest.
    pub versions: i64,
    /// Counter from the backup manifest.
    pub users: i64,
    /// Manifest creation time.
    pub created_at: DateTime<Utc>,
    /// Archived `config.json`, informational only.
    pub config: BackupConfig,
}

/// `backup.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    /// Layout version.
    pub format_version: u32,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Server version that produced the archive.
    pub server_version: String,
    /// Highest applied SQLx migration.
    pub schema_version: i64,
    /// Row counters read from the snapshot database.
    pub counts: BackupCounts,
    /// Storage layout, informational for restore.
    pub storage: BackupStorage,
    /// Every archived file with its checksum.
    pub files: Vec<BackupFile>,
}

/// Row counters.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackupCounts {
    /// Games.
    pub games: i64,
    /// Versions, including missing ones.
    pub versions: i64,
    /// Users.
    pub users: i64,
    /// Cloud save snapshots.
    pub saves: i64,
}

/// Storage layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStorage {
    /// Backend identifier (`local` or `s3`).
    pub backend: String,
    /// S3 bucket when configured.
    pub bucket: Option<String>,
    /// S3 key prefix when configured.
    pub prefix: Option<String>,
}

/// One file entry in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    /// Path relative to the archive root.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256.
    pub sha256: String,
}

/// `config.json`, safe to commit: no secret is ever written.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupConfig {
    /// Server display name.
    pub server_name: String,
    /// Server version.
    pub server_version: String,
    /// Local mode flag.
    pub local_mode: bool,
    /// Game archive backend.
    pub storage_backend: String,
    /// S3 bucket, when configured.
    pub s3_bucket: Option<String>,
    /// S3 endpoint, when configured.
    pub s3_endpoint: Option<String>,
    /// S3 region, when configured.
    pub s3_region: Option<String>,
    /// S3 key prefix, when configured.
    pub s3_prefix: Option<String>,
    /// Library roots.
    pub library_roots: Vec<String>,
    /// Configured scan interval, in seconds.
    pub scan_interval_seconds: u64,
    /// Whether the folder watcher is enabled.
    pub watcher_enabled: bool,
    /// Whether IGDB credentials are configured.
    pub igdb_enabled: bool,
    /// Whether OIDC is configured.
    pub oidc_enabled: bool,
}

impl BackupConfig {
    fn from_config(config: &Config) -> Self {
        Self {
            server_name: config.server_name.clone(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            local_mode: config.local_mode,
            storage_backend: config.storage_backend.as_str().to_string(),
            s3_bucket: config.s3.as_ref().map(|s3| s3.bucket.clone()),
            s3_endpoint: config
                .s3
                .as_ref()
                .and_then(|s3| s3.endpoint.as_deref())
                .map(redact_endpoint),
            s3_region: config.s3.as_ref().map(|s3| s3.region.clone()),
            s3_prefix: config.s3.as_ref().and_then(|s3| s3.prefix.clone()),
            library_roots: config
                .library_roots
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            scan_interval_seconds: config.scan_interval.as_secs(),
            watcher_enabled: config.watcher_enabled,
            igdb_enabled: config.igdb_enabled(),
            oidc_enabled: config.oidc_enabled(),
        }
    }

    fn placeholder() -> Self {
        Self {
            server_name: "Ludex".to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            local_mode: false,
            storage_backend: StorageBackend::Local.as_str().to_string(),
            s3_bucket: None,
            s3_endpoint: None,
            s3_region: None,
            s3_prefix: None,
            library_roots: Vec::new(),
            scan_interval_seconds: 0,
            watcher_enabled: false,
            igdb_enabled: false,
            oidc_enabled: false,
        }
    }
}

/// Strips credentials from an S3 endpoint before it enters a backup manifest.
fn redact_endpoint(endpoint: &str) -> String {
    match url::Url::parse(endpoint) {
        Ok(mut url) => {
            if !url.username().is_empty() {
                let _ = url.set_username("<redacted>");
            }
            if url.password().is_some() {
                let _ = url.set_password(Some("<redacted>"));
            }
            url.to_string()
        }
        Err(_) => "<invalid>".to_string(),
    }
}

/// Creates a backup archive.
pub async fn create_backup(request: &BackupRequest) -> Result<BackupReport> {
    tokio::fs::create_dir_all(&request.data_dir)
        .await
        .with_context(|| format!("create data directory {}", request.data_dir.display()))?;
    let created_at = Utc::now();
    let output = request.output.clone().unwrap_or_else(|| {
        request.data_dir.join("backups").join(format!(
            "ludex-backup-{}.tar.gz",
            created_at.format("%Y%m%d-%H%M%S")
        ))
    });
    if let Some(parent) = output.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create backup directory {}", parent.display()))?;
    }
    if request.upload_s3
        && !request
            .storage
            .as_ref()
            .is_some_and(|storage| storage.is_remote())
    {
        bail!("--upload-s3 requires LUDEX_STORAGE_BACKEND=s3 with valid S3 settings");
    }

    let staging = tempfile::Builder::new()
        .prefix(".ludex-backup-")
        .tempdir_in(&request.data_dir)
        .context("create backup staging directory")?;
    let staging_path = staging.path().to_path_buf();

    let database = crate::db::sqlite_path_from_url(&request.database_url)
        .context("LUDEX_DATABASE_URL must point to a SQLite file")?;
    if !database.is_file() {
        bail!(
            "database {} does not exist; run the server once before backing up",
            database.display()
        );
    }

    snapshot_database(&database, &staging_path.join("ludex.db")).await?;
    copy_tree(
        &request.data_dir.join("saves"),
        &staging_path.join("saves"),
        false,
    )?;
    if request.include_media {
        copy_tree(&request.media_dir, &staging_path.join("media"), true)?;
    }

    let config = request
        .config
        .as_ref()
        .map(BackupConfig::from_config)
        .unwrap_or_else(BackupConfig::placeholder);
    tokio::fs::write(
        staging_path.join("config.json"),
        serde_json::to_vec_pretty(&config)?,
    )
    .await
    .context("write config manifest")?;

    let (counts, schema_version) = snapshot_counts(&staging_path.join("ludex.db")).await?;
    let storage = request
        .config
        .as_ref()
        .map(|config| BackupStorage {
            backend: config.storage_backend.as_str().to_string(),
            bucket: config.s3.as_ref().map(|s3| s3.bucket.clone()),
            prefix: config.s3.as_ref().and_then(|s3| s3.prefix.clone()),
        })
        .unwrap_or(BackupStorage {
            backend: StorageBackend::Local.as_str().to_string(),
            bucket: None,
            prefix: None,
        });

    let files = checksum_tree(&staging_path).await?;
    let manifest = BackupManifest {
        format_version: BACKUP_FORMAT_VERSION,
        created_at,
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version,
        counts,
        storage,
        files,
    };
    tokio::fs::write(
        staging_path.join("backup.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await
    .context("write backup manifest")?;

    if request.dry_run {
        let bytes = directory_size(&staging_path);
        return Ok(BackupReport {
            path: output.display().to_string(),
            bytes,
            files: manifest.files.len(),
            dry_run: true,
            uploaded_key: None,
            created_at,
        });
    }

    let archive_path = output.clone();
    let source = staging_path.clone();
    tokio::task::spawn_blocking(move || write_tarball(&source, &archive_path))
        .await
        .context("join archive task")?
        .context("write backup archive")?;
    let bytes = tokio::fs::metadata(&output)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let uploaded_key = if request.upload_s3 {
        let storage = request
            .storage
            .as_ref()
            .filter(|storage| storage.is_remote())
            .context("--upload-s3 requires LUDEX_STORAGE_BACKEND=s3")?;
        let file_name = output
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ludex-backup.tar.gz".to_string());
        let stored = storage
            .upload_file(&format!("backups/{file_name}"), &output)
            .await
            .context("upload backup")?
            .context("object storage is not configured")?;
        Some(stored.key)
    } else {
        None
    };

    Ok(BackupReport {
        path: output.display().to_string(),
        bytes,
        files: manifest.files.len(),
        dry_run: false,
        uploaded_key,
        created_at,
    })
}

/// Verifies an archive and restores it when `dry_run` is false.
pub async fn restore_backup(request: &RestoreRequest) -> Result<RestoreReport> {
    let archive = request.archive.clone();
    let staging_parent = if request.data_dir.is_dir() {
        request.data_dir.clone()
    } else {
        std::env::temp_dir()
    };
    let staging = tempfile::Builder::new()
        .prefix(".ludex-restore-")
        .tempdir_in(&staging_parent)
        .context("create restore staging directory")?;
    let staging_path = staging.path().to_path_buf();
    let extract_to = staging_path.clone();
    tokio::task::spawn_blocking(move || extract_tarball(&archive, &extract_to))
        .await
        .context("join extract task")?
        .context("extract backup archive")?;

    let manifest: BackupManifest = serde_json::from_slice(
        &tokio::fs::read(staging_path.join("backup.json"))
            .await
            .context("read backup.json")?,
    )
    .context("parse backup.json")?;
    if manifest.format_version > BACKUP_FORMAT_VERSION {
        bail!(
            "backup format {} is newer than this build supports ({BACKUP_FORMAT_VERSION})",
            manifest.format_version
        );
    }
    let config: BackupConfig = serde_json::from_slice(
        &tokio::fs::read(staging_path.join("config.json"))
            .await
            .context("read config.json")?,
    )
    .unwrap_or_else(|_| BackupConfig::placeholder());

    verify_manifest(&staging_path, &manifest).await?;

    let destination_has_data = target_has_data(&request.data_dir, &request.media_dir).await;
    if destination_has_data && !request.force {
        bail!(
            "destination already contains data ({}); pass --force to overwrite",
            request.data_dir.display()
        );
    }

    let bytes = manifest.files.iter().map(|file| file.size).sum();
    let report = RestoreReport {
        dry_run: request.dry_run,
        overwrote: destination_has_data,
        files: manifest.files.len(),
        bytes,
        games: manifest.counts.games,
        versions: manifest.counts.versions,
        users: manifest.counts.users,
        created_at: manifest.created_at,
        config,
    };
    if request.dry_run {
        return Ok(report);
    }

    tokio::fs::create_dir_all(&request.data_dir)
        .await
        .context("create data directory")?;
    // Remove stale WAL files so SQLite never pairs the restored database with
    // a WAL from the previous installation.
    for suffix in ["ludex.db-wal", "ludex.db-shm"] {
        let _ = tokio::fs::remove_file(request.data_dir.join(suffix)).await;
    }
    move_file(
        &staging_path.join("ludex.db"),
        &request.data_dir.join("ludex.db"),
    )
    .await?;
    restrict_private(&request.data_dir.join("ludex.db")).await;
    merge_tree(&staging_path.join("saves"), &request.data_dir.join("saves")).await?;
    if request.include_media {
        merge_tree(&staging_path.join("media"), &request.media_dir).await?;
    }
    // Keep the informational manifests next to the restored data.
    let _ = tokio::fs::copy(
        staging_path.join("backup.json"),
        request.data_dir.join("last-restore.json"),
    )
    .await;
    let _ = tokio::fs::copy(
        staging_path.join("config.json"),
        request.data_dir.join("last-restore-config.json"),
    )
    .await;

    Ok(report)
}

/// Verifies an archive without touching any destination.
pub async fn verify_backup(archive: &Path) -> Result<BackupManifest> {
    let archive = archive.to_path_buf();
    let staging = tempfile::tempdir().context("create verify directory")?;
    let extract_to = staging.path().to_path_buf();
    tokio::task::spawn_blocking(move || extract_tarball(&archive, &extract_to))
        .await
        .context("join extract task")?
        .context("extract backup archive")?;
    let manifest: BackupManifest = serde_json::from_slice(
        &tokio::fs::read(staging.path().join("backup.json"))
            .await
            .context("read backup.json")?,
    )
    .context("parse backup.json")?;
    verify_manifest(staging.path(), &manifest).await?;
    Ok(manifest)
}

async fn verify_manifest(root: &Path, manifest: &BackupManifest) -> Result<()> {
    let expected: BTreeSet<&str> = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    for file in &manifest.files {
        let path = root.join(&file.path);
        let metadata = tokio::fs::metadata(&path)
            .await
            .with_context(|| format!("{} is listed but missing", file.path))?;
        if metadata.len() != file.size {
            bail!(
                "{} has {} bytes, expected {}",
                file.path,
                metadata.len(),
                file.size
            );
        }
        let actual = hash_file(path).await?;
        if actual != file.sha256 {
            bail!("checksum mismatch for {}", file.path);
        }
    }
    // Every regular file outside the manifest is unexpected; manifests
    // themselves are excluded. This catches truncated extractions.
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.context("walk restore directory")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "backup.json" {
            continue;
        }
        if !expected.contains(relative.as_str()) {
            bail!("archive contains an unexpected file: {relative}");
        }
    }
    Ok(())
}

async fn hash_file(path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || {
        let mut file =
            std::fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok::<_, anyhow::Error>(hex::encode(hasher.finalize()))
    })
    .await
    .context("join hash task")?
}

async fn checksum_tree(root: &Path) -> Result<Vec<BackupFile>> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut files = Vec::new();
        for entry in WalkDir::new(&root).follow_links(false).sort_by_file_name() {
            let entry = entry.context("walk backup directory")?;
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "backup.json" {
                continue;
            }
            let mut file = std::fs::File::open(entry.path())
                .with_context(|| format!("open {}", entry.path().display()))?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            files.push(BackupFile {
                path: relative,
                size: entry.metadata().context("read file metadata")?.len(),
                sha256: hex::encode(hasher.finalize()),
            });
        }
        Ok::<_, anyhow::Error>(files)
    })
    .await
    .context("join checksum task")?
}

/// Copies a directory tree into the staging area. `media` skips temporary
/// files and symlinks.
fn copy_tree(source: &Path, destination: &Path, media: bool) -> Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(destination)
        .with_context(|| format!("create {}", destination.display()))?;
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry.context("walk source directory")?;
        if entry.file_type().is_symlink() {
            continue;
        }
        if entry.file_type().is_dir() {
            let relative = entry.path().strip_prefix(source).unwrap_or(entry.path());
            std::fs::create_dir_all(destination.join(relative))?;
            continue;
        }
        let relative = entry.path().strip_prefix(source).unwrap_or(entry.path());
        // In-progress uploads and editor temporary files are never part of a
        // consistent snapshot. The media cache uses the same convention.
        let name = entry.file_name().to_string_lossy();
        if name.starts_with('.') || name.ends_with(".part") {
            continue;
        }
        let _ = media;
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(entry.path(), &target)
            .with_context(|| format!("copy {}", entry.path().display()))?;
    }
    Ok(())
}

/// Writes a consistent SQLite snapshot with `VACUUM INTO`.
async fn snapshot_database(source: &Path, destination: &Path) -> Result<()> {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
    use sqlx::{ConnectOptions, Connection};

    let options = SqliteConnectOptions::new()
        .filename(source)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(30))
        .log_statements(tracing::log::LevelFilter::Off);
    let mut connection = options
        .connect()
        .await
        .context("open database for backup")?;
    let escaped = destination.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(&mut connection)
        .await
        .context("VACUUM INTO backup")?;
    let _ = connection.close().await;
    Ok(())
}

async fn snapshot_counts(database: &Path) -> Result<(BackupCounts, i64)> {
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{ConnectOptions, Connection};

    let options = SqliteConnectOptions::new()
        .filename(database)
        .read_only(true)
        .log_statements(tracing::log::LevelFilter::Off);
    let mut connection = options.connect().await.context("open snapshot database")?;
    let games = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games")
        .fetch_one(&mut connection)
        .await?;
    let versions = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM game_versions")
        .fetch_one(&mut connection)
        .await?;
    let users = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
        .fetch_one(&mut connection)
        .await?;
    let saves = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM save_snapshots")
        .fetch_one(&mut connection)
        .await
        .unwrap_or(0);
    let schema_version =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(&mut connection)
            .await
            .unwrap_or(0);
    let _ = connection.close().await;
    Ok((
        BackupCounts {
            games,
            versions,
            users,
            saves,
        },
        schema_version,
    ))
}

fn write_tarball(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = destination.with_extension("tar.gz.tmp");
    let file = std::fs::File::create(&temporary)
        .with_context(|| format!("create {}", temporary.display()))?;
    {
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut entries = Vec::new();
        for entry in WalkDir::new(source).follow_links(false).sort_by_file_name() {
            let entry = entry?;
            if entry.file_type().is_file() {
                entries.push(entry.path().to_path_buf());
            }
        }
        for path in entries {
            let relative = path.strip_prefix(source).unwrap_or(&path).to_path_buf();
            builder
                .append_path_with_name(&path, &relative)
                .with_context(|| format!("add {} to archive", relative.display()))?;
        }
        let encoder = builder.into_inner().context("finish tar")?;
        encoder.finish().context("finish gzip")?;
    }
    std::fs::rename(&temporary, destination)
        .with_context(|| format!("move archive to {}", destination.display()))?;
    Ok(())
}

fn extract_tarball(archive: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    let file =
        std::fs::File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    let decoder = GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar.entries().context("read tar entries")?;
    for entry in entries {
        let mut entry = entry.context("read tar entry")?;
        let path = entry.path().context("read tar path")?.into_owned();
        if !safe_relative_path(&path) {
            bail!("archive contains an unsafe path: {}", path.display());
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            bail!("archive contains an unsupported entry: {}", path.display());
        }
        let target = destination.join(&path);
        entry
            .unpack_in(destination)
            .with_context(|| format!("extract {}", path.display()))?;
        if !target.starts_with(destination) {
            bail!("archive entry escapes the destination: {}", path.display());
        }
    }
    Ok(())
}

fn safe_relative_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

async fn target_has_data(data_dir: &Path, media_dir: &Path) -> bool {
    if data_dir.join("ludex.db").is_file() {
        return true;
    }
    for directory in [data_dir.join("saves"), media_dir.to_path_buf()] {
        if let Ok(mut entries) = tokio::fs::read_dir(&directory).await {
            if entries.next_entry().await.ok().flatten().is_some() {
                return true;
            }
        }
    }
    false
}

async fn move_file(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // `rename` replaces an existing file on Unix but fails on Windows.
    if tokio::fs::try_exists(destination).await.unwrap_or(false) {
        tokio::fs::remove_file(destination)
            .await
            .with_context(|| format!("replace {}", destination.display()))?;
    }
    tokio::fs::rename(source, destination)
        .await
        .with_context(|| format!("move {} to {}", source.display(), destination.display()))?;
    Ok(())
}

async fn merge_tree(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    let source_owned = source.to_path_buf();
    let destination_owned = destination.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        for entry in WalkDir::new(&source_owned).follow_links(false) {
            let entry = entry?;
            let relative = entry
                .path()
                .strip_prefix(&source_owned)
                .unwrap_or(entry.path());
            let target = destination_owned.join(relative);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target)?;
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("restore {}", target.display()))?;
        }
        Ok(())
    })
    .await
    .context("join restore task")?
}

async fn restrict_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn directory_size(root: &Path) -> u64 {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_paths_are_rejected() {
        assert!(safe_relative_path(Path::new("media/a.jpg")));
        assert!(safe_relative_path(Path::new("./ludex.db")));
        assert!(!safe_relative_path(Path::new("/etc/passwd")));
        assert!(!safe_relative_path(Path::new("../escape")));
        assert!(!safe_relative_path(Path::new("media/../../escape")));
    }

    #[test]
    fn config_manifest_never_contains_secrets() {
        let mut config = Config::for_local(
            PathBuf::from("/games"),
            PathBuf::from("/data"),
            PathBuf::from("/media"),
        );
        config.storage_backend = StorageBackend::S3;
        config.s3 = Some(crate::config::S3Config {
            endpoint: Some("https://user:pass@minio.lan:9000".to_string()),
            region: "garage".to_string(),
            bucket: "ludex".to_string(),
            access_key: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_key: "super-secret-value".to_string(),
            path_style: true,
            prefix: Some("games".to_string()),
        });
        let manifest = serde_json::to_string(&BackupConfig::from_config(&config)).expect("json");
        assert!(!manifest.contains("super-secret-value"));
        assert!(!manifest.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!manifest.contains("pass@"));
        assert!(manifest.contains("\"s3Bucket\":\"ludex\""));
    }
}
