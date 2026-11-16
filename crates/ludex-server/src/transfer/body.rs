//! Response-body accounting for archive transfers (#195, #217).

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue};
use axum::response::Response;
use bytes::Bytes;
use futures_util::Stream;

use crate::download_limits::DownloadAdmission;
use crate::models::VersionRow;

use super::range::{content_range_bounds, full_range_end};

/// Attaches the per-stream outcome log (#195), the concurrent-part permit
/// (#217) and the full archive digest (#216) to a download response.
pub(crate) fn log_stream(
    response: Response,
    game_id: i64,
    context: StreamContext,
    version: &VersionRow,
) -> Response {
    let (mut parts, body) = response.into_parts();
    insert_archive_sha256(&mut parts.headers, version);
    let status = parts.status.as_u16();
    let (range_start, range_end) = parts
        .headers
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(content_range_bounds)
        .map_or_else(
            || (0, full_range_end(&parts.headers)),
            |(start, end)| (start, Some(end)),
        );
    let log = TransferLog::new(game_id, version, context, status, range_start, range_end);
    let stream = LoggedBody {
        inner: Body::new(body).into_data_stream(),
        log: Some(log),
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

/// Publishes the full archive SHA-256 on a download or stream response (#216).
///
/// The value is the one stored in `game_versions.content_sha256`; a legacy row
/// without one simply omits the header, and clients fall back to the payload
/// field (also `null`) to decide whether verification is possible.
pub(crate) fn insert_archive_sha256(headers: &mut HeaderMap, version: &VersionRow) {
    let Some(digest) = version.content_sha256.as_deref() else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(digest) {
        headers.insert(
            HeaderName::from_static(crate::content_hash::ARCHIVE_SHA256_HEADER),
            value,
        );
    }
}

/// What identifies one download request in the stream log (#217).
pub(crate) struct StreamContext {
    pub(crate) job_id: Option<String>,
    pub(crate) part_index: Option<u32>,
    pub(crate) admission: DownloadAdmission,
}

/// Per-stream outcome, logged once when the body finishes or is dropped.
///
/// The admission is owned here so the concurrent-part permit is released at
/// exactly the moment the stream ends, not when the handler returns.
struct TransferLog {
    game_id: i64,
    version_id: i64,
    status: u16,
    job_id: Option<String>,
    part_index: Option<u32>,
    range_start: u64,
    range_end: Option<u64>,
    /// Size recorded for the archive, used when the response announced none.
    full_size: u64,
    bytes: u64,
    started: Instant,
    finished: bool,
    _admission: DownloadAdmission,
}

impl TransferLog {
    fn new(
        game_id: i64,
        version: &VersionRow,
        context: StreamContext,
        status: u16,
        range_start: u64,
        range_end: Option<u64>,
    ) -> Self {
        let StreamContext {
            job_id,
            part_index,
            admission,
        } = context;
        Self {
            game_id,
            version_id: version.id,
            status,
            job_id,
            part_index,
            range_start,
            range_end,
            full_size: version.size.max(0) as u64,
            bytes: 0,
            started: Instant::now(),
            finished: false,
            _admission: admission,
        }
    }

    fn finish(&mut self, outcome: &'static str) {
        if self.finished {
            return;
        }
        self.finished = true;
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        if outcome == "complete" {
            tracing::info!(
                game = self.game_id,
                version = self.version_id,
                status = self.status,
                job = ?self.job_id,
                part = ?self.part_index,
                range_start = self.range_start,
                range_end = ?self.range_end,
                bytes = self.bytes,
                elapsed_ms,
                outcome,
                "download stream closed"
            );
        } else {
            tracing::warn!(
                game = self.game_id,
                version = self.version_id,
                status = self.status,
                job = ?self.job_id,
                part = ?self.part_index,
                range_start = self.range_start,
                range_end = ?self.range_end,
                bytes = self.bytes,
                elapsed_ms,
                outcome,
                "download stream closed"
            );
        }
    }
}

impl Drop for TransferLog {
    fn drop(&mut self) {
        let outcome =
            transfer_outcome(self.bytes, self.range_start, self.range_end, self.full_size);
        self.finish(outcome);
    }
}

/// Terminal outcome when the body is dropped without an explicit EOF (#211).
///
/// A response with a known `Content-Length` is never polled again after its
/// last byte: hyper stops asking for frames, so `Poll::Ready(None)` does not
/// arrive and the body is dropped right after a complete transfer. Comparing
/// the counted bytes with the announced window is what distinguishes a finished
/// download from a real client abort; without it, every successful transfer was
/// logged as `client_abort`.
fn transfer_outcome(
    bytes: u64,
    range_start: u64,
    range_end: Option<u64>,
    full_size: u64,
) -> &'static str {
    let expected = match range_end {
        Some(end) => end.checked_sub(range_start).map(|span| span + 1),
        // No `Content-Range` and no `Content-Length`: fall back to the archive
        // size recorded at scan time, when it is known.
        None => Some(full_size).filter(|size| *size > 0),
    };
    match expected {
        Some(expected) if bytes == expected => "complete",
        _ => "client_abort",
    }
}

/// Strong-ish tag from size and modification time, matching the bytes on disk.
pub(crate) fn file_etag(metadata: &std::fs::Metadata) -> Option<String> {
    let modified = metadata.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(format!("\"{:x}-{:x}\"", metadata.len(), since.as_nanos()))
}

/// Body wrapper that accounts bytes and logs the terminal outcome (#195).
struct LoggedBody<S> {
    inner: S,
    log: Option<TransferLog>,
}

impl<S> Stream for LoggedBody<S>
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Unpin,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                // EOF is the one unambiguous end of stream. A body with a
                // `Content-Length` usually stops before this point; `Drop`
                // handles that case by comparing the byte count (#211).
                if let Some(mut log) = this.log.take() {
                    log.finish("complete");
                }
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                if let Some(mut log) = this.log.take() {
                    log.finish("error");
                }
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(Some(Ok(bytes))) => {
                if let Some(log) = this.log.as_mut() {
                    log.bytes = log.bytes.saturating_add(bytes.len() as u64);
                }
                Poll::Ready(Some(Ok(bytes)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::transfer_outcome;

    #[test]
    fn transfer_outcome_separates_a_finished_body_from_an_abort() {
        // A ranged body that delivered `end - start + 1` bytes is complete even
        // though hyper never polls it again for EOF (#211, 7.4).
        assert_eq!(transfer_outcome(100, 100, Some(199), 4096), "complete");
        assert_eq!(transfer_outcome(99, 100, Some(199), 4096), "client_abort");
        // A full 200 response: `range_end` is `content-length - 1`.
        assert_eq!(transfer_outcome(4096, 0, Some(4095), 4096), "complete");
        assert_eq!(transfer_outcome(2048, 0, Some(4095), 4096), "client_abort");
        // No announced length: fall back to the recorded archive size.
        assert_eq!(transfer_outcome(4096, 0, None, 4096), "complete");
        assert_eq!(transfer_outcome(4095, 0, None, 4096), "client_abort");
        // Nothing known at all: never claim a completion.
        assert_eq!(transfer_outcome(4096, 0, None, 0), "client_abort");
    }
}
