//! Admin Games manager (#209) and the missing-archive read side (#210).
//!
//! Covers the paginated admin list, the two delete modes with their ownership
//! checks, symlink and path-escape refusals, idempotent missing files, bulk
//! preview/partial failures, the background job, the audit trail and the
//! scan → missing → hidden → restored lifecycle.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{run_scan, TestServer};
use reqwest::StatusCode;
use serde_json::{json, Value};

const MIB: usize = 1024 * 1024;

fn write_archive(server: &TestServer, name: &str, size: usize) -> PathBuf {
    let path = server.games_dir.join(name);
    std::fs::write(&path, vec![7_u8; size]).expect("write game");
    path
}

async fn admin_games(server: &TestServer, token: &str, query: &str) -> Value {
    let (status, body) = server
        .get(&format!("/api/admin/games{query}"), Some(token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn game_ids(server: &TestServer, token: &str) -> Vec<i64> {
    let body = admin_games(server, token, "?status=all&limit=200").await;
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect()
}

#[tokio::test]
async fn admin_games_are_admin_only_and_filter_paginate_and_count() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    server
        .create_user(&admin, "editor", "editorpass123", "editor")
        .await;
    server
        .create_user(&admin, "viewer", "viewerpass123", "user")
        .await;
    let (_, body) = server.login("editor", "editorpass123").await;
    let editor = body["token"].as_str().expect("editor token").to_string();
    let (_, body) = server.login("viewer", "viewerpass123").await;
    let viewer = body["token"].as_str().expect("viewer token").to_string();

    for name in [
        "Hades (2020).zip",
        "Celeste (2018).zip",
        "Hollow Knight (2017).zip",
    ] {
        write_archive(&server, name, MIB + 16);
    }
    run_scan(&server, &admin).await;

    // Editors and players never see the admin table.
    for token in [&editor, &viewer] {
        let (status, body) = server.get("/api/admin/games", Some(token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }

    let first = admin_games(&server, &admin, "?sort=title&order=asc&limit=2").await;
    assert_eq!(first["total"].as_i64(), Some(3), "{first}");
    assert_eq!(first["page"].as_i64(), Some(1), "{first}");
    assert_eq!(
        first["items"].as_array().expect("items").len(),
        2,
        "{first}"
    );
    assert_eq!(first["items"][0]["title"], "Celeste", "{first}");
    assert_eq!(first["items"][0]["status"], "available", "{first}");
    assert_eq!(first["items"][0]["availableVersionCount"], 1, "{first}");
    assert_eq!(first["items"][0]["missingVersionCount"], 0, "{first}");
    assert_eq!(first["counts"]["available"].as_i64(), Some(3), "{first}");
    assert_eq!(first["counts"]["missing"].as_i64(), Some(0), "{first}");
    assert_eq!(first["counts"]["total"].as_i64(), Some(3), "{first}");

    let second = admin_games(&server, &admin, "?sort=title&order=asc&limit=2&page=2").await;
    assert_eq!(
        second["items"].as_array().expect("items").len(),
        1,
        "{second}"
    );

    let search = admin_games(&server, &admin, "?q=hades").await;
    assert_eq!(search["total"].as_i64(), Some(1), "{search}");
    assert_eq!(search["items"][0]["title"], "Hades", "{search}");

    let missing = admin_games(&server, &admin, "?status=missing").await;
    assert_eq!(missing["total"].as_i64(), Some(0), "{missing}");

    let (status, body) = server.get("/api/admin/games?sort=nope", Some(&admin)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_sort");
}

#[tokio::test]
async fn remove_from_library_keeps_the_file_and_creates_an_audit_entry() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    server
        .create_user(&admin, "editor", "editorpass123", "editor")
        .await;
    let (_, body) = server.login("editor", "editorpass123").await;
    let editor = body["token"].as_str().expect("editor token").to_string();
    server
        .create_user(&admin, "viewer", "viewerpass123", "user")
        .await;
    let (_, body) = server.login("viewer", "viewerpass123").await;
    let viewer = body["token"].as_str().expect("viewer token").to_string();

    let path = write_archive(&server, "Hades (2020).zip", MIB + 32);
    run_scan(&server, &admin).await;
    let id = game_ids(&server, &admin).await[0];
    server
        .put(
            &format!("/api/games/{id}/progress"),
            &editor,
            json!({ "state": "playing" }),
        )
        .await;
    server
        .put(
            &format!("/api/games/{id}/bookmark"),
            &editor,
            json!({ "bookmarked": true }),
        )
        .await;

    // A player cannot remove anything, the mode is required, confirm too.
    let (status, _) = server
        .delete(
            &format!("/api/games/{id}?mode=library&confirm=true"),
            &viewer,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = server.delete(&format!("/api/games/{id}"), &editor).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "mode_required");
    let (status, body) = server
        .delete(&format!("/api/games/{id}?mode=library"), &editor)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "confirmation_required");

    let (status, body) = server
        .delete(
            &format!("/api/games/{id}?mode=library&confirm=true"),
            &editor,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "library", "{body}");
    assert_eq!(body["deletedFiles"].as_i64(), Some(0), "{body}");
    assert_eq!(body["keptFiles"].as_i64(), Some(1), "{body}");
    assert_eq!(body["versions"].as_i64(), Some(1), "{body}");
    assert!(path.exists(), "library mode must keep the archive");

    let (status, _) = server.get(&format!("/api/games/{id}"), Some(&admin)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(game_ids(&server, &admin).await.is_empty());

    // The archive is still on disk, so the next scan re-indexes it.
    run_scan(&server, &admin).await;
    assert_eq!(
        game_ids(&server, &admin).await.len(),
        1,
        "a rescan re-adds a library-only removal"
    );

    let (status, audit) = server
        .get("/api/audit?action=game.deleted&limit=10", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{audit}");
    let entry = &audit["items"][0];
    assert_eq!(entry["action"], "game.deleted", "{audit}");
    assert_eq!(entry["targetId"], id.to_string(), "{audit}");
    assert_eq!(entry["metadata"]["mode"], "library", "{audit}");
    assert_eq!(
        entry["metadata"]["deletedFiles"].as_i64(),
        Some(0),
        "{audit}"
    );
}

#[tokio::test]
async fn delete_archives_is_admin_only_idempotent_and_refuses_symlinks() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    server
        .create_user(&admin, "editor", "editorpass123", "editor")
        .await;
    let (_, body) = server.login("editor", "editorpass123").await;
    let editor = body["token"].as_str().expect("editor token").to_string();

    let path = write_archive(&server, "Hades (2020).zip", MIB + 40);
    run_scan(&server, &admin).await;
    let id = game_ids(&server, &admin).await[0];

    let (status, body) = server
        .delete(
            &format!("/api/games/{id}?mode=archives&confirm=true"),
            &editor,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = server
        .delete(
            &format!("/api/games/{id}?mode=archives&confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deletedFiles"].as_i64(), Some(1), "{body}");
    assert_eq!(
        body["freedBytes"].as_i64(),
        Some((MIB + 40) as i64),
        "{body}"
    );
    assert!(!path.exists(), "archives mode deletes the file");

    // A file that is already gone is not an error: the row is still removed.
    let gone = write_archive(&server, "Celeste (2018).zip", MIB + 8);
    run_scan(&server, &admin).await;
    let missing_id = game_ids(&server, &admin).await[0];
    std::fs::remove_file(&gone).expect("remove");
    let (status, body) = server
        .delete(
            &format!("/api/games/{missing_id}?mode=archives&confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deletedFiles"].as_i64(), Some(0), "{body}");

    // A symlink is never followed: the target outside the root survives.
    let outside_dir = tempfile::tempdir().expect("outside");
    let outside = outside_dir.path().join("precious.zip");
    std::fs::write(&outside, vec![9_u8; MIB + 8]).expect("outside");
    let link_path = write_archive(&server, "Hollow Knight (2017).zip", MIB + 8);
    run_scan(&server, &admin).await;
    let link_id = game_ids(&server, &admin).await[0];
    std::fs::remove_file(&link_path).expect("remove");
    std::os::unix::fs::symlink(&outside, &link_path).expect("symlink");

    let (status, body) = server
        .delete(
            &format!("/api/games/{link_id}?mode=archives&confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "unsafe_path", "{body}");
    assert!(outside.exists(), "the symlink target must never be deleted");
    let (status, _) = server
        .get(&format!("/api/games/{link_id}"), Some(&admin))
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a refused delete must not remove rows"
    );
}

#[tokio::test]
async fn bulk_preview_confirms_counts_and_reports_partial_failures() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    for name in ["Hades (2020).zip", "Celeste (2018).zip"] {
        write_archive(&server, name, MIB + 10);
    }
    run_scan(&server, &admin).await;
    let ids = game_ids(&server, &admin).await;
    assert_eq!(ids.len(), 2);

    let (status, preview) = server
        .post(
            "/api/admin/games/bulk",
            Some(&admin),
            Some(json!({ "ids": ids.clone(), "action": "delete_archives", "dryRun": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["dryRun"], true, "{preview}");
    assert_eq!(preview["preview"]["games"].as_i64(), Some(2), "{preview}");
    assert_eq!(preview["preview"]["files"].as_i64(), Some(2), "{preview}");
    assert_eq!(
        preview["preview"]["bytes"].as_i64(),
        Some((2 * (MIB + 10)) as i64),
        "{preview}"
    );
    assert_eq!(
        preview["preview"]["items"][0]["paths"]
            .as_array()
            .expect("paths")
            .len(),
        1,
        "{preview}"
    );

    let (status, body) = server
        .post(
            "/api/admin/games/bulk",
            Some(&admin),
            Some(json!({ "ids": ids.clone(), "action": "delete_archives" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "confirmation_required");

    let mut with_ghost = ids.clone();
    with_ghost.push(999_999);
    let (status, body) = server
        .post(
            "/api/admin/games/bulk",
            Some(&admin),
            Some(json!({
                "ids": with_ghost,
                "action": "delete_archives",
                "confirm": true
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["totals"]["games"].as_i64(), Some(3), "{body}");
    assert_eq!(body["totals"]["succeeded"].as_i64(), Some(2), "{body}");
    assert_eq!(body["totals"]["failed"].as_i64(), Some(1), "{body}");
    let ghost = body["results"]
        .as_array()
        .expect("results")
        .iter()
        .find(|item| item["gameId"] == 999_999)
        .expect("ghost result");
    assert_eq!(ghost["ok"], false, "{body}");
    assert_eq!(ghost["code"], "game_not_found", "{body}");
}

#[tokio::test]
async fn bulk_delete_missing_versions_keeps_the_game() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let path = write_archive(&server, "Hades (2020).zip", MIB + 10);
    run_scan(&server, &admin).await;
    let id = game_ids(&server, &admin).await[0];

    std::fs::remove_file(&path).expect("remove");
    run_scan(&server, &admin).await;

    let (status, body) = server
        .post(
            "/api/admin/games/bulk",
            Some(&admin),
            Some(json!({
                "ids": [id],
                "action": "delete_missing_versions",
                "confirm": true
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["results"][0]["deletedVersions"].as_i64(),
        Some(1),
        "{body}"
    );

    // The metadata row survives with zero versions; no file was touched.
    let (status, detail) = server.get(&format!("/api/games/{id}"), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(
        detail["availableVersionCount"].as_i64(),
        Some(0),
        "{detail}"
    );
    assert_eq!(detail["missingVersionCount"].as_i64(), Some(0), "{detail}");
    assert_eq!(detail["status"], "missing", "{detail}");
}

#[tokio::test]
async fn large_archive_bulk_delete_runs_as_a_background_job() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    for index in 0..26 {
        write_archive(&server, &format!("Game {index:02} (2020).zip"), MIB + 4);
    }
    run_scan(&server, &admin).await;
    let ids = game_ids(&server, &admin).await;
    assert_eq!(ids.len(), 26);

    let (status, body) = server
        .post(
            "/api/admin/games/bulk",
            Some(&admin),
            Some(json!({ "ids": ids, "action": "delete_archives", "confirm": true })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let job_id = body["jobId"].as_str().expect("job id").to_string();

    let mut finished = None;
    for _ in 0..200 {
        let (status, job) = server
            .get(&format!("/api/admin/games/jobs/{job_id}"), Some(&admin))
            .await;
        assert_eq!(status, StatusCode::OK, "{job}");
        if job["status"] == "completed" {
            finished = Some(job);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let job = finished.expect("bulk job did not finish");
    assert_eq!(job["done"].as_i64(), Some(26), "{job}");
    assert_eq!(job["succeeded"].as_i64(), Some(26), "{job}");
    assert_eq!(
        job["results"].as_array().expect("results").len(),
        26,
        "{job}"
    );
    assert!(game_ids(&server, &admin).await.is_empty());
}

#[tokio::test]
async fn scan_marks_missing_hides_the_library_and_restores_the_same_row() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let path = write_archive(&server, "Hades (2020).zip", MIB + 64);
    run_scan(&server, &admin).await;

    let (_, list) = server.get("/api/games?limit=100", Some(&admin)).await;
    let id = list["items"][0]["id"].as_i64().expect("game id");
    server
        .put(
            &format!("/api/games/{id}/progress"),
            &admin,
            json!({ "state": "playing" }),
        )
        .await;

    // Deleting the archive outside Ludex: the scan marks the version missing
    // and keeps the metadata row.
    std::fs::remove_file(&path).expect("remove");
    run_scan(&server, &admin).await;

    let (_, visible) = server.get("/api/games?limit=100", Some(&admin)).await;
    assert_eq!(visible["total"].as_i64(), Some(0), "{visible}");
    let (_, hidden) = server
        .get("/api/games?limit=100&status=missing", Some(&admin))
        .await;
    assert_eq!(hidden["total"].as_i64(), Some(1), "{hidden}");
    assert_eq!(hidden["items"][0]["status"], "missing", "{hidden}");
    assert_eq!(
        hidden["items"][0]["versionCount"].as_i64(),
        Some(0),
        "{hidden}"
    );
    assert_eq!(
        hidden["items"][0]["missingVersionCount"].as_i64(),
        Some(1),
        "{hidden}"
    );

    // Deep link: the page still resolves and explains the state.
    let (status, detail) = server.get(&format!("/api/games/{id}"), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["status"], "missing", "{detail}");
    assert_eq!(
        detail["versions"].as_array().expect("versions").len(),
        0,
        "{detail}"
    );
    let missing = detail["missingVersions"]
        .as_array()
        .expect("missing versions");
    assert_eq!(missing.len(), 1, "{detail}");
    assert_eq!(missing[0]["missing"], true, "{detail}");
    assert_eq!(missing[0]["fileName"], "Hades (2020).zip", "{detail}");
    assert_eq!(detail["state"], "playing", "{detail}");

    // An offline root keeps the same "unavailable" state without deleting.
    // Re-adding the file and rescanning restores the same row and progress.
    write_archive(&server, "Hades (2020).zip", MIB + 64);
    run_scan(&server, &admin).await;
    let (_, restored) = server.get("/api/games?limit=100", Some(&admin)).await;
    assert_eq!(restored["total"].as_i64(), Some(1), "{restored}");
    assert_eq!(restored["items"][0]["id"].as_i64(), Some(id), "{restored}");
    assert_eq!(
        restored["items"][0]["availableVersionCount"].as_i64(),
        Some(1),
        "{restored}"
    );
    assert_eq!(restored["items"][0]["state"], "playing", "{restored}");
    assert_eq!(restored["items"][0]["status"], "available", "{restored}");
}
