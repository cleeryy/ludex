//! Remote install API tests (#167): permissions, idempotency, progress,
//! cancellation and per-account rate limiting.

mod common;

use std::time::Duration;

use common::{run_scan, TestServer};
use reqwest::StatusCode;
use serde_json::{json, Value};

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

fn write_game(server: &TestServer, name: &str, size: u64) {
    let path = server.games_dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    let file = std::fs::File::create(&path).expect("create");
    file.set_len(size).expect("size");
}

/// Adds one archive and returns `(gameId, versionId)`.
async fn game_with_version(server: &TestServer, token: &str) -> (i64, i64) {
    write_game(server, "Hades (2020) v1.2.zip", 1024 * 1024 + 40);
    run_scan(server, token).await;
    let (status, list) = server.get("/api/games?limit=10", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let game_id = list["items"][0]["id"].as_i64().expect("game id");
    let (status, latest) = server
        .get(
            &format!("/api/games/{game_id}/versions/latest"),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{latest}");
    let version_id = latest["versionId"].as_i64().expect("version id");
    (game_id, version_id)
}

#[tokio::test]
async fn queue_is_idempotent_and_progresses_to_ready() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (game_id, version_id) = game_with_version(&server, &token).await;

    let (status, created) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id, "versionId": version_id, "device": "Pixel 8" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["status"], "queued");
    assert_eq!(created["gameId"], game_id);
    assert_eq!(created["versionId"], version_id);
    assert_eq!(created["device"], "Pixel 8");
    assert_eq!(created["progress"], 0);
    let id = created["id"].as_str().expect("id").to_string();

    // A second tap returns the same row instead of queueing another download.
    let (status, again) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["id"], id);
    let (_, list) = server.get("/api/remote-installs", Some(&token)).await;
    assert_eq!(list.as_array().map(Vec::len), Some(1), "{list}");

    // The desktop picks up queued work, claims it, then reports progress.
    let (status, pending) = server
        .get("/api/remote-installs?status=queued", Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    assert_eq!(pending.as_array().map(Vec::len), Some(1), "{pending}");

    // A request cannot skip the download and jump straight to ready.
    let (status, error) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "ready" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "invalid_transition");

    let (status, claimed) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "claimed" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["status"], "claimed");
    assert!(claimed["claimedAt"].as_str().is_some(), "{claimed}");

    let (status, downloading) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "downloading", "progress": 42, "message": "Extracting" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{downloading}");
    assert_eq!(downloading["progress"], 42);
    assert_eq!(downloading["message"], "Extracting");

    let (status, ready) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "ready" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["progress"], 100);
    assert!(ready["completedAt"].as_str().is_some(), "{ready}");

    // Ready is terminal and still idempotent.
    let (status, again) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["status"], "ready");

    // Every queue and terminal transition is audited.
    let requested = wait_for_audit(&server, &token, "remote_install.requested").await;
    assert!(
        requested
            .iter()
            .any(|entry| { entry["targetType"] == "remote_install" && entry["targetId"] == id }),
        "{requested:?}"
    );
    let updated = wait_for_audit(&server, &token, "remote_install.updated").await;
    assert!(
        updated
            .iter()
            .any(|entry| entry["metadata"]["status"] == "ready"),
        "{updated:?}"
    );

    // Clearing a finished request removes the history row.
    let (status, _) = server
        .delete(&format!("/api/remote-installs/{id}"), &token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = server.get("/api/remote-installs", Some(&token)).await;
    assert_eq!(list.as_array().map(Vec::len), Some(0), "{list}");
    let cancelled = wait_for_audit(&server, &token, "remote_install.cancelled").await;
    assert!(
        cancelled.iter().any(|entry| entry["targetId"] == id),
        "{cancelled:?}"
    );

    server.stop().await;
}

#[tokio::test]
async fn active_request_is_cancelled_and_can_be_requeued() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (game_id, _) = game_with_version(&server, &token).await;

    let (_, created) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    let id = created["id"].as_str().expect("id").to_string();

    let (status, _) = server
        .delete(&format!("/api/remote-installs/{id}"), &token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = server.get("/api/remote-installs", Some(&token)).await;
    let rows = list.as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status"], "cancelled");

    // Re-queueing reuses the row, so the phone still has one entry per game.
    let (status, requeued) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{requeued}");
    assert_eq!(requeued["id"], id);
    assert_eq!(requeued["status"], "queued");

    // A queued request cannot pretend to be downloading without claiming.
    let (status, error) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "downloading" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "invalid_transition");

    server.stop().await;
}

#[tokio::test]
async fn remote_installs_require_a_session_and_write_scope() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (game_id, _) = game_with_version(&server, &token).await;

    let (status, error) = server
        .post(
            "/api/remote-installs",
            None,
            Some(json!({ "gameId": game_id })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");

    let (status, created) = server
        .post(
            "/api/keys",
            Some(&token),
            Some(json!({ "name": "phone-readonly", "scopes": ["read"] })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let secret = created["secret"].as_str().expect("secret").to_string();

    // Read keys can follow progress but never queue work.
    let (status, list) = server.get("/api/remote-installs", Some(&secret)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let (status, error) = server
        .post(
            "/api/remote-installs",
            Some(&secret),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert_eq!(error["error"]["code"], "insufficient_scope");

    // Another account never sees or mutates the founder's request.
    server
        .create_user(&token, "phonefriend", "password123", "user")
        .await;
    let (_, login) = server.login("phonefriend", "password123").await;
    let friend = login["token"].as_str().expect("token");
    let (_, created) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    let id = created["id"].as_str().expect("id");
    let (status, _) = server
        .get(&format!("/api/remote-installs/{id}"), Some(friend))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(friend),
            Some(json!({ "status": "claimed" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    server.stop().await;
}

#[tokio::test]
async fn queueing_is_rate_limited_per_account() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (game_id, _) = game_with_version(&server, &token).await;

    // A dedicated account isolates the in-memory budget from the other tests.
    server
        .create_user(&token, "ratelimited", "password123", "user")
        .await;
    let (_, login) = server.login("ratelimited", "password123").await;
    let limiter = login["token"].as_str().expect("token").to_string();
    ludex_server::remote_installs::reset_rate_limit();

    let mut last = Value::Null;
    for _ in 0..ludex_server::remote_installs::RATE_LIMIT {
        let (status, body) = server
            .post(
                "/api/remote-installs",
                Some(&limiter),
                Some(json!({ "gameId": game_id })),
            )
            .await;
        assert!(status.is_success(), "{status}: {body}");
        last = body;
    }
    assert_eq!(last["status"], "queued");
    let (status, error) = server
        .post(
            "/api/remote-installs",
            Some(&limiter),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{error}");
    assert_eq!(error["error"]["code"], "rate_limited");

    // Another account is unaffected.
    let (status, _) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    assert!(status.is_success(), "{status}");

    ludex_server::remote_installs::reset_rate_limit();
    server.stop().await;
}

#[tokio::test]
async fn missing_game_or_version_is_rejected() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (game_id, version_id) = game_with_version(&server, &token).await;

    let (status, error) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": 999_999 })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");

    // A version from another game (or a missing id) is a validation error.
    let (status, error) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id, "versionId": version_id + 999 })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_version");

    let (status, error) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": 0 })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");

    server.stop().await;
}
