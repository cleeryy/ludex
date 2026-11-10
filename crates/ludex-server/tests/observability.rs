//! Prometheus metrics endpoint tests (#48).

use std::path::PathBuf;
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;

const ADMIN_PASSWORD: &str = "password123";
const METRICS_TOKEN: &str = "scrape-token-123";

struct TestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    #[allow(dead_code)]
    temp: TempDir,
    #[allow(dead_code)]
    games_dir: PathBuf,
}

impl TestServer {
    async fn start(metrics_token: Option<&str>) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let mut config = Config::for_local(games_dir.clone(), data_dir, media_dir);
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        config.metrics_token = metrics_token.map(str::to_string);
        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            client: Client::builder().build().expect("client"),
            handle: Some(handle),
            temp,
            games_dir,
        }
    }

    async fn raw(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, String) {
        let mut request = self.client.request(method, format!("{}{}", self.url, path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("request");
        let status = response.status();
        let text = response.text().await.expect("body");
        (status, text)
    }

    async fn scrape(&self, token: Option<&str>) -> (StatusCode, String) {
        self.raw(Method::GET, "/metrics", token, None).await
    }

    async fn setup_admin(&self) -> String {
        let (status, text) = self
            .raw(
                Method::POST,
                "/api/setup",
                None,
                Some(json!({ "username": "admin", "password": ADMIN_PASSWORD })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let body: Value = serde_json::from_str(&text).expect("setup json");
        body["token"].as_str().expect("token").to_string()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
    }
}

#[tokio::test]
async fn metrics_token_protects_the_endpoint() {
    let server = TestServer::start(Some(METRICS_TOKEN)).await;

    let (status, _) = server.scrape(None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = server.scrape(Some("wrong-token")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = server.scrape(Some(METRICS_TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("# TYPE ludex_http_requests_total counter"));
    assert!(body.contains("ludex_build_info{version="));
    assert!(body.contains("ludex_games_total 0"));
    assert!(body.contains("ludex_game_versions_total 0"));
    assert!(body.contains("ludex_media_backend_info{backend=\"local\"} 1"));
    assert!(body.contains("ludex_db_pool_connections{state=\"open\"}"));
    assert!(body.contains("ludex_scan_duration_seconds"));
}

#[tokio::test]
async fn metrics_defaults_to_administrators() {
    let server = TestServer::start(None).await;

    let (status, _) = server.scrape(None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let admin = server.setup_admin().await;
    let (status, body) = server.scrape(Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A regular user is not enough.
    let (status, text) = server
        .raw(
            Method::POST,
            "/api/users",
            Some(&admin),
            Some(json!({
                "username": "player",
                "password": "password123",
                "role": "user"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let (status, text) = server
        .raw(
            Method::POST,
            "/api/auth/login",
            None,
            Some(json!({ "username": "player", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let user_token: Value = serde_json::from_str(&text).expect("login json");
    let user_token = user_token["token"].as_str().expect("token");
    let (status, _) = server.scrape(Some(user_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn metrics_record_requests_and_library_state() {
    let server = TestServer::start(Some(METRICS_TOKEN)).await;
    let (status, _) = server.raw(Method::GET, "/api/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    // Give the metrics layer a moment to record the response.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // The scrape cannot report itself while it is being rendered: read twice.
    let (status, first) = server.scrape(Some(METRICS_TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (status, body) = server.scrape(Some(METRICS_TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(
            "ludex_http_requests_total{method=\"GET\",route=\"/api/health\",status=\"200\"}"
        ),
        "{body}"
    );
    assert!(
        body.contains("ludex_http_request_duration_seconds_count{route=\"/api/health\"}"),
        "{body}"
    );
    // The previous scrape was recorded too, under its own route.
    assert!(body.contains("route=\"/metrics\""), "{body}");
    // No webhook has fired yet: the counter family is absent, not zero-filled.
    assert!(!body.contains("ludex_webhook_deliveries_total{"));

    // Indexing a game updates the library gauges.
    let path = server.games_dir.join("Metrics Game (2024).zip");
    std::fs::write(&path, vec![0_u8; 1024 * 1024 + 1]).expect("write game");
    let admin = server.setup_admin().await;
    for _ in 0..200 {
        let (status, text) = server
            .raw(Method::POST, "/api/library/scan", Some(&admin), None)
            .await;
        assert!(
            matches!(
                status,
                StatusCode::ACCEPTED | StatusCode::OK | StatusCode::CONFLICT
            ),
            "{text}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
        let (_, body) = server.scrape(Some(METRICS_TOKEN)).await;
        if body.contains("ludex_games_total 1") {
            return;
        }
    }
    panic!("games_total gauge never reflected the indexed game");
}
