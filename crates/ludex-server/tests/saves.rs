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
