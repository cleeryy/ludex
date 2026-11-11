//! PostgreSQL backend integration test (#46).
//!
//! Runs only when `LUDEX_TEST_POSTGRES_URL` points at a **disposable**
//! database, for example:
//!
//! ```text
//! docker run --rm -d --name ludex-pg -p 55432:5432 \
//!   -e POSTGRES_PASSWORD=ludex -e POSTGRES_DB=ludex postgres:16-alpine
//! LUDEX_TEST_POSTGRES_URL=postgres://postgres:ludex@127.0.0.1:55432/ludex \
//!   cargo test -p ludex-server --test postgres
//! ```
//!
//! The test resets the `public` schema of that database, then covers
//! migrations, setup, login, the game list, progress and cloud saves against a
//! real server. Without the variable it reports a skip and exits green so the
//! default SQLite suite stays hermetic.

use ludex_server::db::{self, Database};
use ludex_server::{start, Config, ServerHandle};
use reqwest::multipart::{Form, Part};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tempfile::TempDir;

const PASSWORD: &str = "password123";

fn postgres_url() -> Option<String> {
    std::env::var("LUDEX_TEST_POSTGRES_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn reset_schema(url: &str) {
    let admin = sqlx::PgPool::connect(url)
        .await
        .expect("connect to the PostgreSQL test database");
    sqlx::query("DROP SCHEMA IF EXISTS public CASCADE")
        .execute(&admin)
        .await
        .expect("drop schema");
    sqlx::query("CREATE SCHEMA public")
        .execute(&admin)
        .await
        .expect("create schema");
    admin.close().await;
}

struct Fixture {
    #[allow(dead_code)]
    temp: TempDir,
    #[allow(dead_code)]
    handle: ServerHandle,
    url: String,
    client: reqwest::Client,
    token: String,
    game_id: i64,
}

async fn fixture(pg_url: &str) -> Fixture {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ludex_server=debug".into()),
        )
        .try_init();
    reset_schema(pg_url).await;

    let temp = tempfile::tempdir().expect("tempdir");
    let games = temp.path().join("games");
    let data = temp.path().join("data");
    let media = temp.path().join("media");
    for directory in [&games, &data, &media] {
        std::fs::create_dir_all(directory).expect("mkdir");
    }

    let mut config: Config = Config::for_local(games, data, media);
    config.local_mode = false;
    config.bind = "127.0.0.1:0".parse().expect("bind");
    config.database_url = pg_url.to_string();
    config.database_max_connections = 4;
    let handle = start(config).await.expect("server starts on PostgreSQL");
    let url = handle.url.clone();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("client");

    // Health must report the active backend and a live pool.
    let health: Value = client
        .get(format!("{url}/api/health/details"))
        .send()
        .await
        .expect("health")
        .json()
        .await
        .expect("health json");
    assert_eq!(health["database"]["backend"], "postgres");
    assert_eq!(health["database"]["ok"], true);
    assert_eq!(health["database"]["maxConnections"], 4);

    // First-run setup, then a real login.
    let response = client
        .post(format!("{url}/api/setup"))
        .json(&json!({ "username": "admin", "password": PASSWORD }))
        .send()
        .await
        .expect("setup");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("setup json");
    let token = body["token"].as_str().expect("token").to_string();

    let response = client
        .post(format!("{url}/api/auth/login"))
        .json(&json!({ "username": "admin", "password": PASSWORD }))
        .send()
        .await
        .expect("login");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("login json");
    assert!(body["token"].is_string());

    // Seed one indexed game through the same public database layer.
    let pool: Database = db::connect(pg_url).await.expect("second pool");
    let now = db::db_time(db::now());
    let game_id = db::query(
        "INSERT INTO games
            (title, sort_title, group_key, kind, platform, genres_json, created_at, updated_at)
         VALUES (?, ?, ?, 'rom', 'snes', ?, ?, ?)",
    )
    .bind("Celeste")
    .bind("celeste")
    .bind("celeste")
    .bind("[\"Platformer\",\"Adventure\"]")
    .bind(&now)
    .bind(&now)
    .returning_id(&pool)
    .await
    .expect("insert game");
    db::query(
        "INSERT INTO game_versions
            (game_id, root_id, file_path, file_name, size, missing, indexed_at)
         VALUES (?, NULL, ?, 'Celeste.zip', 1024, 0, ?)",
    )
    .bind(game_id)
    .bind("Celeste.zip")
    .bind(&now)
    .execute(&pool)
    .await
    .expect("insert version");
    pool.close().await;

    Fixture {
        temp,
        handle,
        url,
        client,
        token,
        game_id,
    }
}

impl Fixture {
    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let response = self
            .client
            .get(format!("{}{path}", self.url))
            .bearer_auth(&self.token)
            .send()
            .await
            .expect("request");
        let status = response.status();
        let body = response.json().await.unwrap_or(Value::Null);
        (status, body)
    }
}

#[tokio::test]
async fn postgres_backend_covers_core_flows() {
    let Some(url) = postgres_url() else {
        eprintln!("skipped: LUDEX_TEST_POSTGRES_URL is not set");
        return;
    };
    let fixture = fixture(&url).await;

    // Migrations: every SQLite migration has a PostgreSQL mirror.
    let pool = db::connect(&url).await.expect("pool");
    assert_eq!(pool.backend(), db::Backend::Postgres);
    let applied: i64 = db::query_scalar::<i64>("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("migration count");
    let expected = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations_postgres"))
        .expect("read migrations")
        .count() as i64;
    assert_eq!(applied, expected, "all migrations applied");
    pool.close().await;

    // Library list.
    let (status, body) = fixture.get("/api/games").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Celeste");
    let game_id = fixture.game_id;
    assert_eq!(items[0]["id"], game_id);

    // Progress.
    let response = fixture
        .client
        .put(format!("{}/api/games/{game_id}/progress", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "state": "playing" }))
        .send()
        .await
        .expect("progress");
    assert_eq!(response.status(), StatusCode::OK);
    let (status, body) = fixture.get(&format!("/api/games/{game_id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "playing");

    // Playtime uses an upsert on progress.
    let response = fixture
        .client
        .post(format!("{}/api/games/{game_id}/playtime", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "minutes": 42 }))
        .send()
        .await
        .expect("playtime");
    assert_eq!(response.status(), StatusCode::OK);
    let (_, body) = fixture.get(&format!("/api/games/{game_id}")).await;
    assert_eq!(body["minutesPlayed"], 42);

    // Library filters run through the JSON array membership dialect helper.
    let (status, body) = fixture.get("/api/games?genre=Platformer").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().expect("items").len(), 1);
    let (status, body) = fixture.get("/api/games?genre=Horror").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().expect("items").len(), 0);

    // Statistics aggregate BIGINT columns (SUM is NUMERIC on PostgreSQL).
    let (status, body) = fixture.get("/api/stats").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["totalBytes"], 1024);

    // Collections: the case-insensitive unique index is an expression index.
    let response = fixture
        .client
        .post(format!("{}/api/collections", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "name": "Favorites" }))
        .send()
        .await
        .expect("collection");
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = fixture
        .client
        .post(format!("{}/api/collections", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "name": "favorites" }))
        .send()
        .await
        .expect("duplicate collection");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let (status, body) = fixture.get("/api/collections").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body
        .as_array()
        .expect("collections")
        .iter()
        .any(|row| row["name"] == "Favorites"));

    // API keys, audit log and the community feed use their own tables.
    let response = fixture
        .client
        .post(format!("{}/api/keys", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "name": "CI" }))
        .send()
        .await
        .expect("api key");
    assert_eq!(response.status(), StatusCode::CREATED);
    let (status, body) = fixture.get("/api/keys").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().expect("keys").len(), 1);
    let (status, body) = fixture.get("/api/audit?limit=10").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body["items"].as_array().expect("audit").is_empty());
    let (status, body) = fixture.get("/api/community/feed").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"].is_array());
    let (status, body) = fixture.get("/api/games/suggest?q=cel").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["suggestions"].as_array().expect("suggest").len(), 1);

    // Bookmark upsert and the library/integrity admin surfaces.
    let response = fixture
        .client
        .put(format!("{}/api/games/{game_id}/bookmark", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "bookmarked": true }))
        .send()
        .await
        .expect("bookmark");
    assert_eq!(response.status(), StatusCode::OK);
    let (status, body) = fixture.get("/api/library/roots").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().expect("roots").len(), 1);
    let (status, body) = fixture.get("/api/library/folders").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = fixture.get("/api/integrity/reports").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let response = fixture
        .client
        .post(format!("{}/api/integrity/check", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "scope": "archives" }))
        .send()
        .await
        .expect("integrity");
    assert!(
        response.status() == StatusCode::ACCEPTED || response.status() == StatusCode::OK,
        "integrity check returned {}",
        response.status()
    );
    let response = fixture
        .client
        .post(format!("{}/api/media/gc?dryRun=true", fixture.url))
        .bearer_auth(&fixture.token)
        .send()
        .await
        .expect("media gc");
    assert_eq!(response.status(), StatusCode::OK);

    // Shares, news and the admin provider/OIDC settings pages.
    let response = fixture
        .client
        .post(format!("{}/api/games/{game_id}/shares", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "label": "friends" }))
        .send()
        .await
        .expect("share");
    assert_eq!(response.status(), StatusCode::CREATED);
    let (status, body) = fixture.get("/api/shares").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().expect("shares").len(), 1);
    let (status, body) = fixture.get("/api/news").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = fixture.get("/api/admin/metadata-providers").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = fixture.get("/api/admin/oidc").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = fixture.get("/api/users").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = fixture.get("/api/me/achievements").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Achievements: the partial unique index is the ON CONFLICT target, and
    // the duplicate is reported without a panic.
    let pool = db::connect(&url).await.expect("pool");
    let definition = ludex_server::achievements::DefinitionInput {
        key: "first_step".to_string(),
        name: "First step".to_string(),
        description: None,
        icon_path: None,
        points: Some(10),
        hidden: None,
    };
    let first = ludex_server::achievements::create_definition(&pool, game_id, &definition)
        .await
        .expect("create achievement");
    assert!(first > 0);
    let duplicate = ludex_server::achievements::create_definition(&pool, game_id, &definition)
        .await
        .expect_err("duplicate achievement");
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
    let count: i64 = db::query_scalar::<i64>("SELECT COUNT(*) FROM achievements WHERE game_id = ?")
        .bind(game_id)
        .fetch_one(&pool)
        .await
        .expect("count achievements");
    assert_eq!(count, 1);
    pool.close().await;

    // Core gaming tables (#51/#52): emulator registry, case-insensitive name
    // uniqueness and controller profiles.
    let response = fixture
        .client
        .post(format!("{}/api/emulators", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({
            "name": "RetroArch",
            "command": "retroarch %ROM%",
            "platforms": ["snes"]
        }))
        .send()
        .await
        .expect("emulator");
    assert_eq!(response.status(), StatusCode::CREATED);
    let emulator: Value = response.json().await.expect("emulator json");
    let emulator_id = emulator["id"].as_i64().expect("emulator id");
    let response = fixture
        .client
        .post(format!("{}/api/emulators", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "name": "retroarch", "command": "retroarch %ROM%" }))
        .send()
        .await
        .expect("duplicate emulator");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let (status, body) = fixture.get("/api/emulators").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().expect("emulators").len(), 1);
    let response = fixture
        .client
        .put(format!("{}/api/emulators/defaults", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "defaults": [{ "platform": "snes", "emulatorId": emulator_id }] }))
        .send()
        .await
        .expect("emulator defaults");
    assert_eq!(response.status(), StatusCode::OK);
    let response = fixture
        .client
        .put(format!("{}/api/games/{game_id}/emulator", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "emulatorId": emulator_id }))
        .send()
        .await
        .expect("game emulator");
    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::NO_CONTENT,
        "game emulator returned {}",
        response.status()
    );
    let profile = ludex_core::gamepad::preset("dualsense");
    let response = fixture
        .client
        .put(format!("{}/api/controller-profiles", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "gameId": null, "profile": profile }))
        .send()
        .await
        .expect("controller profile");
    assert_eq!(response.status(), StatusCode::OK);
    let (status, body) = fixture.get("/api/controller-profiles").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["global"].is_object() || body.get("profiles").is_some(),
        "{body}"
    );

    // News creation uses RETURNING for the generated id.
    let response = fixture
        .client
        .post(format!("{}/api/news", fixture.url))
        .bearer_auth(&fixture.token)
        .json(&json!({ "title": "Patch 1.0", "body": "Celeste is out." }))
        .send()
        .await
        .expect("news");
    assert_eq!(response.status(), StatusCode::CREATED);
    let (status, body) = fixture.get("/api/news").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body
        .as_array()
        .expect("news")
        .iter()
        .any(|row| row["title"] == "Patch 1.0"));

    // Cloud saves: multipart upload, listing and download.
    let payload = b"save-archive-bytes".to_vec();
    let form = Form::new().text("label", "before the summit").part(
        "file",
        Part::bytes(payload.clone())
            .file_name("celeste.sav")
            .mime_str("application/octet-stream")
            .expect("mime"),
    );
    let response = fixture
        .client
        .post(format!("{}/api/games/{game_id}/saves", fixture.url))
        .bearer_auth(&fixture.token)
        .multipart(form)
        .send()
        .await
        .expect("save upload");
    assert_eq!(response.status(), StatusCode::CREATED);
    let snapshot: Value = response.json().await.expect("snapshot json");
    let snapshot_id = snapshot["id"].as_i64().expect("snapshot id");

    let (status, body) = fixture.get(&format!("/api/games/{game_id}/saves")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let snapshots = body.as_array().expect("snapshots");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0]["label"], "before the summit");

    let response = fixture
        .client
        .get(format!("{}/api/saves/{snapshot_id}/download", fixture.url))
        .bearer_auth(&fixture.token)
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.expect("bytes").as_ref(), payload);

    // Logout records the token in `token_revocations`; the next call is 401.
    let response = fixture
        .client
        .post(format!("{}/api/auth/logout", fixture.url))
        .bearer_auth(&fixture.token)
        .send()
        .await
        .expect("logout");
    assert!(
        response.status() == StatusCode::OK || response.status() == StatusCode::NO_CONTENT,
        "logout returned {}",
        response.status()
    );
    let (status, _) = fixture.get("/api/auth/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn postgres_reports_connection_errors() {
    let Some(_) = postgres_url() else {
        eprintln!("skipped: LUDEX_TEST_POSTGRES_URL is not set");
        return;
    };
    // An unreachable server fails fast with a readable context instead of
    // panicking (the message mentions the URL being invalid).
    let error = db::connect("postgres://ludex:secret@127.0.0.1:1/ludex")
        .await
        .expect_err("connection must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("connect to PostgreSQL"),
        "unexpected error: {message}"
    );
    assert!(
        !message.contains("secret"),
        "the password must not leak in errors: {message}"
    );
}
