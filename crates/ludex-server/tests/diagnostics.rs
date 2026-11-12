//! Opt-in anonymous diagnostics (#171).
//!
//! The endpoint is public but strict: bounded body, per-address throttle,
//! validated fields and a server-side redaction pass. Listing and deletion are
//! admin-only, and reports carry no account id or IP address.

mod common;

use common::{TestServer, ADMIN_PASSWORD};
use reqwest::StatusCode;
use serde_json::{json, Value};

fn report() -> Value {
    json!({
        "installId": "abc12345def67890",
        "appVersion": "0.3.2",
        "platform": "linux",
        "os": "Linux 6.8",
        "locale": "fr",
        "channel": "stable",
        "errorKind": "panic",
        "summary": "panic at /home/alice/Games/Hades: token=abcdef123456",
        "detail": {
            "command": "install_game",
            "gameTitle": "Hades",
            "path": "/home/alice/Games/Hades",
            "url": "http://10.1.20.5:8080/api/games",
            "contact": "alice@example.com"
        }
    })
}

#[tokio::test]
async fn submit_list_and_delete_are_anonymous() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let (status, body) = server.post("/api/diagnostics", None, Some(report())).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["accepted"], true);

    // Listing is admin-only.
    let (status, _) = server.get("/api/diagnostics", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, page) = server.get("/api/diagnostics", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 1);
    assert_eq!(page["retentionDays"], 90);
    assert_eq!(page["byVersion"][0]["value"], "0.3.2");
    assert_eq!(page["byKind"][0]["value"], "panic");
    assert_eq!(page["byPlatform"][0]["value"], "linux");

    let stored = &page["items"][0];
    let text = stored.to_string();
    assert!(!text.contains("/home/alice"), "{text}");
    assert!(!text.contains("abcdef123456"), "{text}");
    assert!(!text.contains("10.1.20.5"), "{text}");
    assert!(!text.contains("alice@example.com"), "{text}");
    assert!(!text.contains("\"alice\""), "{text}");
    // No account or address is stored with the report.
    assert!(stored.get("username").is_none(), "{text}");
    assert!(stored.get("ip").is_none(), "{text}");
    assert_eq!(stored["installId"], "abc12345def67890");

    let id = stored["id"].as_i64().expect("report id");
    let (status, _) = server
        .delete(&format!("/api/diagnostics/{id}"), &admin)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = server
        .delete(&format!("/api/diagnostics/{id}"), &admin)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, page) = server.get("/api/diagnostics", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 0);
}

#[tokio::test]
async fn only_administrators_can_read_reports() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    server
        .create_user(&admin, "member", ADMIN_PASSWORD, "user")
        .await;
    let (_, login) = server.login("member", ADMIN_PASSWORD).await;
    let member = login["token"].as_str().expect("token").to_string();

    let (status, _) = server.get("/api/diagnostics", Some(&member)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invalid_payloads_are_rejected() {
    let server = TestServer::start().await;

    let mut bad = report();
    bad["installId"] = json!("short");
    let (status, body) = server.post("/api/diagnostics", None, Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_install_id");

    let mut bad = report();
    bad["platform"] = json!("solaris");
    let (status, body) = server.post("/api/diagnostics", None, Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_platform");

    let mut bad = report();
    bad["errorKind"] = json!("exploit");
    let (status, body) = server.post("/api/diagnostics", None, Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_error_kind");

    let mut bad = report();
    bad["detail"] = json!(["not", "an", "object"]);
    let (status, body) = server.post("/api/diagnostics", None, Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_detail");

    let mut bad = report();
    bad["unexpected"] = json!(true);
    let (status, body) = server.post("/api/diagnostics", None, Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_report");

    let mut bad = report();
    bad["detail"] = json!({ "blob": "x".repeat(17 * 1024) });
    let (status, body) = server.post("/api/diagnostics", None, Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "detail_too_large");
}

#[tokio::test]
async fn oversized_bodies_and_rate_limits_are_enforced() {
    let server = TestServer::start_with(|config| {
        config.diagnostics_rate_limit = 1;
    })
    .await;

    let (status, body) = server.post("/api/diagnostics", None, Some(report())).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    // The second report in the same window is throttled.
    let (status, body) = server.post("/api/diagnostics", None, Some(report())).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "rate_limited");

    // A body above the 24 KiB cap is rejected before parsing.
    let oversized = "x".repeat(25 * 1024);
    let response = server
        .client
        .post(format!("{}/api/diagnostics", server.url))
        .header("content-type", "application/json")
        .body(oversized)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = response.json().await.expect("body");
    assert_eq!(body["error"]["code"], "report_too_large");
}
