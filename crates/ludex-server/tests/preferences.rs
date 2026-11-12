//! Account preference document sync (#201).
//!
//! The server owns one validated JSON document per account: the allowlisted
//! keys follow the user across browsers, the revision is an optimistic
//! concurrency token behind `If-Match`, and a reset only deletes that row.
//! These tests cover the round trip, the conflict matrix, reset, validation,
//! the size cap, account isolation and authentication.

mod common;

use std::time::Duration;

use common::{TestServer, ADMIN_PASSWORD};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

fn error_code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or_default()
}

/// Request with the headers the client sends, including `If-Match`.
async fn send(
    server: &TestServer,
    method: Method,
    token: Option<&str>,
    if_match: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = server
        .client
        .request(method, format!("{}/api/account/preferences", server.url));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(value) = if_match {
        request = request.header("If-Match", value);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.expect("request");
    let status = response.status();
    let cache_control = response
        .headers()
        .get("Cache-Control")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let etag = response
        .headers()
        .get("ETag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let text = response.text().await.expect("body");
    let mut value: Value = if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    };
    // Fold the transport headers into the body for easy assertions.
    if let Some(object) = value.as_object_mut() {
        object.insert("_cacheControl".into(), json!(cache_control));
        object.insert("_etag".into(), json!(etag));
    }
    (status, value)
}

async fn get_preferences(server: &TestServer, token: &str) -> (StatusCode, Value) {
    send(server, Method::GET, Some(token), None, None).await
}

async fn put_preferences(
    server: &TestServer,
    token: &str,
    if_match: Option<&str>,
    document: Value,
) -> (StatusCode, Value) {
    send(
        server,
        Method::PUT,
        Some(token),
        if_match,
        Some(json!({ "document": document })),
    )
    .await
}

fn sample_document() -> Value {
    json!({
        "theme": { "selection": "midnight", "accent": "#1f6feb", "tint": null },
        "bigPicture": { "layout": "grid", "sound": true, "haptics": false },
        "library": { "view": "list" },
        "updates": { "prompt": "notify" }
    })
}

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

#[tokio::test]
async fn document_round_trips_with_revision_and_private_caching() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let (status, empty) = get_preferences(&server, &admin).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["document"], Value::Null);
    assert_eq!(empty["revision"], 0);
    assert_eq!(empty["schemaVersion"], 1);
    assert_eq!(empty["_cacheControl"], "private, no-store");

    let (status, created) =
        put_preferences(&server, &admin, Some("\"0\""), sample_document()).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["revision"], 1);
    assert_eq!(created["document"], sample_document());
    assert_eq!(created["_etag"], "\"1\"");
    assert!(created["updatedAt"]
        .as_str()
        .is_some_and(|value| value.ends_with('Z')));

    let (status, fetched) = get_preferences(&server, &admin).await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(fetched["document"], sample_document());
    assert_eq!(fetched["revision"], 1);

    // A patch deep-merges one nested field and leaves the rest alone.
    let (status, patched) = send(
        &server,
        Method::PATCH,
        Some(&admin),
        Some("\"1\""),
        Some(json!({ "patch": { "theme": { "accent": "#AABBCC" } } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["revision"], 2);
    assert_eq!(patched["document"]["theme"]["accent"], "#aabbcc");
    assert_eq!(patched["document"]["theme"]["selection"], "midnight");
    assert_eq!(patched["document"]["bigPicture"]["layout"], "grid");

    server.stop().await;
}

#[tokio::test]
async fn stale_revisions_conflict_and_missing_preconditions_are_refused() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let (status, _) = put_preferences(&server, &admin, Some("*"), sample_document()).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = put_preferences(
        &server,
        &admin,
        Some("\"0\""),
        json!({ "library": { "view": "grid" } }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "revision_conflict");
    assert_eq!(body["revision"], 1, "the current revision is returned");

    let (status, body) = put_preferences(&server, &admin, None, sample_document()).await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{body}");
    assert_eq!(error_code(&body), "precondition_required");

    // The ETag we emitted is accepted verbatim, and `*` overwrites on purpose.
    let (status, body) = put_preferences(&server, &admin, Some("\"1\""), sample_document()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], 2);
    let (status, body) = put_preferences(&server, &admin, Some("*"), sample_document()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], 3);

    let (status, body) = send(
        &server,
        Method::PATCH,
        Some(&admin),
        Some("\"1\""),
        Some(json!({ "patch": { "library": { "view": "grid" } } })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["revision"], 3);

    server.stop().await;
}

#[tokio::test]
async fn reset_deletes_the_document_and_recreates_at_revision_one() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    put_preferences(&server, &admin, Some("*"), sample_document()).await;

    let (status, body) = send(&server, Method::DELETE, Some(&admin), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["document"], Value::Null);
    assert_eq!(body["revision"], 0);

    let (status, body) = get_preferences(&server, &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["document"], Value::Null);

    // A stale reset is refused so a device that never saw revision 2 cannot
    // wipe it; an unconditional reset still works.
    put_preferences(&server, &admin, Some("*"), sample_document()).await;
    let (status, body) = send(&server, Method::DELETE, Some(&admin), Some("\"99\""), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["revision"], 1);

    let (status, body) = put_preferences(&server, &admin, Some("\"1\""), sample_document()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], 2);

    server.stop().await;
}

#[tokio::test]
async fn unknown_keys_bad_values_and_oversized_bodies_are_rejected() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    for (document, code) in [
        (
            json!({ "installRoot": "D:/Games" }),
            "unknown_preference_key",
        ),
        (
            json!({ "theme": { "selection": "midnight", "font": "serif" } }),
            "unknown_preference_key",
        ),
        (
            json!({ "theme": { "selection": "https://evil.example" } }),
            "invalid_preference_value",
        ),
        (
            json!({ "theme": { "accent": "ldx_live_123" } }),
            "invalid_preference_value",
        ),
        (
            json!({ "bigPicture": { "layout": "cinema" } }),
            "invalid_preference_value",
        ),
        (
            json!({ "library": { "view": true } }),
            "invalid_preference_value",
        ),
        (
            json!({ "updates": { "prompt": "C:\\Users\\mara" } }),
            "invalid_preference_value",
        ),
        (
            json!({ "theme": { "selection": "a".repeat(65) } }),
            "invalid_preference_value",
        ),
        (json!([1, 2, 3]), "invalid_preferences"),
    ] {
        let (status, body) = put_preferences(&server, &admin, Some("*"), document).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(error_code(&body), code, "{body}");
    }

    // The 64 KiB cap is enforced before parsing, so a huge unknown blob never
    // reaches the validator.
    let huge = json!({ "document": { "theme": { "selection": "x".repeat(70 * 1024) } } });
    let (status, body) = send(&server, Method::PUT, Some(&admin), Some("*"), Some(huge)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(error_code(&body), "preferences_too_large");

    let (status, body) = send(
        &server,
        Method::PUT,
        Some(&admin),
        Some("*"),
        Some(json!({ "document": sample_document(), "schemaVersion": 99 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "unsupported_schema_version");

    // Nothing was stored by the rejected writes.
    let (status, body) = get_preferences(&server, &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["document"], Value::Null);

    server.stop().await;
}

#[tokio::test]
async fn documents_are_isolated_per_account() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    server
        .create_user(&admin, "member", ADMIN_PASSWORD, "user")
        .await;
    let (status, login) = server.login("member", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let member = login["token"].as_str().expect("token").to_string();

    put_preferences(&server, &admin, Some("*"), sample_document()).await;
    let (status, empty) = get_preferences(&server, &member).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["document"], Value::Null);
    assert_eq!(empty["revision"], 0);

    let member_document = json!({ "library": { "view": "grid" } });
    let (status, body) =
        put_preferences(&server, &member, Some("\"0\""), member_document.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], 1);

    let (_, admin_view) = get_preferences(&server, &admin).await;
    assert_eq!(
        admin_view["document"],
        sample_document(),
        "admin document is untouched"
    );

    // Deleting the account cascades to its preference row.
    let url = ludex_server::config::default_database_url(&server.data_dir);
    let pool = ludex_server::db::connect(&url)
        .await
        .expect("connect test db");
    let member_id: i64 =
        ludex_server::db::query_scalar("SELECT id FROM users WHERE username = 'member'")
            .fetch_one(&pool)
            .await
            .expect("member id");
    ludex_server::db::query("DELETE FROM users WHERE id = ?")
        .bind(member_id)
        .execute(&pool)
        .await
        .expect("delete member");
    let remaining: i64 =
        ludex_server::db::query_scalar("SELECT COUNT(*) FROM user_preferences WHERE user_id = ?")
            .bind(member_id)
            .fetch_one(&pool)
            .await
            .expect("count preferences");
    assert_eq!(remaining, 0);
    pool.close().await;

    server.stop().await;
}

#[tokio::test]
async fn authentication_is_required_and_writes_are_audited_without_values() {
    let server = TestServer::start().await;
    let (status, body) = get_preferences(&server, "not-a-token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let (admin, _) = server.setup().await;
    put_preferences(&server, &admin, Some("*"), sample_document()).await;

    let entries = wait_for_audit(&server, &admin, "user.preferences_updated").await;
    let entry = &entries[0];
    assert_eq!(entry["targetType"], "user");
    assert_eq!(entry["metadata"]["revision"], 1);
    assert_eq!(entry["metadata"]["schemaVersion"], 1);
    let keys = entry["metadata"]["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(keys.contains(&"theme"));
    assert!(keys.contains(&"bigPicture"));

    // The trail records the shape, never the values.
    let raw = serde_json::to_string(entry).expect("json");
    for needle in ["#1f6feb", "midnight", "grid", "notify"] {
        assert!(
            !raw.contains(needle),
            "audit metadata leaks {needle}: {raw}"
        );
    }

    server.stop().await;
}
