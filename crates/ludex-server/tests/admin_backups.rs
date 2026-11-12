//! Administrator backup routes (#169): list, create, verify, download,
//! restore and delete, with admin-only access and a confined backup directory.

mod common;

use common::TestServer;
use reqwest::StatusCode;
use serde_json::json;

async fn viewer_token(server: &TestServer, admin: &str) -> String {
    server
        .create_user(admin, "viewer", "password123", "user")
        .await;
    let (status, body) = server.login("viewer", "password123").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["token"].as_str().expect("token").to_string()
}

/// Writes a plausible archive and re-scans so the backup contains one game.
async fn add_library_game(server: &TestServer, admin: &str, name: &str) {
    let path = server.games_dir.join(name);
    let file = std::fs::File::create(&path).expect("create game");
    file.set_len(1024 * 1024 + 10).expect("game size");
    common::run_scan(server, admin).await;
}

#[tokio::test]
async fn backup_routes_require_an_administrator() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let viewer = viewer_token(&server, &admin).await;

    let (status, _) = server.get("/api/admin/backups", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = server.get("/api/admin/backups", Some(&viewer)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server
        .post(
            "/api/admin/backups",
            Some(&viewer),
            Some(json!({ "dryRun": true })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = server.get("/api/admin/backups", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["directory"]
            .as_str()
            .is_some_and(|path| path.ends_with("backups")),
        "{body}"
    );
    assert_eq!(body["backups"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn create_verify_download_and_delete_a_backup() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    add_library_game(&server, &admin, "Backup Quest.zip").await;

    // Dry run: validates and measures without writing.
    let (status, body) = server
        .post(
            "/api/admin/backups",
            Some(&admin),
            Some(json!({ "dryRun": true, "includeMedia": false })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["dryRun"], json!(true));
    assert!(body["files"].as_i64().unwrap_or(0) >= 2, "{body}");
    let (_, listing) = server.get("/api/admin/backups", Some(&admin)).await;
    assert_eq!(listing["backups"].as_array().map(Vec::len), Some(0));

    // Real backup.
    let (status, created) = server
        .post(
            "/api/admin/backups",
            Some(&admin),
            Some(json!({ "includeMedia": true })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["dryRun"], json!(false));
    assert!(created["bytes"].as_u64().unwrap_or(0) > 0, "{created}");
    let name = created["name"].as_str().expect("name").to_string();
    assert!(name.ends_with(".tar.gz"), "{name}");

    let (status, listing) = server.get("/api/admin/backups", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    let entries = listing["backups"].as_array().expect("backups");
    assert_eq!(entries.len(), 1, "{listing}");
    assert_eq!(entries[0]["name"], json!(name));
    assert_eq!(entries[0]["kind"], json!("manual"));
    assert_eq!(entries[0]["verification"]["state"], json!("unverified"));
    assert!(entries[0]["size"].as_u64().unwrap_or(0) > 0);

    // Verify: checksums and manifest counters.
    let (status, verification) = server
        .post(
            &format!("/api/admin/backups/{name}/verify"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{verification}");
    assert_eq!(verification["state"], json!("verified"));
    assert_eq!(verification["games"], json!(1));
    assert!(verification["files"].as_i64().unwrap_or(0) >= 2);

    let (_, listing) = server.get("/api/admin/backups", Some(&admin)).await;
    assert_eq!(
        listing["backups"][0]["verification"]["state"],
        json!("verified")
    );

    // Download requires an authorised session and streams the archive.
    let (status, _) = server
        .get(&format!("/api/admin/backups/{name}/download"), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let viewer = viewer_token(&server, &admin).await;
    let (status, _) = server
        .get(
            &format!("/api/admin/backups/{name}/download"),
            Some(&viewer),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let response = server
        .client
        .get(format!("{}/api/admin/backups/{name}/download", server.url))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/gzip")
    );
    let bytes = response.bytes().await.expect("body");
    assert_eq!(
        bytes.len() as u64,
        created["bytes"].as_u64().expect("bytes"),
        "download must stream the whole archive"
    );

    // Path confinement: a file outside the backup directory is unreachable and
    // nothing outside it is listed.
    std::fs::write(server.data_dir.join("secret.tar.gz"), b"do-not-serve").expect("secret");
    for path in [
        "/api/admin/backups/..%2Fsecret.tar.gz/download",
        "/api/admin/backups/secret.tar.gz/download",
        "/api/admin/backups/not-an-archive.txt/download",
    ] {
        let response = server
            .client
            .get(format!("{}{path}", server.url))
            .bearer_auth(&admin)
            .send()
            .await
            .expect("request");
        assert!(
            response.status() == StatusCode::BAD_REQUEST
                || response.status() == StatusCode::NOT_FOUND
                || response.status() == StatusCode::METHOD_NOT_ALLOWED,
            "{path} returned {}",
            response.status()
        );
        let body = response.text().await.unwrap_or_default();
        assert!(
            !body.contains("do-not-serve"),
            "{path} leaked file contents"
        );
    }

    // Delete needs a typed confirmation.
    let (status, body) = server
        .delete(&format!("/api/admin/backups/{name}"), &admin)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = server
        .delete(
            &format!("/api/admin/backups/{name}?confirm=wrong.tar.gz"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = server
        .delete(&format!("/api/admin/backups/{name}?confirm={name}"), &admin)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, listing) = server.get("/api/admin/backups", Some(&admin)).await;
    assert_eq!(listing["backups"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn restore_previews_then_requires_confirmation_and_a_safety_copy() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    add_library_game(&server, &admin, "Restore Quest.zip").await;

    let (status, created) = server
        .post(
            "/api/admin/backups",
            Some(&admin),
            Some(json!({ "includeMedia": false })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let name = created["name"].as_str().expect("name").to_string();

    // Preview: no confirmation needed, nothing written.
    let (status, preview) = server
        .post(
            &format!("/api/admin/backups/{name}/restore"),
            Some(&admin),
            Some(json!({ "dryRun": true, "includeMedia": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["dryRun"], json!(true));
    assert_eq!(preview["games"], json!(1));
    assert!(preview["safetyBackup"].is_null());
    assert_eq!(preview["restartRequired"], json!(false));

    // A destructive restore requires the exact archive name.
    let (status, body) = server
        .post(
            &format!("/api/admin/backups/{name}/restore"),
            Some(&admin),
            Some(json!({ "confirmation": "wrong.tar.gz", "includeMedia": false })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], json!("confirmation_required"));
    let (status, body) = server
        .post(
            &format!("/api/admin/backups/{name}/restore"),
            Some(&admin),
            Some(json!({ "includeMedia": false })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // The real restore creates a safety copy first, then swaps the database.
    let (status, restored) = server
        .post(
            &format!("/api/admin/backups/{name}/restore"),
            Some(&admin),
            Some(json!({ "confirmation": name, "includeMedia": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["dryRun"], json!(false));
    assert_eq!(restored["games"], json!(1));
    assert_eq!(restored["restartRequired"], json!(true));
    let safety = restored["safetyBackup"]
        .as_str()
        .expect("safety backup name")
        .to_string();
    assert!(safety.starts_with("ludex-pre-restore-"), "{safety}");
    assert!(
        server.data_dir.join("backups").join(&safety).is_file(),
        "the safety archive must exist before the restore"
    );
    let database = server.data_dir.join("ludex.db");
    assert!(database.is_file(), "restore must write the database");
}
