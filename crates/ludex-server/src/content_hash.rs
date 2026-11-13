//! Full archive SHA-256 (#216).
//!
//! `archive_hash` is the cheap sampled fingerprint used for update detection.
//! This module produces the strong hash: every byte of the archive, hashed by a
//! background job (never in a request path), streamed from object storage when
//! the archive lives in S3, and backfilled for legacy rows.
//!
//! The result lands in `game_versions.content_sha256`, is exposed as
//! `contentSha256` in the version payloads and as the
//! `X-Ludex-Archive-Sha256` header on download/stream responses. Clients
//! verify it before extraction, so a truncated body can never be installed as a
//! successful game.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::db;
use crate::state::AppState;

/// Response header carrying the full archive SHA-256 (lowercase hex).
pub const ARCHIVE_SHA256_HEADER: &str = "x-ludex-archive-sha256";

/// Rows hashed between progress updates.
const PROGRESS_EVERY: i64 = 5;

/// Rows a library scan backfills per pass: enough to catch up on a legacy
/// library over a few scans without turning every scan into a full read pass
/// (the admin endpoint can hash everything on demand).
pub const SCAN_LIMIT: i64 = 50;

/// Rows pulled from the database per batch.
const BATCH_ROWS: i64 = 64;

/// Read buffer for streaming hashes (1 MiB: SHA-256 is CPU-bound, large reads
/// keep the syscall overhead low on a 40 GB archive).
const HASH_BUFFER_BYTES: usize = 1024 * 1024;

/// Status of the content-hash background job, shown by the admin API.
#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContentHashStatus {
    /// A job is hashing rows right now.
    pub running: bool,
    /// Rows still missing a full hash (live count at the last refresh).
    pub pending: i64,
    /// Rows hashed by the current or last run.
    pub hashed: i64,
    /// Rows that could not be read or stored.
    pub failed: i64,
    /// Rows targeted by the current or last run.
    pub total: i64,
    /// Start timestamp of the current or last run.
    pub started_at: Option<String>,
    /// End timestamp of the last finished run.
    pub finished_at: Option<String>,
    /// Last failure, for the admin screen.
    pub last_error: Option<String>,
    /// Rows processed per second during the last run, when measurable.
    pub rows_per_second: Option<f64>,
}

/// Single-flight guard plus the last status snapshot.
#[derive(Debug, Default)]
pub struct ContentHasher {
    status: Mutex<ContentHashStatus>,
    busy: AtomicBool,
}

impl ContentHasher {
    /// Snapshot of the current status.
    #[must_use]
    pub fn status(&self) -> ContentHashStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    fn mark_started(&self, total: i64, pending: i64) {
        if let Ok(mut status) = self.status.lock() {
            *status = ContentHashStatus {
                running: true,
                pending,
                total,
                started_at: Some(db::db_time(db::now())),
                ..ContentHashStatus::default()
            };
        }
    }

    fn record(&self, hashed: bool, error: Option<&str>, pending: i64) {
        if let Ok(mut status) = self.status.lock() {
            if hashed {
                status.hashed += 1;
            } else {
                status.failed += 1;
            }
            status.pending = pending;
            if let Some(error) = error {
                status.last_error = Some(error.to_string());
            }
        }
    }

    fn finish(&self, elapsed: std::time::Duration, pending: i64) {
        if let Ok(mut status) = self.status.lock() {
            status.running = false;
            status.pending = pending;
            status.finished_at = Some(db::db_time(db::now()));
            let seconds = elapsed.as_secs_f64();
            if seconds > 0.0 {
                status.rows_per_second = Some(status.hashed as f64 / seconds);
            }
        }
    }
}

/// Counts versions still waiting for a full hash.
pub async fn pending_count(state: &AppState) -> i64 {
    db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM game_versions
         WHERE content_sha256 IS NULL AND missing = 0 AND root_unavailable = 0",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0)
}

/// Queues a background hash run when one is not already active.
///
/// `limit` bounds the rows hashed by this run (`0` means every pending row).
/// Returns `false` when a run was already in flight; the caller never waits.
#[must_use]
pub fn schedule(state: &AppState, limit: i64) -> bool {
    if state
        .content_hashes
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    let state = state.clone();
    tokio::spawn(async move {
        run(&state, limit).await;
        state.content_hashes.busy.store(false, Ordering::SeqCst);
    });
    true
}

/// Runs one backfill pass. Public so tests can exercise it without the spawner.
pub async fn run(state: &AppState, limit: i64) {
    let pending = pending_count(state).await;
    let total = if limit > 0 {
        pending.min(limit)
    } else {
        pending
    };
    state.content_hashes.mark_started(total, pending);
    let started = Instant::now();
    let mut hashed: i64 = 0;
    let mut processed: i64 = 0;

    if total == 0 {
        state.content_hashes.finish(started.elapsed(), 0);
        return;
    }

    'batches: loop {
        if processed >= total || (limit > 0 && processed >= limit) {
            break;
        }
        let rows = match pending_rows(state, BATCH_ROWS).await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(error = %error, "could not list versions to hash");
                state
                    .content_hashes
                    .record(false, Some(&error.to_string()), pending);
                break;
            }
        };
        if rows.is_empty() {
            break;
        }
        for row in rows {
            if processed >= total || (limit > 0 && processed >= limit) {
                break 'batches;
            }
            processed += 1;
            match hash_version(state, &row).await {
                Ok(Some(digest)) => {
                    match db::query(
                        "UPDATE game_versions SET content_sha256 = ?,
                            updated_at = COALESCE(updated_at, indexed_at)
                         WHERE id = ?",
                    )
                    .bind(&digest)
                    .bind(row.id)
                    .execute(&state.pool)
                    .await
                    {
                        Ok(_) => {
                            hashed += 1;
                            state
                                .content_hashes
                                .record(true, None, (pending - processed).max(0));
                        }
                        Err(error) => {
                            tracing::warn!(version = row.id, %error, "could not store the archive hash");
                            state
                                .content_hashes
                                .record(false, Some(&error.to_string()), pending);
                        }
                    }
                }
                Ok(None) => {
                    // The file is gone or unreadable: leave the row for the
                    // next run instead of cleaning content_sha256 (a scan will
                    // mark it missing).
                    state.content_hashes.record(false, None, pending);
                }
                Err(error) => {
                    tracing::warn!(version = row.id, %error, "could not hash archive");
                    state
                        .content_hashes
                        .record(false, Some(&error.to_string()), pending);
                }
            }
            if hashed > 0 && hashed % PROGRESS_EVERY == 0 {
                tracing::debug!(hashed, pending, "archive hash backfill progress");
            }
        }
    }

    let remaining = pending_count(state).await;
    state.content_hashes.finish(started.elapsed(), remaining);
    tracing::info!(
        hashed,
        failed = state.content_hashes.status().failed,
        remaining,
        elapsed_ms = started.elapsed().as_millis(),
        "archive content hash backfill finished"
    );
}

/// One version row awaiting a hash.
#[derive(Debug, Clone)]
pub struct PendingRow {
    /// Version id.
    pub id: i64,
    /// Library root id, when the row is local.
    pub root_id: Option<i64>,
    /// Path relative to the library root.
    pub file_path: String,
    /// Size recorded by the scanner.
    pub size: i64,
    /// `local` or `s3`.
    pub storage_backend: Option<String>,
    /// Object key when stored remotely.
    pub object_key: Option<String>,
}

async fn pending_rows(state: &AppState, limit: i64) -> anyhow::Result<Vec<PendingRow>> {
    let rows = db::query_as::<(
        i64,
        Option<i64>,
        String,
        i64,
        Option<String>,
        Option<String>,
    )>(
        "SELECT id, root_id, file_path, size, storage_backend, object_key
         FROM game_versions
         WHERE content_sha256 IS NULL AND missing = 0 AND root_unavailable = 0
         ORDER BY id LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, root_id, file_path, size, storage_backend, object_key)| PendingRow {
                id,
                root_id,
                file_path,
                size,
                storage_backend,
                object_key,
            },
        )
        .collect())
}

/// Hashes one version: streams the S3 object when that is where the archive
/// lives, otherwise reads the local library file (which is also the fallback
/// when an object read fails).
async fn hash_version(state: &AppState, row: &PendingRow) -> anyhow::Result<Option<String>> {
    if row.storage_backend.as_deref() == Some("s3") {
        if let Some(key) = row.object_key.as_deref() {
            match state.storage.hash_object(key).await {
                Ok((size, digest)) => {
                    if row.size > 0 && size != u64::try_from(row.size).unwrap_or(0) {
                        tracing::warn!(
                            version = row.id,
                            expected = row.size,
                            object = size,
                            "object size differs from the indexed archive; hashing the object anyway"
                        );
                    }
                    return Ok(Some(digest));
                }
                Err(error) => {
                    tracing::warn!(
                        version = row.id,
                        %error,
                        "object storage hash failed; trying the local library file"
                    );
                }
            }
        }
    }
    let Some(path) = local_path(state, row).await else {
        return Ok(None);
    };
    let relative = row.file_path.clone();
    let result = tokio::task::spawn_blocking(move || hash_file(&path)).await;
    match result {
        Ok(Ok((_, digest))) => Ok(Some(digest)),
        Ok(Err(error)) => {
            tracing::debug!(path = %relative, %error, "could not hash archive");
            Ok(None)
        }
        Err(error) => {
            tracing::debug!(path = %relative, %error, "hash task failed");
            Ok(None)
        }
    }
}

async fn local_path(state: &AppState, row: &PendingRow) -> Option<PathBuf> {
    if let Some(root_id) = row.root_id {
        if let Some(path) = crate::roots::resolve_version_path(
            &state.pool,
            &state.config,
            Some(root_id),
            &row.file_path,
        )
        .await
        {
            return Some(path);
        }
    }
    // Root-less rows (older uploads) still live under the games directory.
    let candidate = state.config.games_dir.join(&row.file_path);
    tokio::fs::try_exists(&candidate)
        .await
        .ok()
        .filter(|exists| *exists)
        .map(|_| candidate)
}

/// Hashes a local file with a 1 MiB read buffer.
pub fn hash_file(path: &std::path::Path) -> std::io::Result<(u64, String)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

/// Hashes an async reader end to end. Object storage is adapted to this
/// interface (a stream reader), so the S3 path and the tests share one
/// implementation.
pub async fn hash_reader<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reader_hash_matches_the_known_digest() {
        // The same digest as `scanner::hash_file` and the desktop verifier.
        let (size, digest) = hash_reader(&b"ludex"[..]).await.expect("hash");
        assert_eq!(size, 5);
        assert_eq!(
            digest,
            "44e01fcd380808bc2738f7bfa8bd7b5ef9f387536083fa67215a3f171c9b702e"
        );
    }

    #[tokio::test]
    async fn injected_reader_is_streamed_in_chunks() {
        // A reader that yields one byte per read proves the hash covers every
        // byte across many polls (the S3 stream has the same shape).
        struct Trickle<'a> {
            bytes: &'a [u8],
            position: usize,
        }
        impl AsyncRead for Trickle<'_> {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                let this = self.get_mut();
                if this.position < this.bytes.len() {
                    let byte = this.bytes[this.position];
                    this.position += 1;
                    buf.put_slice(&[byte]);
                }
                std::task::Poll::Ready(Ok(()))
            }
        }
        let payload: Vec<u8> = (0..4096).map(|index| (index % 251) as u8).collect();
        let (size, digest) = hash_reader(Trickle {
            bytes: &payload,
            position: 0,
        })
        .await
        .expect("hash");
        assert_eq!(size, payload.len() as u64);
        let expected = hex::encode(Sha256::digest(&payload));
        assert_eq!(digest, expected);
    }
}
