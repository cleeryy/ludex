//! The file-manifest transport (#231).
//!
//! The server can extract an archive once into its cache and publish a manifest
//! of relative path, size and SHA-256. The client then downloads the files in
//! parallel, verifies each digest before renaming it into staging, and never
//! writes an archive on this device. Every failure after the transfer started
//! falls back to the classic archive once, with the reason visible on the job
//! (#235).
//!
//! The HTTP and SQLite adapters at the bottom are deliberately thin: the
//! retry/checkpoint logic lives in [`crate::file_manifest`], which is testable
//! without a server.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::download::parse_content_range_start;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::file_manifest::{self, FileCheckpoints};
use crate::manifest;
use crate::space::{self, SpacePlan};
use crate::state::{AppState, JobEntry};
use crate::util::{non_empty, now_rfc3339};

use super::job::{Failure, InstallParams, InstallPhase};
use super::plan::free_space;
use super::scheduler::{persist, set_progress};
use super::source::is_local_server;
pub(super) struct ManifestInstallReport {
    pub(super) bytes: u64,
    pub(super) files: usize,
    pub(super) archive_sha256: String,
    /// Per-file digests the install verified, in manifest order (#240).
    pub(super) entries: Vec<manifest::ManifestEntry>,
}

/// Outcome of one file-manifest attempt (#231/#235).
pub(super) enum ManifestAttempt {
    Installed(ManifestInstallReport),
    /// The probe found no ready cache (409 `extracted_cache_unavailable`, a
    /// disabled cache, or an older server): nothing was written, the job
    /// continues on its normal pipeline silently, and the reason is logged.
    NotAvailable,
    /// A manifest that was already downloading cannot continue and nothing
    /// useful was written: this attempt must finish on the classic archive,
    /// with a visible reason on the job.
    Classic,
}

/// Try the file-manifest path for this job.
///
/// [`ManifestAttempt::Installed`] means the files are staged and verified;
/// `NotAvailable`/`Classic` mean the caller continues with another transport
/// (`NotAvailable` silently, `Classic` with the reason on the job); `Err` is a
/// real failure (cancel, pause, checksum).
pub(super) async fn manifest_install(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    params: &InstallParams,
    staging: &Path,
    download_root: &Path,
) -> Result<ManifestAttempt, Failure> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() {
        return Ok(ManifestAttempt::NotAvailable);
    }
    let job_id = entry.snapshot().job_id;
    let token = non_empty(params.token.as_deref());
    let install_root = params
        .install_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| settings.install_path());

    // Only the embedded server guarantees the admin role the extraction
    // endpoint needs. A remote server is still probed; when its cache is not
    // already ready the job simply continues on the next transport.
    let admin = settings.download_policy.warms_cache() && is_local_server(&state, base);

    set_progress(
        app,
        entry,
        InstallPhase::Downloading,
        0,
        None,
        0,
        Some(if admin {
            "Preparing the server's extracted files".to_string()
        } else {
            "Looking for the server's file manifest".to_string()
        }),
    );

    let transport = HttpManifestTransport {
        client: &state.http,
        base,
        version_id: params.version_id,
        token,
        admin,
    };
    let tuning = file_manifest::Tuning {
        parallel: parallel_budget(&state, base, token).await,
        idle_timeout: Duration::from_secs(settings.download_stall_timeout_secs.clamp(5, 600)),
        warm_cache: transport.admin,
        ..file_manifest::Tuning::default()
    };

    let manifest = match file_manifest::prepare(&transport, &tuning).await {
        Ok(manifest) => manifest,
        Err(error) => {
            // #235: automatic falls back silently here. It is a probe, not a
            // transfer: the stream/archive path continues without telling the
            // user about a cache they never asked for.
            tracing::info!(
                game_id = params.game_id,
                version_id = params.version_id,
                reason = %error.message(),
                "download policy automatic: no file manifest; using the next transport"
            );
            return Ok(ManifestAttempt::NotAvailable);
        }
    };
    let plan = match file_manifest::plan(&manifest) {
        Ok(plan) => plan,
        Err(error) => {
            tracing::warn!(
                game_id = params.game_id,
                version_id = params.version_id,
                reason = %error.message(),
                "download policy automatic: the file manifest was rejected; using the next transport"
            );
            return Ok(ManifestAttempt::NotAvailable);
        }
    };

    // #222/#231: charge the exact extracted peak on the install volume and
    // release it with the rest of the job. The manifest is the measured size,
    // so the earlier 1.5× estimate is replaced, not added to.
    let peak = SpacePlan::build(
        None,
        Some(plan.total_bytes),
        &install_root,
        download_root,
        true,
        settings.keep_archives,
    );
    match state
        .space
        .try_reserve(&job_id, &peak, &space::SystemCapacity)
    {
        space::SpaceReservation::Granted(status) => {
            entry.update(|job| job.space = Some(status));
        }
        space::SpaceReservation::Waiting(status) => {
            return Err(Failure::Failed(status.message()));
        }
    }
    ensure_manifest_space(
        &install_root,
        plan.total_bytes,
        settings.floor_for_path(&install_root),
    )?;

    entry.update(|job| {
        job.file_manifest = true;
        job.manifest_fallback = false;
        job.bytes_done = 0;
        job.bytes_total = Some(plan.total_bytes);
        job.entries_done = Some(0);
        job.entries_total = Some(plan.files.len() as u32);
        job.current_entry = None;
        job.resumed_entries = None;
        job.phase = InstallPhase::Downloading;
        job.message = Some(format!(
            "Downloading files 0/{} (no archive)",
            plan.files.len()
        ));
    });
    persist(app, entry);

    let files = HttpFileTransport {
        client: &state.http,
        base,
        version_id: params.version_id,
        token,
    };
    let checkpoints = DbFileCheckpoints {
        db: &state.db,
        job_id: job_id.clone(),
        game_id: params.game_id,
        version_id: params.version_id,
        staging: staging.to_path_buf(),
    };
    let cancel = entry.cancel.clone();
    let pause = entry.pause_token();
    let handle = app.clone();
    let job_entry = Arc::clone(entry);
    let result = file_manifest::install(
        &plan,
        &manifest.archive_sha256,
        staging,
        &files,
        &checkpoints,
        &cancel,
        &pause,
        &tuning,
        &move |progress| {
            job_entry.update(|job| {
                job.file_manifest = true;
                job.phase = if progress.verifying {
                    InstallPhase::Verifying
                } else {
                    InstallPhase::Downloading
                };
                job.bytes_done = progress.bytes_done;
                job.bytes_total = Some(progress.bytes_total);
                job.speed_bps = progress.speed_bps;
                job.entries_done = Some(progress.files_done);
                job.entries_total = Some(progress.files_total);
                job.resumed_entries =
                    (progress.resumed_files > 0).then_some(progress.resumed_files);
                job.current_entry = progress.current.clone();
                // A partial file is a real checkpoint: the next attempt resumes
                // it with Range, so the row offers "Resume" instead of
                // "Restart" (#231).
                job.resumable = progress.bytes_done > 0;
                job.clear_reconnect();
                job.message = Some(if progress.verifying {
                    format!(
                        "Verifying file {}/{}",
                        progress
                            .files_done
                            .saturating_add(1)
                            .min(progress.files_total.max(1)),
                        progress.files_total
                    )
                } else {
                    progress.message
                });
            });
            persist(&handle, &job_entry);
            let _ = handle.emit("install://progress", job_entry.snapshot());
        },
    )
    .await;

    match result {
        Ok(outcome) => {
            tracing::info!(
                game_id = params.game_id,
                version_id = params.version_id,
                files = outcome.files,
                bytes = outcome.bytes,
                resumed = outcome.resumed_files,
                archive_sha256 = %outcome.archive_sha256,
                "file manifest downloaded and verified; no archive on disk"
            );
            Ok(ManifestAttempt::Installed(ManifestInstallReport {
                bytes: outcome.bytes,
                files: outcome.files,
                archive_sha256: outcome.archive_sha256,
                entries: plan
                    .files
                    .iter()
                    .map(|file| manifest::ManifestEntry {
                        path: file.relative.clone(),
                        size: file.size,
                        sha256: file.sha256.clone(),
                    })
                    .collect(),
            }))
        }
        Err(file_manifest::ManifestFailure::Cancelled) => Err(Failure::Cancelled),
        Err(file_manifest::ManifestFailure::Paused) => Err(Failure::Paused),
        Err(file_manifest::ManifestFailure::Fallback(reason)) => {
            // The cache disappeared under us (evicted, 409, a file 404): the
            // classic path starts from a clean staging folder and no
            // checkpoint, so the exit is explicit and visible.
            clear_file_checkpoint(app, &job_id);
            let _ = tokio::fs::remove_dir_all(staging).await;
            entry.update(|job| {
                job.file_manifest = false;
                job.entries_done = None;
                job.entries_total = None;
                job.current_entry = None;
                job.resumed_entries = None;
                job.resumable = false;
            });
            note_manifest_fallback(app, entry, &reason);
            Ok(ManifestAttempt::Classic)
        }
        Err(file_manifest::ManifestFailure::Failed(message)) => {
            // Keep the checkpoint: the next attempt resumes the completed
            // files and the in-flight `.part`.
            let completed = checkpoints
                .completed()
                .map(|files| files.len())
                .unwrap_or(0);
            entry.update(|job| {
                job.entries_done = Some(completed as u32);
                job.resumable = completed > 0;
            });
            persist(app, entry);
            Err(Failure::Failed(message))
        }
    }
}

/// Rewrite the job so the row says a started file-manifest install fell back.
///
/// #235: only a failure **after** the download started is visible; a probe
/// that finds no ready cache falls through silently (`tracing` keeps the
/// reason). The job then finishes on the classic archive, once.
fn note_manifest_fallback(app: &AppHandle, entry: &Arc<JobEntry>, reason: &str) {
    let message = format!(
        "The server could not serve the file manifest ({reason}); installing from the classic archive instead."
    );
    entry.update(|job| {
        job.file_manifest = false;
        job.manifest_fallback = true;
        job.current_entry = None;
        job.speed_bps = 0;
        job.message = Some(message);
    });
    persist(app, entry);
    let _ = app.emit("install://progress", entry.snapshot());
}

/// Disk preflight of the manifest peak, before the first byte (#222/#231).
fn ensure_manifest_space(
    install_root: &Path,
    extracted_bytes: u64,
    min_free_bytes: u64,
) -> Result<(), Failure> {
    if extracted_bytes == 0 {
        return Ok(());
    }
    let Some(free) = free_space(install_root) else {
        return Ok(());
    };
    let required = space::with_margin(extracted_bytes);
    // #290: the per-library floor applies to the file transport too.
    match crate::bridge::mount::check_free_space(free, required, min_free_bytes) {
        Ok(()) => Ok(()),
        Err(shortfall) => Err(Failure::Failed(format!(
            "{} Free space on that volume and resume the install.",
            shortfall.message(install_root)
        ))),
    }
}

/// Job-scoped parallel budget: the server's `parallelPartsMax`, clamped to the
/// 4–8 range this transport is measured for (#231). A server without the field
/// (or an unreachable one) gets the 4-worker default; the value never exceeds
/// the documented per-address budget.
async fn parallel_budget(state: &AppState, base: &str, token: Option<&str>) -> usize {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Config {
        #[serde(default)]
        parallel_parts_max: u32,
    }
    let mut request = state
        .http
        .get(format!("{base}/api/config"))
        .timeout(Duration::from_secs(5));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let advertised = match request.send().await {
        Ok(response) if response.status().is_success() => response
            .json::<Config>()
            .await
            .map(|config| config.parallel_parts_max as usize)
            .unwrap_or(0),
        _ => 0,
    };
    if advertised == 0 {
        file_manifest::DEFAULT_WORKERS
    } else {
        advertised.clamp(4, file_manifest::MAX_WORKERS)
    }
}

/// `GET /api/versions/{id}/manifest` and `POST /api/admin/.../extract` (#231).
struct HttpManifestTransport<'a> {
    client: &'a reqwest::Client,
    base: &'a str,
    version_id: i64,
    token: Option<&'a str>,
    admin: bool,
}

/// Server error body: `{"error":{"code","message"}}`, or the plain status.
fn server_error_message(status: u16, body: &str) -> String {
    #[derive(Deserialize)]
    struct Envelope {
        error: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        #[serde(default)]
        message: String,
        #[serde(default)]
        code: String,
    }
    match serde_json::from_str::<Envelope>(body) {
        Ok(envelope) if !envelope.error.message.is_empty() => envelope.error.message,
        Ok(envelope) if !envelope.error.code.is_empty() => envelope.error.code,
        _ => format!("HTTP {status}"),
    }
}

impl file_manifest::ManifestTransport for HttpManifestTransport<'_> {
    async fn fetch(&self) -> Result<String, file_manifest::TransportError> {
        let mut request = self
            .client
            .get(format!(
                "{}/api/versions/{}/manifest",
                self.base, self.version_id
            ))
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .timeout(Duration::from_secs(30));
        if let Some(token) = self.token {
            request = request.query(&[("token", token)]);
        }
        let response = request
            .send()
            .await
            .map_err(|error| file_manifest::TransportError::offline(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(file_manifest::TransportError::new(
                Some(status.as_u16()),
                server_error_message(status.as_u16(), &body),
            ));
        }
        response
            .text()
            .await
            .map_err(|error| file_manifest::TransportError::offline(error.to_string()))
    }

    async fn request_extract(&self) -> Result<String, file_manifest::TransportError> {
        if !self.admin {
            return Err(file_manifest::TransportError::new(
                None,
                "extract is only requested from the local server",
            ));
        }
        let mut request = self
            .client
            .post(format!(
                "{}/api/admin/versions/{}/extract",
                self.base, self.version_id
            ))
            .timeout(Duration::from_secs(30));
        if let Some(token) = self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| file_manifest::TransportError::offline(error.to_string()))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(file_manifest::TransportError::new(
                Some(status.as_u16()),
                server_error_message(status.as_u16(), &body),
            ));
        }
        // #235: a disabled cache answers 200 with `status: "unavailable"`.
        // Waiting for a manifest that can never appear would stall the default
        // automatic install for the whole cache timeout, so this is a failure
        // the caller falls back on immediately.
        #[derive(Deserialize)]
        struct Ack {
            #[serde(default)]
            status: String,
            #[serde(default)]
            error: Option<String>,
        }
        if let Ok(ack) = serde_json::from_str::<Ack>(&body) {
            if ack.status.eq_ignore_ascii_case("unavailable") {
                return Err(file_manifest::TransportError::new(
                    Some(409),
                    ack.error
                        .unwrap_or_else(|| "the extracted cache is disabled".to_string()),
                ));
            }
        }
        Ok(body)
    }
}

/// `GET /api/versions/{id}/files/{*path}` (#231).
struct HttpFileTransport<'a> {
    client: &'a reqwest::Client,
    base: &'a str,
    version_id: i64,
    token: Option<&'a str>,
}

struct HttpFileBody {
    response: reqwest::Response,
}

impl file_manifest::FileBody for HttpFileBody {
    fn status(&self) -> u16 {
        self.response.status().as_u16()
    }

    fn content_length(&self) -> Option<u64> {
        self.response.content_length()
    }

    fn content_range_start(&self) -> Option<u64> {
        self.response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range_start)
    }

    fn file_sha256(&self) -> Option<String> {
        self.response
            .headers()
            .get("x-ludex-file-sha256")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        match self.response.chunk().await {
            Ok(Some(bytes)) => Ok(Some(bytes.to_vec())),
            Ok(None) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
}

impl file_manifest::FileTransport for HttpFileTransport<'_> {
    type Body = HttpFileBody;

    async fn open(
        &self,
        path: &str,
        range_start: Option<u64>,
    ) -> Result<Self::Body, file_manifest::TransportError> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/api/versions/{}/files/",
            self.base, self.version_id
        ))
        .map_err(|error| file_manifest::TransportError::offline(error.to_string()))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| file_manifest::TransportError::offline("invalid server URL"))?;
            for part in path.split('/') {
                segments.push(part);
            }
        }
        if let Some(token) = self.token {
            url.query_pairs_mut().append_pair("token", token);
        }
        let mut request = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        if let Some(start) = range_start {
            request = request.header(reqwest::header::RANGE, format!("bytes={start}-"));
        }
        let response = request
            .send()
            .await
            .map_err(|error| file_manifest::TransportError::offline(error.to_string()))?;
        Ok(HttpFileBody { response })
    }
}

/// SQLite-backed per-file checkpoint (#231), keyed by job id so a restart that
/// reuses the job id resumes the same files.
struct DbFileCheckpoints<'a> {
    db: &'a std::sync::Mutex<rusqlite::Connection>,
    job_id: String,
    game_id: i64,
    version_id: i64,
    staging: PathBuf,
}

impl DbFileCheckpoints<'_> {
    fn connection(&self) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, String> {
        self.db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())
    }
}

impl file_manifest::FileCheckpoints for DbFileCheckpoints<'_> {
    fn attest(
        &self,
        archive_sha256: &str,
        total_bytes: u64,
        file_count: u64,
    ) -> Result<file_manifest::Resume, String> {
        let connection = self.connection()?;
        let stored = db::get_file_manifest(&connection, &self.job_id)?;
        let resume = match stored {
            Some(record) if record.archive_sha256 == archive_sha256 => {
                file_manifest::Resume::Adopted
            }
            Some(_) => file_manifest::Resume::Changed,
            None => match db::latest_file_manifest_for_version(
                &connection,
                self.game_id,
                self.version_id,
            )? {
                // Another job of the same version may have staged the files in
                // a different folder: only adopt when it is this job's folder,
                // otherwise the completed rows describe bytes we do not have.
                Some(record)
                    if record.archive_sha256 == archive_sha256
                        && Path::new(&record.staging) == self.staging =>
                {
                    file_manifest::Resume::Adopted
                }
                _ => file_manifest::Resume::Fresh,
            },
        };
        if resume != file_manifest::Resume::Adopted {
            db::clear_job_files(&connection, &self.job_id)?;
        }
        db::upsert_file_manifest(
            &connection,
            &db::FileManifestRecord {
                job_id: self.job_id.clone(),
                game_id: self.game_id,
                version_id: self.version_id,
                archive_sha256: archive_sha256.to_string(),
                staging: self.staging.to_string_lossy().to_string(),
                total_bytes: i64::try_from(total_bytes).unwrap_or(i64::MAX),
                file_count: i64::try_from(file_count).unwrap_or(i64::MAX),
                created_at: now_rfc3339(),
            },
        )?;
        db::prune_file_manifests(&connection, self.game_id, self.version_id, &self.job_id)?;
        Ok(resume)
    }

    fn completed(&self) -> Result<Vec<(String, u64)>, String> {
        let connection = self.connection()?;
        db::completed_job_files(&connection, &self.job_id)
    }

    fn record(&self, relative: &str, size: u64, sha256: &str) -> Result<(), String> {
        let connection = self.connection()?;
        db::record_job_file(
            &connection,
            &self.job_id,
            relative,
            size,
            sha256,
            &now_rfc3339(),
        )
    }

    fn forget(&self, relative: &str) -> Result<(), String> {
        let connection = self.connection()?;
        db::forget_job_file(&connection, &self.job_id, relative)
    }

    fn clear(&self) -> Result<(), String> {
        let connection = self.connection()?;
        db::clear_job_files(&connection, &self.job_id)
    }
}

/// Drop the file-manifest checkpoint of a job (success, cancel, fallback).
pub(super) fn clear_file_checkpoint(app: &AppHandle, job_id: &str) {
    let state = app.state::<AppState>();
    let result = match state.db.lock() {
        Ok(connection) => db::delete_file_manifest(&connection, job_id),
        Err(_) => return,
    };
    if let Err(error) = result {
        tracing::warn!(error = %error, "could not delete the file manifest checkpoint");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #231: a file-manifest job keeps no archive, so the ledger must charge
    /// the extracted tree only, on the install volume, and release it with the
    /// job. This mirrors what `manifest_install` reserves once the manifest
    /// reports its real size.
    #[test]
    fn a_file_manifest_job_charges_only_the_extracted_tree() {
        struct Fixed(u64);
        impl space::Capacity for Fixed {
            fn free_bytes(&self, _path: &Path) -> Option<u64> {
                Some(self.0)
            }
        }

        let total = 40 * 1024 * 1024 * 1024_u64;
        let plan = SpacePlan::build(
            None,
            Some(total),
            Path::new("/"),
            Path::new("/"),
            true,
            false,
        );
        assert_eq!(
            plan.archive_bytes, None,
            "no archive is written on this device"
        );
        assert_eq!(plan.extracted_bytes, Some(total));
        assert_eq!(plan.requirements.len(), 1, "one volume: the install one");
        assert_eq!(plan.requirements[0].label, "extracted files");
        assert_eq!(plan.requirements[0].raw_bytes, total);
        assert_eq!(plan.requirements[0].bytes, space::with_margin(total));

        // A peer job holds part of the volume: the reservation is refused, and
        // granting it charges exactly the peak before the release.
        let ledger = space::SpaceLedger::new();
        assert!(ledger
            .try_reserve("job-1", &plan, &Fixed(space::with_margin(total)))
            .is_granted());
        assert_eq!(ledger.job_bytes("job-1"), space::with_margin(total));
        ledger.release("job-1");
        assert_eq!(ledger.job_bytes("job-1"), 0, "released on done/fail");

        let tight = Fixed(total);
        assert!(!ledger.try_reserve("job-2", &plan, &tight).is_granted());
        assert_eq!(ledger.job_bytes("job-2"), 0, "a refusal charges nothing");
    }
}
