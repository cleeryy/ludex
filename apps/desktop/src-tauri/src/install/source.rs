//! Where an install gets its bytes: the local library shortcut, the ZIP stream
//! and the policy that chooses between them.
//!
//! The classic resumable transfer lives in [`super::download`]; this module
//! decides whether it runs at all. The per-entry stream (#115) writes straight
//! into staging and is retried as a whole job against its checkpoint (#212),
//! hence the [`StreamAttempt`] abstraction: the long-horizon retry can be
//! tested without a server or an `AppHandle`.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::net::{CancelSet, RetryPolicy, RetryState};
use crate::settings::{LocalSettings, Mode};
use crate::state::{AppState, JobEntry};
use crate::util::non_empty;

use super::job::{Failure, InstallParams, JobPaths};
pub(crate) async fn local_archive_path(
    state: &AppState,
    params: &InstallParams,
) -> Option<PathBuf> {
    let base = params.server_url.trim_end_matches('/');
    if base.is_empty() || !is_local_server(state, base) {
        return None;
    }

    #[derive(Deserialize)]
    struct LocalPath {
        path: String,
    }

    let url = format!(
        "{base}/api/games/{}/versions/{}/local-path",
        params.game_id, params.version_id
    );
    let mut request = state.http.get(url).timeout(Duration::from_secs(15));
    if let Some(token) = non_empty(params.token.as_deref()) {
        request = request.bearer_auth(token);
    }

    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload: LocalPath = response.json().await.ok()?;
    let path = PathBuf::from(payload.path);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

pub(super) fn is_local_server(state: &AppState, base: &str) -> bool {
    let settings = state.settings_snapshot();
    if settings.mode != Mode::Local {
        return false;
    }
    let known = settings.server_url.trim_end_matches('/');
    if !known.is_empty() && known == base {
        return true;
    }
    is_loopback_url(base)
}

fn is_loopback_url(url: &str) -> bool {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// One whole per-entry stream attempt, abstracted so the long-horizon retry
/// policy can be tested without an `AppHandle` (#212).
#[allow(async_fn_in_trait)]
pub(crate) trait StreamAttempt {
    async fn run(&mut self) -> Result<StreamOutcome, Failure>;
}

/// Retry a whole stream attempt with the classic long-horizon policy (#212).
///
/// After the per-entry attempts are exhausted, the stream job itself backs off
/// and runs again against the same checkpoint instead of failing: completed
/// entries and `.part` files are adopted by `stream_install::install`. A
/// structurally unusable stream ([`Failure::StreamUnavailable`]) still returns
/// at once so the caller can fall back to classic; permanent errors stop too.
pub(super) async fn stream_with_retry<A: StreamAttempt>(
    attempt: &mut A,
    tuning: RetryPolicy<'_>,
    cancel: &CancellationToken,
    pause: &CancellationToken,
    mut on_retry: impl FnMut(&str, u32, Duration),
) -> Result<StreamOutcome, Failure> {
    let mut retry = RetryState::new();
    let stops = CancelSet::new(cancel, pause);
    loop {
        if let Some(network) = tuning.network {
            if !network.is_online() {
                stops.wait_online(network).await?;
            }
        }
        retry.next_attempt();
        match attempt.run().await {
            Ok(outcome) => return Ok(outcome),
            Err(Failure::Cancelled) => return Err(Failure::Cancelled),
            Err(Failure::Paused) => return Err(Failure::Paused),
            Err(Failure::StreamUnavailable(message)) => {
                return Err(Failure::StreamUnavailable(message))
            }
            Err(Failure::Failed(message)) => {
                if crate::stream_install::is_permanent_stream_failure(&message) {
                    return Err(Failure::Failed(message));
                }
                if let Some(reason) = retry.exhausted(tuning) {
                    return Err(Failure::Failed(format!(
                        "The stream install kept failing ({reason}). Last error: {message}. The files already downloaded were kept; retry the install to resume."
                    )));
                }
                let delay = tuning.delay(retry.attempt());
                on_retry(&message, retry.next_attempt_number(), delay);
                tracing::warn!(
                    attempt = retry.attempt(),
                    delay_ms = delay.as_millis() as u64,
                    error = %message,
                    "stream install failed; retrying the same job from its checkpoint"
                );
                if !stops.sleep(delay, tuning.retry_now).await? {
                    return Err(Failure::Cancelled);
                }
            }
        }
    }
}

/// Production [`StreamAttempt`] over the real per-entry installer.
pub(super) struct InstallStreamAttempt<'a> {
    pub(super) app: &'a AppHandle,
    pub(super) entry: &'a Arc<JobEntry>,
    pub(super) params: &'a InstallParams,
    pub(super) staging: &'a Path,
    pub(super) job_paths: &'a mut JobPaths,
}

impl StreamAttempt for InstallStreamAttempt<'_> {
    async fn run(&mut self) -> Result<StreamOutcome, Failure> {
        crate::stream_install::install(
            self.app,
            self.entry,
            self.params,
            self.staging,
            self.job_paths,
        )
        .await
    }
}

/// Honest copy when a per-entry stream is refused and the job falls back (#226).
///
/// The stream protocol only understands ZIP archives; when the server published
/// the archive format, the message names it instead of a generic "not
/// available". A `zip` answer (an older server without the per-entry routes)
/// keeps the format out of the sentence, because it would read as a
/// contradiction.
pub(super) fn stream_fallback_message(format: Option<&str>) -> String {
    let format = format
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| value != "zip");
    match format.as_deref() {
        Some(format) => format!(
            "This archive is a {format} file; streaming is ZIP-only, so Ludex downloads the archive and extracts it instead."
        ),
        None => {
            "Streaming is ZIP-only; Ludex downloads this archive and extracts it instead."
                .to_string()
        }
    }
}

/// Should this install use the ZIP tar stream instead of download+extract?
///
/// The per-call `stream` flag wins over `settings.streamInstall`. Loopback
/// servers never stream: the local-path shortcut is cheaper and lossless.
pub(super) fn stream_requested(
    requested: Option<bool>,
    settings: &LocalSettings,
    server_url: &str,
) -> bool {
    // #235: the policy decides whether the stream is eligible at all; an
    // explicit per-job override ("Restart stream" / "Retry as download") still
    // wins for troubleshooting.
    let enabled = match requested {
        Some(value) => value,
        None => settings.stream_install && settings.download_policy.wants_stream(),
    };
    enabled && !server_url.trim().is_empty() && !is_loopback_url(server_url)
}

#[derive(Debug)]
pub(crate) enum StreamOutcome {
    /// Every entry is on disk; carries the digests collected while writing
    /// them, so the caller can build the manifest without a second pass (#240).
    Done { files: Vec<StreamFileHash> },
    /// The server cannot stream this version (non-ZIP, or a server build
    /// without the per-entry endpoints). Fall back to the next strategy.
    Unsupported(reqwest::StatusCode),
}

/// One streamed file with its digest (#240).
#[derive(Debug)]
pub(crate) struct StreamFileHash {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// Convert a tar entry path to a safe relative path. Rejects absolute paths,
/// `..`, Windows drive/separator tricks and empty paths.
pub(crate) fn safe_tar_path(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str()?;
                if text.is_empty() || text.contains('\\') || text.contains(':') {
                    return None;
                }
                result.push(part);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    if result.as_os_str().is_empty() {
        None
    } else {
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::DownloadTuning;
    use std::time::Instant;

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_url("http://127.0.0.1:4321"));
        assert!(is_loopback_url("http://localhost:1234/api"));
        assert!(is_loopback_url("http://[::1]:9000"));
        assert!(!is_loopback_url("https://ludex.example.com"));
        assert!(!is_loopback_url("http://192.168.1.10:8080"));
    }

    #[test]
    fn stream_flag_precedence_and_loopback_fallback() {
        let mut settings = LocalSettings::for_app_data(Path::new("/data"));
        settings.stream_install = false;
        assert!(!stream_requested(
            None,
            &settings,
            "https://ludex.example.com"
        ));
        assert!(stream_requested(
            Some(true),
            &settings,
            "https://ludex.example.com"
        ));
        assert!(!stream_requested(
            Some(false),
            &settings,
            "https://ludex.example.com"
        ));

        settings.stream_install = true;
        assert!(stream_requested(
            None,
            &settings,
            "https://ludex.example.com"
        ));
        assert!(!stream_requested(None, &settings, "http://127.0.0.1:4123"));
        assert!(!stream_requested(
            Some(true),
            &settings,
            "http://[::1]:8080"
        ));
        assert!(!stream_requested(Some(true), &settings, ""));

        // #235: the download policy gates the stream when the job has no
        // override. `classic` never streams, `automatic` and `stream` do while
        // the stream-install switch is on.
        settings.download_policy = crate::settings::DownloadPolicy::Classic;
        assert!(!stream_requested(
            None,
            &settings,
            "https://ludex.example.com"
        ));
        // An explicit "Restart stream" still works for troubleshooting.
        assert!(stream_requested(
            Some(true),
            &settings,
            "https://ludex.example.com"
        ));
        settings.download_policy = crate::settings::DownloadPolicy::Stream;
        assert!(stream_requested(
            None,
            &settings,
            "https://ludex.example.com"
        ));
        settings.download_policy = crate::settings::DownloadPolicy::Automatic;
        settings.stream_install = false;
        assert!(!stream_requested(
            None,
            &settings,
            "https://ludex.example.com"
        ));
    }

    #[test]
    fn tar_paths_are_guarded() {
        assert_eq!(
            safe_tar_path(Path::new("bin/game.sh")),
            Some(PathBuf::from("bin/game.sh"))
        );
        assert_eq!(
            safe_tar_path(Path::new("./bin/./game.sh")),
            Some(PathBuf::from("bin/game.sh"))
        );
        assert!(safe_tar_path(Path::new("../escape")).is_none());
        assert!(safe_tar_path(Path::new("dir/../../escape")).is_none());
        assert!(safe_tar_path(Path::new("/absolute")).is_none());
        assert!(safe_tar_path(Path::new("C:escape")).is_none());
        assert!(safe_tar_path(Path::new("")).is_none());
    }

    struct ScriptedStreamAttempt {
        results: Vec<Result<StreamOutcome, Failure>>,
        runs: usize,
    }

    impl StreamAttempt for ScriptedStreamAttempt {
        async fn run(&mut self) -> Result<StreamOutcome, Failure> {
            self.runs += 1;
            if self.results.is_empty() {
                return Ok(StreamOutcome::Done { files: Vec::new() });
            }
            self.results.remove(0)
        }
    }

    fn stream_retry_tuning() -> DownloadTuning<'static> {
        DownloadTuning {
            base_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            max_retry_window: Some(Duration::from_secs(30)),
            max_attempts: 5,
            ..DownloadTuning::default()
        }
    }

    #[tokio::test]
    async fn an_exhausted_stream_entry_retries_the_job_from_its_checkpoint() {
        let mut attempt = ScriptedStreamAttempt {
            results: vec![
                Err(Failure::Failed(
                    "Entry Game/big.bin failed after 5 attempts (1048576 bytes kept): proxy returned HTTP 524"
                        .to_string(),
                )),
                Ok(StreamOutcome::Done { files: Vec::new() }),
            ],
            runs: 0,
        };
        let retries = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&retries);
        let outcome = stream_with_retry(
            &mut attempt,
            stream_retry_tuning(),
            &CancellationToken::new(),
            &CancellationToken::new(),
            move |message, next_attempt, delay| {
                sink.lock()
                    .unwrap()
                    .push((message.to_string(), next_attempt, delay));
            },
        )
        .await;

        assert!(
            matches!(outcome, Ok(StreamOutcome::Done { .. })),
            "{outcome:?}"
        );
        assert_eq!(attempt.runs, 2, "the job runs again instead of failing");
        let retries = retries.lock().unwrap();
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0].1, 2, "the retry is visible as attempt 2");
        assert!(retries[0].0.contains("524"), "{}", retries[0].0);
    }

    #[tokio::test]
    async fn a_permanent_stream_error_is_not_retried() {
        let mut attempt = ScriptedStreamAttempt {
            results: vec![Err(Failure::Failed(
                "Entry Game/big.bin failed with HTTP 404. The game version or entry was not found on the server."
                    .to_string(),
            ))],
            runs: 0,
        };
        let outcome = stream_with_retry(
            &mut attempt,
            stream_retry_tuning(),
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _, _| panic!("a 404 must not be retried"),
        )
        .await;
        assert!(matches!(outcome, Err(Failure::Failed(_))));
        assert_eq!(attempt.runs, 1);
    }

    #[tokio::test]
    async fn an_unusable_stream_path_returns_for_the_classic_fallback() {
        let mut attempt = ScriptedStreamAttempt {
            results: vec![Err(Failure::StreamUnavailable(
                "entry game.exe could not be reached after 5 attempts: HTTP 524".to_string(),
            ))],
            runs: 0,
        };
        let outcome = stream_with_retry(
            &mut attempt,
            stream_retry_tuning(),
            &CancellationToken::new(),
            &CancellationToken::new(),
            |_, _, _| panic!("an unusable stream path falls back instead of retrying"),
        )
        .await;
        assert!(matches!(outcome, Err(Failure::StreamUnavailable(_))));
        assert_eq!(attempt.runs, 1);
    }

    #[tokio::test]
    async fn cancel_stops_a_stream_retry_immediately() {
        let mut attempt = ScriptedStreamAttempt {
            results: vec![Err(Failure::Failed("proxy returned 502".to_string()))],
            runs: 0,
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let started = Instant::now();
        let outcome = stream_with_retry(
            &mut attempt,
            DownloadTuning {
                base_backoff: Duration::from_secs(30),
                ..stream_retry_tuning()
            },
            &cancel,
            &CancellationToken::new(),
            |_, _, _| {},
        )
        .await;
        assert!(matches!(outcome, Err(Failure::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(attempt.runs, 1);
    }

    #[test]
    fn stream_fallback_names_the_format_and_the_zip_limit() {
        let seven = stream_fallback_message(Some(" 7Z "));
        assert!(seven.contains("7z"), "{seven}");
        assert!(seven.contains("ZIP-only"), "{seven}");
        assert!(seven.contains("downloads the archive"), "{seven}");

        // Without a published format the message still explains the limit.
        let unknown = stream_fallback_message(None);
        assert!(unknown.contains("ZIP-only"), "{unknown}");
        assert!(!unknown.contains("This archive is a"), "{unknown}");

        // A 415 from an older server on a ZIP build must not contradict itself.
        let zip = stream_fallback_message(Some("zip"));
        assert!(!zip.contains("This archive is a"), "{zip}");
        assert!(zip.contains("ZIP-only"), "{zip}");
    }
}
