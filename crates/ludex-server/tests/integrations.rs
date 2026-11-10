//! Integration tests for API keys (#40), the audit log (#42) and webhooks (#41).

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode as AxumStatus};
use axum::routing::post;
use axum::Router;
use ludex_server::{start, Config, ServerHandle};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;

const ADMIN_PASSWORD: &str = "password123";

struct TestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    #[allow(dead_code)]
    temp: TempDir,
    games_dir: std::path::PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let mut config = Config::for_local(games_dir.clone(), data_dir, media_dir);
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        config.webhooks_allow_private = true;
        config.webhooks_backoff_ms = 20;
        config.ingest_allow_private = true;

        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            client: Client::builder().build().expect("client"),
            handle: Some(handle),
            temp,
            games_dir,
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

    async fn get(&self, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.request(Method::GET, path, token, None).await
    }

    async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.request(Method::POST, path, token, body).await
    }

    async fn patch(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::PATCH, path, Some(token), Some(body))
            .await
    }

    async fn delete(&self, path: &str, token: &str) -> (StatusCode, Value) {
        self.request(Method::DELETE, path, Some(token), None).await
    }

    async fn setup(&self) -> (String, Value) {
        let (status, body) = self
            .post(
                "/api/setup",
                None,
                Some(json!({
                    "username": "admin",
                    "password": ADMIN_PASSWORD,
                    "serverName": "Home Arcade"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        (
            body["token"].as_str().expect("token").to_string(),
            body["user"].clone(),
        )
    }

    async fn login(&self, username: &str, password: &str) -> (StatusCode, Value) {
        self.post(
            "/api/auth/login",
            None,
            Some(json!({ "username": username, "password": password })),
        )
        .await
    }

    fn write_game(&self, name: &str, size: u64) -> std::path::PathBuf {
        let path = self.games_dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        let file = std::fs::File::create(&path).expect("create");
        file.set_len(size).expect("size");
        path
    }

    async fn run_scan(&self, token: &str) {
        // Wait for the initial scan triggered at startup.
        for _ in 0..400 {
            let (_, status) = self.get("/api/library/scan", Some(token)).await;
            if !status["running"].as_bool().unwrap_or(true) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let before = self.get("/api/library/scan", Some(token)).await.1["finishedAt"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        for _ in 0..100 {
            let (status, body) = self.post("/api/library/scan", Some(token), None).await;
            assert_eq!(status, StatusCode::ACCEPTED, "{body}");
            if body["started"].as_bool().unwrap_or(false) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        for _ in 0..400 {
            let (_, status) = self.get("/api/library/scan", Some(token)).await;
            if !status["running"].as_bool().unwrap_or(true)
                && status["finishedAt"].as_str() != Some(before.as_str())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("scan did not finish");
    }

    async fn create_game(&self, token: &str) -> (i64, std::path::PathBuf) {
        let path = self.write_game("Celeste (2018).zip", 1024 * 1024 + 32);
        self.run_scan(token).await;
        let (status, list) = self.get("/api/games?q=celeste", Some(token)).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        (list["items"][0]["id"].as_i64().expect("game id"), path)
    }

    async fn upload_save(&self, token: &str, game_id: i64) -> StatusCode {
        let form = Form::new()
            .part(
                "file",
                Part::bytes(b"PK\x03\x04ludex-save-test".to_vec()).file_name("slot1.zip"),
            )
            .text("label", "test");
        self.client
            .post(format!("{}/api/games/{game_id}/saves", self.url))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .expect("upload")
            .status()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
    }
}

// ---------------------------------------------------------------------------
// Local webhook receiver
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Hit {
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Clone)]
struct ReceiverState {
    hits: Arc<Mutex<Vec<Hit>>>,
    statuses: Arc<Mutex<VecDeque<u16>>>,
    default_status: Arc<AtomicU16>,
}

impl ReceiverState {
    fn new(statuses: &[u16]) -> Self {
        Self {
            hits: Arc::new(Mutex::new(Vec::new())),
            statuses: Arc::new(Mutex::new(statuses.iter().copied().collect())),
            default_status: Arc::new(AtomicU16::new(200)),
        }
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().expect("hits").clone()
    }
}

async fn spawn_receiver(statuses: &[u16]) -> (String, ReceiverState) {
    let state = ReceiverState::new(statuses);
    let app = Router::new()
        .route(
            "/hook",
            post(
                |State(state): State<ReceiverState>, headers: HeaderMap, body: Bytes| async move {
                    let status = state
                        .statuses
                        .lock()
                        .expect("statuses")
                        .pop_front()
                        .unwrap_or_else(|| state.default_status.load(Ordering::SeqCst));
                    state.hits.lock().expect("hits").push(Hit {
                        headers,
                        body: body.to_vec(),
                    });
                    (AxumStatus::from_u16(status).unwrap_or(AxumStatus::OK), "ok")
                },
            ),
        )
        .route(
            "/file",
            axum::routing::get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/zip")],
                    vec![0_u8; 2 * 1024 * 1024],
                )
            }),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind receiver");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}/hook"), state)
}

async fn wait_for_hits(state: &ReceiverState, count: usize) {
    for _ in 0..400 {
        if state.hits().len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("expected {count} webhook hits, got {}", state.hits().len());
}

async fn wait_for_audit(server: &TestServer, token: &str, action: &str, count: usize) -> Value {
    for _ in 0..200 {
        let (status, page) = server
            .get(
                &format!("/api/audit?limit=200&action={action}"),
                Some(token),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        if page["items"].as_array().map(Vec::len).unwrap_or(0) >= count {
            return page;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("audit action {action} did not reach {count} rows");
}

// ---------------------------------------------------------------------------
// API keys
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_key_lifecycle_scope_enforcement_and_revocation() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let (status, created) = server
        .post(
            "/api/keys",
            Some(&admin),
            Some(json!({ "name": "readonly-ci", "scopes": ["read"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let secret = created["secret"].as_str().expect("secret").to_string();
    assert!(secret.starts_with("ldx_"), "{secret}");
    assert!(created["prefix"]
        .as_str()
        .expect("prefix")
        .starts_with("ldx_"));
    assert_eq!(created["scopes"], json!(["read"]));
    assert!(created.get("secretHash").is_none());
    assert!(!created.to_string().contains("secretHash"));
    let read_key_id = created["id"].as_i64().expect("id");

    // The plaintext token works as a bearer credential.
    let (status, games) = server.get("/api/games", Some(&secret)).await;
    assert_eq!(status, StatusCode::OK, "{games}");

    // A read key cannot mutate, nor mint a stronger key.
    let (status, error) = server.post("/api/library/scan", Some(&secret), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert_eq!(error["error"]["code"], "insufficient_scope");
    let (status, error) = server
        .post(
            "/api/keys",
            Some(&secret),
            Some(json!({ "name": "escalated", "scopes": ["write"] })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");

    // Unknown and malformed keys are rejected like a bad JWT.
    let (status, _) = server.get("/api/games", Some("ldx_0000000000000000")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = server.get("/api/games", Some("not-a-token")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A write key can mutate the library.
    let (status, write_key) = server
        .post(
            "/api/keys",
            Some(&admin),
            Some(json!({ "name": "writer", "scopes": ["write"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{write_key}");
    let write_secret = write_key["secret"].as_str().expect("secret").to_string();
    let (status, body) = server
        .post("/api/library/scan", Some(&write_secret), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    // Usage tracking is persisted and listed without secrets.
    let (status, keys) = server.get("/api/keys", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{keys}");
    let listed = keys
        .as_array()
        .expect("keys")
        .iter()
        .find(|key| key["id"] == read_key_id)
        .expect("read key");
    assert!(listed["lastUsedAt"].is_string(), "{listed}");
    assert!(!keys.to_string().contains(&secret));

    // Admin listing includes every account's keys.
    let (status, all) = server.get("/api/keys?all=true", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(all.as_array().map(Vec::len), Some(2));

    // Revocation is immediate.
    let (status, _) = server
        .delete(&format!("/api/keys/{read_key_id}"), &admin)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = server.get("/api/games", Some(&secret)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Revoking an unknown key is a 404.
    let (status, _) = server.delete("/api/keys/9999", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A regular user cannot list other accounts' keys.
    let (status, _) = server
        .post(
            "/api/users",
            Some(&admin),
            Some(json!({ "username": "alice", "password": "password123", "role": "user" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, login) = server.login("alice", "password123").await;
    let alice = login["token"].as_str().expect("token");
    let (status, error) = server.get("/api/keys?all=true", Some(alice)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");

    server.stop().await;
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

#[tokio::test]
async fn audit_records_security_events_is_admin_only_and_paginates() {
    let server = TestServer::start().await;
    let (_admin, _) = server.setup().await;

    let (status, _) = server.login("admin", "wrong-password").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, login) = server.login("admin", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
    let admin_token = login["token"].as_str().expect("token").to_string();

    let (status, user) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({ "username": "bob", "password": "password123", "role": "editor" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    let user_id = user["id"].as_i64().expect("id");
    let (status, _) = server
        .patch(
            &format!("/api/users/{user_id}"),
            &admin_token,
            json!({ "role": "user" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, key) = server
        .post(
            "/api/keys",
            Some(&admin_token),
            Some(json!({ "name": "audited", "scopes": ["read"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{key}");
    let key_id = key["id"].as_i64().expect("key id");
    let (status, _) = server
        .delete(&format!("/api/keys/{key_id}"), &admin_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let page = wait_for_audit(&server, &admin_token, "login.failed", 1).await;
    assert_eq!(page["items"][0]["actorKind"], "user");
    wait_for_audit(&server, &admin_token, "login.succeeded", 1).await;
    wait_for_audit(&server, &admin_token, "user.created", 2).await; // setup + bob
    wait_for_audit(&server, &admin_token, "user.updated", 1).await;
    wait_for_audit(&server, &admin_token, "api_key.created", 1).await;
    wait_for_audit(&server, &admin_token, "api_key.revoked", 1).await;
    wait_for_audit(&server, &admin_token, "settings.updated", 1).await;

    // Filters compose.
    let (status, filtered) = server
        .get(
            "/api/audit?limit=10&actorKind=user&action=login.failed",
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    assert_eq!(filtered["items"].as_array().map(Vec::len), Some(1));

    // Pagination: two pages of two rows never overlap.
    let (status, first) = server.get("/api/audit?limit=2", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let first_items = first["items"].as_array().expect("items").clone();
    assert_eq!(first_items.len(), 2);
    let cursor = first["nextBefore"].as_i64().expect("cursor");
    let (status, second) = server
        .get(
            &format!("/api/audit?limit=2&before={cursor}"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    for item in second["items"].as_array().expect("items") {
        assert!(
            !first_items.iter().any(|first| first["id"] == item["id"]),
            "pages overlap"
        );
    }

    // Secrets never reach the audit trail.
    let (_, everything) = server.get("/api/audit?limit=200", Some(&admin_token)).await;
    let text = everything.to_string();
    assert!(!text.contains(ADMIN_PASSWORD), "password leaked: {text}");
    assert!(!text.contains("password123"), "password leaked: {text}");
    assert!(
        !text.contains(key["secret"].as_str().expect("secret")),
        "API key leaked: {text}"
    );

    // Non-admins cannot read the audit log.
    let (status, login) = server.login("bob", "password123").await;
    assert_eq!(status, StatusCode::OK);
    let bob = login["token"].as_str().expect("token");
    let (status, error) = server.get("/api/audit", Some(bob)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");

    server.stop().await;
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_signs_delivers_and_skips_disabled_hooks() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    // Drain the setup event so the first delivery is deterministic.
    wait_for_audit(&server, &admin, "user.created", 1).await;
    let (url, receiver) = spawn_receiver(&[]).await;
    let (disabled_url, disabled_receiver) = spawn_receiver(&[]).await;

    let (status, hook) = server
        .post(
            "/api/webhooks",
            Some(&admin),
            Some(json!({ "url": url, "events": ["user.created"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{hook}");
    let secret = hook["secret"].as_str().expect("secret").to_string();
    let hook_id = hook["id"].as_i64().expect("id");
    assert_eq!(hook["enabled"], true);

    // The list never repeats the signing secret.
    let (status, hooks) = server.get("/api/webhooks", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{hooks}");
    assert!(!hooks.to_string().contains(&secret));

    let (status, _) = server
        .post(
            "/api/users",
            Some(&admin),
            Some(json!({ "username": "carol", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    wait_for_hits(&receiver, 1).await;

    let hit = &receiver.hits()[0];
    let signature = hit
        .headers
        .get("x-ludex-signature")
        .expect("signature")
        .to_str()
        .expect("header");
    assert_eq!(
        signature,
        ludex_server::webhooks::sign(&secret, &hit.body),
        "signature must cover the raw body"
    );
    assert_eq!(
        hit.headers
            .get("x-ludex-event")
            .expect("event")
            .to_str()
            .expect("header"),
        "user.created"
    );
    let payload: Value = serde_json::from_slice(&hit.body).expect("payload");
    assert_eq!(payload["event"], "user.created");
    assert_eq!(payload["data"]["username"], "carol");

    // Disabled hooks receive nothing.
    let (status, disabled) = server
        .post(
            "/api/webhooks",
            Some(&admin),
            Some(json!({ "url": disabled_url, "events": ["user.created"], "enabled": false })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{disabled}");
    let (status, _) = server
        .post(
            "/api/users",
            Some(&admin),
            Some(json!({ "username": "dave", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    wait_for_hits(&receiver, 2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        disabled_receiver.hits().len(),
        0,
        "disabled hook must be skipped"
    );

    // Test delivery endpoint.
    let (status, test) = server
        .post(&format!("/api/webhooks/{hook_id}/test"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{test}");
    assert!(test["deliveryId"].as_i64().is_some());
    wait_for_hits(&receiver, 3).await;
    let test_hit = &receiver.hits()[2];
    let payload: Value = serde_json::from_slice(&test_hit.body).expect("payload");
    assert_eq!(payload["event"], "webhook.test");

    // Validation and SSRF defaults.
    let (status, error) = server
        .post(
            "/api/webhooks",
            Some(&admin),
            Some(json!({ "url": "http://127.0.0.1:9/hook", "events": ["user.created"] })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "private allowed in tests: {error}"
    );
    let (status, error) = server
        .post(
            "/api/webhooks",
            Some(&admin),
            Some(json!({ "url": url, "events": ["not.an.event"] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");

    // PATCH disables the hook and DELETE removes it.
    let (status, updated) = server
        .patch(
            &format!("/api/webhooks/{hook_id}"),
            &admin,
            json!({ "enabled": false, "rotateSecret": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["enabled"], false);
    assert!(updated["secret"].as_str().expect("rotated").len() == 64);
    let (status, _) = server
        .delete(&format!("/api/webhooks/{hook_id}"), &admin)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    server.stop().await;
}

#[tokio::test]
async fn webhook_retries_with_exponential_backoff() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    wait_for_audit(&server, &admin, "user.created", 1).await;
    // Fail twice, then succeed.
    let (url, receiver) = spawn_receiver(&[500, 500, 200]).await;

    let (status, hook) = server
        .post(
            "/api/webhooks",
            Some(&admin),
            Some(json!({ "url": url, "events": ["user.created"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{hook}");
    let secret = hook["secret"].as_str().expect("secret").to_string();

    let (status, _) = server
        .post(
            "/api/users",
            Some(&admin),
            Some(json!({ "username": "retry-target", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    wait_for_hits(&receiver, 3).await;

    let hits = receiver.hits();
    assert_eq!(hits.len(), 3, "two failures then one success");
    // Every attempt signs the same stored payload.
    for hit in &hits {
        let signature = hit
            .headers
            .get("x-ludex-signature")
            .expect("signature")
            .to_str()
            .expect("header");
        assert_eq!(signature, ludex_server::webhooks::sign(&secret, &hit.body));
    }
    assert_eq!(hits[0].body, hits[1].body);
    assert_eq!(hits[1].body, hits[2].body);

    server.stop().await;
}

#[tokio::test]
async fn webhook_emits_game_and_save_events() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let (url, receiver) = spawn_receiver(&[]).await;

    let (status, hook) = server
        .post(
            "/api/webhooks",
            Some(&admin),
            Some(
                json!({ "url": url, "events": ["game.ingested", "game.deleted", "save.uploaded"] }),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{hook}");

    let (game_id, game_path) = server.create_game(&admin).await;
    wait_for_hits(&receiver, 1).await;
    let payload: Value = serde_json::from_slice(&receiver.hits()[0].body).expect("payload");
    assert_eq!(payload["event"], "game.ingested");
    assert_eq!(payload["data"]["title"], "Celeste");

    let status = server.upload_save(&admin, game_id).await;
    assert_eq!(status, StatusCode::CREATED);
    wait_for_hits(&receiver, 2).await;
    let payload: Value = serde_json::from_slice(&receiver.hits()[1].body).expect("payload");
    assert_eq!(payload["event"], "save.uploaded");
    assert_eq!(payload["data"]["gameId"], game_id);
    assert!(payload["data"]["size"].as_i64().unwrap_or(0) > 0);

    // Deleting the library file is picked up by the next scan.
    std::fs::remove_file(&game_path).expect("remove game");
    server.run_scan(&admin).await;
    wait_for_hits(&receiver, 3).await;
    let payload: Value = serde_json::from_slice(&receiver.hits()[2].body).expect("payload");
    assert_eq!(payload["event"], "game.deleted");
    assert_eq!(payload["data"]["source"], "scan");

    server.stop().await;
}

#[tokio::test]
async fn webhook_emits_download_completed() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    wait_for_audit(&server, &admin, "user.created", 1).await;
    let (hook_url, receiver) = spawn_receiver(&[]).await;
    let file_url = hook_url.replace("/hook", "/file");

    let (status, hook) = server
        .post(
            "/api/webhooks",
            Some(&admin),
            Some(json!({ "url": hook_url, "events": ["download.completed"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{hook}");

    let (status, job) = server
        .post(
            "/api/ingest/downloads",
            Some(&admin),
            Some(json!({ "url": file_url, "title": "Downloaded Game" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{job}");

    wait_for_hits(&receiver, 1).await;
    let payload: Value = serde_json::from_slice(&receiver.hits()[0].body).expect("payload");
    assert_eq!(payload["event"], "download.completed");
    assert_eq!(payload["data"]["title"], "Downloaded Game");
    assert_eq!(payload["data"]["bytes"], 2 * 1024 * 1024);

    server.stop().await;
}
