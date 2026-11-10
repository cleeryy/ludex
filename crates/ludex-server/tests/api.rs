//! End-to-end HTTP tests against a real server on an ephemeral port.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use rand::RngCore;
use reqwest::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
};
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
    games_dir: PathBuf,
    media_dir: PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        Self::start_with_web(false).await
    }

    async fn start_with_web(with_web: bool) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let mut config = Config::for_local(games_dir.clone(), data_dir, media_dir.clone());
        // Server-mode tests exercise first-run setup instead of local bootstrap.
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        if with_web {
            let web_dir = temp.path().join("web");
            std::fs::create_dir_all(&web_dir).expect("web dir");
            std::fs::write(web_dir.join("index.html"), "<html>Ludex UI</html>").expect("index");
            std::fs::write(web_dir.join("app.js"), "console.log('ludex');").expect("asset");
            config.web_dir = Some(web_dir);
        }

        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            client: Client::builder().build().expect("client"),
            handle: Some(handle),
            temp,
            games_dir,
            media_dir,
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

    async fn put(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::PUT, path, Some(token), Some(body))
            .await
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
        let token = body["token"].as_str().expect("token").to_string();
        // The server kicks off an initial scan at startup. Let it finish before
        // the caller writes archives, otherwise that scan can pick up the new
        // files and the following manual scan reports updates instead of adds.
        wait_for_first_scan(self, &token).await;
        (token, body["user"].clone())
    }

    fn write_game(&self, name: &str, size: u64) -> PathBuf {
        let path = self.games_dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        let file = std::fs::File::create(&path).expect("create");
        file.set_len(size).expect("size");
        path
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
    }
}

async fn scan_status(server: &TestServer, token: &str) -> Value {
    let (status, body) = server.get("/api/library/scan", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn wait_for_first_scan(server: &TestServer, token: &str) -> Value {
    for _ in 0..400 {
        let status = scan_status(server, token).await;
        if !status["running"].as_bool().unwrap_or(true) && status["finishedAt"].as_str().is_some() {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("initial scan did not finish");
}

async fn run_scan(server: &TestServer, token: &str) -> Value {
    let before = wait_for_first_scan(server, token).await["finishedAt"]
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

    for _ in 0..400 {
        let status = scan_status(server, token).await;
        if !status["running"].as_bool().unwrap_or(true)
            && status["finishedAt"].as_str() != Some(before.as_str())
        {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("manual scan did not finish");
}

#[tokio::test]
async fn setup_login_and_public_routes() {
    let server = TestServer::start().await;

    let (status, health) = server.get("/api/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["status"], "ok");
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));

    let (status, setup) = server.get("/api/setup/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(setup["needsSetup"], true);
    assert_eq!(setup["serverName"], "Ludex");

    let (status, config) = server.get("/api/config", None).await;
    assert_eq!(status, StatusCode::OK, "config must be public: {config}");
    assert_eq!(config["features"]["steam"], true);
    assert_eq!(config["features"]["ingest"], true);
    assert_eq!(config["maxUploadBytes"], 10 * 1024 * 1024);

    let (status, _) = server.get("/api/games", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, error) = server.get("/api/games", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "unauthorized");

    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": ADMIN_PASSWORD})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (_token, user) = server.setup().await;
    assert_eq!(user["role"], "admin");
    assert_eq!(user["activated"], true);

    let (status, body) = server
        .post(
            "/api/setup",
            None,
            Some(json!({"username": "other", "password": ADMIN_PASSWORD})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let (status, setup) = server.get("/api/setup/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(setup["needsSetup"], false);
    assert_eq!(setup["serverName"], "Home Arcade");

    let (status, config) = server.get("/api/config", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["serverName"], "Home Arcade");

    let (status, error) = server.get("/api/does-not-exist", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "not_found");

    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": "wrong-password"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": ADMIN_PASSWORD})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let login_token = login["token"].as_str().expect("token").to_string();

    let (status, me) = server.get("/api/auth/me", Some(&login_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["user"]["username"], "admin");

    let (status, _) = server
        .post(
            "/api/auth/change-password",
            Some(&login_token),
            Some(json!({"currentPassword": "nope", "newPassword": "password456"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = server
        .post(
            "/api/auth/change-password",
            Some(&login_token),
            Some(json!({"currentPassword": ADMIN_PASSWORD, "newPassword": "password456"})),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": ADMIN_PASSWORD})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": "password456"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let token = login["token"].as_str().expect("token").to_string();

    let (status, _) = server.post("/api/auth/logout", Some(&token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Since #19 logout revokes the token server-side instead of relying on the
    // client to forget it.
    let (status, _) = server.get("/api/library/folders", Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    server.stop().await;
}

#[tokio::test]
async fn library_scan_groups_versions_and_serves_api() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let hades_a = server.write_game("Hades (2020).zip", 1024 * 1024 + 10);
    let hades_b = server.write_game("Hades (2020) v1.1.zip", 1024 * 1024 + 20);
    server.write_game("Celeste (2018).zip", 1024 * 1024 + 30);
    server.write_game("too-small.zip", 512);
    server.write_game("notes.txt", 2 * 1024 * 1024);
    server.write_game("Redist/dependency.zip", 2 * 1024 * 1024);

    let status = run_scan(&server, &token).await;
    assert_eq!(status["scanned"], 3, "{status}");
    assert_eq!(status["added"], 3, "{status}");
    assert_eq!(
        status["errors"].as_array().map(Vec::len),
        Some(0),
        "{status}"
    );

    let (status, list) = server.get("/api/games?limit=10", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 2, "{list}");
    assert_eq!(list["page"], 1);
    assert_eq!(list["limit"], 10);

    let hades = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["title"] == "Hades")
        .expect("Hades");
    assert_eq!(hades["versionCount"], 2);
    assert_eq!(hades["totalSize"], (1024 * 1024 + 10) + (1024 * 1024 + 20));
    assert_eq!(hades["state"], "unplayed");
    assert_eq!(hades["bookmarked"], false);

    let (status, search) = server.get("/api/games?q=hades", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(search["total"], 1, "{search}");

    let game_id = hades["id"].as_i64().expect("id");
    let (status, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["title"], "Hades");
    assert_eq!(detail["versions"].as_array().expect("versions").len(), 2);
    assert_eq!(detail["videos"].as_array().expect("videos").len(), 0);
    assert!(detail["trailers"].is_array());
    assert!(detail["versions"][0]["downloadUrl"]
        .as_str()
        .expect("url")
        .contains("/api/games/"));
    assert_eq!(detail["launch"]["executable"], Value::Null);

    // Editor metadata update.
    let (status, updated) = server
        .patch(
            &format!("/api/games/{game_id}"),
            &token,
            json!({
                "rating": 91.5,
                "genres": ["Action", "Roguelike"],
                "developers": ["Supergiant Games"],
                "earlyAccess": true,
                "launch": {"executable": "Hades.exe", "parameters": "-windowed"}
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["rating"], 91.5);
    assert_eq!(updated["genres"][1], "Roguelike");
    assert_eq!(updated["earlyAccess"], true);
    assert_eq!(updated["launch"]["executable"], "Hades.exe");
    assert_eq!(updated["launch"]["parameters"], "-windowed");

    let (status, filtered) = server.get("/api/games?genre=Roguelike", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered["total"], 1, "{filtered}");

    let (status, early) = server
        .get("/api/games?earlyAccess=true", Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(early["total"], 1, "{early}");

    let (status, page) = server.get("/api/games?limit=1&page=2", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().expect("items").len(), 1);
    assert_eq!(page["page"], 2);

    let (status, random) = server.get("/api/games/random", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{random}");
    assert!(random["id"].as_i64().is_some());

    let (status, _) = server.get("/api/games?sort=bogus", Some(&token)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Remove one version and rescan: it must be marked missing, not deleted.
    std::fs::remove_file(&hades_b).expect("remove");
    let status = run_scan(&server, &token).await;
    assert_eq!(status["removed"], 1, "{status}");

    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    assert_eq!(detail["versions"].as_array().expect("versions").len(), 1);

    // A missing file is not downloadable.
    let (status, _) = server
        .get(
            &format!("/api/games/{game_id}/download?versionId=999999"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Ensure both files still exist on disk except the removed one.
    assert!(hades_a.exists());
    server.stop().await;
}

#[tokio::test]
async fn advanced_filters_combine_and_fuzzy_search_ranks() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    server.write_game("Hades (2020).zip", 3 * 1024 * 1024);
    server.write_game("Celeste (2018).zip", 2 * 1024 * 1024);
    server.write_game("It Takes Two (2021).zip", 4 * 1024 * 1024);
    server.write_game("Overcooked 2 (2018).zip", 1024 * 1024);
    server.write_game("Marvel's Spider-Man 2 (2023).zip", 5 * 1024 * 1024);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games?limit=20", Some(&token)).await;
    let id_for = |title: &str| {
        list["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["title"] == title)
            .and_then(|item| item["id"].as_i64())
            .unwrap_or_else(|| panic!("missing {title}: {list}"))
    };
    let hades = id_for("Hades");
    let celeste = id_for("Celeste");
    let takes_two = id_for("It Takes Two");
    let overcooked = id_for("Overcooked 2");
    let spider = id_for("Marvel's Spider-Man 2");

    let patch = |id: i64, body: Value| {
        let server = &server;
        let token = token.clone();
        async move {
            let (status, body) = server
                .patch(&format!("/api/games/{id}"), &token, body)
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
    };

    patch(
        hades,
        json!({
            "releaseDate": "2020-09-17",
            "rating": 91.0,
            "genres": ["Roguelike", "Action"],
            "players": 1,
            "altTitles": ["Underworld Run"]
        }),
    )
    .await;
    patch(
        celeste,
        json!({
            "title": "Pokémon Édition",
            "releaseDate": "2018-01-25",
            "rating": 92.0,
            "genres": ["Platformer"],
            "players": 1
        }),
    )
    .await;
    patch(
        takes_two,
        json!({
            "releaseDate": "2021-03-26",
            "rating": 88.0,
            "genres": ["Adventure", "Co-op"],
            "players": 2
        }),
    )
    .await;
    patch(
        overcooked,
        json!({
            "releaseDate": "2018-08-07",
            "rating": 80.0,
            "genres": ["Party", "Co-op"],
            "players": 4
        }),
    )
    .await;

    for (id, minutes) in [(hades, 120), (takes_two, 60), (overcooked, 90)] {
        let (status, body) = server
            .post(
                &format!("/api/games/{id}/playtime"),
                Some(&token),
                Some(json!({ "minutes": minutes })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    // Combined filters: co-op-capable, 2015-2024, rating >= 80, played 30-600 min.
    let (status, filtered) = server
        .get(
            "/api/games?players=2&yearFrom=2015&yearTo=2024&minRating=80&playtimeMin=30&playtimeMax=600&sort=playtime&order=desc",
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    assert_eq!(filtered["total"], 2, "{filtered}");
    assert_eq!(filtered["items"][0]["title"], "Overcooked 2", "{filtered}");
    assert_eq!(filtered["items"][1]["title"], "It Takes Two", "{filtered}");

    // Narrowing the year range excludes Overcooked (2018).
    let (_, filtered) = server
        .get(
            "/api/games?players=2&yearFrom=2020&yearTo=2024",
            Some(&token),
        )
        .await;
    assert_eq!(filtered["total"], 1, "{filtered}");
    assert_eq!(filtered["items"][0]["title"], "It Takes Two", "{filtered}");

    // Playtime range and rating together.
    let (_, filtered) = server
        .get("/api/games?playtimeMin=90&minRating=85", Some(&token))
        .await;
    assert_eq!(filtered["total"], 1, "{filtered}");
    assert_eq!(filtered["items"][0]["title"], "Hades", "{filtered}");

    // Fuzzy search: typo, accent, alias and ranking.
    let (status, search) = server.get("/api/games?q=hadse", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{search}");
    assert_eq!(search["total"], 1, "{search}");
    assert_eq!(search["items"][0]["title"], "Hades", "{search}");
    assert_eq!(search["items"][0]["matchedField"], "title", "{search}");

    let (_, search) = server.get("/api/games?q=pokemon", Some(&token)).await;
    assert_eq!(search["total"], 1, "{search}");
    assert_eq!(search["items"][0]["title"], "Pokémon Édition", "{search}");

    let (_, search) = server
        .get("/api/games?q=underworld%20run", Some(&token))
        .await;
    assert_eq!(search["total"], 1, "{search}");
    assert_eq!(search["items"][0]["title"], "Hades", "{search}");
    assert_eq!(search["items"][0]["matchedField"], "altTitle", "{search}");

    let (_, search) = server.get("/api/games?q=zzzzzz", Some(&token)).await;
    assert_eq!(search["total"], 0, "{search}");

    // Suggestions rank the typo match first and carry cover/match metadata.
    let (status, suggest) = server
        .get("/api/games/suggest?q=spidrman", Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{suggest}");
    assert_eq!(suggest["suggestions"][0]["title"], "Marvel's Spider-Man 2");
    assert_eq!(suggest["suggestions"][0]["matchedField"], "title");
    assert_eq!(suggest["suggestions"][0]["id"], spider);
    assert!(suggest["suggestions"][0]["score"].as_f64().unwrap_or(0.0) > 300.0);

    let (status, suggest) = server.get("/api/games/suggest?q=", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(suggest["suggestions"].as_array().map(Vec::len), Some(0));

    // Fuzzy search still honours the other filters.
    let (_, search) = server
        .get("/api/games?q=overcook&players=4", Some(&token))
        .await;
    assert_eq!(search["total"], 1, "{search}");
    assert_eq!(search["items"][0]["title"], "Overcooked 2", "{search}");

    server.stop().await;
}

#[tokio::test]
async fn progress_bookmarks_playtime_and_stats() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    server.write_game("Hades (2020).zip", 1024 * 1024);
    server.write_game("Celeste (2018).zip", 2 * 1024 * 1024);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games?sort=title", Some(&token)).await;
    let items = list["items"].as_array().expect("items");
    let hades = items
        .iter()
        .find(|item| item["title"] == "Hades")
        .expect("Hades");
    let game_id = hades["id"].as_i64().expect("id");

    let (status, progress) = server
        .put(
            &format!("/api/games/{game_id}/progress"),
            &token,
            json!({"state": "completed"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{progress}");
    assert_eq!(progress["state"], "completed");
    assert_eq!(progress["minutesPlayed"], 0);

    let (status, progress) = server
        .post(
            &format!("/api/games/{game_id}/playtime"),
            Some(&token),
            Some(json!({"minutes": 30})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{progress}");
    assert_eq!(progress["minutesPlayed"], 30);
    assert!(progress["lastPlayedAt"].as_str().is_some());

    let (status, progress) = server
        .post(
            &format!("/api/games/{game_id}/playtime"),
            Some(&token),
            Some(json!({"minutes": 5000})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{progress}");
    assert_eq!(progress["minutesPlayed"], 30 + 1440);

    let (status, bookmark) = server
        .put(
            &format!("/api/games/{game_id}/bookmark"),
            &token,
            json!({"bookmarked": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bookmark["bookmarked"], true);

    let (_, filtered) = server.get("/api/games?bookmarked=true", Some(&token)).await;
    assert_eq!(filtered["total"], 1, "{filtered}");

    let (_, filtered) = server.get("/api/games?state=completed", Some(&token)).await;
    assert_eq!(filtered["total"], 1, "{filtered}");

    let (status, stats) = server.get("/api/stats", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{stats}");
    assert_eq!(stats["games"], 2);
    assert_eq!(stats["versions"], 2);
    assert_eq!(stats["totalBytes"], 3 * 1024 * 1024);
    assert_eq!(stats["users"], 1);
    assert_eq!(stats["byState"]["completed"], 1, "{stats}");
    assert_eq!(stats["byState"]["unplayed"], 1, "{stats}");
    assert_eq!(stats["byState"]["playing"], 0, "{stats}");

    let (status, _) = server
        .put(
            &format!("/api/games/{game_id}/progress"),
            &token,
            json!({"state": "not-a-state"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    server.stop().await;
}

#[tokio::test]
async fn downloads_support_tickets_ranges_and_media_cache() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let file_size = 1024 * 1024 + 7;
    server.write_game("Hades (2020).zip", file_size);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games", Some(&token)).await;
    let game_id = list["items"][0]["id"].as_i64().expect("game id");
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    let version_id = detail["versions"][0]["id"].as_i64().expect("version id");

    let (status, ticket) = server
        .post(
            &format!("/api/games/{game_id}/download-ticket"),
            Some(&token),
            Some(json!({"versionId": version_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{ticket}");
    let url = ticket["url"].as_str().expect("ticket url");
    assert!(url.contains("token="));
    assert!(ticket["expiresAt"].as_str().is_some());

    let response = server
        .client
        .get(format!("{}{}", server.url, url))
        .header(RANGE, "bytes=0-99")
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok()),
        Some(format!("bytes 0-99/{file_size}").as_str())
    );
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok()),
        Some("bytes")
    );
    assert!(response
        .headers()
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("attachment;")));
    let bytes = response.bytes().await.expect("body");
    assert_eq!(bytes.len(), 100);

    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/download?versionId={version_id}",
            server.url
        ))
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/download?versionId={version_id}",
            server.url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);

    let (status, _) = server
        .get(
            &format!("/api/games/{game_id}/versions/{version_id}/local-path"),
            Some(&token),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "remote mode has no local paths"
    );

    let (status, _) = server.get("/api/media/missing.jpg", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    std::fs::write(server.media_dir.join("cover.png"), b"\x89PNG\r\n\x1a\nfake").expect("media");
    let response = server
        .client
        .get(format!("{}/api/media/cover.png", server.url))
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
    assert!(response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("immutable")));

    server.stop().await;
}

#[tokio::test]
async fn users_and_news_are_admin_only() {
    let server = TestServer::start().await;
    let (admin_token, admin_user) = server.setup().await;
    let admin_id = admin_user["id"].as_i64().expect("admin id");

    let (status, bob) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({"username": "bob", "password": "password123", "role": "user"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bob}");
    let bob_id = bob["id"].as_i64().expect("bob id");
    assert_eq!(bob["role"], "user");

    let (status, users) = server.get("/api/users", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(users.as_array().expect("users").len(), 2);

    let (status, updated) = server
        .patch(
            &format!("/api/users/{bob_id}"),
            &admin_token,
            json!({"role": "editor", "activated": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["role"], "editor");
    assert_eq!(updated["activated"], false);

    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "password123"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = server
        .patch(
            &format!("/api/users/{admin_id}"),
            &admin_token,
            json!({"role": "user"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "last admin cannot be demoted");

    let (status, _) = server
        .delete(&format!("/api/users/{admin_id}"), &admin_token)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "cannot delete self");

    // A non-admin cannot list users.
    server
        .patch(
            &format!("/api/users/{bob_id}"),
            &admin_token,
            json!({"activated": true}),
        )
        .await;
    let (status, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "password123"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let bob_token = login["token"].as_str().expect("bob token");
    let (status, _) = server.get("/api/users", Some(bob_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = server
        .post(
            "/api/news",
            Some(bob_token),
            Some(json!({"title": "No", "body": "No"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, news) = server
        .post(
            "/api/news",
            Some(&admin_token),
            Some(json!({"title": "Welcome", "body": "Hello **world**"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{news}");
    let news_id = news["id"].as_i64().expect("news id");
    assert_eq!(news["author"], "admin");

    let (status, list) = server.get("/api/news", Some(bob_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().expect("news").len(), 1);

    let (status, updated) = server
        .patch(
            &format!("/api/news/{news_id}"),
            &admin_token,
            json!({"title": "Welcome back"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["title"], "Welcome back");

    let (status, _) = server
        .delete(&format!("/api/news/{news_id}"), &admin_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = server
        .delete(&format!("/api/users/{bob_id}"), &admin_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = server.get("/api/users", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK);
    server.stop().await;
}

#[tokio::test]
async fn ingest_routes_require_editor() {
    let server = TestServer::start().await;
    let (admin_token, _) = server.setup().await;

    let (status, _) = server.get("/api/ingest/downloads", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, jobs) = server
        .get("/api/ingest/downloads", Some(&admin_token))
        .await;
    assert_eq!(status, StatusCode::OK, "{jobs}");
    assert!(jobs.as_array().is_some());

    let (status, stats) = server.get("/api/ingest/stats", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{stats}");

    let preflight = server
        .client
        .request(
            Method::OPTIONS,
            format!("{}/api/ingest/downloads", server.url),
        )
        .header("Origin", "http://localhost:5173")
        .header("Access-Control-Request-Method", "GET")
        .header("Access-Control-Request-Headers", "authorization")
        .send()
        .await
        .expect("preflight");
    assert_eq!(preflight.status(), StatusCode::OK);
    assert_eq!(
        preflight
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:5173")
    );

    let (status, _) = server
        .post(
            "/api/ingest/downloads",
            Some(&admin_token),
            Some(json!({"url": "not-a-url"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, bob) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({"username": "bob", "password": "password123", "role": "user"})),
        )
        .await;
    assert_eq!(bob["role"], "user");
    let (_, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "password123"})),
        )
        .await;
    let bob_token = login["token"].as_str().expect("bob token");
    let (status, _) = server.get("/api/ingest/downloads", Some(bob_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (_, eve) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({"username": "eve", "password": "password123", "role": "editor"})),
        )
        .await;
    assert_eq!(eve["role"], "editor");
    let (_, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "eve", "password": "password123"})),
        )
        .await;
    let eve_token = login["token"].as_str().expect("eve token");
    let (status, _) = server.get("/api/ingest/downloads", Some(eve_token)).await;
    assert_eq!(status, StatusCode::OK);

    server.stop().await;
}

#[tokio::test]
async fn disk_web_dir_serves_spa_and_keeps_api_json() {
    let server = TestServer::start_with_web(true).await;

    let (status, body) = server.get("/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_str().is_some_and(|html| html.contains("Ludex UI")));

    let (status, body) = server.get("/library/some/deep/route", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_str().is_some_and(|html| html.contains("Ludex UI")));

    let (status, body) = server.get("/app.js", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_str().is_some_and(|js| js.contains("ludex")));

    let (status, health) = server.get("/api/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["status"], "ok");

    let (status, error) = server.get("/api/not-a-route", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "not_found");

    server.stop().await;
}

#[tokio::test]
async fn local_mode_provisions_admin_and_local_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let games_dir = temp.path().join("games");
    let data_dir = temp.path().join("data");
    let media_dir = temp.path().join("media");
    std::fs::create_dir_all(&games_dir).expect("games");
    let game_file = games_dir.join("Hades (2020).zip");
    std::fs::File::create(&game_file)
        .expect("file")
        .set_len(1024 * 1024)
        .expect("size");

    let config = Config::for_local(games_dir, data_dir, media_dir);
    let handle = start(config).await.expect("local server");
    assert!(handle.url.starts_with("http://127.0.0.1:"));
    assert!(!handle.token.is_empty());

    let client = Client::new();
    let response = client
        .get(format!("{}/api/auth/me", handle.url))
        .bearer_auth(&handle.token)
        .send()
        .await
        .expect("me");
    assert_eq!(response.status(), StatusCode::OK);
    let me: Value = response.json().await.expect("json");
    assert_eq!(me["user"]["role"], "admin");

    // Wait for the initial scan, then read the version id.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (game_id, version_id) = loop {
        let response = client
            .get(format!("{}/api/games", handle.url))
            .bearer_auth(&handle.token)
            .send()
            .await
            .expect("games");
        let list: Value = response.json().await.expect("json");
        if let Some(game) = list["items"].as_array().and_then(|items| items.first()) {
            let game_id = game["id"].as_i64().expect("game id");
            let detail: Value = client
                .get(format!("{}/api/games/{game_id}", handle.url))
                .bearer_auth(&handle.token)
                .send()
                .await
                .expect("detail")
                .json()
                .await
                .expect("json");
            // The game row is indexed before its first version row; a scan in
            // progress can therefore show a game with no versions for a moment.
            if let Some(version_id) = detail["versions"]
                .as_array()
                .and_then(|versions| versions.first())
                .and_then(|version| version["id"].as_i64())
            {
                break (game_id, version_id);
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "initial scan did not index the game"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let response = client
        .get(format!(
            "{}/api/games/{game_id}/versions/{version_id}/local-path",
            handle.url
        ))
        .bearer_auth(&handle.token)
        .send()
        .await
        .expect("local path");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("json");
    assert_eq!(
        body["path"].as_str().expect("path"),
        game_file
            .canonicalize()
            .expect("canonical")
            .to_string_lossy()
            .as_ref()
    );

    let _ = handle.shutdown.send(());
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn collections_are_per_user_with_smart_rules() {
    let server = TestServer::start().await;
    let (admin_token, admin_user) = server.setup().await;
    let admin_id = admin_user["id"].as_i64().expect("admin id");

    server.write_game("Hades (2020).zip", 3 * 1024 * 1024);
    server.write_game("Celeste (2018).zip", 2 * 1024 * 1024);
    server.write_game("It Takes Two (2021).zip", 4 * 1024 * 1024);
    run_scan(&server, &admin_token).await;

    let (_, list) = server.get("/api/games?limit=10", Some(&admin_token)).await;
    let mut ids = HashMap::new();
    for item in list["items"].as_array().expect("items") {
        ids.insert(
            item["title"].as_str().expect("title").to_string(),
            item["id"].as_i64().expect("id"),
        );
    }
    let hades = ids["Hades"];
    let celeste = ids["Celeste"];
    let it_takes_two = ids["It Takes Two"];

    for (id, genres, rating, release) in [
        (hades, json!(["Action", "Roguelike"]), 92.0, "2020-09-17"),
        (celeste, json!(["Platformer"]), 88.0, "2018-01-25"),
        (it_takes_two, json!(["Adventure"]), 85.0, "2021-03-26"),
    ] {
        let (status, body) = server
            .patch(
                &format!("/api/games/{id}"),
                &admin_token,
                json!({"genres": genres, "rating": rating, "releaseDate": release}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, created) = server
        .post(
            "/api/collections",
            Some(&admin_token),
            Some(json!({
                "name": "Weekend",
                "description": "Two-player nights",
                "gameIds": [hades, celeste]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let manual_id = created["id"].as_i64().expect("id");
    assert_eq!(created["kind"], "manual");
    assert_eq!(created["gameCount"], 2);
    assert_eq!(created["owned"], true);
    assert_eq!(created["owner"]["username"], "admin");
    assert_eq!(created["isPublic"], false);

    let (status, games) = server
        .get(
            &format!("/api/collections/{manual_id}/games"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{games}");
    assert_eq!(games["total"], 2);

    let (status, added) = server
        .post(
            &format!("/api/collections/{manual_id}/games"),
            Some(&admin_token),
            Some(json!({"gameIds": [it_takes_two, hades]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(added["added"], 1);
    assert_eq!(added["total"], 3);

    let (status, _) = server
        .delete(
            &format!("/api/collections/{manual_id}/games/{celeste}"),
            &admin_token,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, games) = server
        .get(
            &format!("/api/collections/{manual_id}/games"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(games["total"], 2);

    // Smart collection: Roguelike with rating >= 90 matches Hades only.
    let (status, smart) = server
        .post(
            "/api/collections",
            Some(&admin_token),
            Some(json!({
                "name": "Roguelike gems",
                "kind": "smart",
                "isPublic": true,
                "rules": {
                    "match": "all",
                    "rules": [
                        {"field": "genre", "value": "Roguelike"},
                        {"field": "rating", "op": "gte", "value": 90}
                    ]
                }
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{smart}");
    let smart_id = smart["id"].as_i64().expect("smart id");
    let (_, smart_games) = server
        .get(
            &format!("/api/collections/{smart_id}/games"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(smart_games["total"], 1, "{smart_games}");
    assert_eq!(smart_games["items"][0]["title"], "Hades");

    let (status, _) = server
        .post(
            &format!("/api/collections/{smart_id}/games"),
            Some(&admin_token),
            Some(json!({"gameIds": [hades]})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "smart collections reject manual adds"
    );

    let (status, _) = server
        .post(
            "/api/collections",
            Some(&admin_token),
            Some(json!({
                "name": "Broken",
                "kind": "smart",
                "rules": {"match": "all", "rules": [{"field": "bogus", "value": 1}]}
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = server
        .post(
            "/api/collections",
            Some(&admin_token),
            Some(json!({"name": "Weekend"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate name per user");

    // Second account: private collections are invisible.
    let (status, bob) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({"username": "bob", "password": "password123", "role": "user"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bob}");
    let (status, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "password123"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let bob_token = login["token"].as_str().expect("bob token").to_string();

    let (_, bob_list) = server.get("/api/collections", Some(&bob_token)).await;
    assert_eq!(
        bob_list.as_array().expect("collections").len(),
        1,
        "only the public smart collection is visible: {bob_list}"
    );
    assert_eq!(bob_list[0]["id"], smart_id);

    let (status, _) = server
        .get(&format!("/api/collections/{manual_id}"), Some(&bob_token))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server
        .get(
            &format!("/api/collections/{manual_id}/games"),
            Some(&bob_token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server
        .patch(
            &format!("/api/collections/{manual_id}"),
            &bob_token,
            json!({"name": "Stolen"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server
        .delete(&format!("/api/collections/{manual_id}"), &bob_token)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, bob_created) = server
        .post(
            "/api/collections",
            Some(&bob_token),
            Some(json!({"name": "Weekend", "gameIds": [hades]})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bob_created}");

    // Publishing makes the manual shelf readable by others.
    let (status, published) = server
        .patch(
            &format!("/api/collections/{manual_id}"),
            &admin_token,
            json!({"isPublic": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    assert_eq!(published["isPublic"], true);
    let (status, games) = server
        .get(
            &format!("/api/collections/{manual_id}/games"),
            Some(&bob_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{games}");
    assert_eq!(games["total"], 2);

    let (_, bob_list) = server.get("/api/collections", Some(&bob_token)).await;
    assert_eq!(bob_list.as_array().expect("collections").len(), 3);

    let (status, _) = server
        .delete(&format!("/api/collections/{manual_id}"), &admin_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = server
        .get(&format!("/api/collections/{manual_id}"), Some(&admin_token))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(admin_id > 0);

    server.stop().await;
}

#[tokio::test]
async fn community_profiles_are_opt_in_and_the_feed_paginates() {
    let server = TestServer::start().await;
    let (admin_token, admin_user) = server.setup().await;
    let admin_id = admin_user["id"].as_i64().expect("admin id");

    server.write_game("Hades (2020).zip", 3 * 1024 * 1024);
    server.write_game("Celeste (2018).zip", 2 * 1024 * 1024);
    run_scan(&server, &admin_token).await;
    let (_, list) = server.get("/api/games?limit=10", Some(&admin_token)).await;
    let hades = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["title"] == "Hades")
        .and_then(|item| item["id"].as_i64())
        .expect("hades");

    // Private by default: only the owner can read the profile.
    let (status, own) = server.get("/api/me/profile", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(own["publicProfile"], false);
    assert_eq!(own["username"], "admin");

    let (status, profile) = server
        .get(
            &format!("/api/community/users/{admin_id}"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "owner reads own profile: {profile}");

    let (status, updated) = server
        .patch(
            "/api/me/profile",
            &admin_token,
            json!({"publicProfile": true, "displayName": "Mara", "bio": "Roguelike and cats."}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["publicProfile"], true);
    assert_eq!(updated["displayName"], "Mara");

    let (status, _) = server
        .patch(
            "/api/me/profile",
            &admin_token,
            json!({"displayName": "x".repeat(80)}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Activities: collection created, game added, game played, installed.
    let (status, collection) = server
        .post(
            "/api/collections",
            Some(&admin_token),
            Some(json!({"name": "Night shelf", "gameIds": [hades]})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{collection}");
    let collection_id = collection["id"].as_i64().expect("collection id");

    let (status, progress) = server
        .put(
            &format!("/api/games/{hades}/progress"),
            &admin_token,
            json!({"state": "playing"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{progress}");
    let (status, playtime) = server
        .post(
            &format!("/api/games/{hades}/playtime"),
            Some(&admin_token),
            Some(json!({"minutes": 45})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{playtime}");
    let (status, first_install) = server
        .post(
            &format!("/api/games/{hades}/installed"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{first_install}");
    assert_eq!(first_install["recorded"], true);
    let (_, second_install) = server
        .post(
            &format!("/api/games/{hades}/installed"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(
        second_install["recorded"], false,
        "installs dedupe within an hour"
    );

    let (status, feed) = server
        .get("/api/community/feed?limit=20", Some(&admin_token))
        .await;
    assert_eq!(status, StatusCode::OK, "{feed}");
    let items = feed["items"].as_array().expect("items");
    assert_eq!(items.len(), 4, "{feed}");
    let kinds: Vec<&str> = items
        .iter()
        .map(|item| item["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "game_installed",
            "game_played",
            "game_added",
            "collection_created"
        ]
    );
    assert_eq!(items[1]["game"]["title"], "Hades");
    assert_eq!(items[3]["collection"]["name"], "Night shelf");
    assert_eq!(items[0]["user"]["displayName"], "Mara");

    // Pagination is newest-first by id cursor.
    let (_, page_one) = server
        .get("/api/community/feed?limit=2", Some(&admin_token))
        .await;
    assert_eq!(page_one["items"].as_array().expect("items").len(), 2);
    let cursor = page_one["nextBefore"].as_i64().expect("cursor");
    let (_, page_two) = server
        .get(
            &format!("/api/community/feed?limit=2&before={cursor}"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(page_two["items"].as_array().expect("items").len(), 2);
    assert_eq!(page_two["nextBefore"], Value::Null);

    // Second account: private until it opts in.
    let (status, bob) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({"username": "bob", "password": "password123", "role": "user"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bob}");
    let bob_id = bob["id"].as_i64().expect("bob id");
    let (_, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "password123"})),
        )
        .await;
    let bob_token = login["token"].as_str().expect("bob token").to_string();

    let (status, _) = server
        .get(
            &format!("/api/community/users/{bob_id}"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "private profile is hidden");

    let (status, admin_profile) = server
        .get(
            &format!("/api/community/users/{admin_id}"),
            Some(&bob_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{admin_profile}");
    assert_eq!(admin_profile["user"]["displayName"], "Mara");
    assert_eq!(admin_profile["stats"]["games"], 1, "{admin_profile}");
    assert_eq!(
        admin_profile["stats"]["minutesPlayed"], 45,
        "{admin_profile}"
    );
    // The manual shelf is private, so another account sees no collection.
    assert_eq!(admin_profile["stats"]["collections"], 0, "{admin_profile}");
    assert_eq!(admin_profile["library"][0]["title"], "Hades");

    let (_, own_profile) = server
        .get(
            &format!("/api/community/users/{admin_id}"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(own_profile["stats"]["collections"], 1, "{own_profile}");
    assert_eq!(own_profile["collections"][0]["id"], collection_id);

    let (status, activity) = server
        .get(
            &format!("/api/community/users/{admin_id}/activity"),
            Some(&bob_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{activity}");
    assert_eq!(activity["items"].as_array().expect("items").len(), 4);

    let (status, _) = server
        .get(
            &format!("/api/community/users/{bob_id}/activity"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Opting in publishes the profile and its future activity.
    server
        .patch(
            "/api/me/profile",
            &bob_token,
            json!({"publicProfile": true}),
        )
        .await;
    server
        .put(
            &format!("/api/games/{hades}/progress"),
            &bob_token,
            json!({"state": "completed"}),
        )
        .await;

    let (status, directory) = server.get("/api/community/users", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{directory}");
    assert_eq!(directory.as_array().expect("users").len(), 2);

    let (_, feed) = server
        .get("/api/community/feed?limit=20", Some(&admin_token))
        .await;
    assert_eq!(feed["items"].as_array().expect("items").len(), 6);

    let (status, bob_profile) = server
        .get(
            &format!("/api/community/users/{bob_id}"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{bob_profile}");
    assert_eq!(bob_profile["user"]["publicProfile"], true);
    assert_eq!(bob_profile["stats"]["completed"], 1);

    // Deleting the account removes its activities from the feed.
    server
        .delete(&format!("/api/users/{bob_id}"), &admin_token)
        .await;
    let (_, feed) = server
        .get("/api/community/feed?limit=20", Some(&admin_token))
        .await;
    assert_eq!(feed["items"].as_array().expect("items").len(), 4);

    server.stop().await;
}

fn zip_options() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
}

fn write_test_zip(path: &Path) {
    let file = std::fs::File::create(path).expect("zip file");
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("Game/game.exe", zip_options())
        .expect("entry");
    archive.write_all(b"MZ ludex stream").expect("write");
    archive
        .start_file("Game/data/readme.txt", zip_options())
        .expect("entry");
    archive.write_all(b"hello from ludex").expect("write");
    archive
        .start_file("Game/pad.bin", zip_options())
        .expect("entry");
    archive
        .write_all(&vec![0_u8; 1024 * 1024 + 1])
        .expect("write");
    let long_name = format!("Game/{}.txt", "long-name-segment-".repeat(8));
    archive
        .start_file(&long_name, zip_options())
        .expect("long entry");
    archive.write_all(b"long name content").expect("write");
    archive.finish().expect("finish");
}

fn write_test_7z(path: &Path) {
    let mut writer = sevenz_rust2::SevenZWriter::create(path).expect("create 7z");
    for (name, data) in [
        ("Seven/game.exe", b"MZ seven stream".as_slice()),
        ("Seven/data.txt", b"seven data".as_slice()),
    ] {
        let entry = sevenz_rust2::SevenZArchiveEntry::new_file(name);
        writer
            .push_archive_entry(entry, Some(Cursor::new(data.to_vec())))
            .expect("7z entry");
    }
    let mut pad = vec![0_u8; 1024 * 1024 + 1];
    rand::thread_rng().fill_bytes(&mut pad);
    let entry = sevenz_rust2::SevenZArchiveEntry::new_file("Seven/pad.bin");
    writer
        .push_archive_entry(entry, Some(Cursor::new(pad)))
        .expect("7z pad");
    writer.finish().expect("finish 7z");
    let size = std::fs::metadata(path).expect("7z metadata").len();
    assert!(
        size > 1024 * 1024,
        "7z fixture must exceed the scanner minimum, got {size} bytes"
    );
}

fn write_test_rar(path: &Path) {
    let mut file = std::fs::File::create(path).expect("rar file");
    file.write_all(b"Rar!\x1a\x07\x00").expect("magic");
    file.set_len(1024 * 1024 + 1).expect("size");
}

fn read_tar(body: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut archive = tar::Archive::new(Cursor::new(body));
    let mut files = HashMap::new();
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let path = entry
            .path()
            .expect("tar path")
            .to_string_lossy()
            .trim_start_matches("./")
            .to_string();
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents).expect("tar contents");
        files.insert(path, contents);
    }
    files
}

#[tokio::test]
async fn stream_install_zip_as_tar() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let zip_path = server.games_dir.join("Stream Game (2020).zip");
    write_test_zip(&zip_path);
    let rar_path = server.games_dir.join("Rar Game (2020).rar");
    write_test_rar(&rar_path);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games?limit=10", Some(&token)).await;
    let items = list["items"].as_array().expect("items");
    let zip_game = items
        .iter()
        .find(|item| item["title"] == "Stream Game")
        .expect("zip game");
    let zip_game_id = zip_game["id"].as_i64().expect("zip game id");
    let (_, detail) = server
        .get(&format!("/api/games/{zip_game_id}"), Some(&token))
        .await;
    let version_id = detail["versions"][0]["id"]
        .as_i64()
        .expect("zip version id");

    let (status, error) = server
        .get(
            &format!("/api/games/{zip_game_id}/versions/{version_id}/stream"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "unauthorized");

    let (status, _) = server
        .get(
            &format!(
                "/api/games/{zip_game_id}/versions/{}/stream",
                version_id + 9999
            ),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = server
        .get(
            &format!(
                "/api/games/{}/versions/{version_id}/stream",
                zip_game_id + 9999
            ),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let response = server
        .client
        .get(format!(
            "{}/api/games/{zip_game_id}/versions/{version_id}/stream",
            server.url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("stream");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/x-tar")
    );
    assert!(response
        .headers()
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("attachment;") && value.contains(".tar")));
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("private, no-store")
    );
    let content_length: u64 = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .expect("content length");
    let body = response.bytes().await.expect("tar body");
    assert_eq!(content_length, body.len() as u64);
    let files = read_tar(&body);
    assert_eq!(
        files.get("Game/game.exe").map(Vec::as_slice),
        Some(b"MZ ludex stream".as_slice())
    );
    assert_eq!(
        files.get("Game/data/readme.txt").map(Vec::as_slice),
        Some(b"hello from ludex".as_slice())
    );
    assert_eq!(
        files.get("Game/pad.bin").map(Vec::len),
        Some(1024 * 1024 + 1)
    );
    let long_name = format!("Game/{}.txt", "long-name-segment-".repeat(8));
    assert_eq!(
        files.get(&long_name).map(Vec::as_slice),
        Some(b"long name content".as_slice())
    );

    // Ticket authentication must work for the stream route too.
    let (status, ticket) = server
        .post(
            &format!("/api/games/{zip_game_id}/download-ticket"),
            Some(&token),
            Some(json!({"versionId": version_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{ticket}");
    let ticket_url = ticket["url"].as_str().expect("ticket url");
    let ticket_token = ticket_url.rsplit_once("token=").expect("ticket token").1;
    let response = server
        .client
        .get(format!(
            "{}/api/games/{zip_game_id}/versions/{version_id}/stream?token={ticket_token}",
            server.url
        ))
        .send()
        .await
        .expect("ticket stream");
    assert_eq!(response.status(), StatusCode::OK);
    let ticket_length: u64 = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .expect("ticket content length");
    let ticket_body = response.bytes().await.expect("ticket tar body");
    assert_eq!(ticket_length, ticket_body.len() as u64);
    assert!(read_tar(&ticket_body).contains_key("Game/game.exe"));

    // Non-ZIP archives are rejected with 415; clients fall back to /download.
    let rar_game = items
        .iter()
        .find(|item| item["title"] == "Rar Game")
        .expect("rar game");
    let rar_game_id = rar_game["id"].as_i64().expect("rar game id");
    let (_, rar_detail) = server
        .get(&format!("/api/games/{rar_game_id}"), Some(&token))
        .await;
    let rar_version_id = rar_detail["versions"][0]["id"]
        .as_i64()
        .expect("rar version id");
    let response = server
        .client
        .get(format!(
            "{}/api/games/{rar_game_id}/versions/{rar_version_id}/stream",
            server.url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("rar stream");
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let error: Value = response.json().await.expect("rar error");
    assert_eq!(error["error"]["code"], "stream_unsupported");

    server.stop().await;
}

#[tokio::test]
async fn stream_install_7z_as_tar() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let seven_path = server.games_dir.join("Seven Game (2021).7z");
    write_test_7z(&seven_path);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games?limit=10", Some(&token)).await;
    let game = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["title"] == "Seven Game")
        .expect("7z game");
    let game_id = game["id"].as_i64().expect("game id");
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    let version_id = detail["versions"][0]["id"].as_i64().expect("version id");

    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/versions/{version_id}/stream",
            server.url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("7z stream");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/x-tar")
    );
    let content_length: u64 = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .expect("content length");
    let body = response.bytes().await.expect("7z tar body");
    assert_eq!(content_length, body.len() as u64);

    let files = read_tar(&body);
    assert_eq!(
        files.get("Seven/game.exe").map(Vec::as_slice),
        Some(b"MZ seven stream".as_slice())
    );
    assert_eq!(
        files.get("Seven/data.txt").map(Vec::as_slice),
        Some(b"seven data".as_slice())
    );
    assert_eq!(
        files.get("Seven/pad.bin").map(Vec::len),
        Some(1024 * 1024 + 1)
    );

    server.stop().await;
}

#[tokio::test]
async fn latest_version_exposes_update_metadata() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let old = server.write_game("Hades (2020) v1.0.zip", 1024 * 1024 + 10);
    let new = server.write_game("Hades (2020) v1.2.zip", 1024 * 1024 + 40);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games?limit=10", Some(&token)).await;
    let game_id = list["items"][0]["id"].as_i64().expect("id");

    let (status, latest) = server
        .get(
            &format!("/api/games/{game_id}/versions/latest"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{latest}");
    assert_eq!(latest["gameId"], game_id);
    assert_eq!(latest["versionLabel"], "1.2");
    assert_eq!(latest["fileName"], "Hades (2020) v1.2.zip");
    assert_eq!(latest["archiveSize"], (1024 * 1024 + 40) as i64);
    assert_eq!(latest["earlyAccess"], false);
    let hash = latest["archiveHash"].as_str().expect("archive fingerprint");
    assert!(hash.starts_with("sha256-sample:"), "{hash}");
    assert!(latest["updatedAt"].as_str().is_some(), "{latest}");
    assert!(latest["downloadUrl"]
        .as_str()
        .expect("download url")
        .contains("/api/games/"));

    // The detail payload carries the same fingerprint on each version.
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    let versions = detail["versions"].as_array().expect("versions");
    assert_eq!(versions.len(), 2);
    assert!(versions.iter().all(|version| version["archiveHash"]
        .as_str()
        .is_some_and(|value| value.starts_with("sha256-sample:"))));

    // Replacing the archive content changes the fingerprint without a rescan.
    let mut replacement = std::fs::OpenOptions::new()
        .write(true)
        .open(&new)
        .expect("open");
    use std::io::{Seek, SeekFrom, Write};
    replacement.seek(SeekFrom::Start(0)).expect("seek");
    replacement
        .write_all(b"replaced archive bytes")
        .expect("write");
    drop(replacement);
    let status = run_scan(&server, &token).await;
    assert_eq!(
        status["errors"].as_array().map(Vec::len),
        Some(0),
        "{status}"
    );

    let (_, updated) = server
        .get(
            &format!("/api/games/{game_id}/versions/latest"),
            Some(&token),
        )
        .await;
    assert_ne!(updated["archiveHash"], latest["archiveHash"]);

    // Unknown games and games without versions are explicit 404s.
    let (status, body) = server
        .get("/api/games/999999/versions/latest", Some(&token))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Authentication is required.
    let (status, _) = server
        .get(&format!("/api/games/{game_id}/versions/latest"), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Both files are still on disk: the scan never rewrites the library.
    assert!(old.exists());
    assert!(new.exists());
    server.stop().await;
}

/// Rewrites the expiry of one share row directly. The test owns the same
/// SQLite file as the server, which lets expiry be exercised without waiting
/// an hour or exposing an internal endpoint.
async fn patch_share_expiry(server: &TestServer, share_id: i64, expires_at: &str) {
    let url = ludex_server::config::default_database_url(&server.temp.path().join("data"));
    let pool = ludex_server::db::connect(&url)
        .await
        .expect("connect test db");
    sqlx::query("UPDATE share_links SET expires_at = ? WHERE id = ?")
        .bind(expires_at)
        .bind(share_id)
        .execute(&pool)
        .await
        .expect("update share expiry");
    pool.close().await;
}

#[tokio::test]
async fn share_links_are_public_and_ticket_based() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    // The scanner ignores archives below 1 MiB; the content does not matter
    // here, only that the ticketed download streams the indexed file.
    let file_size = 1024 * 1024 + 24;
    server.write_game("Hades (2020).zip", file_size);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games", Some(&token)).await;
    let game_id = list["items"][0]["id"].as_i64().expect("game id");
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    let version_id = detail["versions"][0]["id"].as_i64().expect("version id");

    // Create a protected, expiring link for one version.
    let (status, share) = server
        .post(
            &format!("/api/games/{game_id}/shares"),
            Some(&token),
            Some(json!({
                "versionId": version_id,
                "expiresInHours": 24,
                "password": "open sesame",
                "maxDownloads": 5,
                "label": "For Bob",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{share}");
    let share_id = share["id"].as_i64().expect("share id");
    let share_token = share["token"]
        .as_str()
        .expect("plaintext token")
        .to_string();
    assert!(share_token.starts_with("lsx_"), "{share}");
    assert_eq!(share["gameId"], game_id);
    assert_eq!(share["versionId"], version_id);
    assert_eq!(share["label"], "For Bob");
    assert_eq!(share["hasPassword"], true);
    assert_eq!(share["maxDownloads"], 5);
    assert_eq!(share["downloadCount"], 0);
    assert!(share["expiresAt"].as_str().is_some(), "{share}");
    assert!(share["revokedAt"].is_null());
    assert!(share.get("passwordHash").is_none());

    // The metadata page is public and carries no account data.
    let (status, view) = server.get(&format!("/api/share/{share_token}"), None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["game"]["id"], game_id);
    assert_eq!(view["game"]["title"], "Hades");
    assert_eq!(view["version"]["id"], version_id);
    assert_eq!(view["hasPassword"], true);
    assert_eq!(view["remainingDownloads"], 5);
    for field in ["state", "minutesPlayed", "bookmarked", "lastPlayedAt"] {
        assert!(
            view["game"].get(field).is_none(),
            "no user data: {field} in {view}"
        );
    }
    for field in ["filePath", "rootId", "versions", "launch", "provider"] {
        assert!(
            view["game"].get(field).is_none(),
            "no library internals: {field} in {view}"
        );
    }
    assert!(view.get("token").is_none());
    assert!(view.get("id").is_none(), "the public page has no row id");

    // Wrong password is a 401, never an existence oracle for the link.
    let (status, error) = server
        .post(
            &format!("/api/share/{share_token}/access"),
            None,
            Some(json!({"password": "wrong"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");
    assert_eq!(error["error"]["code"], "invalid_share_password");

    // The right password returns a working, scoped download ticket.
    let (status, access) = server
        .post(
            &format!("/api/share/{share_token}/access"),
            None,
            Some(json!({"password": "open sesame"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{access}");
    let url = access["url"].as_str().expect("url").to_string();
    assert!(
        url.starts_with(&format!(
            "/api/games/{game_id}/download?versionId={version_id}"
        )),
        "{url}"
    );
    assert!(url.contains("token="), "{url}");
    assert!(access["expiresAt"].as_str().is_some());

    let response = server
        .client
        .get(format!("{}{}", server.url, url))
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.content_length(), Some(file_size));
    assert_eq!(
        response.bytes().await.expect("body").len() as u64,
        file_size
    );

    // Range requests work unchanged through the ticket.
    let response = server
        .client
        .get(format!("{}{}", server.url, url))
        .header(RANGE, "bytes=0-99")
        .send()
        .await
        .expect("range download");
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.bytes().await.expect("body").len(), 100);

    // The management list has the counter and the game title, never the token.
    let (status, shares) = server.get("/api/shares", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{shares}");
    let items = shares.as_array().expect("shares");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["downloadCount"], 1);
    assert_eq!(items[0]["gameTitle"], "Hades");
    assert!(items[0].get("token").is_none(), "tokens are shown once");
    assert!(items[0].get("passwordHash").is_none());

    // A plain account cannot create, list or revoke share links.
    let (status, viewer) = server
        .post(
            "/api/users",
            Some(&token),
            Some(json!({"username": "viewer", "password": ADMIN_PASSWORD, "role": "user"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{viewer}");
    let (status, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "viewer", "password": ADMIN_PASSWORD})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let viewer_token = login["token"].as_str().expect("viewer token");
    let (status, _) = server
        .post(
            &format!("/api/games/{game_id}/shares"),
            Some(viewer_token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = server.get("/api/shares", Some(viewer_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = server
        .delete(&format!("/api/shares/{share_id}"), viewer_token)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Revocation cuts public access immediately.
    let (status, _) = server
        .delete(&format!("/api/shares/{share_id}"), &token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = server.get(&format!("/api/share/{share_token}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, error) = server
        .post(
            &format!("/api/share/{share_token}/access"),
            None,
            Some(json!({"password": "open sesame"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    assert_eq!(error["error"]["code"], "share_not_found");

    // Unknown tokens answer the same 404.
    let (status, error) = server.get("/api/share/lsx_unknown", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "share_not_found");

    server.stop().await;
}

#[tokio::test]
async fn share_links_enforce_limits_expiry_and_validation() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    server.write_game("Hades (2020).zip", 1024 * 1024 + 32);
    server.write_game("Bastion (2011).zip", 1024 * 1024 + 48);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games", Some(&token)).await;
    let games = list["items"].as_array().expect("games");
    let game_id = |title: &str| {
        games
            .iter()
            .find(|game| game["title"] == title)
            .and_then(|game| game["id"].as_i64())
            .unwrap_or_else(|| panic!("missing game {title}: {list}"))
    };
    let hades = game_id("Hades");
    let bastion = game_id("Bastion");
    let (_, detail) = server
        .get(&format!("/api/games/{hades}"), Some(&token))
        .await;
    let hades_version = detail["versions"][0]["id"].as_i64().expect("version id");
    let (_, other) = server
        .get(&format!("/api/games/{bastion}"), Some(&token))
        .await;
    let bastion_version = other["versions"][0]["id"].as_i64().expect("version id");

    // Payload validation.
    let cases = [
        (json!({"expiresInHours": 0}), "invalid_expiry"),
        (json!({"expiresInHours": 9000}), "invalid_expiry"),
        (json!({"maxDownloads": 0}), "invalid_max_downloads"),
        (json!({"maxDownloads": 100001}), "invalid_max_downloads"),
        (json!({"password": "abc"}), "invalid_password"),
        (json!({"versionId": bastion_version}), "invalid_version"),
    ];
    for (body, code) in cases {
        let (status, error) = server
            .post(
                &format!("/api/games/{hades}/shares"),
                Some(&token),
                Some(body.clone()),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} -> {error}");
        assert_eq!(error["error"]["code"], code, "{body} -> {error}");
    }

    let (status, error) = server
        .post("/api/games/999999/shares", Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    assert_eq!(error["error"]["code"], "game_not_found");

    // A link without a version follows the latest version at access time.
    let (status, share) = server
        .post(
            &format!("/api/games/{hades}/shares"),
            Some(&token),
            Some(json!({"maxDownloads": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{share}");
    let share_id = share["id"].as_i64().expect("id");
    let share_token = share["token"].as_str().expect("token").to_string();
    assert!(share["versionId"].is_null(), "{share}");

    // No body at all: the access route treats it as "no password".
    let (status, access) = server
        .post(&format!("/api/share/{share_token}/access"), None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{access}");
    assert!(
        access["url"]
            .as_str()
            .expect("url")
            .contains(&format!("versionId={hades_version}")),
        "{access}"
    );

    let (status, error) = server
        .post(&format!("/api/share/{share_token}/access"), None, None)
        .await;
    assert_eq!(status, StatusCode::GONE, "{error}");
    assert_eq!(error["error"]["code"], "share_exhausted");
    let (_, shares) = server.get("/api/shares", Some(&token)).await;
    let item = shares
        .as_array()
        .expect("shares")
        .iter()
        .find(|share| share["id"] == share_id)
        .expect("share row");
    assert_eq!(item["downloadCount"], 1);

    // Expiry is checked on both the metadata page (404) and access (410).
    let (status, expiring) = server
        .post(
            &format!("/api/games/{hades}/shares"),
            Some(&token),
            Some(json!({"expiresInHours": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{expiring}");
    let expiring_id = expiring["id"].as_i64().expect("id");
    let expiring_token = expiring["token"].as_str().expect("token").to_string();
    let past = ludex_server::db::db_time(chrono::Utc::now() - chrono::Duration::hours(2));
    patch_share_expiry(&server, expiring_id, &past).await;

    let (status, error) = server
        .get(&format!("/api/share/{expiring_token}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    let (status, error) = server
        .post(&format!("/api/share/{expiring_token}/access"), None, None)
        .await;
    assert_eq!(status, StatusCode::GONE, "{error}");
    assert_eq!(error["error"]["code"], "share_expired");

    // Revoking an unknown row is a 404, not a silent success.
    let (status, error) = server.delete("/api/shares/999999", &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");

    // Public access is rate-limited per token, even without a password.
    let (status, limited) = server
        .post(
            &format!("/api/games/{hades}/shares"),
            Some(&token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{limited}");
    let limited_token = limited["token"].as_str().expect("token").to_string();
    for _ in 0..30 {
        let (status, body) = server
            .post(&format!("/api/share/{limited_token}/access"), None, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, error) = server
        .post(&format!("/api/share/{limited_token}/access"), None, None)
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{error}");
    assert_eq!(error["error"]["code"], "rate_limited");

    server.stop().await;
}
