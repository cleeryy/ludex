//! Account and server interface-language preferences (#188).
//!
//! The server is the source of truth for both: an account stores one canonical
//! locale, the administrator stores a default for new visitors, and `/api/me`,
//! login and the public `/api/config` expose them. Unknown or below-the-gate
//! codes are rejected, because the picker would not offer them either.

mod common;

use common::{TestServer, ADMIN_PASSWORD};
use ludex_server::config::Config;
use reqwest::StatusCode;
use serde_json::{json, Value};

fn error_code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn server_default_locale_is_public_and_admin_managed() {
    let server = TestServer::start().await;
    let (status, config) = server.get("/api/config", None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["defaultLocale"], "", "empty means System/English");

    let (admin, _) = server.setup().await;
    let (status, settings) = server.get("/api/admin/server", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["defaultLocale"], "");

    let (status, settings) = server
        .put(
            "/api/admin/server",
            &admin,
            json!({ "defaultLocale": "FR" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["defaultLocale"], "fr", "codes are canonicalised");

    let (_, config) = server.get("/api/config", None).await;
    assert_eq!(config["defaultLocale"], "fr");

    let (status, settings) = server
        .put("/api/admin/server", &admin, json!({ "defaultLocale": "" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["defaultLocale"], "");
    let (_, config) = server.get("/api/config", None).await;
    assert_eq!(config["defaultLocale"], "");

    server.stop().await;
}

#[tokio::test]
async fn server_default_locale_rejects_unknown_and_gated_codes() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let (status, body) = server
        .put(
            "/api/admin/server",
            &admin,
            json!({ "defaultLocale": "zz" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "invalid_locale");

    let (status, body) = server
        .put(
            "/api/admin/server",
            &admin,
            json!({ "defaultLocale": "ar" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "locale_unavailable");

    let (status, body) = server
        .put(
            "/api/admin/server",
            &admin,
            json!({ "defaultLocale": "en-XA" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "locale_unavailable");

    // A rejected write never changes the setting.
    let (_, config) = server.get("/api/config", None).await;
    assert_eq!(config["defaultLocale"], "");

    server.stop().await;
}

#[tokio::test]
async fn only_an_administrator_can_change_the_server_default() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    server
        .create_user(&admin, "member", ADMIN_PASSWORD, "user")
        .await;
    let (status, login) = server.login("member", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let token = login["token"].as_str().expect("token");

    let (status, body) = server
        .put("/api/admin/server", token, json!({ "defaultLocale": "fr" }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, _) = server.get("/api/admin/server", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    server.stop().await;
}

#[tokio::test]
async fn account_locale_round_trips_through_me_and_login() {
    let server = TestServer::start().await;
    let (token, user) = server.setup().await;
    assert!(user["locale"].is_null(), "a fresh account inherits: {user}");

    let (status, updated) = server
        .patch("/api/account/profile", &token, json!({ "locale": "pt-br" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["locale"], "pt-BR");

    let (status, me) = server.get("/api/auth/me", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["user"]["locale"], "pt-BR");

    let (status, login) = server.login("admin", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert_eq!(login["user"]["locale"], "pt-BR");

    // An absent field leaves the preference alone; null clears it.
    let (status, unchanged) = server
        .patch("/api/account/profile", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{unchanged}");
    assert_eq!(unchanged["locale"], "pt-BR");

    let (status, cleared) = server
        .patch("/api/account/profile", &token, json!({ "locale": null }))
        .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert!(cleared["locale"].is_null(), "{cleared}");

    server.stop().await;
}

#[tokio::test]
async fn account_locale_rejects_unknown_and_gated_codes() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let (status, body) = server
        .patch("/api/account/profile", &token, json!({ "locale": "zz" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "invalid_locale");

    let (status, body) = server
        .patch("/api/account/profile", &token, json!({ "locale": "ar" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "locale_unavailable");

    let (_, me) = server.get("/api/auth/me", Some(&token)).await;
    assert!(me["user"]["locale"].is_null(), "{me}");

    server.stop().await;
}

#[tokio::test]
async fn migration_leaves_existing_accounts_inheriting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let games_dir = temp.path().join("games");
    let data_dir = temp.path().join("data");
    let media_dir = temp.path().join("media");
    let config = Config::for_local(games_dir, data_dir, media_dir);
    for dir in [&config.games_dir, &config.data_dir, &config.media_dir] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let pool = ludex_server::db::connect(&config.database_url)
        .await
        .expect("connect");

    let columns = ludex_server::db::query("PRAGMA table_info(users)")
        .fetch_all(&pool)
        .await
        .expect("columns");
    assert!(
        columns
            .iter()
            .any(|row| row.get::<String, _>("name") == "locale"),
        "0029 must add users.locale"
    );

    // A row written the way every pre-#188 account was: no locale column.
    ludex_server::db::query(
        "INSERT INTO users (username, password_hash, role, activated, created_at, updated_at)
         VALUES ('legacy', 'hash', 'user', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("insert legacy user");

    let locale = ludex_server::db::query_scalar::<Option<String>>(
        "SELECT locale FROM users WHERE username = 'legacy'",
    )
    .fetch_one(&pool)
    .await
    .expect("locale");
    assert_eq!(locale, None, "backfill means inherit, never English");
}
