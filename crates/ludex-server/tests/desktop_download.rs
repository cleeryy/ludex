//! Contract tests for the public desktop download endpoint (#164).
//!
//! The upstream is a local axum mock: no test ever talks to GitHub, and the
//! recorded request headers prove the server never sends a token.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use ludex_server::{start, Config};
use reqwest::Client;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::oneshot;

const RELEASES_PAGE: &str = "https://example.org/ludex/releases";

const SHA256SUMS: &str = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  Ludex_0.3.2_x64-setup.exe
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210  ludex_0.3.2_amd64.AppImage
";

#[derive(Clone)]
struct MockState {
    release_hits: Arc<AtomicUsize>,
    checksum_hits: Arc<AtomicUsize>,
    authorization: Arc<Mutex<Vec<String>>>,
    rate_limited: Arc<AtomicBool>,
    base: String,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            release_hits: Arc::new(AtomicUsize::new(0)),
            checksum_hits: Arc::new(AtomicUsize::new(0)),
            authorization: Arc::new(Mutex::new(Vec::new())),
            rate_limited: Arc::new(AtomicBool::new(false)),
            base: String::new(),
        }
    }
}

fn release_document(base: &str) -> Value {
    let asset = |name: &str| format!("{base}/download/{name}");
    json!({
        "tag_name": "v0.3.2",
        "html_url": "https://example.org/ludex/releases/tag/v0.3.2",
        "published_at": "2026-11-10T09:30:00Z",
        "body": "## Ludex 0.3.2\n\n**Cloud saves** and [controller](https://example.org) support.\n\nFull changelog.",
        "assets": [
            {
                "name": "Ludex_0.3.2_x64-setup.exe",
                "size": 1_000_000,
                "browser_download_url": asset("Ludex_0.3.2_x64-setup.exe")
            },
            {
                "name": "ludex_0.3.2_amd64.AppImage",
                "size": 2_000_000,
                "browser_download_url": asset("ludex_0.3.2_amd64.AppImage")
            },
            {
                "name": "ludex_0.3.2_amd64.deb",
                "size": 3_000_000,
                "browser_download_url": asset("ludex_0.3.2_amd64.deb")
            },
            {
                "name": "Ludex_0.3.2_universal.dmg",
                "size": 4_000_000,
                "browser_download_url": asset("Ludex_0.3.2_universal.dmg")
            },
            {
                "name": "SHA256SUMS",
                "size": 256,
                "browser_download_url": format!("{base}/SHA256SUMS")
            },
            {
                "name": "Ludex_0.3.2_x64-setup.exe.sig",
                "size": 512,
                "browser_download_url": asset("Ludex_0.3.2_x64-setup.exe.sig")
            },
            {
                "name": "latest.json",
                "size": 1024,
                "browser_download_url": asset("latest.json")
            }
        ]
    })
}

async fn release(State(state): State<MockState>, headers: HeaderMap) -> impl IntoResponse {
    state.release_hits.fetch_add(1, Ordering::SeqCst);
    let authorization = headers
        .get("authorization")
        .map(|value| value.to_str().unwrap_or("<binary>").to_string())
        .unwrap_or_default();
    if let Ok(mut seen) = state.authorization.lock() {
        seen.push(authorization);
    }
    if state.rate_limited.load(Ordering::SeqCst) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "API rate limit exceeded" })),
        )
            .into_response();
    }
    (StatusCode::OK, Json(release_document(&state.base))).into_response()
}

async fn checksums(State(state): State<MockState>) -> impl IntoResponse {
    state.checksum_hits.fetch_add(1, Ordering::SeqCst);
    (StatusCode::OK, SHA256SUMS)
}

async fn spawn_mock(mut state: MockState) -> (String, oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    state.base = format!("http://{address}");
    let app = Router::new()
        .route("/releases/latest", get(release))
        .route("/SHA256SUMS", get(checksums))
        .with_state(state);
    let (shutdown, signal) = oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = signal.await;
            })
            .await;
    });
    (format!("http://{address}"), shutdown)
}

struct Running {
    url: String,
    shutdown: Option<oneshot::Sender<()>>,
    #[allow(dead_code)]
    temp: TempDir,
}

impl Running {
    async fn start(configure: impl FnOnce(&mut Config)) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::for_local(
            temp.path().join("games"),
            temp.path().join("data"),
            temp.path().join("media"),
        );
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        config.rate_limit_rps = 0;
        configure(&mut config);
        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url,
            shutdown: Some(handle.shutdown),
            temp,
        }
    }

    async fn get(&self, path: &str) -> (StatusCode, HeaderMap, Value) {
        let response = Client::new()
            .get(format!("{}{path}", self.url))
            .send()
            .await
            .expect("request");
        let status = response.status();
        let headers = response.headers().clone();
        let text = response.text().await.expect("body");
        (
            status,
            headers,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

fn configured(config: &mut Config, mock: &str) {
    config.desktop_releases_api = format!("{mock}/releases/latest");
    config.desktop_releases_page = RELEASES_PAGE.to_string();
}

#[tokio::test]
async fn public_endpoint_maps_platforms_and_never_uses_a_token() {
    let mock = MockState::default();
    let (mock_url, _mock_shutdown) = spawn_mock(mock.clone()).await;
    let server = Running::start(|config| configured(config, &mock_url)).await;

    // No Authorization header on the request: the endpoint is public.
    let (status, headers, body) = server.get("/api/desktop/download").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=300")
    );
    assert_eq!(body["source"], "github");
    assert_eq!(body["stale"], false);
    assert_eq!(body["channel"], "stable");
    assert_eq!(body["version"], "0.3.2");
    assert_eq!(body["tag"], "v0.3.2");
    assert_eq!(body["releasedAt"], "2026-11-10T09:30:00Z");
    assert_eq!(
        body["releaseUrl"],
        "https://example.org/ludex/releases/tag/v0.3.2"
    );
    assert_eq!(body["notes"], "Cloud saves and controller support.");
    assert!(body["checksumsUrl"]
        .as_str()
        .is_some_and(|url| url.ends_with("/SHA256SUMS")));
    assert_eq!(body["platform"], Value::Null);
    assert_eq!(body["recommended"], Value::Null);

    let assets = body["assets"].as_array().expect("assets");
    assert_eq!(
        assets.len(),
        4,
        "updater artifacts must not be offered: {assets:?}"
    );
    let names: Vec<&str> = assets
        .iter()
        .map(|asset| asset["name"].as_str().unwrap_or_default())
        .collect();
    assert!(names.contains(&"Ludex_0.3.2_x64-setup.exe"));
    assert!(!names.contains(&"Ludex_0.3.2_x64-setup.exe.sig"));
    assert!(!names.contains(&"latest.json"));
    let windows = assets
        .iter()
        .find(|asset| asset["name"] == "Ludex_0.3.2_x64-setup.exe")
        .expect("windows asset");
    assert_eq!(windows["platform"], "windows");
    assert_eq!(windows["kind"], "installer");
    assert_eq!(windows["sizeBytes"], 1_000_000);
    assert_eq!(
        windows["sha256"],
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    );

    let (status, _, windows) = server.get("/api/desktop/download?platform=windows").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(windows["platform"], "windows");
    assert_eq!(windows["recommended"]["name"], "Ludex_0.3.2_x64-setup.exe");

    // Aliases are accepted and the AppImage wins over the .deb.
    let (status, _, linux) = server.get("/api/desktop/download?platform=Linux").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(linux["recommended"]["name"], "ludex_0.3.2_amd64.AppImage");
    assert_eq!(linux["recommended"]["kind"], "appimage");

    let (status, _, mac) = server.get("/api/desktop/download?platform=macos").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mac["recommended"]["name"], "Ludex_0.3.2_universal.dmg");

    // Unknown platform is a validation error, not a silent fallback.
    let (status, _, error) = server.get("/api/desktop/download?platform=plan9").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "invalid_platform");

    // One upstream release fetch and one checksum fetch served every request.
    assert_eq!(mock.release_hits.load(Ordering::SeqCst), 1);
    assert_eq!(mock.checksum_hits.load(Ordering::SeqCst), 1);

    let seen = mock.authorization.lock().expect("authorization log");
    assert!(
        seen.iter().all(String::is_empty),
        "the download endpoint must never send a token upstream: {seen:?}"
    );
}

#[tokio::test]
async fn rate_limited_upstream_degrades_to_the_release_page() {
    let mock = MockState::default();
    mock.rate_limited.store(true, Ordering::SeqCst);
    let (mock_url, _mock_shutdown) = spawn_mock(mock.clone()).await;
    let server = Running::start(|config| configured(config, &mock_url)).await;

    let (status, _, body) = server.get("/api/desktop/download?platform=windows").await;
    assert_eq!(status, StatusCode::OK, "offline must not break the page");
    assert_eq!(body["source"], "fallback");
    assert_eq!(body["stale"], true);
    assert_eq!(body["releaseUrl"], RELEASES_PAGE);
    assert_eq!(body["version"], Value::Null);
    assert_eq!(body["assets"], json!([]));
    assert_eq!(body["recommended"], Value::Null);
    assert_eq!(mock.release_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unreachable_upstream_returns_the_fallback_document() {
    // Port 1 is closed, so the fetch fails immediately.
    let server = Running::start(|config| {
        config.desktop_releases_api = "http://127.0.0.1:1/releases/latest".to_string();
        config.desktop_releases_page = RELEASES_PAGE.to_string();
        config.desktop_download_timeout = Duration::from_millis(500);
    })
    .await;

    let (status, _, body) = server.get("/api/desktop/download").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["source"], "fallback");
    assert_eq!(body["releaseUrl"], RELEASES_PAGE);
    assert_eq!(body["assets"], json!([]));
}

#[tokio::test]
async fn stale_cache_is_served_when_the_refresh_fails() {
    let mock = MockState::default();
    let (mock_url, _mock_shutdown) = spawn_mock(mock.clone()).await;
    let server = Running::start(|config| {
        configured(config, &mock_url);
        config.desktop_download_ttl = Duration::from_millis(50);
    })
    .await;

    let (status, _, first) = server.get("/api/desktop/download?platform=linux").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["source"], "github");
    assert_eq!(first["stale"], false);

    tokio::time::sleep(Duration::from_millis(120)).await;
    mock.rate_limited.store(true, Ordering::SeqCst);

    let (status, _, second) = server.get("/api/desktop/download?platform=linux").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["source"], "cache");
    assert_eq!(second["stale"], true);
    assert_eq!(second["version"], "0.3.2");
    assert_eq!(second["recommended"]["name"], "ludex_0.3.2_amd64.AppImage");
    assert_eq!(mock.release_hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn configured_mirror_rewrites_asset_urls() {
    let mock = MockState::default();
    let (mock_url, _mock_shutdown) = spawn_mock(mock.clone()).await;
    let server = Running::start(|config| {
        configured(config, &mock_url);
        config.desktop_download_base_url = Some("https://mirror.example/ludex/".to_string());
    })
    .await;

    let (status, _, body) = server.get("/api/desktop/download?platform=windows").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["source"], "mirror");
    assert_eq!(
        body["recommended"]["url"],
        "https://mirror.example/ludex/Ludex_0.3.2_x64-setup.exe"
    );
    assert!(body["checksumsUrl"]
        .as_str()
        .is_some_and(|url| url == "https://mirror.example/ludex/SHA256SUMS"));
}

#[tokio::test]
async fn plain_download_path_redirects_to_the_public_spa_route() {
    let server = Running::start(|_| {}).await;
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");

    let response = client
        .get(format!("{}/download", server.url))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/#/download")
    );
}
