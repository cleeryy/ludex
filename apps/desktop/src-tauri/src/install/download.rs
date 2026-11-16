//! The resumable archive transfer.
//!
//! `download_into` is the transport-generic core: open a connection, write
//! chunks, flush before every reconnect, and resume at the last flushed byte.
//! [`RetryPolicy`]/[`RetryState`] decide when to stop, [`CancelSet`] decides
//! when the user stops it. The HTTP adapter at the top is the only production
//! implementation; the tests inject stalls, resets and a replaced archive.
//!
//! The app-level `download` wrapper is the one place that maps a transfer
//! update onto a job row, so the core stays runnable without Tauri.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::net::retry::{human_duration, parse_retry_after, retry_at};
use crate::net::{CancelSet, DownloadProgress, ProgressSink, RetryPolicy, RetryState, Stopped};
use crate::state::{AppState, JobEntry};
use crate::util::{human_bytes, non_empty};

use super::job::{Failure, InstallParams, InstallPhase};
use super::scheduler::persist;

/// Why a chunk read ended early.
#[derive(Debug)]
pub(crate) enum ChunkError {
    /// The connection went silent beyond the idle timeout.
    Stalled,
    /// The transport reported an error (reset, truncated body, TLS, …).
    Lost(String),
}

/// The part of an HTTP response the resume loop needs.
#[allow(async_fn_in_trait)]
pub(crate) trait DownloadResponse {
    fn status(&self) -> u16;
    fn content_length(&self) -> Option<u64>;
    fn content_range_total(&self) -> Option<u64>;
    /// First byte of a 206 response; `None` when the header is absent.
    fn content_range_start(&self) -> Option<u64> {
        None
    }
    fn etag(&self) -> Option<String>;
    /// `Retry-After` of a 429/503, in seconds (#212). `None` when absent or
    /// unparsable.
    fn retry_after(&self) -> Option<Duration> {
        None
    }
    /// Full archive SHA-256 advertised by the server (#216), when present.
    fn archive_sha256(&self) -> Option<String> {
        None
    }
    /// Next body chunk, or `None` at end of stream.
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ChunkError>;
    /// Body of a failed response, for the user-facing error.
    async fn error_body(self) -> String
    where
        Self: Sized,
    {
        String::new()
    }
}

/// Opens one response for a byte offset (`None` = from zero).
#[allow(async_fn_in_trait)]
pub(crate) trait DownloadTransport {
    type Response: DownloadResponse;
    async fn open(&mut self, range_start: Option<u64>) -> Result<Self::Response, String>;
}

struct HttpTransport<'a> {
    client: &'a reqwest::Client,
    url: &'a str,
    token: Option<&'a str>,
}

struct HttpDownloadResponse {
    response: reqwest::Response,
}

impl DownloadResponse for HttpDownloadResponse {
    fn status(&self) -> u16 {
        self.response.status().as_u16()
    }

    fn content_length(&self) -> Option<u64> {
        self.response.content_length()
    }

    fn content_range_total(&self) -> Option<u64> {
        self.response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range_total)
    }

    fn content_range_start(&self) -> Option<u64> {
        self.response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range_start)
    }

    fn etag(&self) -> Option<String> {
        self.response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    fn retry_after(&self) -> Option<Duration> {
        self.response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after)
    }

    fn archive_sha256(&self) -> Option<String> {
        self.response
            .headers()
            .get(crate::archive_verify::ARCHIVE_SHA256_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ChunkError> {
        match self.response.chunk().await {
            Ok(Some(bytes)) => Ok(Some(bytes.to_vec())),
            Ok(None) => Ok(None),
            Err(error) => Err(ChunkError::Lost(error.to_string())),
        }
    }

    async fn error_body(self) -> String {
        match tokio::time::timeout(Duration::from_secs(5), self.response.text()).await {
            Ok(Ok(body)) => body,
            _ => String::new(),
        }
    }
}

impl DownloadTransport for HttpTransport<'_> {
    type Response = HttpDownloadResponse;

    async fn open(&mut self, range_start: Option<u64>) -> Result<Self::Response, String> {
        let mut request = self
            .client
            .get(self.url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        if let Some(token) = self.token {
            request = request.bearer_auth(token);
        }
        if let Some(start) = range_start {
            request = request.header(reqwest::header::RANGE, format!("bytes={start}-"));
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        Ok(HttpDownloadResponse { response })
    }
}

/// Paces a copy loop to a KiB/s cap. `0` disables throttling.
#[derive(Debug)]
pub(crate) struct Throttle {
    bytes_per_sec: f64,
    started: Instant,
    sent: u64,
}

impl Throttle {
    pub(crate) fn new(limit_kib: u64) -> Option<Self> {
        (limit_kib > 0).then(|| Self {
            bytes_per_sec: limit_kib.saturating_mul(1024) as f64,
            started: Instant::now(),
            sent: 0,
        })
    }

    /// Record `bytes` and return how long the caller must sleep to stay on
    /// schedule. The average rate converges on the configured cap.
    pub(crate) fn pace(&mut self, bytes: usize) -> Option<Duration> {
        self.sent = self.sent.saturating_add(bytes as u64);
        let expected = Duration::from_secs_f64(self.sent as f64 / self.bytes_per_sec);
        expected.checked_sub(self.started.elapsed())
    }
}

/// What one completed transfer produced, for verification (#216).
#[derive(Debug, Clone, Default)]
pub(crate) struct DownloadReport {
    /// SHA-256 computed while writing, set only when every byte was written
    /// once from offset 0 in this run. A resumed transfer leaves it `None` and
    /// the caller rehashes the file from byte 0.
    pub sha256: Option<String>,
    /// Bytes in the finished file.
    pub bytes: u64,
    /// Full archive SHA-256 the server advertised on the last response.
    pub served_sha256: Option<String>,
    /// ETag of the last response, part of the verification cache key (#223).
    pub etag: Option<String>,
    /// A Range resume or an append happened: the file is not a single
    /// from-zero write.
    pub resumed: bool,
}

/// Download `url` into `part`, resuming an existing file with HTTP Range.
///
/// A silent response is abandoned after `tuning.idle_timeout` and the next
/// request resumes at the last flushed byte; reconnects are bounded by
/// `tuning` (#195). The `.part` file is kept when the user pauses; cancelling
/// is handled by the caller (which deletes it). `progress` is called at most
/// every 250 ms.
///
/// The returned [`DownloadReport`] carries the digests needed to verify the
/// archive before extraction (#216).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_file(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    part: &Path,
    title: &str,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    bandwidth_kib: u64,
    tuning: RetryPolicy<'_>,
    sink: impl FnMut(DownloadProgress) + Send,
) -> Result<DownloadReport, Failure> {
    let mut transport = HttpTransport { client, url, token };
    download_into(
        &mut transport,
        part,
        title,
        cancel,
        pause,
        bandwidth_kib,
        tuning,
        sink,
    )
    .await
}

/// `412 MB/s` for the extraction message (#232); decimal megabytes, like the
/// download UI.
pub(super) fn format_speed(bytes_per_second: u64) -> String {
    let megabytes = bytes_per_second as f64 / 1_000_000.0;
    if megabytes >= 100.0 {
        format!("{megabytes:.0} MB/s")
    } else if megabytes >= 1.0 {
        format!("{megabytes:.1} MB/s")
    } else {
        format!("{:.0} KB/s", bytes_per_second as f64 / 1_000.0)
    }
}

/// `2 min` / `45 s` for the extraction ETA (#232).
pub(super) fn format_eta(seconds: u64) -> String {
    crate::net::retry::human_duration(Duration::from_secs(seconds))
}

fn resume_message(done: u64, total: Option<u64>) -> String {
    match total.filter(|total| *total > 0) {
        Some(total) => format!(
            "Connection lost, resuming from {}%",
            done.saturating_mul(100) / total
        ),
        None => format!("Connection lost, resuming from {}", human_bytes(done)),
    }
}

/// One reconnecting progress snapshot, shared by every retry branch.
#[allow(clippy::too_many_arguments)]
fn reconnect_progress(
    attempt: u32,
    done: u64,
    total: Option<u64>,
    last_error: Option<String>,
    stalled_for_ms: Option<u64>,
    next_retry_at: Option<String>,
    http_status: Option<u16>,
    waiting_for_network: bool,
) -> DownloadProgress {
    DownloadProgress {
        done,
        total,
        speed_bps: 0,
        message: restart_message(next_retry_at.as_deref(), done, total),
        reconnecting: true,
        attempt,
        last_error,
        stalled_for_ms,
        next_retry_at,
        http_status,
        waiting_for_network,
    }
}

/// "Retrying automatically" while a timer runs, the resume point while waiting
/// on the OS network (#212). The UI adds its own countdown from `nextRetryAt`.
fn restart_message(next_retry_at: Option<&str>, done: u64, total: Option<u64>) -> String {
    match next_retry_at {
        Some(_) => format!("{}, retrying automatically", resume_message(done, total)),
        None => "Waiting for the network".to_string(),
    }
}

/// The resumable transfer core, generic over the transport so tests can inject
/// stalls, resets and a changed archive deterministically (#195).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_into<T: DownloadTransport>(
    transport: &mut T,
    part: &Path,
    title: &str,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    bandwidth_kib: u64,
    tuning: RetryPolicy<'_>,
    mut progress: impl FnMut(DownloadProgress) + Send,
) -> Result<DownloadReport, Failure> {
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            Failure::Failed(format!(
                "Could not create the download folder {}: {e}",
                parent.display()
            ))
        })?;
    }

    let mut existing = tokio::fs::metadata(part)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    // The streaming hasher is only trustworthy when the file is written once
    // from byte 0. A pre-existing `.part` (resume) or any append invalidates
    // it and the caller rehashes the file from byte 0 instead (#216).
    let mut hasher = (existing == 0).then(Sha256::new);
    let mut resumed = existing > 0;
    let mut served_sha256: Option<String> = None;

    let mut retry = RetryState::new();
    let stops = CancelSet::new(cancel, pause);
    // Guards the "restart from zero" paths so a flapping server cannot loop.
    let mut restarts = 0u32;
    let mut expected_total: Option<u64> = None;
    let mut expected_etag: Option<String> = None;
    let mut preflight_done = false;
    let mut last_error: Option<String> = None;

    'connect: loop {
        // #212: park the retry while the OS reports no network. This wait does
        // not consume an attempt: nothing was tried, so nothing failed.
        if let Some(network) = tuning.network {
            if !network.is_online() {
                progress(reconnect_progress(
                    retry.next_attempt_number(),
                    existing,
                    expected_total,
                    last_error.clone(),
                    None,
                    None,
                    None,
                    true,
                ));
                stops.wait_online(network).await?;
            }
        }
        retry.next_attempt();

        let opened = stops
            .select(transport.open((existing > 0).then_some(existing)))
            .await?;

        let mut response = match opened {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(error.clone());
                if let Some(reason) = retry.exhausted(tuning) {
                    return Err(Failure::Failed(format!(
                        "The download kept failing ({reason}). Last error: {error}. The partial archive was kept; install again to resume."
                    )));
                }
                let delay = tuning.delay(retry.attempt());
                progress(reconnect_progress(
                    retry.next_attempt_number(),
                    existing,
                    expected_total,
                    Some(error),
                    None,
                    Some(retry_at(delay)),
                    None,
                    false,
                ));
                if !stops.sleep(delay, tuning.retry_now).await? {
                    return Err(Failure::Cancelled);
                }
                continue 'connect;
            }
        };

        let status = response.status();
        let retry_after = response.retry_after();
        if (200..300).contains(&status) {
            if let Some(digest) = response.archive_sha256() {
                served_sha256 = Some(digest);
            }
        }

        if status == 416 {
            let total = response.content_range_total();
            // The `.part` is only complete if it matches the archive the server
            // still has; a replaced archive must restart, not pass as done.
            let unchanged = expected_total.is_none() || expected_total == total;
            if existing > 0 && total == Some(existing) && unchanged {
                // The `.part` file is already complete: a previous run
                // downloaded everything but failed before extraction.
                let mut update = DownloadProgress::plain(
                    retry.attempt(),
                    existing,
                    Some(existing),
                    0,
                    "Download already complete".to_string(),
                );
                update.http_status = Some(status);
                progress(update);
                // The file predates this run: the caller rehashes it from 0.
                return Ok(DownloadReport {
                    sha256: None,
                    bytes: existing,
                    served_sha256,
                    etag: response.etag(),
                    resumed: true,
                });
            }
            if existing > 0 && restarts < 2 {
                restarts += 1;
                tracing::warn!(
                    part = %part.display(),
                    bytes = existing,
                    "server rejected the resume offset; restarting the archive explicitly"
                );
                let _ = tokio::fs::remove_file(part).await;
                existing = 0;
                hasher = Some(Sha256::new());
                resumed = false;
                expected_total = None;
                expected_etag = None;
                progress(DownloadProgress {
                    done: 0,
                    total: None,
                    speed_bps: 0,
                    message: "The server could not resume this file; restarting from 0%"
                        .to_string(),
                    reconnecting: true,
                    attempt: retry.next_attempt_number(),
                    last_error: last_error.clone(),
                    stalled_for_ms: None,
                    next_retry_at: None,
                    http_status: Some(status),
                    waiting_for_network: false,
                });
                continue 'connect;
            }
            // #212: the resume offset was rejected twice; retrying the same
            // Range cannot help, so this is permanent.
            let message = "The server rejected the resume request (HTTP 416). Delete the partial download and retry.".to_string();
            let _ = retry.record_failure(&message, true);
            return Err(Failure::Failed(message));
        }

        if !(200..300).contains(&status) {
            let retryable = matches!(status, 408 | 425 | 429 | 500..=599);
            let body = response.error_body().await;
            let snippet = body.trim().chars().take(300).collect::<String>();
            let hint = match status {
                401 | 403 => " Check that you are signed in to the server.",
                404 => " The game version was not found on the server.",
                _ => "",
            };
            let detail = if snippet.is_empty() {
                String::new()
            } else {
                format!(": {snippet}")
            };
            let message = format!("Download failed with HTTP {status}{hint}{detail}");
            if !retryable {
                // #212: the permanent failures stop at once and keep the
                // `.part`; the circuit breaker only guards against a repeat.
                let _ = retry.record_failure(&message, true);
                return Err(Failure::Failed(message));
            }
            last_error = Some(message.clone());
            if let Some(reason) = retry
                .record_failure(&message, false)
                .or_else(|| retry.exhausted(tuning))
            {
                return Err(Failure::Failed(format!(
                    "The download kept failing ({reason}). Last error: {}. The partial archive was kept; install again to resume.",
                    last_error.clone().unwrap_or_default()
                )));
            }
            // #212: honour `Retry-After` (server reconnect rate limit); the
            // exponential delay is a floor, not a replacement.
            let delay = tuning.delay_with_hint(retry.attempt(), retry_after);
            progress(reconnect_progress(
                retry.next_attempt_number(),
                existing,
                expected_total,
                last_error.clone(),
                None,
                Some(retry_at(delay)),
                Some(status),
                false,
            ));
            if !stops.sleep(delay, tuning.retry_now).await? {
                return Err(Failure::Cancelled);
            }
            continue 'connect;
        }

        let append = status == 206 && existing > 0;
        if !append && existing > 0 {
            // The server ignored `Range` and started from zero. Truncating is
            // the only honest option; say so instead of appending a full body
            // to an existing partial file.
            tracing::warn!(
                part = %part.display(),
                bytes = existing,
                "server ignored the Range request; restarting the archive"
            );
            existing = 0;
            hasher = Some(Sha256::new());
            resumed = false;
        }

        let content_length = response.content_length();
        let content_range_total = response.content_range_total();
        let total = if status == 206 {
            content_range_total.or_else(|| content_length.map(|length| length + existing))
        } else {
            content_length
        };

        // A resumed response must describe the same archive. A size or ETag
        // change means the server replaced the file: restart explicitly (with
        // a visible message) instead of splicing two different archives.
        if append {
            let etag_changed = match (&expected_etag, response.etag()) {
                (Some(known), Some(current)) => *known != current,
                _ => false,
            };
            let total_changed =
                matches!((expected_total, total), (Some(known), Some(current)) if known != current);
            let start_changed =
                matches!(response.content_range_start(), Some(start) if start != existing);
            if start_changed || etag_changed || total_changed {
                if restarts < 2 {
                    restarts += 1;
                    tracing::warn!(
                        part = %part.display(),
                        bytes = existing,
                        ?expected_total,
                        ?total,
                        start = response.content_range_start(),
                        "the archive changed on the server; restarting from zero"
                    );
                    let _ = tokio::fs::remove_file(part).await;
                    existing = 0;
                    hasher = Some(Sha256::new());
                    resumed = false;
                    expected_total = None;
                    expected_etag = None;
                    progress(DownloadProgress {
                        done: 0,
                        total: None,
                        speed_bps: 0,
                        message:
                            "The archive changed on the server; restarting the download from 0%"
                                .to_string(),
                        reconnecting: true,
                        attempt: retry.next_attempt_number(),
                        last_error: last_error.clone(),
                        stalled_for_ms: None,
                        next_retry_at: None,
                        http_status: Some(status),
                        waiting_for_network: false,
                    });
                    continue 'connect;
                }
                // #212: two explicit restarts did not converge; the archive
                // keeps changing, which no retry can fix.
                let message = "The archive changed on the server while the download was resuming. Start the install again to fetch the new version.".to_string();
                let _ = retry.record_failure(&message, true);
                return Err(Failure::Failed(message));
            }
            if expected_total.is_none() {
                expected_total = total;
            }
            if expected_etag.is_none() {
                expected_etag = response.etag();
            }
        } else {
            expected_total = total;
            expected_etag = response.etag();
        }

        if append {
            // Bytes already on disk were not hashed by this run (or were
            // hashed before an uncertain flush): a full rehash is required.
            hasher = None;
            resumed = true;
        }

        let mut done = existing;

        if !preflight_done {
            preflight_done = true;
            // Disk preflight: never start a download that cannot fit.
            // Extraction size is unknown, so only the archive is required.
            let needed = total.unwrap_or(0).saturating_sub(done);
            if needed > 0 {
                if let Some(directory) = part.parent() {
                    match fs2::available_space(directory) {
                        Ok(free) => {
                            if let Err(message) = ensure_download_space(needed, free, directory) {
                                return Err(Failure::Failed(message));
                            }
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, path = %directory.display(), "could not check free disk space");
                        }
                    }
                    // #222: reserve the blocks now, so the filesystem refuses
                    // before 40 GB are on the wire instead of at the end.
                    if let Err(message) = preallocate_blocks(directory, needed) {
                        return Err(Failure::Failed(message));
                    }
                }
            }
        }

        let mut started_update =
            DownloadProgress::plain(retry.attempt(), done, total, 0, download_message(title));
        started_update.http_status = Some(status);
        progress(started_update);

        let file = if append {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(part)
                .await
                .map_err(|e| {
                    Failure::Failed(format!(
                        "Could not open the partial download {}: {e}",
                        part.display()
                    ))
                })?
        } else {
            tokio::fs::File::create(part).await.map_err(|e| {
                Failure::Failed(format!(
                    "Could not create the download file {}: {e}",
                    part.display()
                ))
            })?
        };

        let mut writer = tokio::io::BufWriter::with_capacity(1024 * 1024, file);
        let mut throttle = Throttle::new(bandwidth_kib);
        let mut last_emit = Instant::now();
        let mut last_instant = Instant::now();
        let mut last_bytes = done;
        let mut last_chunk = Instant::now();
        let mut interrupted: Option<(ChunkError, u64)> = None;

        loop {
            let chunk = stops
                .select(tokio::time::timeout(
                    tuning.idle_timeout,
                    response.next_chunk(),
                ))
                .await;
            match chunk {
                Err(Stopped::Cancelled) => {
                    let _ = writer.flush().await;
                    return Err(Failure::Cancelled);
                }
                Err(Stopped::Paused) => {
                    let _ = writer.flush().await;
                    return Err(Failure::Paused);
                }
                Ok(Err(_elapsed)) => {
                    let silent = last_chunk.elapsed();
                    flush_before_resume(&mut writer, part).await?;
                    interrupted = Some((ChunkError::Stalled, silent.as_millis() as u64));
                    break;
                }
                Ok(Ok(Ok(Some(bytes)))) => {
                    writer
                        .write_all(&bytes)
                        .await
                        .map_err(|e| Failure::Failed(download_write_failure(part, &e)))?;
                    if let Some(hasher) = hasher.as_mut() {
                        hasher.update(&bytes);
                    }
                    done = done.saturating_add(bytes.len() as u64);
                    last_chunk = Instant::now();

                    if let Some(throttle) = throttle.as_mut() {
                        if let Some(delay) = throttle.pace(bytes.len()) {
                            if delay > Duration::from_millis(2) {
                                if let Err(stop) = stops.select(tokio::time::sleep(delay)).await {
                                    let _ = writer.flush().await;
                                    return Err(stop.into());
                                }
                            }
                        }
                    }

                    if last_emit.elapsed() >= Duration::from_millis(250) {
                        let elapsed = last_instant.elapsed().as_secs_f64().max(0.001);
                        let speed = ((done.saturating_sub(last_bytes)) as f64 / elapsed) as u64;
                        progress(DownloadProgress::plain(
                            retry.attempt(),
                            done,
                            total,
                            speed,
                            download_message(title),
                        ));
                        last_emit = Instant::now();
                        last_instant = Instant::now();
                        last_bytes = done;
                    }
                }
                Ok(Ok(Ok(None))) => break,
                Ok(Ok(Err(error))) => {
                    let silent = last_chunk.elapsed();
                    flush_before_resume(&mut writer, part).await?;
                    interrupted = Some((error, silent.as_millis() as u64));
                    break;
                }
            }
        }

        if let Some((error, silent_ms)) = interrupted {
            // The connection is gone but every byte written so far is on disk:
            // the next attempt asks for exactly this offset. The hasher may
            // cover bytes past the flushed offset, so it is discarded and the
            // resumed file is rehashed from byte 0 before install (#216).
            hasher = None;
            resumed = true;
            existing = done;
            let reason = match &error {
                ChunkError::Stalled => format!(
                    "no data for {}",
                    human_duration(Duration::from_millis(silent_ms))
                ),
                ChunkError::Lost(message) => message.clone(),
            };
            last_error = Some(reason.clone());
            if let Some(stop) = retry
                .record_failure(&reason, false)
                .or_else(|| retry.exhausted(tuning))
            {
                return Err(Failure::Failed(format!(
                    "The download stopped at {} ({stop}). Last error: {}. The partial archive was kept; install again to resume.",
                    human_bytes(existing),
                    last_error.clone().unwrap_or_default()
                )));
            }
            let delay = tuning.delay(retry.attempt());
            tracing::warn!(
                part = %part.display(),
                bytes = existing,
                attempt = retry.attempt(),
                error = %last_error.clone().unwrap_or_default(),
                "download interrupted; reconnecting from the last flushed byte"
            );
            progress(reconnect_progress(
                retry.next_attempt_number(),
                existing,
                total,
                last_error.clone(),
                Some(silent_ms),
                Some(retry_at(delay)),
                Some(status),
                false,
            ));
            if !stops.sleep(delay, tuning.retry_now).await? {
                return Err(Failure::Cancelled);
            }
            continue 'connect;
        }

        writer.flush().await.map_err(|e| {
            Failure::Failed(format!(
                "Could not flush the download to {}: {e}",
                part.display()
            ))
        })?;
        let file = writer.into_inner();
        file.sync_all().await.map_err(|e| {
            Failure::Failed(format!(
                "Could not flush the download to disk ({}): {e}",
                part.display()
            ))
        })?;

        let mut completed_update = DownloadProgress::plain(
            retry.attempt(),
            done,
            Some(total.unwrap_or(done)),
            0,
            "Download complete".to_string(),
        );
        completed_update.http_status = Some(status);
        progress(completed_update);

        return Ok(DownloadReport {
            sha256: hasher.map(|hasher| hex::encode(hasher.finalize())),
            bytes: done,
            served_sha256,
            etag: expected_etag,
            resumed,
        });
    }
}

/// Flush the buffered bytes before a reconnect. The resume offset is only
/// valid if everything counted in `done` reached the file (#195).
async fn flush_before_resume(
    writer: &mut tokio::io::BufWriter<tokio::fs::File>,
    part: &Path,
) -> Result<(), Failure> {
    writer.flush().await.map_err(|error| {
        Failure::Failed(format!(
            "Could not flush the download to {}: {error}",
            part.display()
        ))
    })
}

/// Fetch the archive of one job into `part`, mapping every update onto the job
/// row. The core does the transfer; this is the only Tauri-aware piece.
pub(super) async fn download(
    app: &AppHandle,
    entry: &Arc<JobEntry>,
    params: &InstallParams,
    part: &Path,
) -> Result<DownloadReport, Failure> {
    let state = app.state::<AppState>();
    let client = state.http.clone();
    let settings = state.settings_snapshot();
    let bandwidth_kib = settings.bandwidth_limit_kib;
    let tuning = RetryPolicy {
        network: Some(&state.network),
        retry_now: Some(entry.retry_signal()),
        ..RetryPolicy::from_settings(
            settings.download_stall_timeout_secs,
            settings.download_retry_window_secs,
        )
    };
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() {
        return Err(Failure::Failed(
            "No server URL was provided for this download.".to_string(),
        ));
    }

    let url = format!(
        "{base}/api/games/{}/download?versionId={}",
        params.game_id, params.version_id
    );
    let cancel = entry.cancel.clone();
    let pause = entry.pause_token();
    let handle = app.clone();
    let job_entry = Arc::clone(entry);

    let mut sink = JobProgressSink {
        handle,
        entry: job_entry,
    };

    download_file(
        &client,
        &url,
        non_empty(params.token.as_deref()),
        part,
        &params.title,
        &cancel,
        &pause,
        bandwidth_kib,
        tuning,
        |update| sink.progress(update),
    )
    .await
}

/// Maps a transfer update onto a job row, persists it and emits the event.
///
/// This used to be an inline closure: the field mapping is the wire contract
/// of `install://progress`, so it lives in one named place.
struct JobProgressSink {
    handle: AppHandle,
    entry: Arc<JobEntry>,
}

impl ProgressSink for JobProgressSink {
    fn progress(&mut self, update: DownloadProgress) {
        self.entry.update(|job| {
            job.phase = InstallPhase::Downloading;
            job.bytes_done = update.done;
            job.bytes_total = update.total;
            job.speed_bps = update.speed_bps;
            job.message = Some(update.message);
            job.reconnecting = update.reconnecting;
            job.attempt = update.attempt;
            job.last_error = update.last_error;
            job.stalled_for_ms = update.stalled_for_ms;
            job.next_retry_at = update.next_retry_at;
            job.waiting_for_network = update.waiting_for_network;
            // Keep the last HTTP status: an open error carries none, and the
            // diagnostics bundle still wants the failing response.
            job.http_status = update.http_status.or(job.http_status);
        });
        persist(&self.handle, &self.entry);
        let _ = self
            .handle
            .emit("install://progress", self.entry.snapshot());
    }
}

/// Delete the temp archive as soon as extraction/copy succeeded, or keep it
/// under a friendly name when `keepArchives` is enabled.
pub(super) async fn cleanup_archive(part: &Path, keep: bool, name: &str) {
    if keep {
        let kept = part
            .parent()
            .map(|directory| directory.join(name))
            .unwrap_or_else(|| PathBuf::from(name));
        if let Err(error) = tokio::fs::rename(part, &kept).await {
            tracing::warn!(error = %error, "could not keep the downloaded archive");
        }
    } else if let Err(error) = tokio::fs::remove_file(part).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(error = %error, "could not delete the downloaded archive");
        }
    }
}

fn ensure_download_space(needed: u64, free: u64, directory: &Path) -> Result<(), String> {
    if needed > 0 && free < needed {
        Err(format!(
            "Not enough free space in {}: {} needed, {} available. Free space or change the download folder.",
            directory.display(),
            human_bytes(needed),
            human_bytes(free)
        ))
    } else {
        Ok(())
    }
}

/// True for a filesystem that ran out of space (#222). `ErrorKind::StorageFull`
/// is still unstable, so the OS error number and the message are used.
pub(crate) fn is_out_of_space(error: &std::io::Error) -> bool {
    // 28 = ENOSPC, 112 = ERROR_DISK_FULL, 39 = ERROR_HANDLE_DISK_FULL.
    if matches!(error.raw_os_error(), Some(28) | Some(112) | Some(39)) {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("no space left") || message.contains("disk full")
}

/// Message for a failed write: a full volume names the volume and keeps the
/// checkpoint, everything else keeps the OS detail (#222).
pub(crate) fn download_write_failure(part: &Path, error: &std::io::Error) -> String {
    if is_out_of_space(error) {
        let volume = part
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| part.display().to_string());
        format!(
            "The download volume {volume} is full. The partial archive was kept at {}; free space on that volume and resume the install.",
            part.display()
        )
    } else {
        format!(
            "Could not write the download to {}: {error}",
            part.display()
        )
    }
}

/// Message for a failed extraction (#222): a full volume names the volume, and
/// the archive is kept so the install can be resumed after freeing space.
pub(crate) fn extraction_failure(staging: &Path, error: String) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("no space left") || lower.contains("disk full") || lower.contains("enospc") {
        format!(
            "Extraction ran out of space in {}. The downloaded archive was kept; free space on that volume and resume the install. ({error})",
            staging.display()
        )
    } else {
        format!(
            "Could not extract the archive into {}: {error}",
            staging.display()
        )
    }
}

/// Reserve the blocks for the remaining bytes before any of them is written
/// (#222), so a nearly-full filesystem fails at once instead of at byte 40 GB.
///
/// The reservation is a **sibling probe file**: growing the real `.part` would
/// make a sparse file look complete to the resume offset, which is the file
/// length. `set_len` alone is sparse, so `fs2::allocate` asks the filesystem to
/// reserve real blocks; a filesystem that cannot answer (EOPNOTSUPP) is
/// accepted, a full one is reported.
pub(crate) fn preallocate_blocks(directory: &Path, bytes: u64) -> Result<(), String> {
    if bytes == 0 {
        return Ok(());
    }
    let probe = directory.join(format!(
        ".ludex-space-{}.probe",
        uuid::Uuid::new_v4().simple()
    ));
    let reserved = (|| -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.set_len(bytes)?;
        match fs2::FileExt::allocate(&file, bytes) {
            Ok(()) => Ok(()),
            // The filesystem cannot reserve blocks (FAT, some network shares):
            // the free-space check is all the protection there is.
            Err(error) if error.raw_os_error() == Some(95) => Ok(()),
            Err(error) => Err(error),
        }
    })();
    let _ = std::fs::remove_file(&probe);
    match reserved {
        Ok(()) => Ok(()),
        Err(error) if is_out_of_space(&error) => Err(format!(
            "Not enough free space in {} to reserve {} for the download. Free space or change the download folder.",
            directory.display(),
            human_bytes(bytes)
        )),
        Err(error) => {
            tracing::debug!(error = %error, path = %directory.display(), "could not reserve download blocks");
            Ok(())
        }
    }
}

fn download_message(title: &str) -> String {
    format!(
        "Downloading \"{title}\" (peak disk use: archive + extracted; archive deleted after extraction)"
    )
}

pub(super) fn parse_content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.trim().parse().ok()
}

/// Parse the first byte of `bytes <start>-<end>/<total>`.
pub(super) fn parse_content_range_start(value: &str) -> Option<u64> {
    let range = value.trim().strip_prefix("bytes ")?;
    let (start, _) = range.split_once('-')?;
    start.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::DownloadTuning;
    use crate::manifest;

    #[test]
    fn content_range_parsing() {
        assert_eq!(parse_content_range_total("bytes 100-199/200"), Some(200));
        assert_eq!(parse_content_range_total("bytes 0-0/*"), None);
        assert_eq!(parse_content_range_total("bytes */4096"), Some(4096));
        assert_eq!(parse_content_range_total("bytes */"), None);
    }

    /// #232: the extraction message states the measured rate and a real ETA,
    /// in units a user can read.
    #[test]
    fn extraction_speed_and_eta_are_human_readable() {
        assert_eq!(format_speed(412_000_000), "412 MB/s");
        assert_eq!(format_speed(47_600_000), "47.6 MB/s");
        assert_eq!(format_speed(512_000), "512 KB/s");
        assert_eq!(format_eta(45), "45 s");
        assert_eq!(format_eta(150), "2 min");
        assert_eq!(format_eta(7_200), "2 h");
    }

    #[test]
    fn download_space_preflight() {
        let dir = Path::new("/downloads");
        assert!(ensure_download_space(100, 200, dir).is_ok());
        let error = ensure_download_space(200, 100, dir).unwrap_err();
        assert!(error.contains("Not enough free space"));
        assert!(error.contains("/downloads"));
        // Unknown size (0) never blocks.
        assert!(ensure_download_space(0, 0, dir).is_ok());
    }

    #[test]
    fn a_full_volume_is_reported_with_the_volume_and_keeps_the_checkpoint() {
        let enospc = std::io::Error::from_raw_os_error(28);
        assert!(is_out_of_space(&enospc));
        assert!(is_out_of_space(&std::io::Error::other(
            "No space left on device"
        )));
        assert!(!is_out_of_space(&std::io::Error::from_raw_os_error(13)));

        let part = Path::new("/mnt/games/.7-9.part");
        let message = download_write_failure(part, &enospc);
        assert!(message.contains("/mnt/games"), "{message}");
        assert!(message.contains("full"), "{message}");
        assert!(message.contains("partial archive was kept"), "{message}");
        assert!(message.contains("/mnt/games/.7-9.part"), "{message}");

        // A non-space error keeps the OS detail instead of blaming the disk.
        let other = std::io::Error::from_raw_os_error(5);
        let message = download_write_failure(part, &other);
        assert!(
            message.contains("Could not write the download"),
            "{message}"
        );
    }

    #[test]
    fn an_extraction_out_of_space_names_the_install_volume() {
        let message = extraction_failure(
            Path::new("/mnt/games/.hades.staging-abc"),
            "7z error: No space left on device".to_string(),
        );
        assert!(
            message.contains("/mnt/games/.hades.staging-abc"),
            "{message}"
        );
        assert!(message.contains("downloaded archive was kept"), "{message}");
        let message =
            extraction_failure(Path::new("/mnt/games/staging"), "bad password".to_string());
        assert!(message.contains("Could not extract"), "{message}");
        assert!(!message.contains("kept"), "{message}");
    }

    #[test]
    fn the_download_blocks_are_reserved_before_the_first_byte() {
        let root = download_temp_dir("preallocate");
        // A tiny reservation succeeds and leaves no probe file behind.
        preallocate_blocks(&root, 64 * 1024).expect("reserve");
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains("space-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "probe files left behind: {leftovers:?}"
        );
        // Zero bytes is a no-op, never an error.
        preallocate_blocks(&root, 0).expect("no-op");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn download_temp_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-download-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[tokio::test]
    async fn pause_then_resume_produces_identical_bytes() {
        let payload: Vec<u8> = (0..512 * 1024).map(|index| (index % 251) as u8).collect();
        let server = crate::test_support::serve_bytes(payload.clone()).await;
        let root = download_temp_dir("pause");
        let part = root.join("archive.part");
        let client = reqwest::Client::builder().build().unwrap();

        let cancel = CancellationToken::new();
        let pause = CancellationToken::new();
        let pause_from_progress = pause.clone();
        let mut paused_once = false;

        let first = download_file(
            &client,
            &server.url,
            None,
            &part,
            "Paused game",
            &cancel,
            &pause,
            256,
            DownloadTuning::default(),
            move |update| {
                if update.done > 0 && !paused_once {
                    paused_once = true;
                    pause_from_progress.cancel();
                }
            },
        )
        .await;
        assert!(matches!(&first, Err(Failure::Paused)));

        let partial = std::fs::metadata(&part).unwrap().len();
        assert!(partial > 0, "the partial archive must be kept");
        assert!(
            partial < payload.len() as u64,
            "the download should stop before the end"
        );

        // Resume from the byte offset with a fresh pause token.
        let pause = CancellationToken::new();
        download_file(
            &client,
            &server.url,
            None,
            &part,
            "Paused game",
            &cancel,
            &pause,
            0,
            DownloadTuning::default(),
            |_| {},
        )
        .await
        .unwrap();

        let (size, sha256) = manifest::hash_file(&part).unwrap();
        assert_eq!(size, payload.len() as u64);
        let source = root.join("source.bin");
        std::fs::write(&source, &payload).unwrap();
        let expected = manifest::hash_file(&source).unwrap().1;
        assert_eq!(sha256, expected, "resume must produce the same bytes");

        let ranges = server.ranges.lock().unwrap().clone();
        assert!(
            ranges.iter().any(|range| range.starts_with("bytes=")),
            "the resume must use a Range request, got {ranges:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn bandwidth_limit_slows_the_download() {
        let payload = vec![3u8; 256 * 1024];
        let server = crate::test_support::serve_bytes(payload.clone()).await;
        let root = download_temp_dir("throttle");
        let part = root.join("archive.part");
        let client = reqwest::Client::builder().build().unwrap();

        let started = Instant::now();
        download_file(
            &client,
            &server.url,
            None,
            &part,
            "Throttled game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            512,
            DownloadTuning::default(),
            |_| {},
        )
        .await
        .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(
            std::fs::metadata(&part).unwrap().len(),
            payload.len() as u64
        );
        // 256 KiB at 512 KiB/s is ~0.5 s; allow a loose lower bound for slow CI.
        assert!(
            elapsed >= Duration::from_millis(300),
            "the download ignored the bandwidth limit: {elapsed:?}"
        );
        assert!(
            elapsed <= Duration::from_secs(10),
            "the throttle stalled the download: {elapsed:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /* --------------------- archive verification (#216) --------------------- */

    #[tokio::test]
    async fn a_fresh_download_reports_a_streaming_digest() {
        let payload: Vec<u8> = (0..200 * 1024).map(|index| (index % 251) as u8).collect();
        let digest = hex::encode(Sha256::digest(&payload));
        let (mut transport, _ranges) = FakeTransport::new(vec![FakeConn {
            status: 200,
            declared_length: Some(payload.len() as u64),
            range_total: None,
            etag: None,
            sha256: Some(digest.clone()),
            body: payload.clone(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let root = download_temp_dir("verify-fresh");
        let part = root.join("archive.part");

        let report = download_into(
            &mut transport,
            &part,
            "Verified game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await
        .expect("download");

        assert!(!report.resumed);
        assert_eq!(report.sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(report.served_sha256.as_deref(), Some(digest.as_str()));
        assert_eq!(report.bytes, payload.len() as u64);

        // The streaming digest is trusted: verifying must not need a re-read.
        let missing = root.join("already-moved.part");
        let verdict = crate::archive_verify::verify_download(
            &missing,
            Some(&digest),
            report.sha256.as_deref(),
            report.served_sha256.as_deref(),
        )
        .expect("verify");
        assert!(
            matches!(verdict, crate::archive_verify::ArchiveVerdict::Verified(_)),
            "{verdict:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_resumed_download_is_rehashed_from_byte_zero() {
        let payload: Vec<u8> = (0..300 * 1024).map(|index| (index % 243) as u8).collect();
        let split = 100 * 1024;
        let digest = hex::encode(Sha256::digest(&payload));
        let root = download_temp_dir("verify-resume");
        let part = root.join("archive.part");
        std::fs::write(&part, &payload[..split]).expect("seed partial");
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 206,
            declared_length: Some((payload.len() - split) as u64),
            range_total: Some(payload.len() as u64),
            etag: Some("\"v1\"".to_string()),
            sha256: Some(digest.clone()),
            body: payload[split..].to_vec(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);

        let report = download_into(
            &mut transport,
            &part,
            "Resumed game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await
        .expect("resume");

        assert_eq!(*ranges.lock().unwrap(), vec![Some(split as u64)]);
        assert!(report.resumed);
        assert!(
            report.sha256.is_none(),
            "a resumed writer cannot produce a from-zero digest"
        );

        // Hashing the file from byte 0 (not only the new bytes) verifies it.
        let verdict = crate::archive_verify::verify_download(
            &part,
            Some(&digest),
            report.sha256.as_deref(),
            report.served_sha256.as_deref(),
        )
        .expect("verify");
        assert_eq!(
            verdict,
            crate::archive_verify::ArchiveVerdict::Verified(digest)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_corrupted_body_never_reaches_the_install_gate() {
        let honest: Vec<u8> = (0..120 * 1024).map(|index| (index % 239) as u8).collect();
        let expected = hex::encode(Sha256::digest(&honest));
        // The server advertises the honest digest, the body is damaged in
        // transit: a transport corruption, not a replaced archive.
        let mut corrupt = honest.clone();
        corrupt[64 * 1024] ^= 0x7F;
        let (mut transport, _ranges) = FakeTransport::new(vec![FakeConn {
            status: 200,
            declared_length: Some(corrupt.len() as u64),
            range_total: None,
            etag: None,
            sha256: Some(expected.clone()),
            body: corrupt,
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let root = download_temp_dir("verify-corrupt");
        let part = root.join("archive.part");

        let report = download_into(
            &mut transport,
            &part,
            "Corrupt game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await
        .expect("download");

        let verdict = crate::archive_verify::verify_download(
            &part,
            Some(&expected),
            report.sha256.as_deref(),
            report.served_sha256.as_deref(),
        )
        .expect("verify");
        let crate::archive_verify::ArchiveVerdict::Mismatch(mismatch) = verdict else {
            panic!("a corrupted body must be rejected: {verdict:?}");
        };
        assert!(!mismatch.changed_on_server);
        let message = mismatch.message("Corrupt game");
        assert!(message.contains(&expected), "{message}");
        assert!(message.contains(&mismatch.actual), "{message}");
        assert!(message.contains("Nothing was installed"), "{message}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /* --------------------- injected-transport resume tests (#195) ---------------- */

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum After {
        End,
        Stall,
        Lost,
    }

    struct FakeConn {
        status: u16,
        declared_length: Option<u64>,
        range_total: Option<u64>,
        etag: Option<String>,
        /// Full archive digest the fake server advertises (#216).
        sha256: Option<String>,
        body: Vec<u8>,
        after: After,
        /// Pause between chunks, to model a slow but healthy transfer.
        chunk_delay: Duration,
    }

    struct FakeTransport {
        conns: std::collections::VecDeque<FakeConn>,
        ranges: Arc<std::sync::Mutex<Vec<Option<u64>>>>,
    }

    struct FakeResponse {
        conn: FakeConn,
        sent: usize,
    }

    impl FakeTransport {
        fn new(conns: Vec<FakeConn>) -> (Self, Arc<std::sync::Mutex<Vec<Option<u64>>>>) {
            let ranges = Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    conns: conns.into(),
                    ranges: Arc::clone(&ranges),
                },
                ranges,
            )
        }
    }

    impl DownloadTransport for FakeTransport {
        type Response = FakeResponse;

        async fn open(&mut self, range_start: Option<u64>) -> Result<Self::Response, String> {
            self.ranges.lock().unwrap().push(range_start);
            let conn = self
                .conns
                .pop_front()
                .ok_or_else(|| "the fake server ran out of scripted responses".to_string())?;
            Ok(FakeResponse { conn, sent: 0 })
        }
    }

    impl DownloadResponse for FakeResponse {
        fn status(&self) -> u16 {
            self.conn.status
        }

        fn content_length(&self) -> Option<u64> {
            self.conn.declared_length
        }

        fn content_range_total(&self) -> Option<u64> {
            self.conn.range_total
        }

        fn etag(&self) -> Option<String> {
            self.conn.etag.clone()
        }

        fn archive_sha256(&self) -> Option<String> {
            self.conn.sha256.clone()
        }

        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ChunkError> {
            const STEP: usize = 16 * 1024;
            if self.sent < self.conn.body.len() {
                if !self.conn.chunk_delay.is_zero() {
                    tokio::time::sleep(self.conn.chunk_delay).await;
                }
                let end = (self.sent + STEP).min(self.conn.body.len());
                let chunk = self.conn.body[self.sent..end].to_vec();
                self.sent = end;
                return Ok(Some(chunk));
            }
            match self.conn.after {
                After::End => Ok(None),
                // A silent connection: the caller's idle timeout must fire.
                After::Stall => {
                    std::future::pending::<()>().await;
                    Ok(None)
                }
                After::Lost => Err(ChunkError::Lost("connection reset by peer".to_string())),
            }
        }

        async fn error_body(self) -> String {
            "fake error".to_string()
        }
    }

    fn test_tuning(idle_ms: u64, max_attempts: u32) -> DownloadTuning<'static> {
        DownloadTuning {
            idle_timeout: Duration::from_millis(idle_ms),
            max_attempts,
            max_retry_window: Some(Duration::from_secs(30)),
            base_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(5),
            ..DownloadTuning::default()
        }
    }

    #[tokio::test]
    async fn stall_resumes_at_the_exact_flushed_offset() {
        let payload: Vec<u8> = (0..300 * 1024).map(|index| (index % 251) as u8).collect();
        let split = 64 * 1024;
        let (mut transport, ranges) = FakeTransport::new(vec![
            FakeConn {
                status: 200,
                declared_length: Some(payload.len() as u64),
                range_total: None,
                etag: Some("\"v1\"".to_string()),
                sha256: None,
                body: payload[..split].to_vec(),
                after: After::Stall,
                chunk_delay: Duration::ZERO,
            },
            FakeConn {
                status: 206,
                declared_length: Some((payload.len() - split) as u64),
                range_total: Some(payload.len() as u64),
                etag: Some("\"v1\"".to_string()),
                sha256: None,
                body: payload[split..].to_vec(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            },
        ]);

        let root = download_temp_dir("stall");
        let part = root.join("archive.part");
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&updates);

        let result = download_into(
            &mut transport,
            &part,
            "Stalled game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(40, 4),
            move |update| sink.lock().unwrap().push(update),
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(std::fs::read(&part).unwrap(), payload);
        assert_eq!(
            *ranges.lock().unwrap(),
            vec![None, Some(split as u64)],
            "the resume must ask for the last flushed byte"
        );
        let updates = updates.lock().unwrap();
        let reconnect = updates
            .iter()
            .find(|update| update.reconnecting)
            .expect("a reconnecting progress update");
        assert_eq!(reconnect.done, split as u64);
        assert_eq!(reconnect.total, Some(payload.len() as u64));
        assert_eq!(reconnect.attempt, 2, "the resume is the second connection");
        assert!(reconnect.stalled_for_ms.is_some());
        assert!(
            reconnect
                .message
                .starts_with("Connection lost, resuming from"),
            "{}",
            reconnect.message
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_slow_but_progressing_transfer_is_not_a_stall() {
        let payload: Vec<u8> = vec![9u8; 128 * 1024];
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 200,
            declared_length: Some(payload.len() as u64),
            range_total: None,
            etag: None,
            sha256: None,
            body: payload.clone(),
            after: After::End,
            chunk_delay: Duration::from_millis(20),
        }]);
        let root = download_temp_dir("slow");
        let part = root.join("archive.part");

        // Eight 16 KiB chunks at 20 ms each: 160 ms total, far beyond the
        // 50 ms idle timeout, but no single read is ever silent that long.
        let result = download_into(
            &mut transport,
            &part,
            "Slow game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(std::fs::read(&part).unwrap(), payload);
        assert_eq!(*ranges.lock().unwrap(), vec![None], "no reconnect needed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn retries_are_bounded_with_a_clear_failure() {
        let payload = vec![7u8; 128 * 1024];
        let conns = (0..3)
            .map(|_| FakeConn {
                status: 200,
                declared_length: Some(payload.len() as u64),
                range_total: None,
                etag: None,
                sha256: None,
                body: Vec::new(),
                after: After::Lost,
                chunk_delay: Duration::ZERO,
            })
            .collect();
        let (mut transport, ranges) = FakeTransport::new(conns);
        let root = download_temp_dir("retry-bound");
        let part = root.join("archive.part");

        let error = download_into(
            &mut transport,
            &part,
            "Dead server",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 3),
            |_| {},
        )
        .await
        .unwrap_err();

        let Failure::Failed(message) = error else {
            panic!("expected a final failure");
        };
        assert!(message.contains("3 times in a row"), "{message}");
        assert!(message.contains("partial archive was kept"), "{message}");
        assert_eq!(ranges.lock().unwrap().len(), 3, "max attempts is honoured");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn changed_archive_restarts_explicitly() {
        let first: Vec<u8> = vec![1u8; 100 * 1024];
        let second: Vec<u8> = vec![2u8; 120 * 1024];
        let split = 50 * 1024;
        let (mut transport, ranges) = FakeTransport::new(vec![
            FakeConn {
                status: 200,
                declared_length: Some(first.len() as u64),
                range_total: None,
                etag: Some("\"v1\"".to_string()),
                sha256: None,
                body: first[..split].to_vec(),
                after: After::Lost,
                chunk_delay: Duration::ZERO,
            },
            // Same total, different ETag: the archive was replaced in place.
            FakeConn {
                status: 206,
                declared_length: Some((first.len() - split) as u64),
                range_total: Some(first.len() as u64),
                etag: Some("\"v2\"".to_string()),
                sha256: None,
                body: Vec::new(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            },
            FakeConn {
                status: 200,
                declared_length: Some(second.len() as u64),
                range_total: None,
                etag: Some("\"v2\"".to_string()),
                sha256: None,
                body: second.clone(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            },
        ]);

        let root = download_temp_dir("changed");
        let part = root.join("archive.part");
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&updates);

        download_into(
            &mut transport,
            &part,
            "Changed game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 5),
            move |update| sink.lock().unwrap().push(update),
        )
        .await
        .expect("the download restarts from zero and completes");

        assert_eq!(std::fs::read(&part).unwrap(), second);
        assert_eq!(
            *ranges.lock().unwrap(),
            vec![None, Some(split as u64), None],
            "the restart must not reuse the stale offset"
        );
        assert!(
            updates
                .lock()
                .unwrap()
                .iter()
                .any(|update| update.message.contains("archive changed")),
            "the restart must be visible, not silent"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn permanent_http_errors_are_not_retried() {
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 403,
            declared_length: Some(0),
            range_total: None,
            etag: None,
            sha256: None,
            body: Vec::new(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let root = download_temp_dir("forbidden");
        let part = root.join("archive.part");

        let error = download_into(
            &mut transport,
            &part,
            "Forbidden game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 5),
            |_| {},
        )
        .await
        .unwrap_err();

        let Failure::Failed(message) = error else {
            panic!("expected a final failure");
        };
        assert!(message.contains("403"), "{message}");
        assert_eq!(ranges.lock().unwrap().len(), 1, "a 403 is final");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn twenty_consecutive_failures_then_success() {
        let payload: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();
        let mut conns: Vec<FakeConn> = (0..20)
            .map(|_| FakeConn {
                status: 503,
                declared_length: Some(0),
                range_total: None,
                etag: None,
                sha256: None,
                body: Vec::new(),
                after: After::End,
                chunk_delay: Duration::ZERO,
            })
            .collect();
        conns.push(FakeConn {
            status: 200,
            declared_length: Some(payload.len() as u64),
            range_total: None,
            etag: None,
            sha256: None,
            body: payload.clone(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        });
        let (mut transport, ranges) = FakeTransport::new(conns);
        let root = download_temp_dir("twenty");
        let part = root.join("archive.part");
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&updates);

        download_into(
            &mut transport,
            &part,
            "Flaky game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 25),
            move |update| sink.lock().unwrap().push(update),
        )
        .await
        .expect("a 20-failure burst must still complete");

        assert_eq!(std::fs::read(&part).unwrap(), payload);
        assert_eq!(
            ranges.lock().unwrap().len(),
            21,
            "twenty retries then the successful connection"
        );
        let last = updates.lock().unwrap().last().cloned();
        let last = last.expect("a completion update");
        assert_eq!(last.attempt, 21, "the job reports the final attempt");
        assert_eq!(last.last_error, None);
        assert_eq!(last.next_retry_at, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_404_stops_at_once_and_keeps_the_part() {
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 404,
            declared_length: Some(0),
            range_total: None,
            etag: None,
            sha256: None,
            body: Vec::new(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let root = download_temp_dir("not-found");
        let part = root.join("archive.part");
        std::fs::write(&part, b"kept").unwrap();

        let error = download_into(
            &mut transport,
            &part,
            "Missing game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            test_tuning(50, 5),
            |_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(error, Failure::Failed(_)));
        assert_eq!(ranges.lock().unwrap().len(), 1, "a 404 is final");
        assert_eq!(std::fs::read(&part).unwrap(), b"kept");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_offline_machine_does_not_open_a_connection_or_burn_an_attempt() {
        let payload = vec![5u8; 32 * 1024];
        let (mut transport, ranges) = FakeTransport::new(vec![FakeConn {
            status: 200,
            declared_length: Some(payload.len() as u64),
            range_total: None,
            etag: None,
            sha256: None,
            body: payload.clone(),
            after: After::End,
            chunk_delay: Duration::ZERO,
        }]);
        let gate = crate::state::NetworkGate::new();
        gate.set_online(false);
        let tuning = DownloadTuning {
            network: Some(&gate),
            ..test_tuning(50, 5)
        };
        let root = download_temp_dir("offline");
        let part = root.join("archive.part");

        let offline = tokio::time::timeout(
            Duration::from_millis(120),
            download_into(
                &mut transport,
                &part,
                "Offline game",
                &CancellationToken::new(),
                &CancellationToken::new(),
                0,
                tuning,
                |_| {},
            ),
        )
        .await;
        assert!(
            offline.is_err(),
            "the download must wait while the OS reports offline"
        );
        assert!(
            ranges.lock().unwrap().is_empty(),
            "no connection may be opened while offline"
        );

        gate.set_online(true);
        download_into(
            &mut transport,
            &part,
            "Offline game",
            &CancellationToken::new(),
            &CancellationToken::new(),
            0,
            tuning,
            |_| {},
        )
        .await
        .expect("the retry starts as soon as the network returns");
        assert_eq!(std::fs::read(&part).unwrap(), payload);
        assert_eq!(*ranges.lock().unwrap(), vec![None]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn throttle_paces_bytes() {
        let mut throttle = Throttle::new(256).unwrap();
        let started = Instant::now();
        for _ in 0..2 {
            if let Some(delay) = throttle.pace(32 * 1024) {
                std::thread::sleep(delay);
            }
        }
        // 64 KiB at 256 KiB/s is ~0.25 s.
        assert!(started.elapsed() >= Duration::from_millis(180));
        assert!(Throttle::new(0).is_none());
    }
}
