//! Achievement endpoints: definitions, unlocks, isolation, privacy and scans.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use tempfile::TempDir;

const PASSWORD: &str = "password123";

struct TestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    #[allow(dead_code)]
    temp: TempDir,
    data_dir: PathBuf,
    scan_root: PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let scan_root = temp.path().join("installs");
        copy_fixture("steam", &scan_root.join("Steam Game"));
        copy_fixture("malformed", &scan_root.join("Broken Game"));

        let mut config = Config::for_local(games_dir, data_dir.clone(), media_dir);
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        config.achievement_scan_roots = vec![scan_root.clone()];
        config.achievement_providers = vec!["game_files".to_string()];
        config.achievement_local_api_urls = Vec::new();
        config.achievement_scan_interval = Duration::ZERO;

        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            client: Client::builder().build().expect("client"),
            handle: Some(handle),
            temp,
            data_dir,
            scan_root,
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

    async fn setup(&self) -> String {
        let (status, body) = self
            .post(
                "/api/setup",
                None,
                Some(json!({
                    "username": "admin",
                    "password": PASSWORD,
                    "serverName": "Achievements"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().expect("token").to_string()
    }

    async fn login(&self, username: &str) -> String {
        let (status, body) = self
            .post(
                "/api/auth/login",
                None,
                Some(json!({ "username": username, "password": PASSWORD })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().expect("token").to_string()
    }

    async fn create_user(&self, admin: &str, username: &str) -> String {
        let (status, body) = self
            .post(
                "/api/users",
                Some(admin),
                Some(json!({
                    "username": username,
                    "password": PASSWORD,
                    "role": "user"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        self.login(username).await
    }

    async fn insert_game(&self, title: &str) -> i64 {
        let pool = self.pool().await;
        let now = "2026-01-01T00:00:00Z";
        let result = sqlx::query(
            "INSERT INTO games (title, sort_title, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(title)
        .bind(title.to_lowercase())
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("insert game");
        pool.close().await;
        result.last_insert_rowid()
    }

    async fn count_definitions(&self, game_id: i64) -> i64 {
        let pool = self.pool().await;
        let count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM achievements WHERE game_id = ?")
                .bind(game_id)
                .fetch_one(&pool)
                .await
                .expect("count");
        pool.close().await;
        count
    }

    async fn pool(&self) -> SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(self.data_dir.join("ludex.db"))
            .create_if_missing(false)
            .busy_timeout(Duration::from_secs(5));
        SqlitePool::connect_with(options).await.expect("db")
    }
}

fn copy_fixture(name: &str, destination: &Path) {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("achievements")
        .join(name);
    copy_tree(&source, destination);
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).expect("mkdir");
    for entry in std::fs::read_dir(source).expect("read fixture") {
        let entry = entry.expect("entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

#[tokio::test]
async fn achievements_framework_and_local_scan() {
    let server = TestServer::start().await;
    let admin = server.setup().await;
    let game = server.insert_game("Steam Game").await;
    let broken = server.insert_game("Broken Game").await;

    // Empty game.
    let (status, body) = server
        .get(&format!("/api/games/{game}/achievements"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 0);
    assert_eq!(body["completion"], 0.0);
    assert_eq!(body["achievements"].as_array().expect("array").len(), 0);

    // Unauthenticated callers are rejected.
    let (status, _) = server
        .get(&format!("/api/games/{game}/achievements"), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Manual definitions: one public, one hidden.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements"),
            Some(&admin),
            Some(json!({
                "key": "first_blood",
                "name": "First Blood",
                "description": "Finish the prologue.",
                "points": 25
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["key"], "first_blood");

    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements"),
            Some(&admin),
            Some(json!({
                "key": "Secret Ending",
                "name": "The Long Way",
                "description": "Find the room nobody talks about.",
                "hidden": true,
                "points": 50
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["key"], "secret_ending");

    // Duplicate keys are rejected.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements"),
            Some(&admin),
            Some(json!({ "key": "first_blood", "name": "Again" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "achievement_exists");

    // Hidden definitions are masked while locked.
    let (status, body) = server
        .get(&format!("/api/games/{game}/achievements"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 2);
    assert_eq!(body["points"], 75);
    assert_eq!(body["unlocked"], 0);
    let achievements = body["achievements"].as_array().expect("array");
    let secret = achievements
        .iter()
        .find(|item| item["key"] == "secret_ending")
        .expect("secret");
    assert_eq!(secret["name"], "Hidden achievement");
    assert!(secret["description"].is_null());
    assert_eq!(secret["hidden"], true);

    // Unlock is idempotent.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/first_blood/unlock"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unlocked"], true);
    assert_eq!(body["achievement"]["key"], "first_blood");

    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/first_blood/unlock"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unlocked"], false);

    let (status, body) = server
        .get(&format!("/api/games/{game}/achievements"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unlocked"], 1);
    assert_eq!(body["unlockedPoints"], 25);
    assert_eq!(body["completion"], 50.0);

    // Unknown keys and unknown games are 404.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/nope/unlock"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "achievement_not_found");
    let (status, _) = server
        .get("/api/games/9999/achievements", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Per-user isolation.
    let alice = server.create_user(&admin, "alice").await;
    let (status, body) = server
        .get(&format!("/api/games/{game}/achievements"), Some(&alice))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unlocked"], 0);
    // A normal account cannot unlock for itself through the admin route.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/first_blood/unlock"),
            Some(&alice),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server.get("/api/me/achievements", Some(&alice)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unlocked"], 0);

    // Private profiles hide the showcase from other accounts.
    let (status, body) = server.get("/api/users/1/achievements", Some(&alice)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "profile_not_found");

    let (status, body) = server
        .patch("/api/me/profile", &admin, json!({ "publicProfile": true }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = server.get("/api/users/1/achievements", Some(&alice)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["username"], "admin");
    assert_eq!(body["unlocked"], 1);
    assert_eq!(body["achievements"][0]["key"], "first_blood");
    // 2 activated accounts, 1 unlock -> 50%.
    assert_eq!(body["achievements"][0]["unlockPercentage"], 50.0);

    // Scans read the Steam fixture and upsert definitions + unlocks.
    let install = server.scan_root.join("Steam Game");
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/scan"),
            Some(&admin),
            Some(json!({ "installPath": install })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["found"], 3);
    assert_eq!(body["unlocked"], 2);
    assert_eq!(body["providers"][0]["provider"], "game_files");
    assert_eq!(body["providers"][0]["status"], "ok");

    let (status, body) = server
        .get(&format!("/api/games/{game}/achievements"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 5);
    assert_eq!(body["unlocked"], 3);
    assert_eq!(body["lastScan"]["providers"][0], "game_files");

    // Rescan is idempotent: no new unlocks, no duplicated rows.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/scan"),
            Some(&admin),
            Some(json!({ "installPath": install })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["found"], 3);
    assert_eq!(body["unlocked"], 0);
    assert_eq!(server.count_definitions(game).await, 5);

    // A malformed source is reported, never fatal.
    let broken_path = server.scan_root.join("Broken Game");
    let (status, body) = server
        .post(
            &format!("/api/games/{broken}/achievements/scan"),
            Some(&admin),
            Some(json!({ "installPath": broken_path })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["found"], 0);
    assert_eq!(body["providers"][0]["status"], "skipped");
    assert_eq!(server.count_definitions(broken).await, 0);

    // Paths outside the allowlist are refused.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/scan"),
            Some(&admin),
            Some(json!({ "installPath": server.temp.path() })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "path_not_allowed");

    // Unknown providers are refused.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/scan"),
            Some(&admin),
            Some(json!({ "installPath": install, "providers": ["nope"] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "unknown_provider");

    // Admin CRUD: update then delete a definition (unlocks cascade).
    let (status, body) = server
        .patch(
            &format!("/api/games/{game}/achievements/first_blood"),
            &admin,
            json!({ "name": "First Blood (edited)", "points": 30 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = server
        .delete(
            &format!("/api/games/{game}/achievements/first_blood"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = server.get("/api/me/achievements", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // first_blood is gone; the two provider unlocks remain.
    assert_eq!(body["unlocked"], 2);

    // Unlocking for another account is an admin action.
    let (status, body) = server
        .post(
            &format!("/api/games/{game}/achievements/ach_secret/unlock"),
            Some(&admin),
            Some(json!({ "userId": 2 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unlocked"], true);
    let (status, body) = server.get("/api/me/achievements", Some(&alice)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["unlocked"], 1);
    assert_eq!(body["achievements"][0]["key"], "ach_secret");
    assert_eq!(body["achievements"][0]["source"], "manual");

    server.stop().await;
}

#[test]
fn openapi_documents_every_achievement_route_and_schema() {
    let operations = ludex_server::openapi::documented_operations();
    for (method, path) in [
        ("GET", "/games/{id}/achievements"),
        ("POST", "/games/{id}/achievements"),
        ("POST", "/games/{id}/achievements/scan"),
        ("PATCH", "/games/{id}/achievements/{key}"),
        ("DELETE", "/games/{id}/achievements/{key}"),
        ("POST", "/games/{id}/achievements/{key}/unlock"),
        ("GET", "/me/achievements"),
        ("GET", "/users/{id}/achievements"),
    ] {
        assert!(
            operations.contains(&(method.to_string(), path.to_string())),
            "{method} {path} is missing from openapi::ROUTES"
        );
    }

    let document = serde_json::to_value(ludex_server::openapi::document()).expect("document");
    let schemas = document["components"]["schemas"]
        .as_object()
        .expect("components.schemas");
    for name in [
        "Achievement",
        "GameAchievements",
        "ScanSummary",
        "UserAchievements",
        "AchievementUser",
        "ScanReport",
        "ProviderReport",
        "ScanRequest",
        "DefinitionInput",
        "DefinitionPatch",
        "UnlockRequest",
        "DefinitionResponse",
        "UnlockResponse",
    ] {
        assert!(
            schemas.contains_key(name),
            "schema {name} is not registered"
        );
    }
    assert!(
        schemas["GameAchievements"]["properties"]["unlockedPoints"].is_object(),
        "GameAchievements schema lost its fields"
    );
    assert!(
        schemas["Achievement"]["properties"]["unlockPercentage"].is_object(),
        "Achievement schema lost its fields"
    );
}
