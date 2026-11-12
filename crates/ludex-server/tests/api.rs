//! End-to-end HTTP tests against a real server on an ephemeral port.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ludex_server::{start, Config, ServerHandle};
use rand::RngCore;
use reqwest::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
    RANGE,
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

/// Links an account to an OIDC identity directly. The test owns the same
/// SQLite file as the server, which lets the SSO read-only rules be exercised
/// without running a real provider.
async fn link_oidc_identity(server: &TestServer, user_id: i64, issuer: &str, subject: &str) {
    let url = ludex_server::config::default_database_url(&server.temp.path().join("data"));
    let pool = ludex_server::db::connect(&url)
        .await
        .expect("connect test db");
    ludex_server::db::query("UPDATE users SET oidc_issuer = ?, oidc_subject = ? WHERE id = ?")
        .bind(issuer)
        .bind(subject)
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("link oidc identity");
    pool.close().await;
}

/// Reads audit entries for one action, waiting for the async writer.
async fn wait_for_audit(server: &TestServer, token: &str, action: &str) -> Vec<Value> {
    for _ in 0..200 {
        let (status, body) = server
            .get(&format!("/api/audit?action={action}&limit=50"), Some(token))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let items = body["items"].as_array().cloned().unwrap_or_default();
        if !items.is_empty() {
            return items;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no audit entry for {action}");
}

/// Asserts the serialised value never carries credential or SSO-subject material.
fn assert_no_secrets(value: &Value, context: &str) {
    let raw = serde_json::to_string(value).expect("json");
    for needle in ["argon2", "passwordHash", "oidcSubject", "subject-123"] {
        assert!(!raw.contains(needle), "{context} leaks {needle}: {raw}");
    }
}

#[tokio::test]
async fn admin_can_edit_local_accounts_and_reset_passwords() {
    let server = TestServer::start().await;
    let (admin_token, admin_user) = server.setup().await;
    let admin_id = admin_user["id"].as_i64().expect("admin id");

    let (status, bob) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({
                "username": "bob",
                "password": "password123",
                "email": "bob@example.com",
                "role": "user"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bob}");
    let bob_id = bob["id"].as_i64().expect("bob id");
    assert_eq!(bob["source"], "local");
    assert_eq!(bob["oidcIssuer"], Value::Null);
    assert_no_secrets(&bob, "created user");

    let (status, updated) = server
        .patch(
            &format!("/api/users/{bob_id}"),
            &admin_token,
            json!({
                "username": "robert",
                "email": "robert@example.com",
                "password": "newpassword123",
                "role": "editor"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["username"], "robert");
    assert_eq!(updated["email"], "robert@example.com");
    assert_eq!(updated["role"], "editor");
    assert_eq!(updated["source"], "local");
    assert_no_secrets(&updated, "updated user");

    // The old username and the old password both stopped working.
    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "newpassword123"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "robert", "password": "password123"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "robert", "password": "newpassword123"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert_eq!(login["user"]["source"], "local");

    // A duplicate username is rejected with a stable code.
    let (status, carol) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({"username": "carol", "password": "password123", "role": "user"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{carol}");
    let carol_id = carol["id"].as_i64().expect("carol id");
    let (status, error) = server
        .patch(
            &format!("/api/users/{carol_id}"),
            &admin_token,
            json!({"username": "robert"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "username_taken");

    let (status, error) = server
        .patch(
            &format!("/api/users/{carol_id}"),
            &admin_token,
            json!({"password": "short"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_password");

    // Password resets get their own event; normal updates list changed fields.
    let resets = wait_for_audit(&server, &admin_token, "user.password_reset").await;
    assert_eq!(resets.len(), 1);
    assert_no_secrets(&resets[0]["metadata"], "password_reset audit");
    let updates = wait_for_audit(&server, &admin_token, "user.updated").await;
    let event = updates
        .iter()
        .find(|event| event["targetId"] == Value::String(bob_id.to_string()))
        .expect("bob update audit");
    let fields: Vec<&str> = event["metadata"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for expected in ["username", "email", "role"] {
        assert!(fields.contains(&expected), "missing {expected}: {fields:?}");
    }
    let raw_events = serde_json::to_string(&updates).expect("json");
    assert!(!raw_events.contains("newpassword123"));
    assert!(!raw_events.contains("robert@example.com"));

    // The list endpoint hides hashes and subjects for every account.
    let (status, users) = server.get("/api/users", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{users}");
    assert_no_secrets(&users, "user list");

    // Changing your own password through the admin form is allowed.
    let (status, _) = server
        .patch(
            &format!("/api/users/{admin_id}"),
            &admin_token,
            json!({"password": "adminpass456"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": "adminpass456"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    server.stop().await;
}

#[tokio::test]
async fn oidc_managed_identities_are_read_only_but_roles_are_not() {
    let server = TestServer::start().await;
    let (admin_token, _) = server.setup().await;

    let (status, bob) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({
                "username": "bob",
                "password": "password123",
                "email": "bob@example.com",
                "role": "user"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bob}");
    let bob_id = bob["id"].as_i64().expect("bob id");
    link_oidc_identity(&server, bob_id, "https://idp.example.com", "subject-123").await;

    let (status, users) = server.get("/api/users", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{users}");
    let listed = users
        .as_array()
        .expect("users")
        .iter()
        .find(|user| user["id"] == json!(bob_id))
        .expect("bob listed");
    assert_eq!(listed["source"], "oidc");
    assert_eq!(listed["oidcIssuer"], "https://idp.example.com");
    assert!(listed.get("oidcSubject").is_none());
    assert_no_secrets(&users, "user list");

    // Identity fields are owned by the provider.
    for patch in [
        json!({"username": "robert"}),
        json!({"email": "robert@example.com"}),
        json!({"password": "newpassword123"}),
    ] {
        let (status, error) = server
            .patch(&format!("/api/users/{bob_id}"), &admin_token, patch.clone())
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{patch}: {error}");
        assert_eq!(error["error"]["code"], "managed_by_idp", "{error}");
        let message = error["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains("https://idp.example.com"), "{message}");
    }

    // Local authorisation controls stay editable.
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
    assert_eq!(updated["username"], "bob");
    assert_eq!(updated["email"], "bob@example.com");
    assert_eq!(updated["source"], "oidc");
    assert_no_secrets(&updated, "updated oidc user");

    // Disabled linked accounts cannot sign in with their local password.
    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "password123"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Neither the response nor the audit trail carries the OIDC subject.
    let updates = wait_for_audit(&server, &admin_token, "user.updated").await;
    let event = updates
        .iter()
        .find(|event| event["targetId"] == Value::String(bob_id.to_string()))
        .expect("bob update audit");
    assert_eq!(event["metadata"]["fields"], json!(["role", "activated"]));
    assert_no_secrets(&Value::Array(updates), "user.updated audit");
    assert_no_secrets(&users, "oidc user list");
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
            Some(json!({"name": "Night shelf", "visibility": "public", "gameIds": [hades]})),
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
    // The shelf was published, so other accounts see it on the profile.
    assert_eq!(admin_profile["stats"]["collections"], 1, "{admin_profile}");
    assert_eq!(admin_profile["collections"][0]["id"], collection_id);
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

fn write_manifest_zip(path: &Path) {
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(path).expect("zip file");
    let mut archive = zip::ZipWriter::new(file);
    archive
        .add_directory("Resumable/", SimpleFileOptions::default())
        .expect("directory");
    archive
        .start_file("Resumable/game.exe", zip_options())
        .expect("stored entry");
    archive.write_all(b"MZ resumable").expect("write");
    archive
        .start_file(
            "Resumable/big.bin",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .expect("deflated entry");
    archive
        .write_all(
            &(0..300_000)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<u8>>(),
        )
        .expect("write big");
    // The scanner ignores archives smaller than 1 MiB.
    archive
        .start_file("Resumable/pad.bin", zip_options())
        .expect("pad");
    archive
        .write_all(&vec![0_u8; 1024 * 1024 + 1])
        .expect("write pad");
    archive
        .start_file("Resumable/run.sh", zip_options())
        .expect("script");
    archive
        .write_all(b"#!/bin/sh\nexit 0\n")
        .expect("write script");
    archive.finish().expect("finish zip");
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

/// Copies the committed RAR5 fixture and pads it past the scanner's 1 MiB
/// minimum. libunrar stops at the end-of-archive block, so trailing bytes are
/// ignored; the entries themselves are a real RAR5 archive created with
/// RAR 6.24.
fn write_test_rar(path: &Path) {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ludex-core/tests/fixtures/rar5.rar");
    let mut bytes = std::fs::read(&fixture).expect("read rar fixture");
    let mut pad = vec![0_u8; 1024 * 1024 + 1 - bytes.len()];
    rand::thread_rng().fill_bytes(&mut pad);
    bytes.extend_from_slice(&pad);
    std::fs::write(path, bytes).expect("write rar");
    let size = std::fs::metadata(path).expect("rar metadata").len();
    assert!(
        size > 1024 * 1024,
        "rar fixture must exceed the scanner minimum, got {size} bytes"
    );
}

/// A `.zip`-named file whose magic bytes are not a supported archive. The
/// scanner still indexes it, and `/stream` must answer 415 so clients fall back
/// to `/download`.
fn write_unsupported_archive(path: &Path) {
    let mut file = std::fs::File::create(path).expect("unsupported file");
    file.write_all(b"plain text, not an archive")
        .expect("magic");
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
    let unsupported_path = server.games_dir.join("Unreadable Game (2020).zip");
    write_unsupported_archive(&unsupported_path);
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
    assert_eq!(
        response
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "a reverse proxy must not buffer the stream"
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

    // Files that are not a supported archive are rejected with 415; clients
    // fall back to /download.
    let unsupported_game = items
        .iter()
        .find(|item| item["title"] == "Unreadable Game")
        .expect("unsupported game");
    let unsupported_game_id = unsupported_game["id"].as_i64().expect("unsupported id");
    let (_, unsupported_detail) = server
        .get(&format!("/api/games/{unsupported_game_id}"), Some(&token))
        .await;
    let unsupported_version_id = unsupported_detail["versions"][0]["id"]
        .as_i64()
        .expect("unsupported version id");
    let response = server
        .client
        .get(format!(
            "{}/api/games/{unsupported_game_id}/versions/{unsupported_version_id}/stream",
            server.url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("unsupported stream");
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let error: Value = response.json().await.expect("unsupported error");
    assert_eq!(error["error"]["code"], "stream_unsupported");

    server.stop().await;
}

#[tokio::test]
async fn stream_manifest_and_entry_ranges() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let zip_path = server.games_dir.join("Resumable Game (2022).zip");
    write_manifest_zip(&zip_path);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games?limit=10", Some(&token)).await;
    let game_id = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["title"] == "Resumable Game")
        .and_then(|item| item["id"].as_i64())
        .expect("game id");
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    let version_id = detail["versions"][0]["id"].as_i64().expect("version id");

    // Same auth rules as the tar stream.
    let (status, error) = server
        .get(
            &format!("/api/games/{game_id}/versions/{version_id}/stream/manifest"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "unauthorized");
    let (status, _) = server
        .get(
            &format!(
                "/api/games/{game_id}/versions/{}/stream/manifest",
                version_id + 9999
            ),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, manifest) = server
        .get(
            &format!("/api/games/{game_id}/versions/{version_id}/stream/manifest"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{manifest}");
    assert_eq!(manifest["format"], "zip");
    let entries = manifest["entries"].as_array().expect("entries");
    let paths: Vec<&str> = entries
        .iter()
        .map(|entry| entry["path"].as_str().expect("path"))
        .collect();
    assert_eq!(
        paths,
        vec![
            "Resumable",
            "Resumable/game.exe",
            "Resumable/big.bin",
            "Resumable/pad.bin",
            "Resumable/run.sh"
        ],
        "entries follow the central directory order"
    );
    assert_eq!(manifest["entryCount"].as_u64(), Some(5));
    assert_eq!(entries[0]["isDir"], true);
    assert_eq!(entries[1]["isDir"], false);
    assert_eq!(entries[1]["size"].as_u64(), Some(12));
    assert_eq!(
        manifest["totalBytes"].as_u64(),
        Some(12 + 300_000 + 1024 * 1024 + 1 + 17)
    );
    assert!(entries[2]["crc32"].as_u64().unwrap_or(0) > 0);
    assert_eq!(entries[4]["mode"].as_u64(), Some(0o755));
    let big_size = entries[2]["size"].as_u64().expect("big size");
    let big_index = entries[2]["index"].as_u64().expect("big index");
    let stored_index = entries[1]["index"].as_u64().expect("stored index");
    let dir_index = entries[0]["index"].as_u64().expect("dir index");

    // Full entry, stored: exact bytes and Content-Length.
    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/versions/{version_id}/stream/entries/{stored_index}",
            server.url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("stored entry");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some("12")
    );
    assert_eq!(
        response
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok()),
        Some("bytes")
    );
    assert_eq!(
        response
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "a reverse proxy must not buffer an entry"
    );
    assert_eq!(
        response.bytes().await.expect("body").as_ref(),
        b"MZ resumable"
    );

    // A single range on a deflated entry: 206 + Content-Range.
    let range_start = big_size - 10;
    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/versions/{version_id}/stream/entries/{big_index}",
            server.url
        ))
        .bearer_auth(&token)
        .header(RANGE, format!("bytes={range_start}-"))
        .send()
        .await
        .expect("ranged entry");
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let expected_range = format!("bytes {range_start}-{}/{big_size}", big_size - 1);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok()),
        Some(expected_range.as_str())
    );
    let expected: Vec<u8> = (0..300_000).map(|index| (index % 251) as u8).collect();
    let body = response.bytes().await.expect("range body");
    assert_eq!(body.as_ref(), &expected[range_start as usize..]);

    // Suffix ranges and a middle range are supported too.
    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/versions/{version_id}/stream/entries/{big_index}",
            server.url
        ))
        .bearer_auth(&token)
        .header(RANGE, "bytes=-4")
        .send()
        .await
        .expect("suffix entry");
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.bytes().await.expect("suffix body").as_ref(),
        &expected[expected.len() - 4..]
    );

    // Unsatisfiable and malformed ranges answer 416 with the total size.
    for range in ["bytes=999999999-", "bytes=10-5", "bytes=0-1,3-4"] {
        let response = server
            .client
            .get(format!(
                "{}/api/games/{game_id}/versions/{version_id}/stream/entries/{stored_index}",
                server.url
            ))
            .bearer_auth(&token)
            .header(RANGE, range)
            .send()
            .await
            .expect("bad range");
        assert_eq!(
            response.status(),
            StatusCode::RANGE_NOT_SATISFIABLE,
            "{range}"
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            Some("bytes */12"),
            "{range}"
        );
    }

    // Unknown indexes and directory entries are 404s.
    let (status, error) = server
        .get(
            &format!(
                "/api/games/{game_id}/versions/{version_id}/stream/entries/{}",
                entries.len() + 10
            ),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "entry_not_found");
    let (status, error) = server
        .get(
            &format!("/api/games/{game_id}/versions/{version_id}/stream/entries/{dir_index}"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "entry_not_found");

    // Download tickets authenticate both endpoints.
    let (_, ticket) = server
        .post(
            &format!("/api/games/{game_id}/download-ticket"),
            Some(&token),
            Some(json!({"versionId": version_id})),
        )
        .await;
    let ticket_token = ticket["url"]
        .as_str()
        .expect("ticket url")
        .rsplit_once("token=")
        .expect("ticket token")
        .1;
    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/versions/{version_id}/stream/entries/{stored_index}?token={ticket_token}",
            server.url
        ))
        .send()
        .await
        .expect("ticket entry");
    assert_eq!(response.status(), StatusCode::OK);
    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/versions/{version_id}/stream/manifest?token={ticket_token}",
            server.url
        ))
        .send()
        .await
        .expect("ticket manifest");
    assert_eq!(response.status(), StatusCode::OK);

    // Non-ZIP archives still answer 415 so the client falls back.
    let rar_path = server.games_dir.join("Resumable Rar (2022).rar");
    write_test_rar(&rar_path);
    run_scan(&server, &token).await;
    let (_, list) = server.get("/api/games?limit=10", Some(&token)).await;
    let rar_id = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["title"] == "Resumable Rar")
        .and_then(|item| item["id"].as_i64())
        .expect("rar game");
    let (_, rar_detail) = server
        .get(&format!("/api/games/{rar_id}"), Some(&token))
        .await;
    let rar_version = rar_detail["versions"][0]["id"]
        .as_i64()
        .expect("rar version");
    let (status, error) = server
        .get(
            &format!("/api/games/{rar_id}/versions/{rar_version}/stream/manifest"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(error["error"]["code"], "stream_unsupported");
    let (status, _) = server
        .get(
            &format!("/api/games/{rar_id}/versions/{rar_version}/stream/entries/0"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    server.stop().await;
}

#[tokio::test]
async fn stream_manifest_rejects_path_traversal() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let zip_path = server.games_dir.join("Traversal Game (2022).zip");
    let file = std::fs::File::create(&zip_path).expect("zip file");
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("Game/../../escape.txt", zip_options())
        .expect("entry");
    archive.write_all(b"nope").expect("write");
    // The scanner ignores archives smaller than 1 MiB.
    archive
        .start_file("Game/pad.bin", zip_options())
        .expect("pad");
    archive
        .write_all(&vec![0_u8; 1024 * 1024 + 1])
        .expect("write pad");
    archive.finish().expect("finish");
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games?limit=10", Some(&token)).await;
    let game_id = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["title"] == "Traversal Game")
        .and_then(|item| item["id"].as_i64())
        .expect("game id");
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    let version_id = detail["versions"][0]["id"].as_i64().expect("version id");

    let (status, error) = server
        .get(
            &format!("/api/games/{game_id}/versions/{version_id}/stream/manifest"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error["error"]["code"], "stream_failed");

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
async fn stream_install_rar_as_tar() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let rar_path = server.games_dir.join("Rar Stream Game (2022).rar");
    write_test_rar(&rar_path);
    run_scan(&server, &token).await;

    let (_, list) = server.get("/api/games?limit=10", Some(&token)).await;
    let game = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["title"] == "Rar Stream Game")
        .expect("rar game");
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
        .expect("rar stream");
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
    let content_length: u64 = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .expect("content length");
    let body = response.bytes().await.expect("rar tar body");
    assert_eq!(content_length, body.len() as u64);

    let files = read_tar(&body);
    assert_eq!(files.len(), 4, "directory entries must not be streamed");
    assert_eq!(
        files.get("Game/readme.txt").map(Vec::as_slice),
        Some(b"hello".as_slice())
    );
    assert_eq!(
        files.get("Game/sub/game.sh").map(Vec::as_slice),
        Some(b"#!/bin/sh\n".as_slice())
    );
    assert_eq!(
        files.get("Game/unicode-\u{e9}.txt").map(Vec::as_slice),
        Some("café".as_bytes())
    );
    assert_eq!(files.get("Game/empty.txt").map(Vec::len), Some(0));

    // Streaming must not leave anything behind beyond the source archive: no
    // staged object and no core scratch directory in the server data dir.
    let leftovers: Vec<String> = std::fs::read_dir(&server.games_dir)
        .expect("games dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert_eq!(
        leftovers,
        vec!["Rar Stream Game (2022).rar".to_string()],
        "unexpected files in the games dir"
    );
    let staging_dir = server.temp.path().join("data/tmp");
    let staged = std::fs::read_dir(&staging_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(staged, 0, "staged copies must be removed after streaming");

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
    assert!(versions.iter().all(|version| {
        version["archiveHash"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256-sample:"))
    }));

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
    ludex_server::db::query("UPDATE share_links SET expires_at = ? WHERE id = ?")
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

/* ----------------------------- OIDC admin UI (#103) ----------------------- */

/// Minimal discovery-only identity provider used by the admin OIDC tests.
async fn spawn_discovery_provider() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider");
    let address = listener.local_addr().expect("mock provider address");
    let issuer = format!("http://{address}");
    let base = issuer.clone();
    let router = axum::Router::new().route(
        "/.well-known/openid-configuration",
        axum::routing::get(move || {
            let base = base.clone();
            async move {
                axum::Json(json!({
                    "issuer": base,
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "jwks_uri": format!("{base}/jwks"),
                    "id_token_signing_alg_values_supported": ["RS256"],
                    "token_endpoint_auth_methods_supported": [
                        "client_secret_basic",
                        "client_secret_post"
                    ],
                }))
            }
        }),
    );
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (issuer, handle)
}

async fn member_token(server: &TestServer, admin: &str) -> String {
    let (status, body) = server
        .post(
            "/api/users",
            Some(admin),
            Some(json!({
                "username": "member",
                "password": "password123",
                "role": "user"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({ "username": "member", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().expect("member token").to_string()
}

#[tokio::test]
async fn admin_oidc_settings_require_an_administrator() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let (status, error) = server.get("/api/admin/oidc", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");

    let member = member_token(&server, &admin).await;
    let (status, error) = server.get("/api/admin/oidc", Some(&member)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");

    let (status, error) = server
        .put(
            "/api/admin/oidc",
            &member,
            json!({ "enabled": true, "issuer": "https://idp.example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");

    let (status, error) = server
        .post(
            "/api/admin/oidc/test",
            Some(&member),
            Some(json!({ "issuer": "https://idp.example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");

    server.stop().await;
}

#[tokio::test]
async fn admin_oidc_save_hot_reloads_and_a_bad_issuer_keeps_the_old_config() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let (issuer, provider) = spawn_discovery_provider().await;

    let (status, body) = server.get("/api/admin/oidc", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["settings"]["enabled"], false);
    assert_eq!(body["settings"]["secretConfigured"], false);
    assert_eq!(body["envLockedFields"], json!([]));
    assert_eq!(body["status"]["checkedAt"], Value::Null);

    // Discovery test runs against the submitted values and does not persist.
    let (status, tested) = server
        .post(
            "/api/admin/oidc/test",
            Some(&admin),
            Some(json!({
                "issuer": issuer,
                "clientId": "ludex",
                "scopes": "openid profile email"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{tested}");
    assert_eq!(tested["ok"], true);
    assert_eq!(tested["discovery"]["issuer"], issuer);
    assert_eq!(
        tested["discovery"]["authorizationEndpoint"],
        format!("{issuer}/authorize")
    );

    // A failed save must leave the previous (disabled) configuration active.
    let (status, error) = server
        .put(
            "/api/admin/oidc",
            &admin,
            json!({
                "enabled": true,
                "issuer": "http://127.0.0.1:1",
                "clientId": "ludex",
                "displayName": "Broken"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{error}");
    assert_eq!(error["error"]["code"], "oidc_discovery_failed");
    assert!(error["error"]["message"]
        .as_str()
        .expect("message")
        .contains("Could not reach"));
    let (_, config) = server.get("/api/config", None).await;
    assert_eq!(config["features"]["oidc"], false);
    assert_eq!(config["oidcDisplayName"], "SSO");

    // Saving a reachable provider swaps the client without a restart.
    let secret = "top-secret-client-value";
    let (status, saved) = server
        .put(
            "/api/admin/oidc",
            &admin,
            json!({
                "enabled": true,
                "issuer": issuer,
                "clientId": "ludex",
                "displayName": "Authentik",
                "publicBaseUrl": "https://ludex.example",
                "scopes": "openid profile email groups",
                "groupsClaim": "groups",
                "adminGroups": [" admins ", "", "Admins", "operators"],
                "replaceSecret": secret
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["settings"]["enabled"], true);
    assert_eq!(saved["settings"]["secretConfigured"], true);
    assert_eq!(saved["settings"]["displayName"], "Authentik");
    assert_eq!(
        saved["settings"]["adminGroups"],
        json!(["admins", "operators"])
    );
    assert_eq!(saved["status"]["ok"], true);
    let wire = serde_json::to_string(&saved).expect("json");
    assert!(
        !wire.contains(secret),
        "the client secret must never be returned by the API"
    );

    let (_, config) = server.get("/api/config", None).await;
    assert_eq!(config["features"]["oidc"], true);
    assert_eq!(config["oidcDisplayName"], "Authentik");

    // The login route uses the new client immediately.
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    let response = client
        .get(format!("{}/api/auth/oidc/start", server.url))
        .send()
        .await
        .expect("start");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("location")
        .to_string();
    assert!(
        location.starts_with(&format!("{issuer}/authorize")),
        "{location}"
    );
    assert!(location.contains("client_id=ludex"));
    assert!(location.contains("code_challenge_method=S256"));

    // A later failing save keeps the working provider.
    let (status, error) = server
        .put(
            "/api/admin/oidc",
            &admin,
            json!({
                "enabled": true,
                "issuer": "http://127.0.0.1:1",
                "clientId": "ludex",
                "displayName": "Broken again"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{error}");
    let (_, config) = server.get("/api/config", None).await;
    assert_eq!(config["features"]["oidc"], true);
    assert_eq!(config["oidcDisplayName"], "Authentik");
    let (_, admin_view) = server.get("/api/admin/oidc", Some(&admin)).await;
    assert_eq!(admin_view["settings"]["issuer"], issuer);
    assert_eq!(admin_view["settings"]["displayName"], "Authentik");

    // Resetting restores environment-only configuration (here: disabled).
    let (status, reset) = server
        .post("/api/admin/oidc/reset", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["settings"]["enabled"], false);
    let (_, config) = server.get("/api/config", None).await;
    assert_eq!(config["features"]["oidc"], false);

    provider.abort();
    server.stop().await;
}

#[tokio::test]
async fn saved_oidc_settings_survive_a_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (issuer, provider) = spawn_discovery_provider().await;
    let mut config = Config::for_local(
        temp.path().join("games"),
        temp.path().join("data"),
        temp.path().join("media"),
    );
    config.local_mode = false;
    config.bind = "127.0.0.1:0".parse().expect("bind");
    let client = Client::builder().build().expect("client");

    let handle = start(config.clone()).await.expect("first start");
    let response = client
        .post(format!("{}/api/setup", handle.url))
        .json(&json!({ "username": "admin", "password": ADMIN_PASSWORD }))
        .send()
        .await
        .expect("setup");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.json::<Value>().await.expect("setup json");
    let admin = body["token"].as_str().expect("token").to_string();

    let response = client
        .put(format!("{}/api/admin/oidc", handle.url))
        .bearer_auth(&admin)
        .json(&json!({
            "enabled": true,
            "issuer": issuer,
            "clientId": "ludex",
            "displayName": "Restart SSO",
            "replaceSecret": "restart-secret"
        }))
        .send()
        .await
        .expect("save");
    assert_eq!(response.status(), StatusCode::OK);

    let _ = handle.shutdown.send(());
    tokio::time::sleep(Duration::from_millis(150)).await;

    let handle = start(config).await.expect("second start");
    let config_body = client
        .get(format!("{}/api/config", handle.url))
        .send()
        .await
        .expect("config")
        .json::<Value>()
        .await
        .expect("config json");
    assert_eq!(config_body["features"]["oidc"], true);
    assert_eq!(config_body["oidcDisplayName"], "Restart SSO");

    let admin_view = client
        .get(format!("{}/api/admin/oidc", handle.url))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("admin oidc")
        .json::<Value>()
        .await
        .expect("admin json");
    assert_eq!(admin_view["settings"]["secretConfigured"], true);
    assert_eq!(admin_view["settings"]["issuer"], issuer);

    let _ = handle.shutdown.send(());
    provider.abort();
}
