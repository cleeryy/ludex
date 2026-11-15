//! Server-side extracted cache and file manifest (#231).
//!
//! The prototype is gated: off by default, the classic download stays the
//! fallback, and the public routes answer `409 extracted_cache_unavailable`
//! while the cache is disabled or before an archive was extracted. These tests
//! cover the contract the desktop lane codes against: extraction dedupe,
//! manifest content, per-file SHA-256, Range/If-Range, path traversal and
//! symlink refusal, admin auth, restart recovery and eviction on version
//! delete.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{run_scan, TestServer};
use ludex_server::{start, Config, ServerHandle};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn sha256_hex(payload: &[u8]) -> String {
    hex::encode(Sha256::digest(payload))
}

/// Zip with two files (one executable), a nested directory and an empty one.
fn write_game_zip(path: &Path) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(path).expect("create zip");
    let mut writer = zip::ZipWriter::new(file);
    writer
        .add_directory("Game/empty/", SimpleFileOptions::default())
        .expect("empty dir");
    writer
        .start_file(
            "Game/bin/game",
            SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(0o755),
        )
        .expect("start file");
    writer.write_all(b"#!/bin/sh\necho ludex\n").expect("write");
    writer
        .start_file(
            "Game/readme.txt",
            SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(0o640),
        )
        .expect("start file");
    writer.write_all(b"hello manifest\n").expect("write");
    // Above the scanner's 1 MiB archive floor (#180).
    writer
        .start_file(
            "Game/pad.bin",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
        )
        .expect("start file");
    let pad: Vec<u8> = (0..1024 * 1024 + 2048)
        .map(|index| (index % 251) as u8)
        .collect();
    writer.write_all(&pad).expect("write");
    writer.finish().expect("finish zip");
}

async fn only_game(server: &TestServer, token: &str) -> i64 {
    let (status, list) = server.get("/api/games?limit=10", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"][0]["id"].as_i64().expect("game id")
}

async fn version_id(server: &TestServer, token: &str, game_id: i64) -> i64 {
    let (status, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(token))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    detail["versions"][0]["id"].as_i64().expect("version id")
}

/// Waits until the background content-hash job published the digest.
async fn content_sha256(server: &TestServer, token: &str, game_id: i64) -> String {
    for _ in 0..400 {
        let (_, detail) = server
            .get(&format!("/api/games/{game_id}"), Some(token))
            .await;
        if let Some(digest) = detail["versions"][0]["contentSha256"].as_str() {
            return digest.to_string();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the content hash job never published a digest");
}

async fn ticket(server: &TestServer, token: &str, game_id: i64, version: i64) -> String {
    let (status, body) = server
        .post(
            &format!("/api/games/{game_id}/download-ticket"),
            Some(token),
            Some(json!({ "versionId": version })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["url"]
        .as_str()
        .and_then(|url| url.split("token=").nth(1))
        .expect("ticket token")
        .to_string()
}

async fn extract_once(server: &TestServer, admin: &str, version: i64) -> Value {
    let (status, body) = server
        .post(
            &format!("/api/admin/versions/{version}/extract"),
            Some(admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn wait_ready(server: &TestServer, admin: &str, version: i64) -> Value {
    let mut last = Value::Null;
    for _ in 0..400 {
        let (status, body) = server
            .post(
                &format!("/api/admin/versions/{version}/extract"),
                Some(admin),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        match body["status"].as_str() {
            Some("ready") => return body,
            Some("failed") => panic!("extraction failed: {body}"),
            _ => last = body,
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("extraction never finished: {last}");
}

fn cache_entry_dir(data_dir: &Path, sha256: &str) -> PathBuf {
    data_dir.join("extracted").join(sha256)
}

#[tokio::test]
async fn manifest_and_file_transport_serve_the_extracted_tree() {
    let server = TestServer::start_with(|config| config.extracted_cache_enabled = true).await;
    let (admin, _) = server.setup().await;

    write_game_zip(&server.games_dir.join("Hollow (2024) v1.0.zip"));
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;
    let version = version_id(&server, &admin, game_id).await;
    let digest = content_sha256(&server, &admin, game_id).await;
    assert!(ludex_server::extracted_cache::is_sha256_hex(&digest));

    // Before extraction the manifest endpoint answers the fallback signal.
    let ticket = ticket(&server, &admin, game_id, version).await;
    let (status, body) = server
        .get(
            &format!("/api/versions/{version}/manifest?token={ticket}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["error"]["code"], "extracted_cache_unavailable",
        "{body}"
    );

    let started = extract_once(&server, &admin, version).await;
    assert_eq!(started["versionId"], json!(version));
    assert_eq!(started["archiveSha256"], json!(digest));
    assert!(
        matches!(
            started["status"].as_str(),
            Some("queued" | "extracting" | "ready")
        ),
        "{started}"
    );

    let ready = wait_ready(&server, &admin, version).await;
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["files"], json!(3), "{ready}");
    assert!(ready["bytes"].as_u64().unwrap_or(0) > 0, "{ready}");

    let (status, manifest) = server
        .get(
            &format!("/api/versions/{version}/manifest?token={ticket}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{manifest}");
    assert_eq!(manifest["status"], "ready");
    assert_eq!(manifest["versionId"], json!(version));
    assert_eq!(manifest["archiveSha256"], json!(digest));
    assert_eq!(manifest["fileCount"], json!(3));
    let files = manifest["files"].as_array().expect("files");
    assert_eq!(files.len(), 3);
    let game_file = files
        .iter()
        .find(|file| file["path"] == "Game/bin/game")
        .expect("game file");
    assert_eq!(
        game_file["sha256"],
        json!(sha256_hex(b"#!/bin/sh\necho ludex\n"))
    );
    assert_eq!(game_file["executable"], json!(true));
    assert_eq!(game_file["mode"], json!(0o755));
    let readme = files
        .iter()
        .find(|file| file["path"] == "Game/readme.txt")
        .expect("readme");
    assert_eq!(readme["mode"], json!(0o640));
    assert_eq!(readme["executable"], json!(false));
    assert_eq!(manifest["directories"], json!(["Game/empty"]));

    // The cache lives under {data_dir}/extracted/<sha256>/.
    let entry = cache_entry_dir(&server.data_dir, &digest);
    assert!(entry.join("manifest.json").is_file());
    assert!(entry.join("meta.json").is_file());
    assert!(entry.join("files/Game/readme.txt").is_file());

    // Full file: exact length, digest header and ETag.
    let response = server
        .client
        .get(format!(
            "{}/api/versions/{version}/files/Game/readme.txt?token={ticket}",
            server.url
        ))
        .send()
        .await
        .expect("file");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-ludex-file-sha256")
            .and_then(|value| value.to_str().ok()),
        Some(sha256_hex(b"hello manifest\n").as_str())
    );
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some("15")
    );
    assert_eq!(
        response.bytes().await.expect("body").as_ref(),
        b"hello manifest\n"
    );

    // Range resume and If-Range.
    let ranged = server
        .client
        .get(format!(
            "{}/api/versions/{version}/files/Game/readme.txt?token={ticket}",
            server.url
        ))
        .header(reqwest::header::RANGE, "bytes=6-")
        .send()
        .await
        .expect("range");
    assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        ranged
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok()),
        Some("bytes 6-14/15")
    );
    assert_eq!(ranged.bytes().await.expect("body").as_ref(), b"manifest\n");

    let stale = server
        .client
        .get(format!(
            "{}/api/versions/{version}/files/Game/readme.txt?token={ticket}",
            server.url
        ))
        .header(reqwest::header::RANGE, "bytes=6-")
        .header(reqwest::header::IF_RANGE, "\"stale-digest\"")
        .send()
        .await
        .expect("if-range");
    assert_eq!(stale.status(), StatusCode::OK);
    assert_eq!(stale.bytes().await.expect("body").len(), 15);

    let unsatisfiable = server
        .client
        .get(format!(
            "{}/api/versions/{version}/files/Game/readme.txt?token={ticket}",
            server.url
        ))
        .header(reqwest::header::RANGE, "bytes=99-")
        .send()
        .await
        .expect("bad range");
    assert_eq!(unsatisfiable.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        unsatisfiable
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok()),
        Some("bytes */15")
    );

    // Unknown file and traversal attempts are plain not-found, never a
    // directory listing or a path outside the entry.
    for path in [
        "Game/missing.txt",
        "%2e%2e%2fmeta.json",
        "Game%2f..%2f..%2fmeta.json",
        "%2fetc%2fpasswd",
    ] {
        let (status, body) = server
            .get(
                &format!("/api/versions/{version}/files/{path}?token={ticket}"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }

    // A symlink planted inside the entry never serves outside the cache root.
    let readme = entry.join("files/Game/readme.txt");
    std::fs::remove_file(&readme).expect("remove");
    std::os::unix::fs::symlink("/etc/passwd", &readme).expect("symlink");
    let (status, body) = server
        .get(
            &format!("/api/versions/{version}/files/Game/readme.txt?token={ticket}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Admin listing reports the entry and its budget.
    let (status, overview) = server.get("/api/admin/extracted-cache", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{overview}");
    assert_eq!(overview["enabled"], json!(true));
    assert_eq!(overview["entries"].as_array().map(Vec::len), Some(1));
    assert_eq!(overview["entries"][0]["sha256"], json!(digest));
    assert_eq!(overview["entries"][0]["versionId"], json!(version));
    assert!(
        overview["root"]
            .as_str()
            .is_some_and(|root| root.ends_with("extracted")),
        "{overview}"
    );

    // Explicit removal, then the entry is gone from disk and from the list.
    let (status, body) = server
        .delete(&format!("/api/admin/extracted-cache/{digest}"), &admin)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!entry.exists());
    let (_, overview) = server.get("/api/admin/extracted-cache", Some(&admin)).await;
    assert_eq!(overview["entries"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn concurrent_extract_requests_share_one_job() {
    let server = TestServer::start_with(|config| config.extracted_cache_enabled = true).await;
    let (admin, _) = server.setup().await;
    write_game_zip(&server.games_dir.join("Hollow (2024) v1.0.zip"));
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;
    let version = version_id(&server, &admin, game_id).await;
    let digest = content_sha256(&server, &admin, game_id).await;

    let mut handles = Vec::new();
    for _ in 0..4 {
        let client: Client = server.client.clone();
        let url = format!("{}/api/admin/versions/{version}/extract", server.url);
        let token = admin.clone();
        handles.push(tokio::spawn(async move {
            let response = client
                .post(url)
                .bearer_auth(token)
                .send()
                .await
                .expect("extract");
            assert_eq!(response.status(), StatusCode::OK);
            response.json::<Value>().await.expect("body")
        }));
    }
    for handle in handles {
        let body = handle.await.expect("join");
        assert!(
            matches!(
                body["status"].as_str(),
                Some("queued" | "extracting" | "ready")
            ),
            "{body}"
        );
    }
    let ready = wait_ready(&server, &admin, version).await;
    assert_eq!(ready["status"], "ready", "{ready}");

    let (_, overview) = server.get("/api/admin/extracted-cache", Some(&admin)).await;
    assert_eq!(overview["entries"].as_array().map(Vec::len), Some(1));
    assert_eq!(overview["entries"][0]["sha256"], json!(digest));
}

#[tokio::test]
async fn a_disabled_cache_writes_nothing_and_answers_409() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    write_game_zip(&server.games_dir.join("Hollow (2024) v1.0.zip"));
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;
    let version = version_id(&server, &admin, game_id).await;
    let _digest = content_sha256(&server, &admin, game_id).await;

    let body = extract_once(&server, &admin, version).await;
    assert_eq!(body["status"], "unavailable", "{body}");

    let ticket = ticket(&server, &admin, game_id, version).await;
    let (status, body) = server
        .get(
            &format!("/api/versions/{version}/manifest?token={ticket}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "extracted_cache_unavailable");

    let (status, body) = server
        .get(
            &format!("/api/versions/{version}/files/Game/readme.txt?token={ticket}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "extracted_cache_unavailable");

    assert!(
        !server.data_dir.join("extracted").exists(),
        "a disabled cache must not create its root"
    );
}

#[tokio::test]
async fn only_administrators_can_extract_or_inspect_the_cache() {
    let server = TestServer::start_with(|config| config.extracted_cache_enabled = true).await;
    let (admin, _) = server.setup().await;
    write_game_zip(&server.games_dir.join("Hollow (2024) v1.0.zip"));
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;
    let version = version_id(&server, &admin, game_id).await;
    let _digest = content_sha256(&server, &admin, game_id).await;

    let _player = server
        .create_user(&admin, "player", "password123", "user")
        .await;
    let (status, login) = server.login("player", "password123").await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let player = login["token"].as_str().expect("token").to_string();

    let (status, body) = server
        .post(
            &format!("/api/admin/versions/{version}/extract"),
            Some(&player),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server
        .get("/api/admin/extracted-cache", Some(&player))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = server
        .post(
            &format!("/api/admin/versions/{version}/extract"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = server.get("/api/admin/extracted-cache", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A ticket from another version is refused on these routes.
    let ticket = ticket(&server, &admin, game_id, version).await;
    let (status, body) = server
        .get(
            &format!("/api/versions/{}/manifest?token={ticket}", version + 99),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn deleting_a_version_evicts_its_cache_entry() {
    let server = TestServer::start_with(|config| config.extracted_cache_enabled = true).await;
    let (admin, _) = server.setup().await;
    let path = server.games_dir.join("Hollow (2024) v1.0.zip");
    write_game_zip(&path);
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;
    let version = version_id(&server, &admin, game_id).await;
    let digest = content_sha256(&server, &admin, game_id).await;
    wait_ready(&server, &admin, version).await;

    // A byte-identical copy shares the archive digest; a third, different
    // archive keeps the game alive so the last-version guard passes.
    let shared = server.games_dir.join("Hollow (2024) v1.1.zip");
    std::fs::copy(&path, &shared).expect("copy archive");
    std::fs::write(
        server.games_dir.join("Hollow (2024) v1.2.zip"),
        vec![7_u8; 1024 * 1024 + 1],
    )
    .expect("write");
    run_scan(&server, &admin).await;
    let versions = {
        let (_, detail) = server
            .get(&format!("/api/games/{game_id}"), Some(&admin))
            .await;
        detail["versions"].as_array().cloned().unwrap_or_default()
    };
    assert_eq!(versions.len(), 3, "{versions:?}");
    let shared_id = versions
        .iter()
        .find(|entry| entry["fileName"] == "Hollow (2024) v1.1.zip")
        .and_then(|entry| entry["id"].as_i64())
        .expect("shared version");
    // The backfill hashes every pending row; the shared copy ends up with the
    // same digest, which is what keeps the entry alive after the first delete.
    let mut shared_digest = Value::Null;
    for _ in 0..400 {
        let (_, detail) = server
            .get(&format!("/api/games/{game_id}"), Some(&admin))
            .await;
        shared_digest = detail["versions"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row["fileName"] == "Hollow (2024) v1.1.zip")
            })
            .map(|row| row["contentSha256"].clone())
            .unwrap_or(Value::Null);
        if shared_digest == json!(digest) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(shared_digest, json!(digest));

    // Another version still references the digest: the cache entry is kept.
    let (status, body) = server
        .delete(
            &format!("/api/games/{game_id}/versions/{version}?confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(cache_entry_dir(&server.data_dir, &digest).exists());

    // Last reference gone: the bytes go with it.
    let (status, body) = server
        .delete(
            &format!("/api/games/{game_id}/versions/{shared_id}?confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!cache_entry_dir(&server.data_dir, &digest).exists());
    let (_, overview) = server.get("/api/admin/extracted-cache", Some(&admin)).await;
    assert_eq!(
        overview["entries"].as_array().map(Vec::len),
        Some(0),
        "{overview}"
    );
}

#[tokio::test]
async fn the_cache_is_rebuilt_after_a_restart() {
    let temp = TempDir::new().expect("tempdir");
    let games_dir = temp.path().join("games");
    let data_dir = temp.path().join("data");
    let media_dir = temp.path().join("media");
    std::fs::create_dir_all(&games_dir).expect("games");
    write_game_zip(&games_dir.join("Hollow (2024) v1.0.zip"));

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let config = |games: &Path, data: &Path, media: &Path| {
        let mut config =
            Config::for_local(games.to_path_buf(), data.to_path_buf(), media.to_path_buf());
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        config.extracted_cache_enabled = true;
        config
    };

    let first: ServerHandle = start(config(&games_dir, &data_dir, &media_dir))
        .await
        .expect("first start");
    let response = client
        .post(format!("{}/api/setup", first.url))
        .json(&json!({ "username": "admin", "password": "password123" }))
        .send()
        .await
        .expect("setup");
    assert_eq!(response.status(), StatusCode::OK);
    let admin = response.json::<Value>().await.expect("body")["token"]
        .as_str()
        .expect("token")
        .to_string();
    run_scan_http(&client, &first.url, &admin).await;

    let (_, list) = get_json(&client, &first.url, "/api/games?limit=10", &admin).await;
    let game_id = list["items"][0]["id"].as_i64().expect("game id");
    let (_, detail) = get_json(
        &client,
        &first.url,
        &format!("/api/games/{game_id}"),
        &admin,
    )
    .await;
    let version = detail["versions"][0]["id"].as_i64().expect("version");
    let digest = wait_hash_http(&client, &first.url, &admin, game_id).await;

    // Extract through the admin route and wait for `ready`.
    let mut ready = false;
    for _ in 0..400 {
        let response = client
            .post(format!(
                "{}/api/admin/versions/{version}/extract",
                first.url
            ))
            .bearer_auth(&admin)
            .send()
            .await
            .expect("extract");
        let body: Value = response.json().await.expect("body");
        match body["status"].as_str() {
            Some("ready") => {
                ready = true;
                break;
            }
            Some("failed") => panic!("extraction failed: {body}"),
            _ => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
    assert!(ready, "extraction never became ready");
    let ticket = {
        let response = client
            .post(format!("{}/api/games/{game_id}/download-ticket", first.url))
            .bearer_auth(&admin)
            .json(&json!({ "versionId": version }))
            .send()
            .await
            .expect("ticket");
        let body: Value = response.json().await.expect("body");
        body["url"]
            .as_str()
            .and_then(|url| url.split("token=").nth(1))
            .expect("ticket token")
            .to_string()
    };
    let (status, _) = get_json(
        &client,
        &first.url,
        &format!("/api/versions/{version}/manifest?token={ticket}"),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = first.shutdown.send(());

    tokio::time::sleep(Duration::from_millis(250)).await;
    let second: ServerHandle = start(config(&games_dir, &data_dir, &media_dir))
        .await
        .expect("second start");
    let response = client
        .post(format!("{}/api/auth/login", second.url))
        .json(&json!({ "username": "admin", "password": "password123" }))
        .send()
        .await
        .expect("login");
    assert_eq!(response.status(), StatusCode::OK);
    let admin2 = response.json::<Value>().await.expect("body")["token"]
        .as_str()
        .expect("token")
        .to_string();

    // The manifest survives: the index was rebuilt from disk at startup.
    let (status, manifest) = get_json(
        &client,
        &second.url,
        &format!("/api/versions/{version}/manifest?token={ticket}"),
        &admin2,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{manifest}");
    assert_eq!(manifest["archiveSha256"], json!(digest));
    assert_eq!(manifest["files"].as_array().map(Vec::len), Some(3));
    let _ = second.shutdown.send(());
}

async fn get_json(client: &Client, base: &str, path: &str, token: &str) -> (StatusCode, Value) {
    let mut request = client.get(format!("{base}{path}"));
    if !token.is_empty() {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.expect("request");
    let status = response.status();
    let body = response.text().await.expect("body");
    let value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body).unwrap_or(Value::String(body))
    };
    (status, value)
}

async fn run_scan_http(client: &Client, base: &str, token: &str) {
    for _ in 0..200 {
        let (_, status) = get_json(client, base, "/api/library/scan", token).await;
        if status["running"].as_bool() == Some(false) && status["finishedAt"].is_string() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let before = get_json(client, base, "/api/library/scan", token).await.1["finishedAt"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for _ in 0..100 {
        let response = client
            .post(format!("{base}/api/library/scan"))
            .bearer_auth(token)
            .send()
            .await
            .expect("scan");
        let body: Value = response.json().await.expect("body");
        if body["started"].as_bool() == Some(true) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for _ in 0..600 {
        let (_, status) = get_json(client, base, "/api/library/scan", token).await;
        if status["finishedAt"].as_str() != Some(before.as_str()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("scan did not finish");
}

async fn wait_hash_http(client: &Client, base: &str, token: &str, game_id: i64) -> String {
    for _ in 0..400 {
        let (_, detail) = get_json(client, base, &format!("/api/games/{game_id}"), token).await;
        if let Some(digest) = detail["versions"][0]["contentSha256"].as_str() {
            return digest.to_string();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no content hash");
}
