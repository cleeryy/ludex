//! Device registry and targeted remote installs (#184).
//!
//! Covers registration/upsert, the 60 s online window, revocation, targeting,
//! account isolation and the legacy account-wide queue.

mod common;

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use common::{run_scan, TestServer};
use ludex_server::db;
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

async fn heartbeat(
    server: &TestServer,
    token: &str,
    client_id: &str,
    name: &str,
    capabilities: Value,
) -> Value {
    let (status, device) = server
        .post(
            "/api/devices/heartbeat",
            Some(token),
            Some(json!({
                "clientId": client_id,
                "name": name,
                "platform": "linux",
                "appVersion": "0.4.0",
                "capabilities": capabilities,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{device}");
    device
}

/// Rewrites `last_seen_at` so the online window can be tested without sleeping.
async fn backdate_device(server: &TestServer, client_id: &str, seconds: i64) {
    let url = format!("sqlite://{}/ludex.db?mode=rwc", server.data_dir.display());
    let pool = db::connect(&url).await.expect("connect database");
    let past = db::db_time(Utc::now() - ChronoDuration::seconds(seconds));
    db::query("UPDATE devices SET last_seen_at = ? WHERE client_id = ?")
        .bind(&past)
        .bind(client_id)
        .execute(&pool)
        .await
        .expect("backdate device");
    pool.close().await;
}

#[tokio::test]
async fn heartbeat_registers_upserts_and_reports_the_online_window() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;

    let first = heartbeat(
        &server,
        &token,
        "desktop-alpha",
        "Gaming PC",
        json!({
            "installedGameIds": [1, 2, 3],
            "freeDiskBytes": 123_456_789,
            "streamingAvailable": true,
        }),
    )
    .await;
    let id = first["id"].as_str().expect("device id").to_string();
    assert_eq!(first["name"], "Gaming PC");
    assert_eq!(first["platform"], "linux");
    assert_eq!(first["appVersion"], "0.4.0");
    assert_eq!(first["online"], true);
    assert_eq!(first["installedCount"], 3);
    assert_eq!(first["freeDiskBytes"], 123_456_789);
    assert_eq!(first["streamingAvailable"], true);
    assert!(first["lastSeenAt"].as_str().is_some());

    // A second heartbeat upserts the same row and refreshes the capabilities.
    let second = heartbeat(
        &server,
        &token,
        "desktop-alpha",
        "Bureau",
        json!({ "installedCount": 7, "freeDiskBytes": 42 }),
    )
    .await;
    assert_eq!(second["id"], id);
    assert_eq!(second["name"], "Bureau");
    assert_eq!(second["installedCount"], 7);
    assert_eq!(second["freeDiskBytes"], 42);

    let (status, devices) = server.get("/api/devices", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{devices}");
    let rows = devices.as_array().expect("devices");
    assert_eq!(rows.len(), 1, "{devices}");
    assert_eq!(rows[0]["online"], true, "{devices}");

    // Older than the 60 s window: offline but still listed.
    backdate_device(&server, "desktop-alpha", 61).await;
    let (_, devices) = server.get("/api/devices", Some(&token)).await;
    let rows = devices.as_array().expect("devices");
    assert_eq!(rows.len(), 1, "{devices}");
    assert_eq!(rows[0]["online"], false, "{devices}");

    let registered = wait_for_audit(&server, &token, "device.registered").await;
    assert!(
        registered
            .iter()
            .any(|entry| entry["targetType"] == "device" && entry["targetId"] == id),
        "{registered:?}"
    );

    server.stop().await;
}

#[tokio::test]
async fn revoked_device_cannot_heartbeat_or_claim() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (game_id, _) = game_with_version(&server, &token).await;

    let revoked = heartbeat(&server, &token, "desktop-revoked", "Old PC", json!({})).await;
    let revoked_id = revoked["id"].as_str().expect("id");
    let active = heartbeat(&server, &token, "desktop-active", "New PC", json!({})).await;

    let (status, _) = server
        .delete(&format!("/api/devices/{revoked_id}"), &token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Revoked devices disappear from the phone's list.
    let (_, devices) = server.get("/api/devices", Some(&token)).await;
    let rows = devices.as_array().expect("devices");
    assert_eq!(rows.len(), 1, "{devices}");
    assert_eq!(rows[0]["id"], active["id"]);

    let (status, error) = server
        .post(
            "/api/devices/heartbeat",
            Some(&token),
            Some(json!({ "clientId": "desktop-revoked", "name": "Old PC" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert_eq!(error["error"]["code"], "device_revoked");

    // The queue is account-wide, but a revoked client cannot claim it.
    let (_, created) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    let id = created["id"].as_str().expect("id").to_string();
    let (status, error) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "claimed", "clientId": "desktop-revoked" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert_eq!(error["error"]["code"], "device_revoked");

    let (status, claimed) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "claimed", "clientId": "desktop-active" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["claimedByDeviceId"], active["id"]);

    let revoked_audit = wait_for_audit(&server, &token, "device.revoked").await;
    assert!(
        revoked_audit
            .iter()
            .any(|entry| entry["targetId"] == revoked_id),
        "{revoked_audit:?}"
    );

    server.stop().await;
}

#[tokio::test]
async fn targeted_install_is_only_visible_to_and_claimed_by_the_target() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (game_id, _) = game_with_version(&server, &token).await;

    let first = heartbeat(&server, &token, "pc-one", "Gaming PC", json!({})).await;
    let second = heartbeat(&server, &token, "pc-two", "Bureau", json!({})).await;
    let first_id = first["id"].as_str().expect("id").to_string();
    let second_id = second["id"].as_str().expect("id").to_string();

    let (status, created) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id, "targetDeviceId": first_id })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["targetDeviceId"], first_id);
    assert_eq!(created["targetDeviceName"], "Gaming PC");

    // Each PC only sees its own targeted work plus the account-wide backlog.
    let (_, for_first) = server
        .get(
            &format!("/api/remote-installs?status=queued&deviceId={first_id}"),
            Some(&token),
        )
        .await;
    assert_eq!(for_first.as_array().map(Vec::len), Some(1), "{for_first}");
    let (_, for_second) = server
        .get(
            &format!("/api/remote-installs?status=queued&deviceId={second_id}"),
            Some(&token),
        )
        .await;
    assert_eq!(for_second.as_array().map(Vec::len), Some(0), "{for_second}");

    // A second desktop racing for the request is rejected.
    let (status, error) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "claimed", "clientId": "pc-two" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "wrong_device");

    // Even omitting the client id cannot claim someone else's request.
    let (status, error) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "claimed" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["error"]["code"], "wrong_device");

    let (status, claimed) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "claimed", "clientId": "pc-one" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["claimedByDeviceId"], first_id);
    assert_eq!(claimed["targetDeviceId"], first_id);

    server.stop().await;
}

#[tokio::test]
async fn devices_and_targets_are_isolated_per_account() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let mine = heartbeat(&server, &admin, "admin-pc", "Admin PC", json!({})).await;
    let mine_id = mine["id"].as_str().expect("id");

    server
        .create_user(&admin, "phonefriend", "password123", "user")
        .await;
    let (_, login) = server.login("phonefriend", "password123").await;
    let friend = login["token"].as_str().expect("token").to_string();

    let theirs = heartbeat(&server, &friend, "friend-pc", "Friend PC", json!({})).await;
    assert_ne!(theirs["id"], mine_id);

    let (_, friend_devices) = server.get("/api/devices", Some(&friend)).await;
    let rows = friend_devices.as_array().expect("devices");
    assert_eq!(rows.len(), 1, "{friend_devices}");
    assert_eq!(rows[0]["id"], theirs["id"]);

    let (status, _) = server
        .delete(&format!("/api/devices/{mine_id}"), &friend)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Another account's device cannot be chosen as a target.
    // The admin owns the scan; the friend only needs a game to target.
    let (game_id, _) = game_with_version(&server, &admin).await;
    let (status, error) = server
        .post(
            "/api/remote-installs",
            Some(&friend),
            Some(json!({ "gameId": game_id, "targetDeviceId": mine_id })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    assert_eq!(error["error"]["code"], "device_not_found");

    server.stop().await;
}

#[tokio::test]
async fn legacy_untargeted_queue_still_works_without_a_device() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (game_id, _) = game_with_version(&server, &token).await;
    heartbeat(&server, &token, "desktop-legacy", "Gaming PC", json!({})).await;

    // No `targetDeviceId`: the request stays account-wide.
    let (status, created) = server
        .post(
            "/api/remote-installs",
            Some(&token),
            Some(json!({ "gameId": game_id })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert!(created["targetDeviceId"].is_null(), "{created}");
    let id = created["id"].as_str().expect("id").to_string();

    // Old desktops may claim without sending a client id at all.
    let (status, claimed) = server
        .post(
            &format!("/api/remote-installs/{id}/status"),
            Some(&token),
            Some(json!({ "status": "claimed" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["status"], "claimed");
    assert!(claimed["claimedByDeviceId"].is_null(), "{claimed}");

    server.stop().await;
}
