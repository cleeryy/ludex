//! Account avatar upload tests (#183).

use std::path::PathBuf;
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;

const PASSWORD: &str = "password123";

/// A real 1×1 RGBA PNG.
const PNG_1X1: &str = "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c63f8cfc0f01f00050001ff89993d1d0000000049454e44ae426082";
/// A real 2×2 lossy WebP.
const WEBP_2X2: &str = "524946463e000000574542505650382032000000d001009d012a0200020001402625a00274ba01f80003b000fee9221ffbcf9fb9f3f73e7fd19fff94fdf238fe471ffca04000";

struct TestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    /// Keeps the temporary library/data/media tree alive for the test.
    _temp: TempDir,
    media_dir: PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let config = Config::for_local(games_dir, data_dir, media_dir.clone());
        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("client"),
            handle: Some(handle),
            _temp: temp,
            media_dir,
        }
    }

    fn token(&self) -> String {
        self.handle.as_ref().expect("handle").token.clone()
    }

    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
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

    async fn upload(
        &self,
        token: Option<&str>,
        bytes: Vec<u8>,
        file_name: &str,
    ) -> (StatusCode, Value) {
        let form = Form::new().part("file", Part::bytes(bytes).file_name(file_name.to_string()));
        let mut request = self
            .client
            .post(format!("{}/api/account/avatar", self.url))
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

    async fn delete_avatar(&self, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = self
            .client
            .delete(format!("{}/api/account/avatar", self.url));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.expect("delete");
        let status = response.status();
        let text = response.text().await.expect("body");
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        (status, value)
    }

    async fn media_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(&self.media_dir).await else {
            return names;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        names
    }

    fn media_path(&self, url: &str) -> PathBuf {
        let name = url
            .rsplit('/')
            .next()
            .expect("media name")
            .split('?')
            .next()
            .expect("media name without query");
        self.media_dir.join(name)
    }
}

fn png_bytes() -> Vec<u8> {
    hex::decode(PNG_1X1).expect("png")
}

fn webp_bytes() -> Vec<u8> {
    hex::decode(WEBP_2X2).expect("webp")
}

/// PNG magic plus a 5000-pixel-wide IHDR. The CRC is stale on purpose: the
/// server only reads the header dimensions.
fn oversized_png() -> Vec<u8> {
    let mut bytes = png_bytes();
    bytes[16..20].copy_from_slice(&5000_u32.to_be_bytes());
    bytes
}

fn avatar_name(url: &str) -> &str {
    url.rsplit('/').next().expect("media name")
}

#[tokio::test]
async fn uploads_a_png_and_exposes_the_avatar_url() {
    let server = TestServer::start().await;
    let token = server.token();

    let (status, body) = server.upload(Some(&token), png_bytes(), "logo.png").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = body["avatarUrl"].as_str().expect("avatarUrl").to_string();
    assert!(
        avatar_name(&url).starts_with("avatar-1-"),
        "unexpected name {url}"
    );
    assert!(avatar_name(&url).ends_with(".png"), "{url}");
    assert!(body["username"].is_string(), "{body}");

    // `/api/auth/me` carries the refreshed serializer.
    let (status, me) = server
        .request(reqwest::Method::GET, "/api/auth/me", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["user"]["avatarUrl"], url);

    // The media route serves the file publicly.
    let response = server
        .client
        .get(format!("{}{}", server.url, url))
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
    assert!(server.media_path(&url).exists());

    server.stop().await;
}

#[tokio::test]
async fn uploads_a_webp_and_replaces_the_previous_file() {
    let server = TestServer::start().await;
    let token = server.token();

    let (status, first) = server.upload(Some(&token), png_bytes(), "logo.png").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let first_url = first["avatarUrl"].as_str().expect("url").to_string();
    let first_path = server.media_path(&first_url);
    assert!(first_path.exists());

    let (status, second) = server.upload(Some(&token), webp_bytes(), "logo.webp").await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let second_url = second["avatarUrl"].as_str().expect("url").to_string();
    assert!(avatar_name(&second_url).ends_with(".webp"), "{second_url}");
    assert_ne!(first_url, second_url);

    // Only the new avatar remains; the old managed file is gone.
    assert!(!first_path.exists(), "old avatar was not removed");
    assert!(server.media_path(&second_url).exists());
    let names = server.media_names().await;
    let avatars: Vec<_> = names
        .iter()
        .filter(|name| name.starts_with("avatar-1-"))
        .collect();
    assert_eq!(avatars.len(), 1, "{names:?}");

    server.stop().await;
}

#[tokio::test]
async fn rejects_wrong_types_oversize_and_mismatched_extensions() {
    let server = TestServer::start().await;
    let token = server.token();

    // GIF is not an avatar format.
    let (status, error) = server
        .upload(Some(&token), b"GIF89a\x01\x00\x01\x00".to_vec(), "logo.gif")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_avatar_type");

    // SVG and HTML are rejected by the magic-byte sniff.
    let (status, error) = server
        .upload(
            Some(&token),
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec(),
            "logo.svg",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_avatar_type");

    let (status, error) = server
        .upload(
            Some(&token),
            b"<html><body><script>alert(1)</script></body></html>".to_vec(),
            "logo.png",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_avatar_type");

    // The extension must agree with the bytes.
    let (status, error) = server.upload(Some(&token), png_bytes(), "logo.webp").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_avatar_type");

    // 2 MiB + 1 byte is refused before the file is written.
    let mut oversized = png_bytes();
    oversized.resize(2 * 1024 * 1024 + 1, 0);
    let (status, error) = server.upload(Some(&token), oversized, "huge.png").await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{error}");
    assert_eq!(error["error"]["code"], "avatar_too_large");

    // The header dimensions are capped.
    let (status, error) = server
        .upload(Some(&token), oversized_png(), "wide.png")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_avatar_dimensions");

    // Nothing was stored.
    assert!(
        server
            .media_names()
            .await
            .iter()
            .all(|name| !name.starts_with("avatar-")),
        "a rejected upload leaked a file"
    );

    server.stop().await;
}

#[tokio::test]
async fn delete_restores_the_initials_and_keeps_other_media() {
    let server = TestServer::start().await;
    let token = server.token();

    // A game artwork file shares the media directory and must survive.
    std::fs::create_dir_all(&server.media_dir).expect("media dir");
    std::fs::write(server.media_dir.join("custom-1-cover.png"), b"art").expect("write artwork");

    let (status, body) = server.upload(Some(&token), png_bytes(), "logo.png").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let url = body["avatarUrl"].as_str().expect("url").to_string();
    let path = server.media_path(&url);
    assert!(path.exists());

    let (status, body) = server.delete_avatar(Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["avatarUrl"].is_null(), "{body}");
    assert!(!path.exists(), "avatar file was not removed");
    assert!(server.media_dir.join("custom-1-cover.png").exists());

    let (status, me) = server
        .request(reqwest::Method::GET, "/api/auth/me", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert!(me["user"]["avatarUrl"].is_null(), "{me}");

    // Deleting again is a no-op that still returns the account.
    let (status, body) = server.delete_avatar(Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["avatarUrl"].is_null());

    server.stop().await;
}

#[tokio::test]
async fn avatar_routes_require_authentication() {
    let server = TestServer::start().await;

    let (status, error) = server.upload(None, png_bytes(), "logo.png").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");

    let (status, error) = server.delete_avatar(None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");

    server.stop().await;
}

#[tokio::test]
async fn avatar_url_is_scoped_to_the_uploader() {
    let server = TestServer::start().await;
    let admin = server.token();

    let (status, created) = server
        .request(
            reqwest::Method::POST,
            "/api/users",
            Some(&admin),
            Some(json!({
                "username": "player",
                "password": PASSWORD,
                "role": "user"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let (status, login) = server
        .request(
            reqwest::Method::POST,
            "/api/auth/login",
            None,
            Some(json!({ "username": "player", "password": PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let player = login["token"].as_str().expect("token");

    // A non-admin account manages its own avatar.
    let (status, body) = server.upload(Some(player), webp_bytes(), "me.webp").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], 2);
    assert!(avatar_name(body["avatarUrl"].as_str().expect("url")).starts_with("avatar-2-"));

    // The administrator keeps no avatar.
    let (status, me) = server
        .request(reqwest::Method::GET, "/api/auth/me", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert!(me["user"]["avatarUrl"].is_null(), "{me}");

    server.stop().await;
}
