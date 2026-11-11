//! Core gaming API (#51, #52): controller profiles and emulators.

use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;

struct TestServer {
    url: String,
    token: String,
    client: Client,
    handle: Option<ServerHandle>,
    #[allow(dead_code)]
    temp: TempDir,
}

impl TestServer {
    async fn start() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = Config::for_local(
            temp.path().join("games"),
            temp.path().join("data"),
            temp.path().join("media"),
        );
        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            token: handle.token.clone(),
            client: Client::new(),
            handle: Some(handle),
            temp,
        }
    }

    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.url, path))
            .bearer_auth(&self.token);
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

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.request(Method::GET, path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::POST, path, Some(body)).await
    }

    async fn put(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::PUT, path, Some(body)).await
    }
}

#[tokio::test]
async fn controller_profiles_round_trip_and_validate() {
    let server = TestServer::start().await;

    let (status, body) = server.get("/api/controller-profiles").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["presets"].as_array().unwrap().len(), 4);
    assert!(body["global"].is_null());

    let profile = json!({
        "name": "Smoke",
        "deadzone": 0.22,
        "sensitivity": 1.3,
        "curve": 1.1,
        "invertX": true,
        "invertY": false,
        "triggerThreshold": 0.4,
        "vibration": false,
        "mappings": { "button0": "confirm", "axis1-": "up" }
    });
    let (status, saved) = server
        .put(
            "/api/controller-profiles",
            json!({ "gameId": null, "profile": profile }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["profile"]["deadzone"], 0.22);
    assert!(saved["gameId"].is_null());

    let (status, listed) = server.get("/api/controller-profiles").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["global"]["profile"]["name"], "Smoke");

    let mut invalid = profile.clone();
    invalid["deadzone"] = json!(2.0);
    let (status, body) = server
        .put("/api/controller-profiles", json!({ "profile": invalid }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, _) = server
        .request(Method::DELETE, "/api/controller-profiles/0", None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    server.stop().await;
}

#[tokio::test]
async fn emulators_validate_commands_and_reject_shell_syntax() {
    let server = TestServer::start().await;

    let (status, emulator) = server
        .post(
            "/api/emulators",
            json!({
                "name": "Snes9x",
                "command": "snes9x %ROM%",
                "platforms": ["snes"]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{emulator}");
    let id = emulator["id"].as_i64().expect("id");

    let (status, defaults) = server
        .put(
            "/api/emulators/defaults",
            json!({ "defaults": [{ "platform": "snes", "emulatorId": id }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{defaults}");
    assert_eq!(defaults[0]["platform"], "snes");

    let (status, valid) = server
        .post(
            "/api/emulators/validate",
            json!({ "command": "retroarch -L snes9x_libretro.so %ROM%" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(valid["valid"], true);
    assert_eq!(valid["program"], "retroarch");

    let (_, invalid) = server
        .post(
            "/api/emulators/validate",
            json!({ "command": "retroarch %ROM%; rm -rf /" }),
        )
        .await;
    assert_eq!(invalid["valid"], false);
    assert!(invalid["error"].as_str().unwrap().contains("shell syntax"));

    let (status, body) = server
        .post(
            "/api/emulators",
            json!({ "name": "Bad", "command": "snes9x", "platforms": ["snes"] }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, platforms) = server.get("/api/platforms").await;
    assert_eq!(status, StatusCode::OK);
    assert!(platforms
        .as_array()
        .unwrap()
        .iter()
        .any(|platform| platform["id"] == "snes"));
    server.stop().await;
}
