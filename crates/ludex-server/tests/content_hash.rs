//! Full archive SHA-256 contract (#216).
//!
//! The server must publish the strong hash in the version payload, on the
//! download response as `X-Ludex-Archive-Sha256`, advertise the capability, and
//! keep legacy rows without a hash downloadable while a background job
//! backfills them.

mod common;

use std::time::Duration;

use common::{run_scan, TestServer};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};

const DIGEST_LEN: usize = 64;

fn write_game(server: &TestServer, name: &str, payload: &[u8]) -> String {
    std::fs::write(server.games_dir.join(name), payload).expect("write game");
    sha256_hex(payload)
}

fn sha256_hex(payload: &[u8]) -> String {
    hex::encode(Sha256::digest(payload))
}

/// Opens the test server's SQLite index, so a test can simulate a legacy row
/// indexed before `content_sha256` existed.
async fn database(server: &TestServer) -> ludex_server::db::Database {
    let url = ludex_server::config::default_database_url(&server.data_dir);
    ludex_server::db::connect(&url)
        .await
        .expect("test database")
}

async fn only_game(server: &TestServer, token: &str) -> i64 {
    let (status, list) = server.get("/api/games?limit=10", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let (_, scan) = server.get("/api/library/scan", Some(token)).await;
    list["items"][0]["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("game id in {list} (scan {scan})"))
}

async fn version_hash(server: &TestServer, token: &str, game_id: i64) -> Option<String> {
    let (status, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(token))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["versions"][0]["contentSha256"]
        .as_str()
        .map(str::to_string)
}

async fn wait_for_hash(server: &TestServer, token: &str, game_id: i64) -> String {
    for _ in 0..200 {
        if let Some(hash) = version_hash(server, token, game_id).await {
            return hash;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the background content hash job never published a digest");
}

#[tokio::test]
async fn payload_header_and_capability_publish_the_full_sha256() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let payload: Vec<u8> = (0..1024 * 1024 + 4096)
        .map(|index| (index % 241) as u8)
        .collect();
    let expected = write_game(&server, "Hades (2020).zip", &payload);
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;

    // The scan scheduled the background job; the payload exposes the digest
    // only once the job stored it.
    let content_sha256 = wait_for_hash(&server, &admin, game_id).await;
    assert_eq!(content_sha256, expected);
    assert_eq!(content_sha256.len(), DIGEST_LEN);

    // The sampled fingerprint is kept for update detection.
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&admin))
        .await;
    assert!(
        detail["versions"][0]["archiveHash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256-sample:")),
        "{detail}"
    );

    // A download advertises the same digest without changing the body.
    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/download?versionId={}",
            server.url,
            detail["versions"][0]["id"].as_i64().expect("version id")
        ))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-ludex-archive-sha256")
            .and_then(|value| value.to_str().ok()),
        Some(expected.as_str())
    );
    let range = response
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(range, "bytes");
    assert_eq!(response.bytes().await.expect("body").as_ref(), payload);

    // The capability is advertised before any version is fetched.
    let (status, config) = server.get("/api/config", None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["features"]["archiveSha256"], true, "{config}");
}

#[tokio::test]
async fn legacy_rows_stay_downloadable_and_the_backfill_fills_them() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let mut payload = vec![9_u8; 1024 * 1024 + 128];
    payload[0] = 4;
    let expected = write_game(&server, "Hades (2020) v1.0.zip", &payload);
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;
    let _ = wait_for_hash(&server, &admin, game_id).await;

    // Simulate a row indexed before the column existed.
    let pool = database(&server).await;
    ludex_server::db::query("UPDATE game_versions SET content_sha256 = NULL")
        .execute(&pool)
        .await
        .expect("clear the hash");

    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&admin))
        .await;
    assert!(detail["versions"][0]["contentSha256"].is_null(), "{detail}");
    let version_id = detail["versions"][0]["id"].as_i64().expect("version id");

    // Still downloadable, without the strong header: the client warns instead
    // of blocking a legacy install.
    let response = server
        .client
        .get(format!(
            "{}/api/games/{game_id}/download?versionId={version_id}",
            server.url
        ))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("x-ludex-archive-sha256").is_none());
    assert_eq!(response.bytes().await.expect("body").as_ref(), payload);

    // Players cannot start a backfill; an administrator can.
    let _player = server
        .create_user(&admin, "player", "password123", "user")
        .await;
    let (status, login) = server.login("player", "password123").await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let player = login["token"].as_str().expect("player token").to_string();
    let (status, body) = server
        .post("/api/integrity/content-hashes", Some(&player), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, started) = server
        .post("/api/integrity/content-hashes", Some(&admin), None)
        .await;
    assert!(
        matches!(status, StatusCode::ACCEPTED | StatusCode::OK),
        "{status} {started}"
    );

    // The job reports progress and the row becomes verifiable again.
    let hash = wait_for_hash(&server, &admin, game_id).await;
    assert_eq!(hash, expected);

    let (status, state) = server
        .get("/api/integrity/content-hashes", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["pending"], 0, "{state}");
    assert!(state["hashed"].as_i64().unwrap_or(0) >= 1, "{state}");
    assert!(state["running"].is_boolean(), "{state}");
}
