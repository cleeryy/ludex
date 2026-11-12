//! Administrator-only backup, verify, download, restore and delete routes
//! (#169).
//!
//! Every operation is confined to the configured backup directory
//! (`LUDEX_BACKUP_DIR`, default `{DATA_DIR}/backups`). Clients only ever name
//! an archive by its file name; the server resolves and canonicalises it, and
//! rejects anything that escapes the directory. Secrets never enter an audit
//! event, a response or a verification sidecar.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::backup::{self, BackupManifest, BackupRequest, RestoreRequest};
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::state::AppState;

/// Backup routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/backups", get(list).post(create))
        .route("/admin/backups/{name}", delete(delete_archive))
        .route("/admin/backups/{name}/verify", post(verify))
        .route("/admin/backups/{name}/restore", post(restore))
        .route("/admin/backups/{name}/download", get(download))
}

/// One archive in the backup directory.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    /// File name inside the backup directory.
    pub name: String,
    /// Absolute path of the archive, inside the configured directory.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Archive modification time.
    pub created_at: DateTime<Utc>,
    /// `manual`, `pre_restore` or `other`.
    pub kind: String,
    /// Last verification result.
    pub verification: BackupVerification,
}

/// Verification state of one archive, persisted next to it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupVerification {
    /// `unverified`, `verified` or `failed`.
    pub state: String,
    /// When the archive was last checked.
    pub checked_at: Option<DateTime<Utc>>,
    /// Failure detail, never a secret.
    pub message: Option<String>,
    /// Games recorded in the manifest.
    pub games: Option<i64>,
    /// Versions recorded in the manifest.
    pub versions: Option<i64>,
    /// Users recorded in the manifest.
    pub users: Option<i64>,
    /// Cloud saves recorded in the manifest.
    pub saves: Option<i64>,
    /// Files listed in the manifest.
    pub files: Option<i64>,
    /// Total bytes listed in the manifest.
    pub bytes: Option<u64>,
    /// Server version that produced the archive.
    pub server_version: Option<String>,
}

impl Default for BackupVerification {
    fn default() -> Self {
        Self {
            state: "unverified".to_string(),
            checked_at: None,
            message: None,
            games: None,
            versions: None,
            users: None,
            saves: None,
            files: None,
            bytes: None,
            server_version: None,
        }
    }
}

/// Backup directory plus its archives, newest first.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupList {
    /// Configured backup directory.
    pub directory: String,
    /// Archives found there.
    pub backups: Vec<BackupEntry>,
}

/// Options accepted by `POST /admin/backups`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateBackupRequest {
    /// Validate and measure without writing the archive.
    #[serde(default)]
    pub dry_run: bool,
    /// Include the media cache (default true).
    #[serde(default = "default_true")]
    pub include_media: bool,
    /// Upload the finished archive with the configured S3 storage.
    #[serde(default)]
    pub upload_s3: bool,
}

/// Result of a backup run.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupCreated {
    /// File name inside the backup directory.
    pub name: String,
    /// Absolute archive path, inside the configured directory.
    pub path: String,
    /// Archive size in bytes (0 in dry-run).
    pub bytes: u64,
    /// Files included in the snapshot.
    pub files: i64,
    /// Whether nothing was written.
    pub dry_run: bool,
    /// Object key when `uploadS3` succeeded.
    pub uploaded_key: Option<String>,
    /// Creation timestamp written in `backup.json`.
    pub created_at: DateTime<Utc>,
}

/// Options accepted by `POST /admin/backups/{name}/restore`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoreBackupRequest {
    /// Verify and report what would change without writing.
    #[serde(default)]
    pub dry_run: bool,
    /// Restore the media cache (default true).
    #[serde(default = "default_true")]
    pub include_media: bool,
    /// Typed confirmation: the archive file name. Required unless `dryRun`.
    #[serde(default)]
    pub confirmation: Option<String>,
}

/// Restore report returned to the admin UI.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoreBackupResponse {
    /// Whether nothing was written.
    pub dry_run: bool,
    /// Whether the destination already had data.
    pub overwrote: bool,
    /// Files verified in the archive.
    pub files: i64,
    /// Bytes extracted (or that would be extracted in dry-run).
    pub bytes: u64,
    /// Games recorded in the manifest.
    pub games: i64,
    /// Versions recorded in the manifest.
    pub versions: i64,
    /// Users recorded in the manifest.
    pub users: i64,
    /// Manifest creation time.
    pub created_at: DateTime<Utc>,
    /// Pre-restore safety copy, when a real restore ran.
    pub safety_backup: Option<String>,
    /// Whether the server must be restarted to serve the restored database.
    pub restart_required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteQuery {
    #[serde(default)]
    confirm: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// Exclusive guard for operations that read or rewrite the backup directory.
struct BusyGuard(Arc<std::sync::atomic::AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn acquire(state: &AppState) -> Result<BusyGuard, ApiError> {
    if state
        .backup_busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(ApiError::conflict(
            "backup_busy",
            "Another backup operation is already running",
        ));
    }
    Ok(BusyGuard(state.backup_busy.clone()))
}

/// Whether `name` looks like an archive this API may touch.
fn valid_archive_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name.ends_with(".tar.gz")
        && !name.starts_with('.')
        && !name.contains("..")
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn validate_archive_name(name: &str) -> Result<(), ApiError> {
    if valid_archive_name(name) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "invalid_backup_name",
            "A backup is named <file>.tar.gz and cannot contain a path",
        ))
    }
}

fn archive_kind(name: &str) -> &'static str {
    if name.starts_with("ludex-pre-restore-") {
        "pre_restore"
    } else if name.starts_with("ludex-backup-") {
        "manual"
    } else {
        "other"
    }
}

fn verification_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}.verify.json"))
}

async fn read_verification(directory: &Path, name: &str) -> BackupVerification {
    match tokio::fs::read(verification_path(directory, name)).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => BackupVerification::default(),
    }
}

async fn write_verification(directory: &Path, name: &str, verification: &BackupVerification) {
    let path = verification_path(directory, name);
    let bytes = match serde_json::to_vec_pretty(verification) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(error = %error, "could not serialise backup verification state");
            return;
        }
    };
    if let Err(error) = tokio::fs::write(&path, bytes).await {
        tracing::warn!(error = %error, path = %path.display(), "could not persist backup verification state");
    }
}

/// Resolves a client-provided archive name inside the backup directory.
///
/// The canonical path must stay under the canonical backup directory, so a
/// symlink or a crafted name can never reach another file.
async fn resolve_archive(state: &AppState, name: &str) -> Result<PathBuf, ApiError> {
    validate_archive_name(name)?;
    let directory = &state.config.backup_dir;
    let root = tokio::fs::canonicalize(directory).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found("backup_not_found", "Backup archive not found")
        } else {
            ApiError::from(error)
        }
    })?;
    let candidate = directory.join(name);
    let canonical = tokio::fs::canonicalize(&candidate)
        .await
        .map_err(|_| ApiError::not_found("backup_not_found", "Backup archive not found"))?;
    if !canonical.starts_with(&root) || !canonical.is_file() {
        return Err(ApiError::not_found(
            "backup_not_found",
            "Backup archive not found",
        ));
    }
    Ok(canonical)
}

/// Chooses a free archive path with a timestamped name.
async fn next_archive_path(directory: &Path, prefix: &str) -> Result<PathBuf, ApiError> {
    tokio::fs::create_dir_all(directory).await?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    for attempt in 0..100_u32 {
        let name = if attempt == 0 {
            format!("{prefix}-{stamp}.tar.gz")
        } else {
            format!("{prefix}-{stamp}-{attempt}.tar.gz")
        };
        let path = directory.join(name);
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(path);
        }
    }
    Err(ApiError::conflict(
        "backup_name_taken",
        "Could not allocate a free archive name",
    ))
}

fn backup_error(error: anyhow::Error) -> ApiError {
    tracing::warn!(error = %error, "backup operation failed");
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "backup_failed",
        format!("{error:#}"),
    )
}

async fn list(State(state): State<AppState>, user: AuthUser) -> Result<Json<BackupList>, ApiError> {
    user.require_admin_access()?;
    let directory = state.config.backup_dir.clone();
    tokio::fs::create_dir_all(&directory).await?;
    let mut backups = Vec::new();
    let mut reader = tokio::fs::read_dir(&directory).await?;
    while let Some(entry) = reader.next_entry().await? {
        if !entry
            .file_type()
            .await
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !valid_archive_name(&name) {
            continue;
        }
        let metadata = entry.metadata().await?;
        let created_at = metadata
            .modified()
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now());
        backups.push(BackupEntry {
            path: entry.path().display().to_string(),
            size: metadata.len(),
            created_at,
            kind: archive_kind(&name).to_string(),
            verification: read_verification(&directory, &name).await,
            name,
        });
    }
    backups.sort_by_key(|left| std::cmp::Reverse(left.created_at));
    Ok(Json(BackupList {
        directory: directory.display().to_string(),
        backups,
    }))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<CreateBackupRequest>,
) -> Result<(StatusCode, Json<BackupCreated>), ApiError> {
    user.require_admin_access()?;
    let _guard = acquire(&state)?;
    let planned = next_archive_path(&state.config.backup_dir, "ludex-backup").await?;
    let name = planned
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ludex-backup.tar.gz".to_string());
    let report = backup::create_backup(&BackupRequest {
        data_dir: state.config.data_dir.clone(),
        media_dir: state.config.media_dir.clone(),
        games_dir: state.config.games_dir.clone(),
        database_url: state.config.database_url.clone(),
        output: Some(planned),
        dry_run: request.dry_run,
        include_media: request.include_media,
        upload_s3: request.upload_s3,
        storage: Some(state.storage.clone()),
        config: Some((*state.config).clone()),
    })
    .await
    .map_err(backup_error)?;

    if !report.dry_run {
        state.events.emit(
            Event::new(actions::BACKUP_CREATED, user.actor())
                .target("backup", name.clone())
                .metadata(serde_json::json!({
                    "name": name,
                    "bytes": report.bytes,
                    "files": report.files,
                    "includeMedia": request.include_media,
                    "uploadS3": request.upload_s3,
                    "uploadedKey": report.uploaded_key,
                })),
        );
    }

    Ok((
        StatusCode::CREATED,
        Json(BackupCreated {
            name,
            path: report.path,
            bytes: report.bytes,
            files: i64::try_from(report.files).unwrap_or(i64::MAX),
            dry_run: report.dry_run,
            uploaded_key: report.uploaded_key,
            created_at: report.created_at,
        }),
    ))
}

async fn verify(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<BackupVerification>, ApiError> {
    user.require_admin_access()?;
    let _guard = acquire(&state)?;
    let archive = resolve_archive(&state, &name).await?;
    match backup::verify_backup(&archive).await {
        Ok(manifest) => {
            let verification = verification_from(&manifest);
            write_verification(&state.config.backup_dir, &name, &verification).await;
            Ok(Json(verification))
        }
        Err(error) => {
            let verification = BackupVerification {
                state: "failed".to_string(),
                checked_at: Some(Utc::now()),
                message: Some(format!("{error:#}")),
                ..BackupVerification::default()
            };
            write_verification(&state.config.backup_dir, &name, &verification).await;
            Err(backup_error(error))
        }
    }
}

fn verification_from(manifest: &BackupManifest) -> BackupVerification {
    BackupVerification {
        state: "verified".to_string(),
        checked_at: Some(Utc::now()),
        message: None,
        games: Some(manifest.counts.games),
        versions: Some(manifest.counts.versions),
        users: Some(manifest.counts.users),
        saves: Some(manifest.counts.saves),
        files: Some(i64::try_from(manifest.files.len()).unwrap_or(i64::MAX)),
        bytes: Some(manifest.files.iter().map(|file| file.size).sum()),
        server_version: Some(manifest.server_version.clone()),
    }
}

async fn restore(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(name): ApiPath<String>,
    JsonBody(request): JsonBody<RestoreBackupRequest>,
) -> Result<Json<RestoreBackupResponse>, ApiError> {
    user.require_admin_access()?;
    let archive = resolve_archive(&state, &name).await?;
    if !request.dry_run {
        if request.confirmation.as_deref() != Some(name.as_str()) {
            return Err(ApiError::bad_request(
                "confirmation_required",
                "Type the archive file name to confirm a destructive restore",
            ));
        }
        if state.scan.read().await.running {
            return Err(ApiError::conflict(
                "restore_blocked",
                "A library scan is running; wait for it to finish before restoring",
            ));
        }
    }
    let _guard = acquire(&state)?;

    // Preview first: it verifies every checksum and reports the counters
    // without writing anything, and also runs before a real restore.
    let preview = backup::restore_backup(&RestoreRequest {
        archive: archive.clone(),
        data_dir: state.config.data_dir.clone(),
        media_dir: state.config.media_dir.clone(),
        force: true,
        dry_run: true,
        include_media: request.include_media,
    })
    .await
    .map_err(backup_error)?;

    if request.dry_run {
        return Ok(Json(restore_response(preview, None, false)));
    }

    // A safety copy is mandatory: never overwrite the live data without one.
    let safety = next_archive_path(&state.config.backup_dir, "ludex-pre-restore").await?;
    let safety_name = safety
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ludex-pre-restore.tar.gz".to_string());
    backup::create_backup(&BackupRequest {
        data_dir: state.config.data_dir.clone(),
        media_dir: state.config.media_dir.clone(),
        games_dir: state.config.games_dir.clone(),
        database_url: state.config.database_url.clone(),
        output: Some(safety),
        dry_run: false,
        include_media: request.include_media,
        upload_s3: false,
        storage: Some(state.storage.clone()),
        config: Some((*state.config).clone()),
    })
    .await
    .map_err(backup_error)?;

    let report = backup::restore_backup(&RestoreRequest {
        archive,
        data_dir: state.config.data_dir.clone(),
        media_dir: state.config.media_dir.clone(),
        force: true,
        dry_run: false,
        include_media: request.include_media,
    })
    .await
    .map_err(backup_error)?;

    state.events.emit(
        Event::new(actions::BACKUP_RESTORED, user.actor())
            .target("backup", name.clone())
            .metadata(serde_json::json!({
                "name": name,
                "safetyBackup": safety_name,
                "games": report.games,
                "versions": report.versions,
                "users": report.users,
                "includeMedia": request.include_media,
            })),
    );

    Ok(Json(restore_response(report, Some(safety_name), true)))
}

fn restore_response(
    report: backup::RestoreReport,
    safety_backup: Option<String>,
    restart_required: bool,
) -> RestoreBackupResponse {
    RestoreBackupResponse {
        dry_run: report.dry_run,
        overwrote: report.overwrote,
        files: i64::try_from(report.files).unwrap_or(i64::MAX),
        bytes: report.bytes,
        games: report.games,
        versions: report.versions,
        users: report.users,
        created_at: report.created_at,
        safety_backup,
        restart_required,
    }
}

async fn delete_archive(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(name): ApiPath<String>,
    ApiQuery(query): ApiQuery<DeleteQuery>,
) -> Result<StatusCode, ApiError> {
    user.require_admin_access()?;
    if query.confirm.as_deref() != Some(name.as_str()) {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Pass ?confirm=<file name> to delete a backup archive",
        ));
    }
    let archive = resolve_archive(&state, &name).await?;
    let bytes = tokio::fs::metadata(&archive)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    tokio::fs::remove_file(&archive).await?;
    let _ = tokio::fs::remove_file(verification_path(&state.config.backup_dir, &name)).await;
    state.events.emit(
        Event::new(actions::BACKUP_DELETED, user.actor())
            .target("backup", name.clone())
            .metadata(serde_json::json!({
                "name": name,
                "bytes": bytes,
            })),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn download(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(name): ApiPath<String>,
) -> Result<Response, ApiError> {
    user.require_admin_access()?;
    let archive = resolve_archive(&state, &name).await?;
    let file = tokio::fs::File::open(&archive)
        .await
        .map_err(ApiError::from)?;
    let metadata = file.metadata().await.map_err(ApiError::from)?;
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .map_err(|_| ApiError::internal())?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/gzip")
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|_| ApiError::internal())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_cannot_escape_the_backup_directory() {
        assert!(valid_archive_name("ludex-backup-20261112-120000.tar.gz"));
        assert!(valid_archive_name(
            "ludex-pre-restore-20261112-120000.tar.gz"
        ));
        assert!(!valid_archive_name("../ludex.db"));
        assert!(!valid_archive_name("..%2Fludex.db.tar.gz"));
        assert!(!valid_archive_name("media/../../etc/passwd.tar.gz"));
        assert!(!valid_archive_name("/etc/passwd.tar.gz"));
        assert!(!valid_archive_name(".hidden.tar.gz"));
        assert!(!valid_archive_name("ludex.db"));
        assert!(!valid_archive_name(""));
    }

    #[test]
    fn archive_kind_is_derived_from_the_file_name() {
        assert_eq!(
            archive_kind("ludex-backup-20261112-120000.tar.gz"),
            "manual"
        );
        assert_eq!(
            archive_kind("ludex-pre-restore-20261112-120000.tar.gz"),
            "pre_restore"
        );
        assert_eq!(archive_kind("custom.tar.gz"), "other");
    }

    #[test]
    fn verification_defaults_to_unverified() {
        assert_eq!(BackupVerification::default().state, "unverified");
    }
}
