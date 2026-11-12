//! HTTP tests for the Library Health Center (#165).
//!
//! Covers the editor/administrator split, the confirmation gate on destructive
//! actions and the cached report lifecycle.

mod common;

use std::time::Duration;

use common::TestServer;
use reqwest::StatusCode;
use serde_json::{json, Value};

async fn wait_for_report(server: &TestServer, token: &str) -> Value {
    for _ in 0..400 {
        let (status, body) = server.get("/api/admin/health/library", Some(token)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["report"]["status"].as_str() == Some("completed") {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("library health report did not finish");
}

#[tokio::test]
async fn health_report_is_editor_readable_and_destructive_actions_are_admin_only() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    server
        .create_user(&admin, "editor", "editorpass123", "editor")
        .await;
    server
        .create_user(&admin, "viewer", "viewerpass123", "user")
        .await;
    let (status, body) = server.login("editor", "editorpass123").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let editor = body["token"].as_str().expect("editor token").to_string();
    let (status, body) = server.login("viewer", "viewerpass123").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let viewer = body["token"].as_str().expect("viewer token").to_string();

    let (status, body) = server.get("/api/admin/health/library", Some(&viewer)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server.get("/api/admin/health/library", Some(&editor)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["report"].is_null(), "no report has run yet: {body}");

    let (status, body) = server
        .post("/api/admin/health/library/scan", Some(&viewer), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server
        .post("/api/admin/health/library/scan", Some(&editor), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let report = wait_for_report(&server, &editor).await;
    assert_eq!(report["report"]["progress"].as_i64(), Some(100));
    assert!(
        report["report"]["counts"]["total"].as_i64().is_some(),
        "{report}"
    );

    let (status, body) = server
        .post(
            "/api/admin/health/library/actions",
            Some(&editor),
            Some(json!({ "action": "media_gc_apply", "confirm": true })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = server
        .post(
            "/api/admin/health/library/actions",
            Some(&admin),
            Some(json!({ "action": "media_gc_apply", "confirm": false })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "confirmation_required");

    let (status, body) = server
        .post(
            "/api/admin/health/library/actions",
            Some(&editor),
            Some(json!({ "action": "media_gc_dry_run" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mediaGc"]["dryRun"], true);
    assert_eq!(body["mediaGc"]["deletedFiles"], 0);

    let (status, body) = server
        .post(
            "/api/admin/health/library/actions",
            Some(&admin),
            Some(json!({ "action": "media_gc_apply", "confirm": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mediaGc"]["dryRun"], false);

    // An unknown action is a validation error, never a silent no-op.
    let (status, body) = server
        .post(
            "/api/admin/health/library/actions",
            Some(&admin),
            Some(json!({ "action": "delete_everything", "confirm": true })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "unknown_health_action");
}

#[tokio::test]
async fn health_report_survives_an_empty_library() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let (status, body) = server
        .post("/api/admin/health/library/scan", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let report = wait_for_report(&server, &admin).await;
    assert_eq!(report["findings"].as_array().map(Vec::len), Some(0));
    assert_eq!(report["report"]["status"], "completed");
    assert!(report["report"]["finishedAt"].as_str().is_some());

    let (status, body) = server
        .get(
            "/api/admin/health/library?severity=error&category=missing",
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["findingsTotal"], 0);
}
