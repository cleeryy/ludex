//! Shared harness for the HTTP integration tests.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;

pub const ADMIN_PASSWORD: &str = "password123";

pub struct TestServer {
    pub url: String,
    pub data_dir: PathBuf,
    pub games_dir: PathBuf,
    pub client: Client,
    handle: Option<ServerHandle>,
    #[allow(dead_code)]
    temp: TempDir,
}

impl TestServer {
    pub async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    pub async fn start_with(configure: impl FnOnce(&mut Config)) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let mut config = Config::for_local(games_dir.clone(), data_dir.clone(), media_dir);
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        configure(&mut config);
        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            data_dir,
            games_dir,
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("client"),
            handle: Some(handle),
            temp,
        }
    }

    pub async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
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
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        (status, value)
    }

    pub async fn get(&self, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.request(Method::GET, path, token, None).await
    }

    pub async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.request(Method::POST, path, token, body).await
    }

    pub async fn put(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::PUT, path, Some(token), Some(body))
            .await
    }

    pub async fn patch(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::PATCH, path, Some(token), Some(body))
            .await
    }

    pub async fn delete(&self, path: &str, token: &str) -> (StatusCode, Value) {
        self.request(Method::DELETE, path, Some(token), None).await
    }

    pub async fn put_bytes(&self, path: &str, token: &str, body: Vec<u8>) -> (StatusCode, Value) {
        let response = self
            .client
            .put(format!("{}{}", self.url, path))
            .bearer_auth(token)
            .body(body)
            .send()
            .await
            .expect("request");
        let status = response.status();
        let text = response.text().await.expect("body");
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        (status, value)
    }

    pub async fn setup(&self) -> (String, Value) {
        let (status, body) = self
            .post(
                "/api/setup",
                None,
                Some(json!({
                    "username": "admin",
                    "password": ADMIN_PASSWORD,
                    "serverName": "Test Arcade"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let token = body["token"].as_str().expect("token").to_string();
        wait_for_scan(self, &token).await;
        (token, body["user"].clone())
    }

    pub async fn login(&self, username: &str, password: &str) -> (StatusCode, Value) {
        self.post(
            "/api/auth/login",
            None,
            Some(json!({ "username": username, "password": password })),
        )
        .await
    }

    pub async fn create_user(
        &self,
        admin_token: &str,
        username: &str,
        password: &str,
        role: &str,
    ) -> Value {
        let (status, body) = self
            .post(
                "/api/users",
                Some(admin_token),
                Some(json!({ "username": username, "password": password, "role": role })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body
    }

    /// Writes a fake archive into the library root and scans it.
    pub async fn add_game(&self, token: &str, name: &str, size: usize) -> i64 {
        let path = self.games_dir.join(name);
        std::fs::write(&path, vec![7_u8; size]).expect("write game");
        run_scan(self, token).await;
        let (status, games) = self.get("/api/games?limit=100", Some(token)).await;
        assert_eq!(status, StatusCode::OK, "{games}");
        let id = games["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|game| {
                game["title"]
                    .as_str()
                    .is_some_and(|title| name.starts_with(title))
            })
            .map(|game| game["id"].as_i64().expect("id"))
            .unwrap_or_else(|| {
                // Fall back to the newest game when the parsed title differs.
                games["items"]
                    .as_array()
                    .and_then(|items| items.first())
                    .and_then(|game| game["id"].as_i64())
                    .expect("game id")
            });
        id
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
    }
}

pub async fn scan_status(server: &TestServer, token: &str) -> Value {
    let (status, body) = server.get("/api/library/scan", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

pub async fn wait_for_scan(server: &TestServer, token: &str) {
    for _ in 0..600 {
        let status = scan_status(server, token).await;
        if !status["running"].as_bool().unwrap_or(true) && status["finishedAt"].as_str().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("scan did not finish");
}

pub async fn run_scan(server: &TestServer, token: &str) {
    wait_for_scan(server, token).await;
    let before = scan_status(server, token).await["finishedAt"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for _ in 0..100 {
        let (status, body) = server.post("/api/library/scan", Some(token), None).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        if body["started"].as_bool().unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for _ in 0..600 {
        let status = scan_status(server, token).await;
        if !status["running"].as_bool().unwrap_or(true)
            && status["finishedAt"].as_str() != Some(before.as_str())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("manual scan did not finish");
}
