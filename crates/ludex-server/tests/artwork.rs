//! Custom artwork upload tests (#38).

use std::path::PathBuf;
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;

const PASSWORD: &str = "password123";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

struct TestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    /// Keeps the temporary library/data/media tree alive for the test.
    _temp: TempDir,
    data_dir: PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let mut config = Config::for_local(games_dir, data_dir.clone(), media_dir);
        config.bind = "127.0.0.1:0".parse().expect("bind");
        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            client: Client::builder().no_proxy().build().expect("client"),
            handle: Some(handle),
            _temp: temp,
            data_dir,
        }
    }

    fn token(&self) -> String {
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

    async fn upload(
        &self,
        game_id: i64,
        token: &str,
        kind: &str,
        bytes: Vec<u8>,
        file_name: &str,
    ) -> (StatusCode, Value) {
        let form = Form::new()
            .text("kind", kind.to_string())
            .part("file", Part::bytes(bytes).file_name(file_name.to_string()));
        let response = self
            .client
            .post(format!("{}/api/games/{game_id}/artwork", self.url))
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await
            .expect("upload");
        let status = response.status();
        let text = response.text().await.expect("body");
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        (status, value)
    }

    async fn login(&self, username: &str) -> String {
        let (status, body) = self
            .request(
                reqwest::Method::POST,
                "/api/auth/login",
                None,
                Some(json!({"username": username, "password": PASSWORD})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().expect("token").to_string()
    }

    async fn create_user(&self, username: &str, role: &str) -> String {
        let token = self.token();
        let (status, body) = self
            .request(
                reqwest::Method::POST,
                "/api/users",
                Some(&token),
                Some(json!({"username": username, "password": PASSWORD, "role": role})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        self.login(username).await
    }

    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn png_bytes() -> Vec<u8> {
    let mut bytes = PNG.to_vec();
    bytes.extend_from_slice(&[0_u8; 128]);
    bytes
}

#[tokio::test]
async fn editors_can_replace_and_reset_cover_art() {
    let server = TestServer::start().await;
    let game_id = server.insert_game("Artwork Game").await;
    let editor = server.create_user("artist", "editor").await;

    let (status, body) = server
        .upload(game_id, &editor, "cover", png_bytes(), "cover.png")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "cover");
    assert_eq!(body["format"], "png");
    let url = body["url"].as_str().expect("url");
    assert!(url.starts_with("/api/media/custom-"), "{url}");
    assert!(body["sha256"].as_str().is_some_and(|hash| hash.len() == 64));

    // Served publicly and cache-busted by the upload name.
    let response = server
        .client
        .get(format!("{}{url}", server.url))
        .send()
        .await
        .expect("media");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );

    // The game detail points at the custom art.
    let (status, detail) = server
        .request(
            reqwest::Method::GET,
            &format!("/api/games/{game_id}"),
            Some(&editor),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["coverUrl"], url);

    // Reset clears the column and removes the custom file.
    let (status, _) = server
        .request(
            reqwest::Method::DELETE,
            &format!("/api/games/{game_id}/artwork?kind=cover"),
            Some(&editor),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let response = server
        .client
        .get(format!("{}{url}", server.url))
        .send()
        .await
        .expect("media");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    server.stop().await;
}

#[tokio::test]
async fn artwork_upload_is_validated_and_role_gated() {
    let server = TestServer::start().await;
    let game_id = server.insert_game("Artwork Validation").await;
    let admin = server.token();
    let user = server.create_user("player", "user").await;

    let (status, error) = server
        .upload(game_id, &user, "cover", png_bytes(), "cover.png")
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");

    let (status, error) = server
        .upload(game_id, &admin, "poster", png_bytes(), "cover.png")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_kind");

    let (status, error) = server
        .upload(
            game_id,
            &admin,
            "cover",
            b"<svg></svg>".to_vec(),
            "cover.svg",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_image");

    let oversized = vec![0x42_u8; 8 * 1024 * 1024 + 1];
    let (status, error) = server
        .upload(game_id, &admin, "cover", oversized, "huge.png")
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{error}");

    let (status, error) = server
        .upload(999_999, &admin, "cover", png_bytes(), "cover.png")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");

    server.stop().await;
}
