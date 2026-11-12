//! Admin observability dashboard (#170).
//!
//! Proves the snapshot shape, the window contract, the admin-only boundary and
//! the empty states a freshly installed server reports.

mod common;

use common::{TestServer, ADMIN_PASSWORD};
use reqwest::StatusCode;
use serde_json::Value;

async fn member_token(server: &TestServer, admin: &str) -> String {
    server
        .create_user(admin, "member", ADMIN_PASSWORD, "user")
        .await;
    let (status, body) = server.login("member", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().expect("token").to_string()
}

async fn dashboard(server: &TestServer, token: &str, window: Option<&str>) -> (StatusCode, Value) {
    match window {
        Some(window) => {
            server
                .get(
                    &format!("/api/admin/observability?window={window}"),
                    Some(token),
                )
                .await
        }
        None => server.get("/api/admin/observability", Some(token)).await,
    }
}

#[tokio::test]
async fn admin_gets_bounded_snapshot_without_paths_or_secrets() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    // Generate traffic before the snapshot: one success and one 404.
    let (status, _) = server.get("/api/health", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = server.get("/api/does-not-exist", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let (status, body) = dashboard(&server, &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(body["window"], "1h");
    assert!(body["generatedAt"].as_str().is_some());
    assert!(body["server"]["version"].as_str().is_some());
    assert!(body["server"]["commit"].as_str().is_some());
    assert!(body["server"]["uptimeSeconds"].as_u64().is_some());
    assert!(body["server"]["startedAt"].as_str().is_some());

    assert!(body["requests"]["total"].as_u64().unwrap_or(0) >= 2);
    assert!(body["requests"]["perSecond"].as_f64().is_some());
    assert!(body["requests"]["success"].as_u64().unwrap_or(0) >= 1);
    assert!(body["requests"]["serverErrors"].as_u64().is_some());
    assert!(body["requests"]["errorRate"].as_f64().is_some());
    assert!(body["requests"]["p95Ms"].as_f64().is_some());

    let series = body["series"].as_array().expect("series");
    assert!(!series.is_empty(), "{body}");
    assert!(
        series.len() <= 96,
        "series must stay capped: {}",
        series.len()
    );
    let counted: u64 = series
        .iter()
        .map(|point| point["requests"].as_u64().unwrap_or(0))
        .sum();
    assert!(counted >= 2, "{body}");

    let top_errors = body["topErrors"].as_array().expect("topErrors");
    assert!(top_errors.len() <= 5, "error list must stay capped");
    let missing_route = top_errors
        .iter()
        .find(|row| row["status"].as_u64() == Some(404))
        .expect("the 404 request is listed");
    assert!(missing_route["count"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(missing_route["method"], "GET");
    assert!(missing_route["route"].as_str().is_some());

    assert_eq!(body["active"]["scans"].as_i64(), Some(0));
    assert_eq!(body["active"]["ingestRunning"].as_i64(), Some(0));
    assert_eq!(body["active"]["ingestQueued"].as_i64(), Some(0));
    assert_eq!(body["active"]["downloads"].as_u64(), Some(0));
    assert_eq!(body["active"]["integrityRunning"].as_bool(), Some(false));

    assert_eq!(body["database"]["ok"].as_bool(), Some(true));
    assert_eq!(body["database"]["backend"], "sqlite");
    assert!(body["database"]["sizeBytes"].as_u64().unwrap_or(0) > 0);
    assert!(body["database"]["maxConnections"].as_u64().unwrap_or(0) > 0);
    assert!(body["database"]["latencyMs"].as_f64().is_some());

    assert_eq!(body["storage"]["backend"], "local");
    assert!(body["storage"]["media"]["files"].as_u64().is_some());
    assert!(body["storage"]["saves"]["snapshots"].as_i64().is_some());
    assert_eq!(body["storage"]["backups"]["archives"].as_u64(), Some(0));

    let roots = body["storage"]["roots"].as_array().expect("roots");
    assert!(!roots.is_empty());
    for root in roots {
        assert!(root["label"]
            .as_str()
            .is_some_and(|label| !label.is_empty()));
        assert!(root.get("path").is_none(), "root paths are not exposed");
    }

    assert!(body["recentEvents"].is_array());
    assert!(body["webhooks"]["pending"].as_i64().is_some());
    assert!(body["webhooks"]["failed"].as_i64().is_some());
    assert_eq!(
        body["webhooks"]["recentFailures"].as_array().map(Vec::len),
        Some(0)
    );
    assert!(body["integrity"]["last"].is_null());
    assert_eq!(
        body["integrity"]["recentFindings"].as_array().map(Vec::len),
        Some(0)
    );

    assert_eq!(body["prometheus"]["endpoint"], "/metrics");
    assert_eq!(body["prometheus"]["tokenProtected"].as_bool(), Some(false));

    // The temp directories used by the test never leak through the snapshot.
    let temp = server.data_dir.parent().expect("parent").to_string_lossy();
    assert!(!body.to_string().contains(temp.as_ref()), "{body}");
}

#[tokio::test]
async fn window_accepts_24h_and_rejects_unknown_values() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let (status, body) = dashboard(&server, &admin, Some("24h")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["window"], "24h");
    assert!(
        body["series"].as_array().expect("series").len() <= 96,
        "24h series must stay capped"
    );

    for invalid in ["7d", "1", "", "all"] {
        let (status, body) = dashboard(&server, &admin, Some(invalid)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "window={invalid}: {body}");
        assert_eq!(body["error"]["code"], "invalid_window");
    }
}

#[tokio::test]
async fn dashboard_is_admin_only() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let member = member_token(&server, &admin).await;

    let (status, body) = server.get("/api/admin/observability", Some(&member)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden");

    let (status, body) = server.get("/api/admin/observability", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

#[tokio::test]
async fn dashboard_reports_token_protection_without_leaking_it() {
    let server = TestServer::start_with(|config| {
        config.metrics_token = Some("scrape-secret".to_string());
    })
    .await;
    let (admin, _) = server.setup().await;

    let (status, body) = dashboard(&server, &admin, Some("1h")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["prometheus"]["tokenProtected"].as_bool(), Some(true));
    assert!(!body.to_string().contains("scrape-secret"), "{body}");
}
