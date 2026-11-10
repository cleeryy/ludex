//! Prometheus metrics and detailed health (#48).

use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::Client;
use tempfile::TempDir;

struct Server {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    #[allow(dead_code)]
    temp: TempDir,
}

impl Server {
    async fn start(metrics_token: Option<&str>) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games = temp.path().join("games");
        let data = temp.path().join("data");
        let media = temp.path().join("media");
        let mut config = Config::for_local(games, data, media);
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        config.metrics_token = metrics_token.map(str::to_string);
        let handle = start(config).await.expect("server");
        let url = handle.url.clone();
        Self {
            url,
            client: Client::new(),
            handle: Some(handle),
            temp,
        }
    }

    async fn get(&self, path: &str, token: Option<&str>) -> (reqwest::StatusCode, String) {
        let mut request = self.client.get(format!("{}{}", self.url, path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.expect("request");
        let status = response.status();
        let body = response.text().await.expect("body");
        (status, body)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
    }
}

#[tokio::test]
async fn metrics_expose_counters_gauges_and_histograms() {
    let server = Server::start(None).await;
    // Generate one counted request before scraping.
    let (status, _) = server.get("/api/health", None).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (status, body) = server.get("/metrics", None).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    for expected in [
        "ludex_http_requests_total",
        "ludex_http_request_duration_seconds_bucket",
        "ludex_http_request_duration_seconds_count",
        "ludex_games",
        "ludex_game_versions",
        "ludex_library_roots",
        "ludex_ingest_jobs",
        "ludex_scan_running",
        "ludex_webhook_deliveries",
        "ludex_webhook_retries",
        "ludex_media_cache_bytes",
        "ludex_process_uptime_seconds",
        "ludex_database_size_bytes",
        "ludex_build_info",
        "ludex_storage_backend_info",
    ] {
        assert!(body.contains(expected), "missing metric {expected}");
    }
    assert!(body.contains("route=\"/api/health\""), "{body}");
    assert!(body.contains("status=\"200\""), "{body}");
}

#[tokio::test]
async fn metrics_token_is_enforced_when_set() {
    let server = Server::start(Some("scrape-token")).await;
    let (status, _) = server.get("/metrics", None).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    let (status, _) = server.get("/metrics", Some("wrong")).await;
    assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    let (status, body) = server.get("/metrics", Some("scrape-token")).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    assert!(body.contains("ludex_http_requests_total"));
}

#[tokio::test]
async fn public_metrics_are_rate_limited() {
    let server = Server::start(None).await;
    let mut last = reqwest::StatusCode::OK;
    for _ in 0..31 {
        let (status, _) = server.get("/metrics", None).await;
        last = status;
    }
    assert_eq!(last, reqwest::StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn health_details_report_shape_without_paths() {
    let server = Server::start(None).await;
    let (status, body) = server.get("/api/health/details", None).await;
    assert_eq!(status, reqwest::StatusCode::OK);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(value["status"], "ok");
    assert!(value["version"].as_str().is_some());
    assert!(value["commit"].as_str().is_some());
    assert!(value["uptimeSeconds"].as_u64().is_some());
    assert_eq!(value["database"]["ok"], true);
    assert!(value["database"]["sizeBytes"].as_u64().unwrap_or(0) > 0);
    assert!(value["disk"]["dataFreeBytes"].as_u64().is_some());
    assert_eq!(value["library"]["games"], 0);
    assert!(value["library"]["rootsTotal"].as_i64().unwrap_or(0) >= 1);
    assert_eq!(value["storageBackend"], "local");
    assert_eq!(value["watcher"]["enabled"], false);
    assert_eq!(value["ingest"]["queued"], 0);
    assert!(value["integrity"].is_null());
    // Paths never leak through the public health endpoint.
    let temp = server.temp.path().to_string_lossy().into_owned();
    assert!(!body.contains(&temp), "unexpected path leak: {body}");
}
