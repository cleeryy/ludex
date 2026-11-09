//! Tests for the ingest manager. Every test uses a loopback axum server; no
//! external network access is required.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State as AxumState;
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use futures_util::{stream, StreamExt};
use http_body_util::BodyExt;
use serde_json::json;
use tokio::sync::mpsc;
use tower::ServiceExt;

use super::manager::{self, state_for_tests};
use super::types::{
    infer_title, is_private_ip, sanitize_title, IngestJob, Timings, MAX_TITLE_CHARS,
};
use super::{router, IngestContext, IngestStatus};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct TestEnv {
    root: tempfile::TempDir,
    games: PathBuf,
    data: PathBuf,
    ctx: IngestContext,
    scan_rx: mpsc::Receiver<()>,
}

impl TestEnv {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let games = root.path().join("games");
        let data = root.path().join("data");
        std::fs::create_dir_all(&games).expect("games dir");
        std::fs::create_dir_all(&data).expect("data dir");
        let (scan_tx, scan_rx) = mpsc::channel(16);
        let ctx = IngestContext {
            games_dir: games.clone(),
            data_dir: data.clone(),
            scan_tx,
            allow_private: true,
        };
        Self {
            root,
            games,
            data,
            ctx,
            scan_rx,
        }
    }

    fn router(&self) -> Router {
        let _ = state_for_tests(&self.ctx, Timings::fast());
        router(self.ctx.clone())
    }
}

struct TestServer {
    addr: SocketAddr,
    ranges: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

#[derive(Clone)]
struct FileServerState {
    data: Arc<Vec<u8>>,
    ranges: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    cut_first_at: Option<usize>,
}

async fn spawn_file_server(data: Vec<u8>, cut_first_at: Option<usize>) -> TestServer {
    let state = FileServerState {
        data: Arc::new(data),
        ranges: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(AtomicUsize::new(0)),
        cut_first_at,
    };
    let ranges = Arc::clone(&state.ranges);
    let requests = Arc::clone(&state.requests);
    let app = Router::new()
        .route("/file", get(file_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("test server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestServer {
        addr,
        ranges,
        requests,
    }
}

async fn file_handler(
    AxumState(state): AxumState<FileServerState>,
    headers: HeaderMap,
) -> Response {
    let index = state.requests.fetch_add(1, Ordering::SeqCst);
    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if range_header.is_some() {
        state.ranges.fetch_add(1, Ordering::SeqCst);
    }
    let total = state.data.len();

    if let Some(range) = range_header {
        return match parse_range(&range, total) {
            Some((start, end)) => {
                partial_response(state.data[start..end].to_vec(), start, end, total)
            }
            None => {
                let mut response = Response::new(Body::empty());
                *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                response.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{total}"))
                        .expect("content-range header"),
                );
                response
            }
        };
    }

    if let Some(cut) = state.cut_first_at {
        if index == 0 {
            let end = cut.min(total);
            return partial_response(state.data[..end].to_vec(), 0, end, total);
        }
    }

    let mut response = Response::new(Body::from(state.data.as_ref().clone()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response
}

fn partial_response(body: Vec<u8>, start: usize, end: usize, total: usize) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::PARTIAL_CONTENT;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response.headers_mut().insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{}/{total}", end.saturating_sub(1)))
            .expect("content-range header"),
    );
    response
}

fn parse_range(value: &str, len: usize) -> Option<(usize, usize)> {
    let rest = value.trim().strip_prefix("bytes=")?;
    let (start, end) = rest.split_once('-')?;
    let start: usize = start.trim().parse().ok()?;
    let end = end.trim();
    let end = if end.is_empty() {
        len
    } else {
        end.parse::<usize>().ok()?.saturating_add(1).min(len)
    };
    if start >= len || start >= end {
        return None;
    }
    Some((start, end))
}

async fn spawn_slow_server() -> TestServer {
    let app = Router::new().route("/slow", get(slow_handler));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind slow server");
    let addr = listener.local_addr().expect("slow server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestServer {
        addr,
        ranges: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(AtomicUsize::new(0)),
    }
}

async fn slow_handler() -> Response {
    let first = vec![7u8; 64 * 1024];
    let tail = stream::pending::<Result<Vec<u8>, std::io::Error>>();
    let body =
        Body::from_stream(stream::once(async move { Ok::<_, std::io::Error>(first) }).chain(tail));
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(64 * 1024 * 64u64));
    response
}

fn fake_zip(len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(len);
    data.extend_from_slice(b"PK\x03\x04");
    let mut value = 0x9e37_79b9u32;
    while data.len() < len {
        value = value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.truncate(len);
    data
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn http(app: &Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = app.clone().oneshot(request).await.expect("router call");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn post_json(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

async fn create_job(app: &Router, url: &str, title: &str) -> IngestJob {
    let (status, body) = http(
        app,
        post_json(
            "/api/ingest/downloads",
            json!({ "url": url, "title": title }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    serde_json::from_slice(&body).expect("created job json")
}

async fn fetch_job(app: &Router, id: &str) -> IngestJob {
    let request = Request::builder()
        .uri(format!("/api/ingest/downloads/{id}"))
        .body(Body::empty())
        .expect("request");
    let (status, body) = http(app, request).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).expect("job json")
}

async fn wait_for_status(app: &Router, id: &str, want: IngestStatus) -> IngestJob {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let job = fetch_job(app, id).await;
        if job.status == want {
            return job;
        }
        assert!(
            !matches!(
                job.status,
                IngestStatus::Done | IngestStatus::Error | IngestStatus::Cancelled
            ),
            "job reached {:?} while waiting for {:?} ({:?})",
            job.status,
            want,
            job.error
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {:?}, last status {:?} ({:?})",
            want,
            job.status,
            job.error
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn download_completes_and_detects_format() {
    let mut env = TestEnv::new();
    let payload = fake_zip(96 * 1024);
    let server = spawn_file_server(payload.clone(), None).await;
    let app = env.router();
    std::fs::write(env.games.join("My Game.zip"), b"older").expect("seed collision");

    let job = create_job(&app, &server.url("/file"), "My Game").await;
    assert_eq!(job.status, IngestStatus::Queued);
    assert_eq!(job.title, "My Game");

    let done = wait_for_status(&app, &job.id, IngestStatus::Done).await;
    assert_eq!(done.file_name.as_deref(), Some("My Game (2).zip"));
    assert_eq!(done.downloaded_bytes, payload.len() as u64);
    assert_eq!(
        std::fs::read(env.games.join("My Game (2).zip")).expect("finished file"),
        payload
    );
    assert!(
        env.scan_rx.try_recv().is_ok(),
        "a library scan should be triggered"
    );
}

#[tokio::test]
async fn cancel_deletes_the_partial_file() {
    let env = TestEnv::new();
    let server = spawn_slow_server().await;
    let app = env.router();
    let job = create_job(&app, &server.url("/slow"), "Slow Game").await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let current = fetch_job(&app, &job.id).await;
        if current.downloaded_bytes > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no download progress before timeout"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/api/ingest/downloads/{}", job.id))
        .body(Body::empty())
        .expect("request");
    let (status, _) = http(&app, request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let cancelled = fetch_job(&app, &job.id).await;
    assert_eq!(cancelled.status, IngestStatus::Cancelled);
    assert!(cancelled.file_name.is_none());

    let leftovers: Vec<String> = std::fs::read_dir(&env.games)
        .expect("games dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        leftovers.iter().all(|name| !name.ends_with(".part")),
        "partial file left behind: {leftovers:?}"
    );
}

#[tokio::test]
async fn retry_resumes_with_http_range() {
    let env = TestEnv::new();
    let payload = fake_zip(160 * 1024);
    let cut = 64 * 1024;
    let server = spawn_file_server(payload.clone(), Some(cut)).await;
    let app = env.router();
    let job = create_job(&app, &server.url("/file"), "Resume Game").await;

    let done = wait_for_status(&app, &job.id, IngestStatus::Done).await;
    assert_eq!(done.file_name.as_deref(), Some("Resume Game.zip"));
    assert_eq!(
        std::fs::read(env.games.join("Resume Game.zip")).expect("finished file"),
        payload
    );
    assert_eq!(
        server.ranges.load(Ordering::SeqCst),
        1,
        "the retry must use a Range request"
    );
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        2,
        "expected a cut attempt followed by a resume"
    );
}

#[tokio::test]
async fn delete_file_refuses_traversal() {
    let env = TestEnv::new();
    let app = env.router();
    std::fs::write(env.root.path().join("secret.txt"), b"secret").expect("secret file");

    for name in ["..%2Fsecret.txt", "..%5Csecret.txt", "..", "."] {
        let request = Request::builder()
            .method("DELETE")
            .uri(format!("/api/ingest/files/{name}"))
            .body(Body::empty())
            .expect("request");
        let (status, body) = http(&app, request).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "name {name} was not rejected"
        );
        let envelope: serde_json::Value = serde_json::from_slice(&body).expect("error json");
        assert_eq!(envelope["error"]["code"], "invalid_file_name");
    }
    assert!(env.root.path().join("secret.txt").exists());

    std::fs::write(env.games.join("Game.zip"), b"zip").expect("game file");
    let request = Request::builder()
        .method("DELETE")
        .uri("/api/ingest/files/Game.zip")
        .body(Body::empty())
        .expect("request");
    let (status, _) = http(&app, request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn boot_resumes_an_interrupted_download() {
    let env = TestEnv::new();
    let payload = fake_zip(128 * 1024);
    let server = spawn_file_server(payload.clone(), None).await;
    let half = payload.len() / 2;
    std::fs::write(env.games.join(".Pending Game.part"), &payload[..half]).expect("partial file");

    let now = Utc::now();
    let job = IngestJob {
        id: "boot-job".to_string(),
        title: "Pending Game".to_string(),
        url: Some(server.url("/file")),
        status: IngestStatus::Downloading,
        file_name: None,
        downloaded_bytes: 0,
        total_bytes: None,
        speed_bps: 0,
        attempt: 4,
        error: Some("interrupted".to_string()),
        created_at: now,
        updated_at: now,
    };
    std::fs::write(
        env.data.join("ingest-jobs.json"),
        serde_json::to_vec(&vec![job]).expect("jobs json"),
    )
    .expect("jobs file");
    std::fs::write(
        env.data.join("ingest-parts.json"),
        serde_json::to_vec(&json!({ "boot-job": ".Pending Game.part" })).expect("parts json"),
    )
    .expect("parts file");

    let _ = state_for_tests(&env.ctx, Timings::fast());
    super::resume_pending(env.ctx.clone()).await;
    let app = router(env.ctx.clone());

    let done = wait_for_status(&app, "boot-job", IngestStatus::Done).await;
    assert_eq!(done.file_name.as_deref(), Some("Pending Game.zip"));
    assert_eq!(
        std::fs::read(env.games.join("Pending Game.zip")).expect("finished file"),
        payload
    );
    assert_eq!(
        server.ranges.load(Ordering::SeqCst),
        1,
        "the restart must resume the partial file"
    );
}

#[tokio::test]
async fn files_and_stats_report_the_library() {
    let env = TestEnv::new();
    let app = env.router();
    let archive = fake_zip(4096);
    std::fs::write(env.games.join("Game.zip"), &archive).expect("archive");
    std::fs::write(env.games.join(".Game.part"), vec![1u8; 1024]).expect("partial");
    std::fs::write(env.games.join(".hidden"), b"ignored").expect("dotfile");

    let request = Request::builder()
        .uri("/api/ingest/files")
        .body(Body::empty())
        .expect("request");
    let (status, body) = http(&app, request).await;
    assert_eq!(status, StatusCode::OK);
    let files: Vec<serde_json::Value> = serde_json::from_slice(&body).expect("files json");
    assert_eq!(files.len(), 2);
    let partial = files
        .iter()
        .find(|file| file["name"] == ".Game.part")
        .expect("partial entry");
    assert_eq!(partial["partial"], true);
    assert_eq!(partial["size"], 1024);
    assert!(files.iter().any(|file| file["name"] == "Game.zip"));

    let request = Request::builder()
        .uri("/api/ingest/stats")
        .body(Body::empty())
        .expect("request");
    let (status, body) = http(&app, request).await;
    assert_eq!(status, StatusCode::OK);
    let stats: serde_json::Value = serde_json::from_slice(&body).expect("stats json");
    assert_eq!(stats["gameCount"], 1);
    assert_eq!(stats["libraryBytes"], archive.len());
    assert_eq!(stats["activeJobs"], 0);
    assert!(stats["diskTotal"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test]
async fn retry_after_cancel_restarts_cleanly() {
    let env = TestEnv::new();
    let payload = fake_zip(64 * 1024);
    let server = spawn_file_server(payload.clone(), None).await;
    let app = env.router();
    let job = create_job(&app, &server.url("/file"), "Retry Game").await;

    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/api/ingest/downloads/{}", job.id))
        .body(Body::empty())
        .expect("request");
    let (status, _) = http(&app, request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        fetch_job(&app, &job.id).await.status,
        IngestStatus::Cancelled
    );

    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/ingest/downloads/{}/retry", job.id))
        .body(Body::empty())
        .expect("request");
    let (status, body) = http(&app, request).await;
    assert_eq!(status, StatusCode::OK);
    let retried: IngestJob = serde_json::from_slice(&body).expect("retried job");
    assert_eq!(retried.status, IngestStatus::Queued);

    let done = wait_for_status(&app, &job.id, IngestStatus::Done).await;
    assert_eq!(done.file_name.as_deref(), Some("Retry Game.zip"));
    assert_eq!(
        std::fs::read(env.games.join("Retry Game.zip")).expect("finished file"),
        payload
    );
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[test]
fn sanitize_title_strips_separators_and_keeps_accents() {
    assert_eq!(
        sanitize_title("  Assassin's Creed: Édition  "),
        "Assassin's Creed Édition"
    );
    assert_eq!(sanitize_title("///"), "Game");
    assert_eq!(sanitize_title("..."), "Game");
    assert_eq!(
        sanitize_title(&"x".repeat(MAX_TITLE_CHARS + 50))
            .chars()
            .count(),
        MAX_TITLE_CHARS
    );
}

#[test]
fn infer_title_reads_the_last_url_segment() {
    let url = url::Url::parse("https://example.com/games/My%20Game_v1.2.zip?token=abc")
        .expect("valid url");
    assert_eq!(infer_title(&url), "My Game v1.2");

    let url = url::Url::parse("https://example.com/").expect("valid url");
    assert_eq!(infer_title(&url), "Game");
}

#[test]
fn private_addresses_are_refused() {
    for address in [
        "127.0.0.1",
        "10.0.0.5",
        "192.168.1.10",
        "169.254.1.1",
        "100.64.0.1",
        "0.0.0.0",
        "::1",
        "fd00::1",
        "fe80::1",
    ] {
        let ip: IpAddr = address.parse().expect("ip");
        assert!(is_private_ip(ip), "{address} should be refused");
    }
    for address in ["8.8.8.8", "1.1.1.1", "2001:4860:4860::8888"] {
        let ip: IpAddr = address.parse().expect("ip");
        assert!(!is_private_ip(ip), "{address} should be allowed");
    }
}

#[test]
fn file_names_reject_traversal() {
    assert!(super::safe_file_name("Game.zip").is_ok());
    assert!(super::safe_file_name(".Game.part").is_ok());
    assert!(super::safe_file_name("").is_err());
    assert!(super::safe_file_name(".").is_err());
    assert!(super::safe_file_name("..").is_err());
    assert!(super::safe_file_name("../secret").is_err());
    assert!(super::safe_file_name("a/b").is_err());
    assert!(super::safe_file_name("a\\b").is_err());
    assert!(super::safe_file_name("Game .. zip").is_err());
}

#[test]
fn retry_ladder_matches_the_contract() {
    let timings = Timings::default();
    assert_eq!(timings.retry_delay(0), Duration::from_secs(5));
    assert_eq!(timings.retry_delay(1), Duration::from_secs(10));
    assert_eq!(timings.retry_delay(2), Duration::from_secs(30));
    assert_eq!(timings.retry_delay(3), Duration::from_secs(60));
    assert_eq!(timings.retry_delay(4), Duration::from_secs(300));
    assert_eq!(timings.retry_delay(99), Duration::from_secs(300));
}

#[test]
fn content_range_is_parsed() {
    assert_eq!(
        manager::parse_content_range("bytes 100-199/200"),
        Some((100, Some(200)))
    );
    assert_eq!(manager::parse_content_range("bytes 5-9/*"), Some((5, None)));
    assert_eq!(manager::parse_content_range("nonsense"), None);
}

#[test]
fn job_json_matches_the_contract() {
    let now = Utc::now();
    let job = IngestJob {
        id: "job-1".to_string(),
        title: "Game".to_string(),
        url: None,
        status: IngestStatus::Queued,
        file_name: None,
        downloaded_bytes: 0,
        total_bytes: None,
        speed_bps: 0,
        attempt: 0,
        error: None,
        created_at: now,
        updated_at: now,
    };
    let value = serde_json::to_value(&job).expect("job json");
    for key in [
        "id",
        "title",
        "url",
        "status",
        "fileName",
        "downloadedBytes",
        "totalBytes",
        "speedBps",
        "attempt",
        "error",
        "createdAt",
        "updatedAt",
    ] {
        assert!(value.get(key).is_some(), "missing contract field {key}");
    }
    assert_eq!(value["status"], "queued");
}
