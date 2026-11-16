//! Progress reporting for transfers.
//!
//! A download reports through [`ProgressSink`], a one-method trait. The
//! production implementation lives in the install layer ([`crate::install`]):
//! it updates the job row, persists it and emits `install://progress`. The
//! download loop itself never touches Tauri, which is what keeps its tests
//! able to inject a stall or a reset without building an app handle.
//!
//! Closures implement the trait, so a test can pass `|_| {}` and the install
//! code can pass a stateful sink without a second abstraction.

/// Snapshot passed to the download progress callback.
#[derive(Debug, Clone)]
pub(crate) struct DownloadProgress {
    pub(crate) done: u64,
    pub(crate) total: Option<u64>,
    pub(crate) speed_bps: u64,
    pub(crate) message: String,
    /// True while the client waits to reconnect after a stall or a dropped
    /// response (#195). The UI shows the resume point instead of a frozen rate.
    pub(crate) reconnecting: bool,
    /// Connection attempt of this download, starting at 1.
    pub(crate) attempt: u32,
    /// Last transport error, kept until the transfer completes.
    pub(crate) last_error: Option<String>,
    /// How long the connection was silent when the stall was detected.
    pub(crate) stalled_for_ms: Option<u64>,
    /// RFC 3339 time of the next automatic retry (#212).
    pub(crate) next_retry_at: Option<String>,
    /// HTTP status of the last response, `None` before the first one (#212).
    pub(crate) http_status: Option<u16>,
    /// True while the retry waits for the OS to report a network (#212).
    pub(crate) waiting_for_network: bool,
}

impl DownloadProgress {
    pub(crate) fn plain(
        attempt: u32,
        done: u64,
        total: Option<u64>,
        speed_bps: u64,
        message: String,
    ) -> Self {
        Self {
            done,
            total,
            speed_bps,
            message,
            reconnecting: false,
            attempt,
            last_error: None,
            stalled_for_ms: None,
            next_retry_at: None,
            http_status: None,
            waiting_for_network: false,
        }
    }
}

/// Where a transfer reports progress.
///
/// Kept object-free on purpose: the download loop is generic over the sink, so
/// a test closure and the job-backed sink share one code path with no `dyn`.
pub(crate) trait ProgressSink {
    fn progress(&mut self, update: DownloadProgress);
}

impl<F> ProgressSink for F
where
    F: FnMut(DownloadProgress),
{
    fn progress(&mut self, update: DownloadProgress) {
        self(update);
    }
}
