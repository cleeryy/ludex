//! HTTP tests for the metadata provider plugin system (#28).
//!
//! Proves the admin CRUD contract (admin only, token write-only), the test
//! endpoint, and that enrichment uses a provider only when IGDB is absent and
//! survives a failing provider.

mod common;

use std::time::Duration;

use common::{TestServer, ADMIN_PASSWORD};
use reqwest::StatusCode;
use serde_json::{json, Value};

/// Minimal local provider answering every request with `body`.
struct MockProvider {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl MockProvider {
    async fn start(body: &'static str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock provider");
        let address = listener.local_addr().expect("address");
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buffer = [0_u8; 4096];
                    let _ = socket.read(&mut buffer).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}"
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            url: format!("http://{address}"),
            task,
        }
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn admin_server() -> (TestServer, String) {
    let server = TestServer::start_with(|config| {
        config.providers_allow_private = true;
    })
    .await;
    let (status, body) = server
        .post(
            "/api/setup",
            None,
            Some(json!({
                "username": "admin",
                "password": ADMIN_PASSWORD,
                "serverName": "Provider Test",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["token"].as_str().expect("token").to_string();
    (server, token)
}

fn provider_body(name: &str, base_url: &str, token: Option<&str>, priority: i64) -> Value {
    json!({
        "name": name,
        "baseUrl": base_url,
        "token": token,
        "priority": priority,
        "enabled": true,
        "mapping": {},
    })
}

#[tokio::test]
async fn admin_crud_never_returns_the_token() {
    let (server, token) = admin_server().await;
    let mock = MockProvider::start(r#"{"results":[]}"#).await;

    let (status, created) = server
        .post(
            "/api/admin/metadata-providers",
            Some(&token),
            Some(provider_body(
                "Fixture",
                &mock.url,
                Some("super-secret-token"),
                20,
            )),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_i64().expect("id");
    assert_eq!(created["name"], "Fixture");
    assert_eq!(created["secretConfigured"], true);
    assert_eq!(created["priority"], 20);
    assert_eq!(created["enabled"], true);
    assert!(
        !created.to_string().contains("super-secret-token"),
        "token leaked by create: {created}"
    );

    let (status, list) = server
        .get("/api/admin/metadata-providers", Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list.as_array().expect("array").len(), 1);
    assert!(!list.to_string().contains("super-secret-token"));

    // A blank token keeps the stored one.
    let (status, updated) = server
        .put(
            &format!("/api/admin/metadata-providers/{id}"),
            &token,
            json!({ "token": "", "priority": 30, "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["secretConfigured"], true);
    assert_eq!(updated["priority"], 30);
    assert_eq!(updated["enabled"], false);
    assert!(!updated.to_string().contains("super-secret-token"));

    // Clearing removes the stored token.
    let (status, cleared) = server
        .put(
            &format!("/api/admin/metadata-providers/{id}"),
            &token,
            json!({ "clearToken": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["secretConfigured"], false);

    // The plaintext token is never stored.
    let pool = ludex_server::db::connect(&format!(
        "sqlite://{}?mode=rwc",
        server.data_dir.join("ludex.db").display()
    ))
    .await
    .expect("open database");
    let stored = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT token_enc FROM metadata_providers WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("row");
    assert!(stored.0.is_none());

    let (status, _) = server
        .delete(&format!("/api/admin/metadata-providers/{id}"), &token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, list) = server
        .get("/api/admin/metadata-providers", Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().expect("array").len(), 0);
}

#[tokio::test]
async fn non_admins_are_rejected_and_reorder_rewrites_priorities() {
    let (server, token) = admin_server().await;
    let mock = MockProvider::start(r#"{"results":[]}"#).await;
    let _member = server
        .create_user(&token, "member", "password123", "user")
        .await;
    let (status, body) = server.login("member", "password123").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let member_token = body["token"].as_str().expect("token").to_string();

    let (status, body) = server
        .get("/api/admin/metadata-providers", Some(&member_token))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server
        .post(
            "/api/admin/metadata-providers",
            Some(&member_token),
            Some(provider_body("Nope", &mock.url, None, 10)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (_, first) = server
        .post(
            "/api/admin/metadata-providers",
            Some(&token),
            Some(provider_body("First", &mock.url, None, 100)),
        )
        .await;
    let (_, second) = server
        .post(
            "/api/admin/metadata-providers",
            Some(&token),
            Some(provider_body("Second", &mock.url, None, 200)),
        )
        .await;
    let first_id = first["id"].as_i64().expect("id");
    let second_id = second["id"].as_i64().expect("id");

    let (status, reordered) = server
        .post(
            "/api/admin/metadata-providers/reorder",
            Some(&token),
            Some(json!({ "ids": [second_id, first_id] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reordered}");
    let items = reordered.as_array().expect("array");
    assert_eq!(items[0]["id"], second_id);
    assert_eq!(items[0]["priority"], 10);
    assert_eq!(items[1]["id"], first_id);
    assert_eq!(items[1]["priority"], 20);
}

#[tokio::test]
async fn test_endpoint_returns_candidates_and_provider_errors() {
    let (server, token) = admin_server().await;
    let mock = MockProvider::start(
        r#"{"results":[{"id":"1","title":"Celeste","releaseDate":"2018-01-25","rating":92}]}"#,
    )
    .await;
    let (_, created) = server
        .post(
            "/api/admin/metadata-providers",
            Some(&token),
            Some(provider_body("Fixture", &mock.url, Some("t"), 10)),
        )
        .await;
    let id = created["id"].as_i64().expect("id");

    let (status, result) = server
        .post(
            &format!("/api/admin/metadata-providers/{id}/test"),
            Some(&token),
            Some(json!({ "title": "Celeste", "year": 2018 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["ok"], true);
    assert_eq!(result["candidates"][0]["title"], "Celeste");
    assert_eq!(result["candidates"][0]["id"], "1");

    // A provider whose target is unreachable reports the error instead of 5xx.
    let (_, broken) = server
        .post(
            "/api/admin/metadata-providers",
            Some(&token),
            Some(provider_body("Broken", "http://127.0.0.1:1", None, 20)),
        )
        .await;
    let broken_id = broken["id"].as_i64().expect("id");
    let (status, result) = server
        .post(
            &format!("/api/admin/metadata-providers/{broken_id}/test"),
            Some(&token),
            Some(json!({ "title": "Celeste" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["ok"], false);
    assert!(result["error"]
        .as_str()
        .is_some_and(|error| !error.is_empty()));
}

#[tokio::test]
async fn enrichment_uses_a_provider_only_without_igdb_and_survives_failures() {
    let (server, token) = admin_server().await;
    let mock = MockProvider::start(
        r#"{"results":[{
            "id":"celeste-1",
            "title":"Celeste",
            "description":"Help Madeline survive her inner demons.",
            "releaseDate":"2018-01-25",
            "genres":["Platformer"],
            "developers":["Maddy Makes Games"],
            "rating":92,
            "providerUrl":"https://example.test/celeste"
        }]}"#,
    )
    .await;

    // The broken provider runs first: its failure must not stop the chain.
    let (status, _) = server
        .post(
            "/api/admin/metadata-providers",
            Some(&token),
            Some(provider_body("Broken", "http://127.0.0.1:1", None, 10)),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, created) = server
        .post(
            "/api/admin/metadata-providers",
            Some(&token),
            Some(provider_body("Fixture", &mock.url, None, 20)),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // Seed one game and ask for an enrichment pass (IGDB is not configured).
    let pool = ludex_server::db::connect(&format!(
        "sqlite://{}?mode=rwc",
        server.data_dir.join("ludex.db").display()
    ))
    .await
    .expect("open database");
    let now = ludex_server::db::db_time(ludex_server::db::now());
    let game_id = sqlx::query(
        "INSERT INTO games (title, sort_title, created_at, updated_at) VALUES (?, ?, ?, ?)",
    )
    .bind("Celeste")
    .bind("celeste")
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("insert game")
    .last_insert_rowid();
    let (status, queued) = server
        .post(
            &format!("/api/games/{game_id}/refresh-metadata"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");

    let mut detail = Value::Null;
    for _ in 0..200 {
        let (status, body) = server
            .get(&format!("/api/games/{game_id}"), Some(&token))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["description"].as_str() == Some("Help Madeline survive her inner demons.") {
            detail = body;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        detail["description"], "Help Madeline survive her inner demons.",
        "provider metadata was not applied: {detail}"
    );
    assert_eq!(detail["provider"]["slug"], "fixture");
    assert_eq!(detail["provider"]["id"], "celeste-1");
    assert_eq!(detail["provider"]["url"], "https://example.test/celeste");

    // The fetch log records both attempts (one failure, one success).
    let failures: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_fetches WHERE ok = 0")
        .fetch_one(&pool)
        .await
        .expect("failure count");
    let successes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_fetches WHERE ok = 1")
        .fetch_one(&pool)
        .await
        .expect("success count");
    assert!(failures >= 1, "failing provider was not recorded");
    assert!(successes >= 1, "successful provider was not recorded");
}
