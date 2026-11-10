//! Parental-control tests (#39).

use std::path::PathBuf;
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;

const PASSWORD: &str = "password123";

struct TestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
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

    fn admin(&self) -> String {
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

    async fn insert_game(&self, title: &str, age_rating: Option<i64>) -> i64 {
        let url = format!(
            "sqlite://{}?mode=rwc",
            self.data_dir.join("ludex.db").display()
        );
        let pool = ludex_server::db::connect(&url).await.expect("db");
        let now = chrono::Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO games (title, sort_title, age_rating, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(title)
        .bind(title.to_ascii_lowercase())
        .bind(age_rating)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("insert game");
        let id = result.last_insert_rowid();
        sqlx::query(
            "INSERT INTO game_versions (game_id, root_id, file_path, file_name, size, indexed_at)
             VALUES (?, 1, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(format!("{title}.zip"))
        .bind(format!("{title}.zip"))
        .bind(1024 * 1024_i64)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("insert version");
        pool.close().await;
        id
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

    async fn create_child(&self, age_limit: Option<i64>) -> (i64, String) {
        let admin = self.admin();
        let (status, body) = self
            .request(
                reqwest::Method::POST,
                "/api/users",
                Some(&admin),
                Some(json!({"username": "kid", "password": PASSWORD, "role": "user"})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = body["id"].as_i64().expect("user id");
        let (status, updated) = self
            .request(
                reqwest::Method::PATCH,
                &format!("/api/users/{id}"),
                Some(&admin),
                Some(json!({"ageLimit": age_limit})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        match age_limit {
            Some(limit) => assert_eq!(updated["ageLimit"], limit),
            None => assert_eq!(updated["ageLimit"], Value::Null),
        }
        (id, self.login("kid").await)
    }

    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn age_limits_hide_games_and_block_downloads() {
    let server = TestServer::start().await;
    let child_game = server.insert_game("Kart Fun", Some(7)).await;
    let adult_game = server.insert_game("Blood Shooter", Some(18)).await;
    let unrated_game = server.insert_game("Mystery Project", None).await;
    let (user_id, token) = server.create_child(Some(12)).await;

    // Lists: the 18+ game is absent, the child-safe and unrated ones remain.
    let (status, list) = server
        .request(reqwest::Method::GET, "/api/games", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let titles: Vec<&str> = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["title"].as_str())
        .collect();
    assert!(titles.contains(&"Kart Fun"), "{titles:?}");
    assert!(titles.contains(&"Mystery Project"), "{titles:?}");
    assert!(!titles.contains(&"Blood Shooter"), "{titles:?}");

    // Detail: 404 rather than 403 so the library cannot be enumerated.
    let (status, error) = server
        .request(
            reqwest::Method::GET,
            &format!("/api/games/{adult_game}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    assert_eq!(error["error"]["code"], "game_not_found");

    let (status, _) = server
        .request(
            reqwest::Method::GET,
            &format!("/api/games/{child_game}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Download tickets are refused for restricted games.
    let (status, error) = server
        .request(
            reqwest::Method::POST,
            &format!("/api/games/{adult_game}/download-ticket"),
            Some(&token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");

    // Suggestions never surface a restricted title.
    let (status, suggest) = server
        .request(
            reqwest::Method::GET,
            "/api/games/suggest?q=Blood",
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{suggest}");
    assert_eq!(suggest["suggestions"].as_array().map(Vec::len), Some(0));

    // An administrator can raise or clear the limit.
    let admin = server.admin();
    let (status, updated) = server
        .request(
            reqwest::Method::PATCH,
            &format!("/api/users/{user_id}"),
            Some(&admin),
            Some(json!({"ageLimit": 18})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["ageLimit"], 18);
    let (status, _) = server
        .request(
            reqwest::Method::GET,
            &format!("/api/games/{adult_game}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Clearing the limit restores the full library.
    let (status, cleared) = server
        .request(
            reqwest::Method::PATCH,
            &format!("/api/users/{user_id}"),
            Some(&admin),
            Some(json!({"ageLimit": null})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["ageLimit"], Value::Null);
    let _ = unrated_game;

    server.stop().await;
}

#[tokio::test]
async fn invalid_age_limits_are_rejected() {
    let server = TestServer::start().await;
    let admin = server.admin();
    let (status, body) = server
        .request(
            reqwest::Method::POST,
            "/api/users",
            Some(&admin),
            Some(json!({"username": "kid", "password": PASSWORD, "role": "user"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_i64().expect("id");
    let (status, error) = server
        .request(
            reqwest::Method::PATCH,
            &format!("/api/users/{id}"),
            Some(&admin),
            Some(json!({"ageLimit": 99})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_age_limit");
    server.stop().await;
}
