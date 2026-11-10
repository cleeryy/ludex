//! Resumable upload tests (#21): chunked round-trip, resume, idempotency,
//! hash mismatch, permissions and stale cleanup.

mod common;

use std::time::Duration;

use common::TestServer;
use reqwest::StatusCode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::time::sleep;

const CHUNK: u64 = 1024 * 1024;

async fn start_upload(
    server: &TestServer,
    token: &str,
    file_name: &str,
    bytes: &[u8],
    sha256: Option<String>,
) -> Value {
    let body = json!({
        "fileName": file_name,
        "size": bytes.len() as u64,
        "sha256": sha256,
    });
    let (status, upload) = server.post("/api/uploads", Some(token), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{upload}");
    upload
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn part(bytes: &[u8], chunk: u64, index: u64) -> &[u8] {
    let start = (index * chunk) as usize;
    let end = ((index + 1) * chunk).min(bytes.len() as u64) as usize;
    &bytes[start..end]
}

async fn put_part(
    server: &TestServer,
    token: &str,
    id: &str,
    index: u64,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    server
        .put_bytes(&format!("/api/uploads/{id}/parts/{index}"), token, bytes)
        .await
}

#[tokio::test]
async fn chunked_upload_round_trip_indexes_the_game() {
    let server = TestServer::start_with(|config| config.upload_chunk_bytes = CHUNK).await;
    let (token, _) = server.setup().await;
    let bytes: Vec<u8> = (0..(2 * CHUNK as usize + 1234))
        .map(|index| (index % 251) as u8)
        .collect();
    let upload = start_upload(
        &server,
        &token,
        "Uploaded Adventure (2025).zip",
        &bytes,
        Some(hash(&bytes)),
    )
    .await;
    let id = upload["id"].as_str().expect("id").to_string();
    assert_eq!(upload["chunkSize"], CHUNK);
    assert_eq!(upload["totalParts"], 3);
    assert_eq!(upload["receivedBytes"], 0);

    // First chunk is idempotent: sending it twice is accepted and counts once.
    let (status, body) = put_part(&server, &token, &id, 0, part(&bytes, CHUNK, 0).to_vec()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["receivedBytes"], CHUNK);
    let (status, body) = put_part(&server, &token, &id, 0, part(&bytes, CHUNK, 0).to_vec()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["receivedParts"], 1);

    // Resume after a page reload: progress reports the stored chunk.
    let (status, progress) = server
        .get(&format!("/api/uploads/{id}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{progress}");
    assert_eq!(progress["receivedParts"], json!([0]));
    assert_eq!(progress["receivedBytes"], CHUNK);
    assert_eq!(progress["status"], "open");

    for index in [1_u64, 2] {
        let (status, body) = put_part(
            &server,
            &token,
            &id,
            index,
            part(&bytes, CHUNK, index).to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, completed) = server
        .post(&format!("/api/uploads/{id}/complete"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    let game_id = completed["gameId"].as_i64().expect("gameId");
    let version_id = completed["versionId"].as_i64().expect("versionId");
    assert_eq!(completed["size"], bytes.len() as u64);
    assert_eq!(completed["sha256"], hash(&bytes));

    let (status, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert!(detail["versions"]
        .as_array()
        .is_some_and(|versions| versions.iter().any(|version| version["id"] == version_id)));
    assert!(server
        .games_dir
        .join("Uploaded Adventure (2025).zip")
        .is_file());

    // Completing twice is idempotent and returns the same ids.
    let (status, again) = server
        .post(&format!("/api/uploads/{id}/complete"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["gameId"], game_id);
    server.stop().await;
}

#[tokio::test]
async fn hash_mismatch_is_rejected_without_indexing() {
    let server = TestServer::start_with(|config| config.upload_chunk_bytes = CHUNK).await;
    let (token, _) = server.setup().await;
    let before = server.get("/api/stats", Some(&token)).await.1["games"]
        .as_i64()
        .unwrap_or(0);

    let bytes = vec![3_u8; CHUNK as usize + 500];
    let wrong = "0".repeat(64);
    let upload = start_upload(
        &server,
        &token,
        "Broken Hash (2024).zip",
        &bytes,
        Some(wrong),
    )
    .await;
    let id = upload["id"].as_str().expect("id").to_string();
    for index in [0_u64, 1] {
        let (status, body) = put_part(
            &server,
            &token,
            &id,
            index,
            part(&bytes, CHUNK, index).to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body) = server
        .post(&format!("/api/uploads/{id}/complete"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "hash_mismatch");
    assert!(!server.games_dir.join("Broken Hash (2024).zip").exists());

    let (status, progress) = server
        .get(&format!("/api/uploads/{id}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(progress["status"], "open", "a failed verify can be retried");
    assert!(progress["error"].as_str().is_some());

    let after = server.get("/api/stats", Some(&token)).await.1["games"]
        .as_i64()
        .unwrap_or(0);
    assert_eq!(before, after);
    server.stop().await;
}

#[tokio::test]
async fn uploads_require_an_editor() {
    let server = TestServer::start_with(|config| config.upload_chunk_bytes = CHUNK).await;
    let (admin_token, _) = server.setup().await;
    server
        .create_user(&admin_token, "viewer", "password123", "user")
        .await;
    server
        .create_user(&admin_token, "curator", "password123", "editor")
        .await;
    let (_, viewer) = server.login("viewer", "password123").await;
    let viewer_token = viewer["token"].as_str().expect("token");
    let (status, body) = server
        .post(
            "/api/uploads",
            Some(viewer_token),
            Some(json!({ "fileName": "Nope.zip", "size": 16 })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (_, curator) = server.login("curator", "password123").await;
    let curator_token = curator["token"].as_str().expect("token");
    let upload = start_upload(&server, curator_token, "Editor Game.zip", &[1_u8; 16], None).await;
    let id = upload["id"].as_str().expect("id").to_string();
    let (status, _) = server
        .post(
            &format!("/api/uploads/{id}/complete"),
            Some(curator_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Another editor cannot read or complete someone else's upload.
    let (status, body) = server
        .get(&format!("/api/uploads/{id}"), Some(&admin_token))
        .await;
    assert_eq!(status, StatusCode::OK, "admins can inspect uploads: {body}");
    server.stop().await;
}

#[tokio::test]
async fn incomplete_and_invalid_uploads_are_rejected() {
    let server = TestServer::start_with(|config| config.upload_chunk_bytes = CHUNK).await;
    let (token, _) = server.setup().await;
    let bytes = vec![9_u8; CHUNK as usize + 10];
    let upload = start_upload(&server, &token, "Partial (2024).zip", &bytes, None).await;
    let id = upload["id"].as_str().expect("id").to_string();

    let (status, body) = server
        .post(&format!("/api/uploads/{id}/complete"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "upload_incomplete");

    let (status, body) = put_part(&server, &token, &id, 5, vec![1, 2, 3]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_part");

    let (status, body) = put_part(&server, &token, &id, 0, vec![1, 2, 3]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_part_size");

    let (status, body) = server
        .post(
            "/api/uploads",
            Some(&token),
            Some(json!({ "fileName": "notes.txt", "size": 42 })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_file_name");

    let (status, _) = server.delete(&format!("/api/uploads/{id}"), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = server
        .get(&format!("/api/uploads/{id}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    server.stop().await;
}

#[tokio::test]
async fn stale_uploads_are_cleaned_up() {
    let server = TestServer::start_with(|config| {
        config.upload_chunk_bytes = CHUNK;
        config.upload_ttl = Duration::from_millis(1);
    })
    .await;
    let (token, _) = server.setup().await;
    let stale = start_upload(&server, &token, "Stale (2023).zip", &[1_u8; 32], None).await;
    let stale_id = stale["id"].as_str().expect("id").to_string();
    assert!(server.data_dir.join("uploads").join(&stale_id).is_dir());

    // Cleanup runs when a new upload starts; timestamps have second precision,
    // so wait for the clock to move.
    sleep(Duration::from_millis(1_100)).await;
    let _fresh = start_upload(&server, &token, "Fresh (2025).zip", &[2_u8; 32], None).await;

    let (status, body) = server
        .get(&format!("/api/uploads/{stale_id}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(!server.data_dir.join("uploads").join(&stale_id).exists());
    server.stop().await;
}
