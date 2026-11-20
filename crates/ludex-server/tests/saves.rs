//! End-to-end tests for per-user cloud saves.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_LENGTH};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const ADMIN_PASSWORD: &str = "password123";

struct TestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    temp: TempDir,
    data_dir: PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    async fn start_with(configure: impl FnOnce(&mut Config)) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        // Local mode auto-provisions an administrator and returns its token.
        let mut config = Config::for_local(games_dir, data_dir.clone(), media_dir);
        config.bind = "127.0.0.1:0".parse().expect("bind");
        configure(&mut config);

        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            client: Client::builder().no_proxy().build().expect("client"),
            handle: Some(handle),
            temp,
            data_dir,
        }
    }

    fn admin_token(&self) -> String {
        self.handle.as_ref().expect("handle").token.clone()
    }

    async fn request(
        &self,
        method: reqwest::Method,
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
        self.request(reqwest::Method::GET, path, token, None).await
    }

    async fn post(&self, path: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
        self.request(reqwest::Method::POST, path, token, Some(body))
            .await
    }

    async fn delete(&self, path: &str, token: &str) -> (StatusCode, Value) {
        self.request(reqwest::Method::DELETE, path, Some(token), None)
            .await
    }

    async fn send_form(
        &self,
        game_id: i64,
        token: Option<&str>,
        form: Form,
    ) -> (StatusCode, Value) {
        let mut request = self
            .client
            .post(format!("{}/api/games/{game_id}/saves", self.url))
            .multipart(form);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.expect("upload");
        let status = response.status();
        let text = response.text().await.expect("body");
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        (status, value)
    }

    async fn upload(
        &self,
        game_id: i64,
        token: &str,
        file_name: &str,
        bytes: Vec<u8>,
        label: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut form =
            Form::new().part("file", Part::bytes(bytes).file_name(file_name.to_string()));
        if let Some(label) = label {
            form = form.text("label", label.to_string());
        }
        self.send_form(game_id, Some(token), form).await
    }

    /// Upload with the automatic-sync fields (#108).
    async fn upload_as(
        &self,
        game_id: i64,
        token: &str,
        file_name: &str,
        bytes: Vec<u8>,
        source: &str,
        device_id: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut form =
            Form::new().part("file", Part::bytes(bytes).file_name(file_name.to_string()));
        form = form.text("source", source.to_string());
        if let Some(device_id) = device_id {
            form = form.text("deviceId", device_id.to_string());
        }
        self.send_form(game_id, Some(token), form).await
    }

    /// Automatic upload carrying the multi-device metadata (#296): device
    /// label, save mtime, device clock and the game's own counter.
    #[allow(clippy::too_many_arguments)]
    async fn upload_meta(
        &self,
        game_id: i64,
        token: &str,
        bytes: Vec<u8>,
        device_id: &str,
        device_label: &str,
        client_mtime: &str,
        client_sent_at: &str,
        save_seq: Option<i64>,
    ) -> (StatusCode, Value) {
        let mut form = Form::new().part(
            "file",
            Part::bytes(bytes).file_name(format!("{device_id}.zip")),
        );
        form = form
            .text("source", "auto")
            .text("deviceId", device_id.to_string())
            .text("deviceLabel", device_label.to_string())
            .text("clientMtime", client_mtime.to_string())
            .text("clientSentAt", client_sent_at.to_string());
        if let Some(save_seq) = save_seq {
            form = form.text("saveSeq", save_seq.to_string());
        }
        self.send_form(game_id, Some(token), form).await
    }

    /// Direct SQLite pool on the running server's database.
    async fn direct_pool(&self) -> ludex_server::db::Database {
        let url = format!(
            "sqlite://{}?mode=rwc",
            self.data_dir.join("ludex.db").display()
        );
        ludex_server::db::connect(&url).await.expect("db")
    }

    async fn download(&self, snapshot_id: i64, token: &str) -> reqwest::Response {
        self.client
            .get(format!("{}/api/saves/{snapshot_id}/download", self.url))
            .bearer_auth(token)
            .send()
            .await
            .expect("download")
    }

    async fn current_user_id(&self, token: &str) -> i64 {
        let (status, body) = self.get("/api/auth/me", Some(token)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["user"]["id"].as_i64().expect("user id")
    }

    async fn create_user(&self, admin_token: &str, username: &str, role: &str) -> i64 {
        let (status, body) = self
            .post(
                "/api/users",
                Some(admin_token),
                json!({"username": username, "password": ADMIN_PASSWORD, "role": role}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["id"].as_i64().expect("user id")
    }

    async fn login(&self, username: &str) -> String {
        let (status, body) = self
            .post(
                "/api/auth/login",
                None,
                json!({"username": username, "password": ADMIN_PASSWORD}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().expect("token").to_string()
    }

    /// Inserts a game row directly; saves do not depend on the scanner.
    async fn insert_game(&self, title: &str) -> i64 {
        let url = format!(
            "sqlite://{}?mode=rwc",
            self.data_dir.join("ludex.db").display()
        );
        let pool = ludex_server::db::connect(&url).await.expect("db");
        let now = chrono::Utc::now().to_rfc3339();
        let id = ludex_server::db::query(
            "INSERT INTO games (title, sort_title, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(title)
        .bind(title.to_ascii_lowercase())
        .bind(&now)
        .bind(&now)
        .returning_id(&pool)
        .await
        .expect("insert game");
        pool.close().await;
        id
    }

    async fn save_count(&self, user_id: i64) -> i64 {
        let url = format!(
            "sqlite://{}?mode=rwc",
            self.data_dir.join("ludex.db").display()
        );
        let pool = ludex_server::db::connect(&url).await.expect("db");
        let count = ludex_server::db::query_scalar::<i64>(
            "SELECT COUNT(*) FROM save_snapshots WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("count");
        pool.close().await;
        count
    }

    fn snapshot_path(&self, user_id: i64, game_id: i64, snapshot_id: i64, name: &str) -> PathBuf {
        self.data_dir
            .join("saves")
            .join(user_id.to_string())
            .join(game_id.to_string())
            .join(format!("{snapshot_id}-{name}"))
    }

    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
    }
}

fn test_zip() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut archive = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        archive
            .start_file("save/slot1.sav", options)
            .expect("entry");
        archive.write_all(b"ludex save data").expect("write");
        archive.finish().expect("finish");
    }
    buffer.into_inner()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[tokio::test]
async fn upload_list_download_delete_roundtrip() {
    let server = TestServer::start().await;
    let token = server.admin_token();
    let admin_id = server.current_user_id(&token).await;
    let game_id = server.insert_game("Hades").await;
    let archive = test_zip();

    let (status, snapshot) = server
        .upload(
            game_id,
            &token,
            "Hades save.zip",
            archive.clone(),
            Some("Before final boss"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{snapshot}");
    assert_eq!(snapshot["gameId"], game_id);
    assert_eq!(snapshot["label"], "Before final boss");
    assert_eq!(snapshot["fileName"], "Hades save.zip");
    assert_eq!(snapshot["size"], archive.len());
    assert_eq!(snapshot["sha256"], sha256_hex(&archive));
    assert!(snapshot["createdAt"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    let first_id = snapshot["id"].as_i64().expect("id");

    // A second upload is newer; same-second timestamps fall back to id DESC.
    let (status, second) = server
        .upload(
            game_id,
            &token,
            "slot2.zip",
            b"second archive".to_vec(),
            Some("After boss"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let second_id = second["id"].as_i64().expect("id");

    let (status, list) = server
        .get(&format!("/api/games/{game_id}/saves"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], second_id);
    assert_eq!(items[1]["id"], first_id);

    // On-disk layout: {data_dir}/saves/{user}/{game}/{id}-{name}
    let path = server.snapshot_path(admin_id, game_id, first_id, "Hades save.zip");
    assert!(path.is_file(), "expected {}", path.display());

    let response = server.download(first_id, &token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    let disposition = headers
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(disposition.contains("Hades save.zip"), "{disposition}");
    let length = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(length, Some(archive.len().to_string()));
    assert_eq!(
        headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/zip")
    );
    let body = response.bytes().await.expect("bytes");
    assert_eq!(body.as_ref(), archive.as_slice());

    let (status, _) = server
        .delete(&format!("/api/saves/{first_id}"), &token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!path.exists(), "snapshot file must be deleted");

    assert_eq!(
        server.download(first_id, &token).await.status(),
        StatusCode::NOT_FOUND
    );
    let (status, error) = server
        .delete(&format!("/api/saves/{first_id}"), &token)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "save_not_found");

    let (status, list) = server
        .get(&format!("/api/games/{game_id}/saves"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().expect("array").len(), 1);

    server.stop().await;
}

#[tokio::test]
async fn snapshots_are_isolated_per_user_but_admins_can_manage() {
    let server = TestServer::start().await;
    let admin = server.admin_token();
    let game_id = server.insert_game("Celeste").await;
    let bob_id = server.create_user(&admin, "bob", "user").await;
    let bob = server.login("bob").await;

    let (status, bob_snapshot) = server
        .upload(game_id, &bob, "bob.zip", b"bob save".to_vec(), Some("bob"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bob_snapshot}");
    let bob_snapshot_id = bob_snapshot["id"].as_i64().expect("id");

    let (status, admin_snapshot) = server
        .upload(
            game_id,
            &admin,
            "admin.zip",
            b"admin save".to_vec(),
            Some("admin"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{admin_snapshot}");
    let admin_snapshot_id = admin_snapshot["id"].as_i64().expect("id");

    // Listing always stays scoped to the caller.
    let (status, list) = server
        .get(&format!("/api/games/{game_id}/saves"), Some(&bob))
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], bob_snapshot_id);

    let (status, list) = server
        .get(&format!("/api/games/{game_id}/saves"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], admin_snapshot_id);

    // Bob cannot see, download or delete the admin's snapshot.
    assert_eq!(
        server.download(admin_snapshot_id, &bob).await.status(),
        StatusCode::NOT_FOUND
    );
    let (status, error) = server
        .delete(&format!("/api/saves/{admin_snapshot_id}"), &bob)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "save_not_found");

    // The admin can download and delete Bob's snapshot.
    let response = server.download(bob_snapshot_id, &admin).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.expect("bytes").as_ref(), b"bob save");

    let (status, _) = server
        .delete(&format!("/api/saves/{bob_snapshot_id}"), &admin)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        server.download(bob_snapshot_id, &bob).await.status(),
        StatusCode::NOT_FOUND
    );

    // Deleting an account cascades to its snapshot rows.
    let (status, _) = server
        .upload(game_id, &bob, "again.zip", b"again".to_vec(), None)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(server.save_count(bob_id).await, 1);
    let (status, _) = server.delete(&format!("/api/users/{bob_id}"), &admin).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(server.save_count(bob_id).await, 0);

    server.stop().await;
}

#[tokio::test]
async fn upload_validation_limits_and_filename_sanitizing() {
    let server = TestServer::start_with(|config| config.saves_max_bytes = 64 * 1024).await;
    let token = server.admin_token();
    let admin_id = server.current_user_id(&token).await;
    let game_id = server.insert_game("Celeste").await;

    // Authentication is required.
    let (status, error) = server
        .get(&format!("/api/games/{game_id}/saves"), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "unauthorized");
    let form = Form::new().part("file", Part::bytes(b"data".to_vec()).file_name("x.zip"));
    let (status, _) = server.send_form(game_id, None, form).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        server.download(1, "").await.status(),
        StatusCode::UNAUTHORIZED
    );

    // Missing game.
    let (status, error) = server.get("/api/games/999999/saves", Some(&token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "game_not_found");
    let (status, error) = server
        .upload(999999, &token, "save.zip", b"data".to_vec(), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "game_not_found");

    // Missing file field.
    let form = Form::new().text("label", "no file");
    let (status, error) = server.send_form(game_id, Some(&token), form).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_file");

    // File part without a filename.
    let form = Form::new().part("file", Part::bytes(b"data".to_vec()));
    let (status, error) = server.send_form(game_id, Some(&token), form).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_file");

    // Empty archive.
    let (status, error) = server
        .upload(game_id, &token, "empty.zip", Vec::new(), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_file");

    // Over the configured limit.
    let (status, error) = server
        .upload(game_id, &token, "big.zip", vec![0_u8; 70 * 1024], None)
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{error}");
    assert_eq!(error["error"]["code"], "save_too_large");

    // Exactly at the limit is accepted.
    let (status, snapshot) = server
        .upload(game_id, &token, "exact.zip", vec![7_u8; 64 * 1024], None)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{snapshot}");
    assert_eq!(snapshot["size"], 64 * 1024);

    // Path traversal is reduced to a plain file name.
    let (status, snapshot) = server
        .upload(
            game_id,
            &token,
            "../../etc/passwd.zip",
            b"trav".to_vec(),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{snapshot}");
    assert_eq!(snapshot["fileName"], "passwd.zip");
    let snapshot_id = snapshot["id"].as_i64().expect("id");
    let expected = server.snapshot_path(admin_id, game_id, snapshot_id, "passwd.zip");
    assert!(expected.is_file(), "expected {}", expected.display());
    assert!(!server.temp.path().join("etc").exists());

    let (status, snapshot) = server
        .upload(
            game_id,
            &token,
            "..\\..\\windows\\system32\\evil.zip",
            b"trav".to_vec(),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{snapshot}");
    assert_eq!(snapshot["fileName"], "evil.zip");

    // Unknown snapshot ids are 404.
    assert_eq!(
        server.download(999999, &token).await.status(),
        StatusCode::NOT_FOUND
    );

    server.stop().await;
}

#[tokio::test]
async fn save_uploads_allow_more_than_the_json_body_limit() {
    const LIMIT: u64 = 12 * 1024 * 1024;
    let server = TestServer::start_with(|config| config.saves_max_bytes = LIMIT).await;
    let token = server.admin_token();
    let game_id = server.insert_game("Big Save").await;

    // 11 MiB is above the global 10 MiB JSON body limit.
    let archive = vec![0x5A_u8; 11 * 1024 * 1024];
    let (status, snapshot) = server
        .upload(game_id, &token, "big.zip", archive.clone(), Some("big"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{snapshot}");
    assert_eq!(snapshot["size"], archive.len());
    assert_eq!(snapshot["sha256"], sha256_hex(&archive));

    // JSON routes keep the 10 MiB limit: the login extractor rejects the body
    // before credentials are checked.
    let (status, error) = server
        .post(
            "/api/auth/login",
            None,
            json!({"username": "x".repeat(11 * 1024 * 1024), "password": ADMIN_PASSWORD}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_body");

    server.stop().await;
}

#[tokio::test]
async fn auto_snapshots_are_metadata_tagged_idempotent_and_retained() {
    let server = TestServer::start().await;
    let token = server.admin_token();
    let user_id = server.current_user_id(&token).await;
    let game_id = server.insert_game("Save Sync Game").await;

    // A manual snapshot must survive every automatic prune.
    let (status, manual) = server
        .upload(
            game_id,
            &token,
            "manual.zip",
            b"manual-save".to_vec(),
            Some("before"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{manual}");
    assert_eq!(manual["source"], "manual");
    assert_eq!(manual["deviceId"], Value::Null);

    // Twelve distinct automatic snapshots, one per second: retention keeps 10.
    for index in 0..12 {
        let (status, body) = server
            .upload_as(
                game_id,
                &token,
                "auto.zip",
                format!("auto-save-{index}").into_bytes(),
                "auto",
                Some("device-a"),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["source"], "auto");
        assert_eq!(body["deviceId"], "device-a");
    }
    let (status, list) = server
        .get(&format!("/api/games/{game_id}/saves"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list.as_array().expect("array");
    assert_eq!(
        items.iter().filter(|item| item["source"] == "auto").count(),
        10,
        "{list}"
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["source"] == "manual")
            .count(),
        1,
        "manual snapshots are never pruned: {list}"
    );

    // Re-uploading unchanged bytes as auto is idempotent and returns the row.
    let (status, again) = server
        .upload_as(
            game_id,
            &token,
            "auto.zip",
            b"auto-save-11".to_vec(),
            "auto",
            Some("device-a"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["source"], "auto");
    let after = server.save_count(user_id).await;
    assert_eq!(after, 11, "11 rows: 10 auto + 1 manual");

    // The auto snapshot is downloadable like any other.
    let snapshot_id = again["id"].as_i64().expect("snapshot id");
    let response = server.download(snapshot_id, &token).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.expect("bytes").as_ref(),
        b"auto-save-11"
    );

    server.stop().await;
}

#[tokio::test]
async fn auto_upload_rejects_invalid_sources_and_device_ids() {
    let server = TestServer::start().await;
    let token = server.admin_token();
    let game_id = server.insert_game("Save Validation Game").await;

    let (status, error) = server
        .upload_as(
            game_id,
            &token,
            "save.zip",
            b"data".to_vec(),
            "scheduled",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_source");

    let long_device = "d".repeat(200);
    let (status, error) = server
        .upload_as(
            game_id,
            &token,
            "save.zip",
            b"data".to_vec(),
            "auto",
            Some(&long_device),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_device_id");

    server.stop().await;
}

fn stamp(hours_from_now: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::hours(hours_from_now))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// #296 acceptance: a 3 h gap with both sides modified keeps both archives,
/// offers the choice, and choosing a side keeps the other as a restore point.
#[tokio::test]
async fn both_sides_modified_keep_both_and_offer_a_choice() {
    let server = TestServer::start().await;
    let token = server.admin_token();
    let user_id = server.current_user_id(&token).await;
    let game_id = server.insert_game("Multi Device").await;

    // Remote played three hours ago and produced a 200-byte archive.
    let (status, existing) = server
        .upload_meta(
            game_id,
            &token,
            b"a".repeat(200),
            "steam-deck",
            "Steam Deck",
            &stamp(-3),
            &stamp(0),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{existing}");
    let existing_id = existing["id"].as_i64().expect("id");

    // This laptop played elsewhere: a different, smaller save an hour ago.
    // Neither side can be a descendant of the other, so the policy refuses to
    // overwrite and keeps the upload aside.
    let (status, candidate) = server
        .upload_meta(
            game_id,
            &token,
            b"b".repeat(150),
            "laptop",
            "Laptop",
            &stamp(-1),
            &stamp(0),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{candidate}");
    let candidate_id = candidate["id"].as_i64().expect("id");
    assert_eq!(candidate["conflictOf"], existing_id);
    let label = candidate["label"].as_str().expect("label");
    assert!(
        label.starts_with("save (conflict Laptop "),
        "candidate label: {label}"
    );
    assert!(candidate["clockOffsetSecs"].as_i64().is_some());

    // Nothing was deleted: both archives are listed and downloadable.
    assert_eq!(server.save_count(user_id).await, 2);
    let (status, list) = server
        .get(&format!("/api/games/{game_id}/saves"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list.as_array().expect("array").len(), 2);

    // The conflict is offered with both candidates side by side.
    let (status, conflicts) = server
        .get(
            &format!("/api/games/{game_id}/saves/conflicts"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{conflicts}");
    let open = conflicts.as_array().expect("array");
    assert_eq!(open.len(), 1, "{conflicts}");
    assert_eq!(open[0]["status"], "open");
    assert_eq!(open[0]["reason"], "both_changed");
    assert_eq!(open[0]["existing"]["id"], existing_id);
    assert_eq!(open[0]["existing"]["size"], 200);
    assert_eq!(open[0]["existing"]["deviceId"], "steam-deck");
    assert_eq!(open[0]["candidate"]["id"], candidate_id);
    assert_eq!(open[0]["candidate"]["size"], 150);
    assert_eq!(open[0]["candidate"]["deviceId"], "laptop");
    assert!(open[0]["existing"]["createdAt"].as_str().is_some());
    let conflict_id = open[0]["id"].as_i64().expect("conflict id");

    // Choosing the server copy keeps the laptop upload as a restore point.
    let (status, resolved) = server
        .post(
            &format!("/api/games/{game_id}/saves/conflicts/{conflict_id}/resolve"),
            Some(&token),
            json!({"strategy": "keep_remote"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved["strategy"], "keep_remote");
    assert_eq!(resolved["existing"]["id"], existing_id);
    assert_eq!(resolved["existing"]["conflictOf"], Value::Null);
    assert_eq!(resolved["candidate"]["conflictOf"], existing_id);
    assert!(resolved["candidate"]["label"]
        .as_str()
        .expect("label")
        .starts_with("save (conflict Laptop"));

    // Resolving twice is a conflict, not a silent second mutation.
    let (status, error) = server
        .post(
            &format!("/api/games/{game_id}/saves/conflicts/{conflict_id}/resolve"),
            Some(&token),
            json!({"strategy": "keep_local"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "conflict_resolved");

    // Both archives survive the resolution.
    assert_eq!(server.save_count(user_id).await, 2);
    assert_eq!(
        server.download(existing_id, &token).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        server.download(candidate_id, &token).await.status(),
        StatusCode::OK
    );

    // The other choice: keeping local promotes the candidate and demotes the
    // server copy, again without deleting it.
    let second_game = server.insert_game("Multi Device Two").await;
    let (_, old) = server
        .upload_meta(
            second_game,
            &token,
            b"c".repeat(220),
            "steam-deck",
            "Steam Deck",
            &stamp(-3),
            &stamp(0),
            None,
        )
        .await;
    let old_id = old["id"].as_i64().expect("id");
    let (_, local) = server
        .upload_meta(
            second_game,
            &token,
            b"d".repeat(140),
            "laptop",
            "Laptop",
            &stamp(-1),
            &stamp(0),
            None,
        )
        .await;
    let local_id = local["id"].as_i64().expect("id");
    let (status, conflicts) = server
        .get(
            &format!("/api/games/{second_game}/saves/conflicts"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{conflicts}");
    let conflict_id = conflicts[0]["id"].as_i64().expect("conflict id");
    let (status, resolved) = server
        .post(
            &format!("/api/games/{second_game}/saves/conflicts/{conflict_id}/resolve"),
            Some(&token),
            json!({"strategy": "keep_local"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved["candidate"]["id"], local_id);
    assert_eq!(resolved["candidate"]["conflictOf"], Value::Null);
    assert_eq!(resolved["candidate"]["label"], "save");
    assert_eq!(resolved["existing"]["id"], old_id);
    assert_eq!(resolved["existing"]["conflictOf"], local_id);
    assert!(resolved["existing"]["label"]
        .as_str()
        .expect("label")
        .starts_with("save (conflict"));

    // The promoted save is now the newest non-conflict row on the server.
    let (_, list) = server
        .get(&format!("/api/games/{second_game}/saves"), Some(&token))
        .await;
    let latest = list
        .as_array()
        .expect("array")
        .iter()
        .find(|row| row["conflictOf"].is_null())
        .expect("latest");
    assert_eq!(latest["id"], local_id);
    assert_eq!(server.save_count(user_id).await, 4);
    assert_eq!(
        server.download(old_id, &token).await.status(),
        StatusCode::OK
    );

    server.stop().await;
}

/// #296 acceptance: a device whose clock is two hours ahead never wins
/// silently; the server turns the ambiguity into a conflict.
#[tokio::test]
async fn a_device_two_hours_ahead_does_not_win() {
    let server = TestServer::start().await;
    let token = server.admin_token();
    let game_id = server.insert_game("Skewed Clock").await;

    let (status, existing) = server
        .upload_meta(
            game_id,
            &token,
            b"a".repeat(100),
            "steam-deck",
            "Steam Deck",
            &stamp(-1),
            &stamp(0),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{existing}");
    let existing_id = existing["id"].as_i64().expect("id");

    // Larger and "newer", but only because this device's clock runs +2 h.
    let (status, candidate) = server
        .upload_meta(
            game_id,
            &token,
            b"b".repeat(300),
            "laptop",
            "Laptop",
            &stamp(2),
            &stamp(2),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{candidate}");
    assert_eq!(candidate["conflictOf"], existing_id);
    let offset = candidate["clockOffsetSecs"].as_i64().expect("clock offset");
    assert!(offset >= 7_000, "expected a +2 h skew, got {offset}");

    let (status, conflicts) = server
        .get(
            &format!("/api/games/{game_id}/saves/conflicts"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{conflicts}");
    assert_eq!(conflicts[0]["reason"], "clock_skew", "{conflicts}");
    assert_eq!(conflicts[0]["existing"]["id"], existing_id);
    assert_eq!(conflicts[0]["candidate"]["id"], candidate["id"]);

    server.stop().await;
}

/// #296 override: "always prefer this device" skips the question in both
/// directions, and still keeps the other archive.
#[tokio::test]
async fn prefer_device_policy_skips_the_question() {
    let server = TestServer::start().await;
    let token = server.admin_token();
    let user_id = server.current_user_id(&token).await;
    let game_id = server.insert_game("Preferred Device").await;

    let (status, policy) = server
        .request(
            reqwest::Method::PUT,
            &format!("/api/games/{game_id}/saves/policy"),
            Some(&token),
            Some(json!({"policy": "prefer_device", "deviceId": "laptop"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{policy}");
    assert_eq!(policy["policy"], "prefer_device");
    assert_eq!(policy["deviceId"], "laptop");

    // Another device's save is the current one.
    let (status, other) = server
        .upload_meta(
            game_id,
            &token,
            b"a".repeat(200),
            "steam-deck",
            "Steam Deck",
            &stamp(0),
            &stamp(0),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{other}");
    let _other_id = other["id"].as_i64().expect("id");

    // The preferred device wins even though its save is older and smaller.
    let (status, preferred) = server
        .upload_meta(
            game_id,
            &token,
            b"b".repeat(120),
            "laptop",
            "Laptop",
            &stamp(-2),
            &stamp(0),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{preferred}");
    assert_eq!(preferred["conflictOf"], Value::Null);
    let preferred_id = preferred["id"].as_i64().expect("id");
    let (_, conflicts) = server
        .get(
            &format!("/api/games/{game_id}/saves/conflicts"),
            Some(&token),
        )
        .await;
    assert!(
        conflicts.as_array().expect("array").is_empty(),
        "{conflicts}"
    );

    // A third device never displaces the preferred one, but its archive is
    // still kept as a restore point and no question is raised.
    let (status, third) = server
        .upload_meta(
            game_id,
            &token,
            b"c".repeat(400),
            "console",
            "Console",
            &stamp(1),
            &stamp(0),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{third}");
    assert_eq!(third["conflictOf"], preferred_id);
    let (_, conflicts) = server
        .get(
            &format!("/api/games/{game_id}/saves/conflicts"),
            Some(&token),
        )
        .await;
    assert!(
        conflicts.as_array().expect("array").is_empty(),
        "{conflicts}"
    );
    assert_eq!(server.save_count(user_id).await, 3);

    // Back to asking: the policy is removable per game.
    let (status, policy) = server
        .request(
            reqwest::Method::PUT,
            &format!("/api/games/{game_id}/saves/policy"),
            Some(&token),
            Some(json!({"policy": "ask"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{policy}");
    assert_eq!(policy["policy"], "ask");
    assert_eq!(policy["deviceId"], Value::Null);

    server.stop().await;
}

/// #296 migration round-trip: the new columns and tables survive an insert,
/// a look-up, a parent deletion (`ON DELETE SET NULL`) and a user cascade.
#[tokio::test]
async fn save_conflict_schema_roundtrips() {
    let server = TestServer::start().await;
    let token = server.admin_token();
    let user_id = server.current_user_id(&token).await;
    let game_id = server.insert_game("Schema Round Trip").await;
    let (_, snapshot) = server
        .upload_meta(
            game_id,
            &token,
            b"a".repeat(64),
            "steam-deck",
            "Steam Deck",
            &stamp(0),
            &stamp(0),
            Some(7),
        )
        .await;
    let snapshot_id = snapshot["id"].as_i64().expect("id");
    assert_eq!(snapshot["saveSeq"], 7);

    let pool = server.direct_pool().await;
    let columns: i64 = ludex_server::db::query_scalar::<i64>(
        "SELECT COUNT(*) FROM pragma_table_info('save_snapshots')
         WHERE name IN ('client_mtime', 'clock_offset_secs', 'save_seq', 'conflict_of')",
    )
    .fetch_one(&pool)
    .await
    .expect("columns");
    assert_eq!(columns, 4);

    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let second_id = ludex_server::db::query(
        "INSERT INTO save_snapshots
            (user_id, game_id, version_id, label, file_name, size, sha256, created_at,
             source, device_id, client_mtime, clock_offset_secs, save_seq, conflict_of)
         VALUES (?, ?, NULL, 'save (conflict laptop)', 'b.zip', 32, 'deadbeef', ?, 'auto',
                 'laptop', ?, 0, NULL, ?)",
    )
    .bind(user_id)
    .bind(game_id)
    .bind(&now)
    .bind(&now)
    .bind(snapshot_id)
    .returning_id(&pool)
    .await
    .expect("second snapshot");

    let row = ludex_server::db::query(
        "SELECT conflict_of, save_seq, clock_offset_secs, client_mtime
         FROM save_snapshots WHERE id = ?",
    )
    .bind(second_id)
    .fetch_one(&pool)
    .await
    .expect("row");
    let conflict_of: Option<i64> = row.try_get("conflict_of").expect("conflict_of");
    assert_eq!(conflict_of, Some(snapshot_id));
    let save_seq: Option<i64> = row.try_get("save_seq").expect("save_seq");
    assert_eq!(save_seq, None);
    let client_mtime: Option<String> = row.try_get("client_mtime").expect("mtime");
    assert!(client_mtime.is_some());

    ludex_server::db::query(
        "INSERT INTO save_conflicts
            (user_id, game_id, existing_snapshot_id, candidate_snapshot_id, reason,
             status, detected_at)
         VALUES (?, ?, ?, ?, 'both_changed', 'open', ?)",
    )
    .bind(user_id)
    .bind(game_id)
    .bind(snapshot_id)
    .bind(second_id)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("conflict row");
    ludex_server::db::query(
        "INSERT INTO save_sync_policies (user_id, game_id, prefer_device_id, updated_at)
         VALUES (?, ?, 'laptop', ?)
         ON CONFLICT (user_id, game_id) DO UPDATE SET
             prefer_device_id = excluded.prefer_device_id",
    )
    .bind(user_id)
    .bind(game_id)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("policy row");
    ludex_server::db::query(
        "INSERT INTO save_devices
            (user_id, device_id, label, clock_offset_secs, first_seen_at, last_seen_at)
         VALUES (?, 'laptop', 'Laptop', 7200, ?, ?)",
    )
    .bind(user_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("device row");

    // Deleting the parent snapshot clears the reference, never the child.
    ludex_server::db::query("DELETE FROM save_snapshots WHERE id = ?")
        .bind(snapshot_id)
        .execute(&pool)
        .await
        .expect("delete parent");
    let conflict_of: Option<i64> =
        ludex_server::db::query("SELECT conflict_of FROM save_snapshots WHERE id = ?")
            .bind(second_id)
            .fetch_one(&pool)
            .await
            .map(|row| row.get("conflict_of"))
            .expect("surviving child");
    assert_eq!(conflict_of, None);

    // Deleting the account cascades every new table.
    ludex_server::db::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("delete user");
    for table in [
        "save_snapshots",
        "save_conflicts",
        "save_sync_policies",
        "save_devices",
    ] {
        let count: i64 = ludex_server::db::query_scalar::<i64>(&format!(
            "SELECT COUNT(*) FROM {table} WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("count");
        assert_eq!(count, 0, "{table} kept rows after the user deletion");
    }
    pool.close().await;

    server.stop().await;
}
