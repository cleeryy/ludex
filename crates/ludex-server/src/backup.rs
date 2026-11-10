//! Backup and restore (#49).
//!
//! A backup is a deterministic `tar.gz` containing:
//!
//! ```text
//! manifest.json        format version, Ludex version, hashes of every entry
//! data/ludex.db        consistent snapshot produced by `VACUUM INTO`
//! data/<other files>   `jwt.secret`, ingest state, ...
//! media/<files>        cached artwork (when --media-dir is given)
//! saves/<files>        optional save exports on disk
//! ```
//!
//! Every file entry carries its size and SHA-256 in `manifest.json`, so a
//! restore can reject a truncated or corrupted archive before writing
//! anything. The archive is written with sorted entries, zeroed ownership,
//! `mtime = 0` and a gzip header without timestamp: two runs over the same
//! content only differ by `createdAt`.
//!
//! Cloud saves live in SQLite, so they are always covered by the database
//! snapshot; `--saves-dir` exists for operators who also keep exported save
//! archives on disk.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::storage::{safe_object_key, StorageBackend};

/// Archive format version.
pub const FORMAT_VERSION: u32 = 1;
/// Prefix of generated archive names.
pub const ARCHIVE_PREFIX: &str = "ludex-backup-";
/// Suffix of generated archive names.
pub const ARCHIVE_SUFFIX: &str = ".tar.gz";
/// Name of the manifest entry.
pub const MANIFEST_NAME: &str = "manifest.json";
/// Name of the instance lock inside the data directory.
pub const LOCK_NAME: &str = "ludex.lock";
/// Component directories stored in the archive.
pub const COMPONENTS: [&str; 3] = ["data", "media", "saves"];

const CHUNK: usize = 64 * 1024;
const DATA_SKIPPED: &[&str] = &["ludex.db", "ludex.db-wal", "ludex.db-shm", LOCK_NAME];

/// One file listed in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    /// Path inside the archive, `/` separated.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256.
    pub sha256: String,
}

/// Backup metadata stored as `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Archive format version.
    pub format_version: u32,
    /// Server version that produced the archive.
    pub ludex_version: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Component directories present in the archive.
    pub components: Vec<String>,
    /// Every stored file, sorted by path.
    pub entries: Vec<ManifestEntry>,
    /// Number of entries.
    pub total_files: usize,
    /// Sum of the entry sizes.
    pub total_bytes: u64,
}

/// Source of one backup file.
struct SourceFile {
    archive_path: String,
    source: PathBuf,
    size: u64,
}

/// What to back up and where to write it.
#[derive(Debug, Clone)]
pub struct BackupRequest {
    /// Directory holding `ludex.db`.
    pub data_dir: PathBuf,
    /// Media cache directory, when it should be included.
    pub media_dir: Option<PathBuf>,
    /// Optional on-disk saves directory.
    pub saves_dir: Option<PathBuf>,
    /// Archive file, or a directory when the name should be generated.
    pub out: PathBuf,
    /// Retention: keep this many archives when `out` is a directory.
    pub keep: Option<usize>,
}

/// Result of a completed backup.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupReport {
    /// Written archive.
    pub archive: PathBuf,
    /// Number of archived files.
    pub files: usize,
    /// Archived bytes.
    pub bytes: u64,
    /// Components included.
    pub components: Vec<String>,
    /// Archives removed by retention.
    pub pruned: Vec<PathBuf>,
}

/// Restore inputs.
#[derive(Debug, Clone)]
pub struct RestoreRequest {
    /// Archive to read.
    pub archive: PathBuf,
    /// Target data directory.
    pub data_dir: PathBuf,
    /// Target media directory, required when the archive contains media.
    pub media_dir: Option<PathBuf>,
    /// Target saves directory, required when the archive contains saves.
    pub saves_dir: Option<PathBuf>,
    /// Overwrite an existing database.
    pub force: bool,
    /// Validate only.
    pub dry_run: bool,
}

/// Result of a restore (or of a dry run).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    /// Whether nothing was written.
    pub dry_run: bool,
    /// Number of restored files.
    pub files: usize,
    /// Restored bytes.
    pub bytes: u64,
    /// Components restored.
    pub components: Vec<String>,
    /// Archive creation time.
    pub created_at: DateTime<Utc>,
    /// Server version that produced the archive.
    pub ludex_version: String,
    /// Previous directories kept aside (`<dir>.pre-restore-<stamp>`).
    pub previous: Vec<PathBuf>,
}

/// Result of a media migration.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateReport {
    /// Files already present in the target backend.
    pub skipped: usize,
    /// Files uploaded.
    pub uploaded: usize,
    /// Bytes uploaded.
    pub bytes: u64,
    /// Source files removed after upload.
    pub deleted: usize,
}

// ---------------------------------------------------------------------------
// Instance lock
// ---------------------------------------------------------------------------

/// Takes the exclusive instance lock for a data directory.
///
/// The lock is an advisory `flock` on `{data_dir}/ludex.lock`. The server holds
/// it for its whole lifetime, which is how `restore` detects a running
/// instance. The lock is released when the returned file is dropped.
pub fn lock_instance(data_dir: &Path) -> Result<File> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create data directory {}", data_dir.display()))?;
    let path = data_dir.join(LOCK_NAME);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open instance lock {}", path.display()))?;
    file.try_lock_exclusive().map_err(|error| {
        anyhow::anyhow!(
            "another Ludex instance is using {} ({error}); stop it before restoring",
            data_dir.display()
        )
    })?;
    Ok(file)
}

/// Whether another process holds the instance lock.
#[must_use]
pub fn instance_is_running(data_dir: &Path) -> bool {
    let path = data_dir.join(LOCK_NAME);
    if !path.exists() {
        return false;
    }
    match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => match file.try_lock_exclusive() {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                false
            }
            Err(_) => true,
        },
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Backup
// ---------------------------------------------------------------------------

/// Creates a backup archive.
pub async fn create_backup(request: &BackupRequest) -> Result<BackupReport> {
    create_backup_at(request, Utc::now()).await
}

async fn create_backup_at(
    request: &BackupRequest,
    created_at: DateTime<Utc>,
) -> Result<BackupReport> {
    let database = request.data_dir.join("ludex.db");
    anyhow::ensure!(
        database.is_file(),
        "{} does not exist: nothing to back up",
        database.display()
    );

    let (archive_path, out_dir, generated) = resolve_output(&request.out, created_at)?;
    if let Some(parent) = archive_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    // 1. Consistent SQLite snapshot.
    let staging = tempfile::tempdir().context("create snapshot directory")?;
    let snapshot = staging.path().join("ludex.db");
    snapshot_database(&database, &snapshot).await?;

    // 2. Collect every file with its archive path.
    let mut files = Vec::new();
    files.push(SourceFile {
        archive_path: "data/ludex.db".to_string(),
        size: std::fs::metadata(&snapshot)?.len(),
        source: snapshot,
    });
    collect_data_files(&request.data_dir, &mut files)?;
    if let Some(media_dir) = &request.media_dir {
        collect_tree("media", media_dir, &mut files)?;
    }
    if let Some(saves_dir) = &request.saves_dir {
        collect_tree("saves", saves_dir, &mut files)?;
    }
    files.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));

    // 3. Hash every file (also rejects unreadable sources before writing).
    let mut entries = Vec::with_capacity(files.len());
    let mut total_bytes = 0_u64;
    for file in &files {
        let digest = hash_file(&file.source)?;
        total_bytes += file.size;
        entries.push(ManifestEntry {
            path: file.archive_path.clone(),
            size: file.size,
            sha256: digest,
        });
    }
    let components = components_from(&entries);
    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        ludex_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at,
        components: components.clone(),
        entries,
        total_files: files.len(),
        total_bytes,
    };

    // 4. Write the archive next to its final name, then rename atomically.
    let temporary = archive_path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let files_for_archive = files
        .iter()
        .map(|file| (file.archive_path.clone(), file.source.clone()))
        .collect::<Vec<_>>();
    let manifest_for_archive = manifest.clone();
    let temporary_for_archive = temporary.clone();
    let written = tokio::task::spawn_blocking(move || -> Result<()> {
        write_archive(
            &temporary_for_archive,
            &manifest_for_archive,
            &files_for_archive,
        )
    })
    .await
    .context("archive task")?;
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temporary, &archive_path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("rename {}", archive_path.display()));
    }

    // 5. Retention applies only to generated archives in a backup directory.
    let mut pruned = Vec::new();
    if generated {
        if let (Some(keep), Some(directory)) = (request.keep, out_dir.as_deref()) {
            pruned = prune_backups(directory, keep)?;
        }
    }

    Ok(BackupReport {
        archive: archive_path,
        files: manifest.total_files,
        bytes: manifest.total_bytes,
        components,
        pruned,
    })
}

fn resolve_output(
    out: &Path,
    created_at: DateTime<Utc>,
) -> Result<(PathBuf, Option<PathBuf>, bool)> {
    let looks_like_file = out
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(ARCHIVE_SUFFIX));
    if looks_like_file {
        let parent = out.parent().map(Path::to_path_buf);
        return Ok((out.to_path_buf(), parent, false));
    }
    let stamp = created_at
        .to_rfc3339_opts(SecondsFormat::Secs, true)
        .replace(':', "-");
    let name = format!("{ARCHIVE_PREFIX}{stamp}{ARCHIVE_SUFFIX}");
    Ok((out.join(name), Some(out.to_path_buf()), true))
}

async fn snapshot_database(database: &Path, destination: &Path) -> Result<()> {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
    use sqlx::Connection;

    let options = SqliteConnectOptions::new()
        .filename(database)
        .create_if_missing(false);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .with_context(|| format!("open {} for snapshot", database.display()))?;
    let escaped = destination.display().to_string().replace('\'', "''");
    let statement = format!("VACUUM INTO '{escaped}'");
    sqlx::query(&statement)
        .execute(&mut connection)
        .await
        .with_context(|| format!("VACUUM INTO {}", destination.display()))?;
    connection.close().await.ok();
    anyhow::ensure!(
        destination.is_file(),
        "SQLite snapshot was not written to {}",
        destination.display()
    );
    Ok(())
}

fn collect_data_files(data_dir: &Path, files: &mut Vec<SourceFile>) -> Result<()> {
    let entries = match std::fs::read_dir(data_dir) {
        Ok(entries) => entries,
        Err(error) => return Err(error).with_context(|| format!("read {}", data_dir.display())),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if DATA_SKIPPED.contains(&name.as_str()) {
            continue;
        }
        if name.starts_with(".tmp-") || name.starts_with(".restore-") {
            continue;
        }
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        files.push(SourceFile {
            archive_path: format!("data/{name}"),
            size: metadata.len(),
            source: entry.path(),
        });
    }
    Ok(())
}

fn collect_tree(component: &str, root: &Path, files: &mut Vec<SourceFile>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = entry.with_context(|| format!("walk {}", root.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .with_context(|| format!("strip {}", root.display()))?;
        let relative = relative
            .components()
            .map(|part| match part {
                Component::Normal(value) => Ok(value.to_string_lossy().to_string()),
                _ => Err(anyhow::anyhow!(
                    "unsupported path component in {}",
                    entry.path().display()
                )),
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        anyhow::ensure!(
            safe_object_key(&relative),
            "invalid file name in {}: {relative}",
            root.display()
        );
        files.push(SourceFile {
            archive_path: format!("{component}/{relative}"),
            size: entry.metadata()?.len(),
            source: entry.path().to_path_buf(),
        });
    }
    Ok(())
}

fn components_from(entries: &[ManifestEntry]) -> Vec<String> {
    let mut components: BTreeSet<String> = BTreeSet::new();
    for entry in entries {
        if let Some((component, _)) = entry.path.split_once('/') {
            components.insert(component.to_string());
        }
    }
    components.into_iter().collect()
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("open {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn write_archive(
    destination: &Path,
    manifest: &Manifest,
    files: &[(String, PathBuf)],
) -> Result<()> {
    let file =
        File::create(destination).with_context(|| format!("create {}", destination.display()))?;
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(BufWriter::new(file), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.mode(tar::HeaderMode::Deterministic);

    // The manifest comes first: a streaming reader can validate before
    // extracting anything.
    let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    append_bytes(&mut builder, MANIFEST_NAME, &manifest_bytes)?;

    for entry in &manifest.entries {
        let source = files
            .iter()
            .find(|(path, _)| path == &entry.path)
            .map(|(_, source)| source)
            .with_context(|| format!("missing source for {}", entry.path))?;
        let mut handle =
            File::open(source).with_context(|| format!("open {}", source.display()))?;
        let mut header = deterministic_header(entry.size);
        builder
            .append_data(&mut header, &entry.path, &mut handle)
            .with_context(|| format!("archive {}", entry.path))?;
    }

    let encoder = builder.into_inner().context("finish tar archive")?;
    let mut writer = encoder.finish().context("finish gzip stream")?;
    writer.flush()?;
    Ok(())
}

fn append_bytes(
    builder: &mut tar::Builder<GzEncoder<BufWriter<File>>>,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut header = deterministic_header(bytes.len() as u64);
    builder
        .append_data(&mut header, path, bytes)
        .with_context(|| format!("archive {path}"))?;
    Ok(())
}

fn deterministic_header(size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_size(size);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    header
}

/// Removes the oldest generated archives, keeping the `keep` most recent.
pub fn prune_backups(directory: &Path, keep: usize) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(keep > 0, "--keep must be greater than zero");
    let mut archives: Vec<PathBuf> = std::fs::read_dir(directory)
        .with_context(|| format!("read {}", directory.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(ARCHIVE_PREFIX) && name.ends_with(ARCHIVE_SUFFIX)
                })
        })
        .collect();
    archives.sort();
    if archives.len() <= keep {
        return Ok(Vec::new());
    }
    let excess = archives.len() - keep;
    let mut removed = Vec::new();
    for path in archives.into_iter().take(excess) {
        std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        removed.push(path);
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Restore
// ---------------------------------------------------------------------------

/// Validates and restores an archive.
pub async fn restore_backup(request: &RestoreRequest) -> Result<RestoreReport> {
    let archive = request.archive.clone();
    let manifest = tokio::task::spawn_blocking(move || read_manifest(&archive))
        .await
        .context("manifest task")??;
    anyhow::ensure!(
        manifest.format_version == FORMAT_VERSION,
        "unsupported archive format version {} (expected {FORMAT_VERSION})",
        manifest.format_version
    );

    let targets = resolve_targets(request, &manifest)?;

    // A running server holds the instance lock; never restore underneath it.
    // A dry run only probes the lock: it must not create the data directory.
    let lock = if request.dry_run {
        anyhow::ensure!(
            !instance_is_running(&request.data_dir),
            "refusing to restore into a running instance: another Ludex instance is using {}",
            request.data_dir.display()
        );
        None
    } else if targets.data.is_some() {
        let lock = lock_instance(&request.data_dir).map_err(|error| {
            anyhow::anyhow!("refusing to restore into a running instance: {error}")
        })?;
        Some(lock)
    } else {
        None
    };

    let database = request.data_dir.join("ludex.db");
    if database.exists() && !request.force && !request.dry_run {
        anyhow::bail!(
            "{} already exists; pass --force to replace it",
            database.display()
        );
    }

    let report = RestoreReport {
        dry_run: request.dry_run,
        files: manifest.total_files,
        bytes: manifest.total_bytes,
        components: manifest.components.clone(),
        created_at: manifest.created_at,
        ludex_version: manifest.ludex_version.clone(),
        previous: Vec::new(),
    };
    if request.dry_run {
        return Ok(report);
    }

    // Extract every component next to its target, then swap directories.
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let mut staged: Vec<(String, PathBuf, PathBuf)> = Vec::new();
    for (component, target) in &targets.by_component() {
        if !manifest.components.contains(component) {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let staging = target.with_file_name(format!(
            ".restore-{}-{}",
            target
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "target".to_string()),
            stamp
        ));
        if staging.exists() {
            std::fs::remove_dir_all(&staging).ok();
        }
        std::fs::create_dir_all(&staging)?;
        staged.push((component.clone(), staging, target.clone()));
    }

    let archive = request.archive.clone();
    let manifest_for_extract = manifest.clone();
    let staged_for_extract = staged.clone();
    let extracted = tokio::task::spawn_blocking(move || {
        extract_archive(&archive, &manifest_for_extract, &staged_for_extract)
    })
    .await
    .context("extract task")?;
    if let Err(error) = extracted {
        for (_, staging, _) in &staged {
            let _ = std::fs::remove_dir_all(staging);
        }
        return Err(error);
    }

    let mut previous = Vec::new();
    for (component, staging, target) in &staged {
        if is_effectively_empty(target) {
            // A directory created for the instance lock holds no data: do not
            // keep an empty `.pre-restore` copy around.
            std::fs::remove_dir_all(target).ok();
        }
        if target.exists() {
            let backup = target.with_file_name(format!(
                "{}.pre-restore-{stamp}",
                target
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "target".to_string())
            ));
            std::fs::rename(target, &backup).with_context(|| {
                format!(
                    "move {} aside before restoring {component}",
                    target.display()
                )
            })?;
            previous.push(backup);
        }
        if let Err(error) = std::fs::rename(staging, target) {
            // Roll the previous directory back so the instance stays usable.
            if let Some(backup) = previous.last() {
                let _ = std::fs::rename(backup, target);
            }
            return Err(error).with_context(|| format!("install {}", target.display()));
        }
    }

    drop(lock);
    let mut report = report;
    report.previous = previous;
    Ok(report)
}

/// True when a directory is missing, empty, or only holds the instance lock.
fn is_effectively_empty(directory: &Path) -> bool {
    if !directory.exists() {
        return true;
    }
    let Ok(mut entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.all(|entry| entry.is_ok_and(|entry| entry.file_name().to_string_lossy() == LOCK_NAME))
}

struct Targets {
    data: Option<PathBuf>,
    media: Option<PathBuf>,
    saves: Option<PathBuf>,
}

impl Targets {
    fn by_component_ref(&self) -> Vec<(&'static str, &Path)> {
        [
            ("data", self.data.as_deref()),
            ("media", self.media.as_deref()),
            ("saves", self.saves.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, path)| path.map(|path| (name, path)))
        .collect()
    }

    fn by_component(&self) -> Vec<(String, PathBuf)> {
        [
            ("data", self.data.clone()),
            ("media", self.media.clone()),
            ("saves", self.saves.clone()),
        ]
        .into_iter()
        .filter_map(|(name, path)| path.map(|path| (name.to_string(), path)))
        .collect()
    }
}

fn resolve_targets(request: &RestoreRequest, manifest: &Manifest) -> Result<Targets> {
    anyhow::ensure!(
        manifest
            .components
            .iter()
            .any(|component| component == "data"),
        "archive does not contain a data/ component"
    );
    if manifest
        .components
        .iter()
        .any(|component| component == "media")
    {
        anyhow::ensure!(
            request.media_dir.is_some(),
            "archive contains media files; pass --media-dir to restore them"
        );
    }
    if manifest
        .components
        .iter()
        .any(|component| component == "saves")
    {
        anyhow::ensure!(
            request.saves_dir.is_some(),
            "archive contains saves; pass --saves-dir to restore them"
        );
    }
    let targets = Targets {
        data: Some(request.data_dir.clone()),
        media: request.media_dir.clone(),
        saves: request.saves_dir.clone(),
    };
    // Two components in the same directory would fight over the same staging
    // path and the same final rename.
    let mut seen: BTreeSet<&Path> = BTreeSet::new();
    for (_, path) in targets.by_component_ref() {
        anyhow::ensure!(
            seen.insert(path),
            "restore targets must be distinct directories ({} is used twice)",
            path.display()
        );
    }
    Ok(targets)
}

fn read_manifest(archive: &Path) -> Result<Manifest> {
    let file = File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    let mut tar = tar::Archive::new(GzDecoder::new(BufReader::new(file)));
    let mut manifest: Option<Manifest> = None;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut buffer = vec![0_u8; CHUNK];

    for entry in tar.entries().context("read archive entries")? {
        let mut entry = entry.context("read archive entry")?;
        let path = entry_path(&entry)?;
        anyhow::ensure!(seen.insert(path.clone()), "duplicate archive entry {path}");
        if path == MANIFEST_NAME {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            manifest = Some(
                serde_json::from_slice(&bytes)
                    .context("decode manifest.json; is this a Ludex backup?")?,
            );
            continue;
        }
        let expected = manifest
            .as_ref()
            .and_then(|manifest| manifest.entries.iter().find(|item| item.path == path))
            .cloned();
        let Some(expected) = expected else {
            anyhow::bail!("archive entry {path} is not listed in manifest.json");
        };
        verify_entry(&mut entry, &expected, &mut buffer)?;
    }

    let manifest = manifest.context("archive has no manifest.json")?;
    anyhow::ensure!(
        manifest.total_files == manifest.entries.len(),
        "manifest totalFiles does not match its entries"
    );
    let expected_paths: BTreeSet<&str> = manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    let archived: BTreeSet<&str> = seen
        .iter()
        .map(String::as_str)
        .filter(|path| *path != MANIFEST_NAME)
        .collect();
    anyhow::ensure!(
        expected_paths == archived,
        "archive content does not match manifest.json"
    );
    Ok(manifest)
}

fn extract_archive(
    archive: &Path,
    manifest: &Manifest,
    staged: &[(String, PathBuf, PathBuf)],
) -> Result<()> {
    let file = File::open(archive).with_context(|| format!("open {}", archive.display()))?;
    let mut tar = tar::Archive::new(GzDecoder::new(BufReader::new(file)));
    let mut buffer = vec![0_u8; CHUNK];

    for entry in tar.entries().context("read archive entries")? {
        let mut entry = entry.context("read archive entry")?;
        let path = entry_path(&entry)?;
        if path == MANIFEST_NAME {
            continue;
        }
        let Some(expected) = manifest.entries.iter().find(|item| item.path == path) else {
            anyhow::bail!("archive entry {path} is not listed in manifest.json");
        };
        let Some((component, relative)) = path.split_once('/') else {
            anyhow::bail!("archive entry {path} has no component directory");
        };
        let Some((_, staging, _)) = staged.iter().find(|(name, _, _)| name == component) else {
            anyhow::bail!("archive component {component} has no restore target");
        };
        let destination = staging.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = File::create(&destination)
            .with_context(|| format!("create {}", destination.display()))?;
        let (copied, digest) = copy_and_hash(&mut entry, &mut output, &mut buffer)?;
        verify_hash(&digest, &expected.sha256, &path)?;
        anyhow::ensure!(
            copied == expected.size,
            "size mismatch for {path}: manifest {}, archive {copied}",
            expected.size
        );
    }
    Ok(())
}

fn entry_path<R: Read>(entry: &tar::Entry<'_, R>) -> Result<String> {
    anyhow::ensure!(
        entry.header().entry_type().is_file(),
        "archive contains a non-file entry"
    );
    let path = entry.path().context("decode entry path")?;
    let path = path
        .to_str()
        .context("archive entry path is not UTF-8")?
        .to_string();
    let relative = Path::new(&path);
    anyhow::ensure!(
        relative
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "archive entry {path} escapes the restore directory"
    );
    anyhow::ensure!(safe_object_key(&path), "invalid archive entry {path}");
    Ok(path)
}

fn verify_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    expected: &ManifestEntry,
    buffer: &mut [u8],
) -> Result<()> {
    let (copied, digest) = copy_and_hash(entry, &mut std::io::sink(), buffer)?;
    anyhow::ensure!(
        copied == expected.size,
        "size mismatch for {}: manifest {}, archive {copied}",
        expected.path,
        expected.size
    );
    verify_hash(&digest, &expected.sha256, &expected.path)
}

fn copy_and_hash<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    buffer: &mut [u8],
) -> Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    loop {
        let read = reader.read(buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        writer.write_all(&buffer[..read])?;
        total += read as u64;
    }
    Ok((total, hex::encode(hasher.finalize())))
}

fn verify_hash(actual: &str, expected: &str, path: &str) -> Result<()> {
    anyhow::ensure!(
        actual == expected,
        "checksum mismatch for {path}: manifest {expected}, archive {actual}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Media migration
// ---------------------------------------------------------------------------

/// Uploads a local media directory into the configured storage backend.
pub async fn migrate_media(
    source: &Path,
    storage: &dyn StorageBackend,
    delete_source: bool,
) -> Result<MigrateReport> {
    anyhow::ensure!(source.is_dir(), "{} is not a directory", source.display());
    let mut report = MigrateReport {
        skipped: 0,
        uploaded: 0,
        bytes: 0,
        deleted: 0,
    };
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if !crate::media::safe_media_name(&name) {
            if entry
                .metadata()
                .await
                .is_ok_and(|metadata| metadata.is_file())
            {
                tracing::warn!(file = %name, "skipping file with an unsupported media name");
            }
            continue;
        }
        let metadata = entry.metadata().await?;
        if !metadata.is_file() {
            continue;
        }
        if storage.exists_media(&name).await? {
            report.skipped += 1;
        } else {
            let bytes = tokio::fs::read(entry.path()).await?;
            let content_type = mime_guess::from_path(&name)
                .first_or_octet_stream()
                .to_string();
            storage
                .put_media(&name, bytes.into(), &content_type)
                .await?;
            report.uploaded += 1;
            report.bytes += metadata.len();
        }
        if delete_source {
            tokio::fs::remove_file(entry.path()).await?;
            report.deleted += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::storage::LocalStorage;

    fn write(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    async fn sample_database(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let config = Config::for_local(dir.join("games"), dir.to_path_buf(), dir.join("media"));
        let pool = crate::db::connect(&config.database_url).await.unwrap();
        crate::roots::bootstrap(&pool, &config).await.unwrap();
        sqlx::query("INSERT INTO settings (key, value) VALUES ('server_name', 'backup-test')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    #[tokio::test]
    async fn backup_and_restore_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let saves_dir = temp.path().join("saves");
        sample_database(&data_dir).await;
        write(&data_dir.join("jwt.secret"), b"secret");
        write(&media_dir.join("cover.jpg"), b"cover");
        write(&media_dir.join("shot.png"), b"screenshot");
        write(&saves_dir.join("slot.sav"), b"save-data");

        let out = temp.path().join("backups");
        let request = BackupRequest {
            data_dir: data_dir.clone(),
            media_dir: Some(media_dir.clone()),
            saves_dir: Some(saves_dir.clone()),
            out: out.clone(),
            keep: Some(2),
        };
        let report = create_backup(&request).await.unwrap();
        assert!(report.archive.exists());
        assert_eq!(report.files, 5);
        assert_eq!(report.components, vec!["data", "media", "saves"]);
        let archive_name = report
            .archive
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(archive_name.starts_with(ARCHIVE_PREFIX));
        assert!(archive_name.ends_with(ARCHIVE_SUFFIX));

        let restore_root = temp.path().join("restore");
        let restore = RestoreRequest {
            archive: report.archive.clone(),
            data_dir: restore_root.join("data"),
            media_dir: Some(restore_root.join("media")),
            saves_dir: Some(restore_root.join("saves")),
            force: false,
            dry_run: true,
        };
        let dry = restore_backup(&restore).await.unwrap();
        assert!(dry.dry_run);
        assert!(!restore_root.exists());
        assert_eq!(dry.files, 5);

        let restore = RestoreRequest {
            dry_run: false,
            ..restore
        };
        let restored = restore_backup(&restore).await.unwrap();
        assert!(!restored.dry_run);
        assert_eq!(
            std::fs::read(restore_root.join("media/cover.jpg")).unwrap(),
            b"cover"
        );
        assert_eq!(
            std::fs::read(restore_root.join("saves/slot.sav")).unwrap(),
            b"save-data"
        );
        assert_eq!(
            std::fs::read(restore_root.join("data/jwt.secret")).unwrap(),
            b"secret"
        );
        let restored_url = crate::config::default_database_url(&restore_root.join("data"));
        let pool = crate::db::connect(&restored_url).await.unwrap();
        let name: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'server_name'")
                .fetch_one(&pool)
                .await
                .unwrap();
        pool.close().await;
        assert_eq!(name, "backup-test");
    }

    #[tokio::test]
    async fn restore_refuses_to_overwrite_without_force() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        sample_database(&data_dir).await;
        let request = BackupRequest {
            data_dir: data_dir.clone(),
            media_dir: None,
            saves_dir: None,
            out: temp.path().join("one.tar.gz"),
            keep: None,
        };
        let report = create_backup(&request).await.unwrap();

        let restore = RestoreRequest {
            archive: report.archive,
            data_dir: data_dir.clone(),
            media_dir: None,
            saves_dir: None,
            force: false,
            dry_run: false,
        };
        let error = restore_backup(&restore).await.unwrap_err();
        assert!(error.to_string().contains("--force"));
    }

    #[tokio::test]
    async fn corrupted_archive_is_rejected_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        sample_database(&data_dir).await;
        let archive = temp.path().join("broken.tar.gz");
        let request = BackupRequest {
            data_dir: data_dir.clone(),
            media_dir: None,
            saves_dir: None,
            out: archive.clone(),
            keep: None,
        };
        create_backup(&request).await.unwrap();

        // Flip one byte near the end of the gzip stream.
        let mut bytes = std::fs::read(&archive).unwrap();
        let index = bytes.len() / 2;
        bytes[index] ^= 0xFF;
        std::fs::write(&archive, bytes).unwrap();

        let restore_root = temp.path().join("restore");
        let restore = RestoreRequest {
            archive,
            data_dir: restore_root.clone(),
            media_dir: None,
            saves_dir: None,
            force: false,
            dry_run: true,
        };
        assert!(restore_backup(&restore).await.is_err());
        assert!(!restore_root.exists());
    }

    #[test]
    fn archive_is_deterministic_for_the_same_content() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.bin");
        std::fs::write(&source, b"payload").unwrap();
        let files = vec![("data/source.bin".to_string(), source.clone())];
        let created_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let manifest = Manifest {
            format_version: FORMAT_VERSION,
            ludex_version: "test".to_string(),
            created_at,
            components: vec!["data".to_string()],
            entries: vec![ManifestEntry {
                path: "data/source.bin".to_string(),
                size: 7,
                sha256: hash_file(&source).unwrap(),
            }],
            total_files: 1,
            total_bytes: 7,
        };
        let first = temp.path().join("first.tar.gz");
        let second = temp.path().join("second.tar.gz");
        write_archive(&first, &manifest, &files).unwrap();
        write_archive(&second, &manifest, &files).unwrap();
        assert_eq!(
            std::fs::read(&first).unwrap(),
            std::fs::read(&second).unwrap()
        );
    }

    #[tokio::test]
    async fn retention_keeps_the_newest_archives() {
        let temp = tempfile::tempdir().unwrap();
        let out = temp.path().join("backups");
        std::fs::create_dir_all(&out).unwrap();
        for index in 0..4 {
            write(
                &out.join(format!(
                    "{ARCHIVE_PREFIX}2026-01-0{index}T00-00-00Z{ARCHIVE_SUFFIX}"
                )),
                b"archive",
            );
        }
        write(&out.join("notes.txt"), b"keep me");
        let removed = prune_backups(&out, 2).unwrap();
        assert_eq!(removed.len(), 2);
        let remaining: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(remaining.len(), 3);
        assert!(remaining.contains(&"notes.txt".to_string()));
        assert!(removed[0].to_string_lossy().contains("2026-01-00"));
        assert!(prune_backups(&out, 0).is_err());
    }

    #[tokio::test]
    async fn a_locked_instance_blocks_restore() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        sample_database(&data_dir).await;
        let request = BackupRequest {
            data_dir: data_dir.clone(),
            media_dir: None,
            saves_dir: None,
            out: temp.path().join("one.tar.gz"),
            keep: None,
        };
        let report = create_backup(&request).await.unwrap();

        let _lock = lock_instance(&data_dir).unwrap();
        assert!(instance_is_running(&data_dir));
        let restore = RestoreRequest {
            archive: report.archive,
            data_dir,
            media_dir: None,
            saves_dir: None,
            force: true,
            dry_run: false,
        };
        let error = restore_backup(&restore).await.unwrap_err();
        assert!(error.to_string().contains("running instance"));
    }

    #[tokio::test]
    async fn migrate_media_uploads_to_the_backend() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("media");
        write(&source.join("cover.jpg"), b"cover");
        let backend = temp.path().join("objects");
        let storage = LocalStorage::new(&backend);
        let report = migrate_media(&source, &storage, true).await.unwrap();
        assert_eq!(report.uploaded, 1);
        assert_eq!(report.deleted, 1);
        assert!(!source.join("cover.jpg").exists());
        assert_eq!(std::fs::read(backend.join("cover.jpg")).unwrap(), b"cover");
        // Second run is a no-op.
        let report = migrate_media(&source, &storage, false).await.unwrap();
        assert_eq!(report.uploaded, 0);
    }
}
