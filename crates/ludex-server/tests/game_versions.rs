//! Version management and rollback contract (#168).
//!
//! The editor version manager lists archives with their state, pins a preferred
//! version (a rollback keeps newer ones available), and deletes one archive
//! without touching the game, its saves or the last available version.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{run_scan, TestServer};
use reqwest::StatusCode;
use serde_json::{json, Value};

const MIB: u64 = 1024 * 1024;

fn write_game(server: &TestServer, name: &str, size: u64) -> PathBuf {
    let path = server.games_dir.join(name);
    std::fs::write(&path, vec![3_u8; usize::try_from(size).expect("size")]).expect("write game");
    path
}

async fn only_game(server: &TestServer, token: &str) -> i64 {
    let (status, list) = server.get("/api/games?limit=10", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"][0]["id"].as_i64().expect("game id")
}

fn version_by_label<'a>(versions: &'a Value, label: &str) -> &'a Value {
    versions
        .as_array()
        .expect("versions")
        .iter()
        .find(|version| version["version"] == label)
        .unwrap_or_else(|| panic!("version {label} missing from {versions}"))
}

#[tokio::test]
async fn versions_are_listed_pinned_and_drive_latest_and_downloads() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let older = write_game(&server, "Hades (2020) v1.0.zip", MIB + 10);
    let newer = write_game(&server, "Hades (2020) v1.2.zip", MIB + 40);
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;

    // Detail keeps the existing shape: available versions plus the default id.
    let (status, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert!(detail["defaultVersionId"].is_null(), "{detail}");
    let detail_versions = detail["versions"].as_array().expect("versions");
    assert_eq!(detail_versions.len(), 2);
    let older_id = version_by_label(&detail["versions"], "1.0")["id"]
        .as_i64()
        .expect("older id");
    let newer_id = version_by_label(&detail["versions"], "1.2")["id"]
        .as_i64()
        .expect("newer id");

    // Players cannot manage versions, editors can list every archive.
    let _player = server
        .create_user(&admin, "player", "password123", "user")
        .await;
    let (status, login) = server.login("player", "password123").await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let player_token = login["token"].as_str().expect("player token").to_string();

    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/versions"),
            Some(&player_token),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server
        .put(
            &format!("/api/games/{game_id}/versions/default"),
            &player_token,
            json!({ "versionId": older_id }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, listing) = server
        .get(&format!("/api/games/{game_id}/versions"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing["gameId"], game_id);
    assert!(listing["defaultVersionId"].is_null(), "{listing}");
    let versions = listing["versions"].as_array().expect("versions");
    assert_eq!(versions.len(), 2);
    // Newest first, mirroring the detail ordering.
    assert_eq!(versions[0]["version"], "1.2", "{listing}");
    let first = &versions[0];
    assert_eq!(first["fileName"], "Hades (2020) v1.2.zip");
    assert_eq!(first["size"], (MIB + 40) as i64);
    assert_eq!(first["earlyAccess"], false);
    assert_eq!(first["missing"], false);
    assert_eq!(first["rootUnavailable"], false);
    assert!(first["releaseDate"].as_str().is_some(), "{first}");
    assert!(first["indexedAt"].as_str().is_some(), "{first}");
    assert!(
        first["archiveHash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256-sample:")),
        "{first}"
    );
    assert!(first["downloadUrl"]
        .as_str()
        .is_some_and(|url| url.contains(&format!("versionId={newer_id}"))));

    // Pin the older version: a rollback that keeps the newer archive.
    let (status, pinned) = server
        .put(
            &format!("/api/games/{game_id}/versions/default"),
            &admin,
            json!({ "versionId": older_id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{pinned}");
    assert_eq!(pinned["defaultVersionId"], older_id, "{pinned}");
    assert_eq!(
        pinned["versions"].as_array().expect("versions").len(),
        2,
        "rollback keeps both archives"
    );

    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&admin))
        .await;
    assert_eq!(detail["defaultVersionId"], older_id);

    // Update checks and default downloads follow the pin.
    let (status, latest) = server
        .get(
            &format!("/api/games/{game_id}/versions/latest"),
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{latest}");
    assert_eq!(latest["versionId"], older_id);
    assert_eq!(latest["versionLabel"], "1.0");

    let (status, ticket) = server
        .post(
            &format!("/api/games/{game_id}/download-ticket"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{ticket}");
    assert!(
        ticket["url"]
            .as_str()
            .is_some_and(|url| url.contains(&format!("versionId={older_id}"))),
        "{ticket}"
    );
    let (status, ticket) = server
        .post(
            &format!("/api/games/{game_id}/download-ticket"),
            Some(&admin),
            Some(json!({ "versionId": newer_id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{ticket}");
    assert!(
        ticket["url"]
            .as_str()
            .is_some_and(|url| url.contains(&format!("versionId={newer_id}"))),
        "explicit version still wins: {ticket}"
    );

    // Pinning is validated: unknown rows and other games are 404s.
    let (status, body) = server
        .put(
            &format!("/api/games/{game_id}/versions/default"),
            &admin,
            json!({ "versionId": 999_999 }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A missing archive is listed with its state and cannot be pinned; latest
    // falls back to the newest available version.
    std::fs::remove_file(&older).expect("remove older archive");
    run_scan(&server, &admin).await;
    let (status, listing) = server
        .get(&format!("/api/games/{game_id}/versions"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(listing["versions"].as_array().expect("versions").len(), 2);
    let missing = version_by_label(&listing["versions"], "1.0");
    assert_eq!(missing["missing"], true, "{listing}");
    assert_eq!(missing["rootUnavailable"], false);

    let (status, body) = server
        .put(
            &format!("/api/games/{game_id}/versions/default"),
            &admin,
            json!({ "versionId": older_id }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "version_missing");

    let (status, latest) = server
        .get(
            &format!("/api/games/{game_id}/versions/latest"),
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{latest}");
    assert_eq!(latest["versionId"], newer_id, "{latest}");

    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&admin))
        .await;
    assert_eq!(detail["versions"].as_array().expect("versions").len(), 1);
    assert!(newer.exists());

    // Unknown games and unauthenticated callers are explicit errors.
    let (status, body) = server.get("/api/games/999999/versions", Some(&admin)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _) = server
        .get(&format!("/api/games/{game_id}/versions"), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    server.stop().await;
}

#[tokio::test]
async fn deleting_a_version_keeps_the_game_and_refuses_unsafe_deletions() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let old = write_game(&server, "Doom (2016) v1.0.zip", MIB + 10);
    let middle = write_game(&server, "Doom (2016) v1.1.zip", MIB + 20);
    let newest = write_game(&server, "Doom (2016) v1.2.zip", MIB + 30);
    run_scan(&server, &admin).await;
    let game_id = only_game(&server, &admin).await;

    let (_, listing) = server
        .get(&format!("/api/games/{game_id}/versions"), Some(&admin))
        .await;
    let old_id = version_by_label(&listing["versions"], "1.0")["id"]
        .as_i64()
        .expect("old id");
    let middle_id = version_by_label(&listing["versions"], "1.1")["id"]
        .as_i64()
        .expect("middle id");
    let newest_id = version_by_label(&listing["versions"], "1.2")["id"]
        .as_i64()
        .expect("newest id");

    // Explicit confirmation, editor role and an existing row are required.
    let (status, body) = server
        .delete(&format!("/api/games/{game_id}/versions/{old_id}"), &admin)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "confirmation_required");

    let _player = server
        .create_user(&admin, "player", "password123", "user")
        .await;
    let (_, login) = server.login("player", "password123").await;
    let player_token = login["token"].as_str().expect("player token").to_string();
    let (status, body) = server
        .delete(
            &format!("/api/games/{game_id}/versions/{old_id}?confirm=true"),
            &player_token,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = server
        .delete(
            &format!("/api/games/{game_id}/versions/999999?confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Deleting an archive removes the file and the row, not the game.
    let (status, deleted) = server
        .delete(
            &format!("/api/games/{game_id}/versions/{old_id}?confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["versionId"], old_id);
    assert_eq!(deleted["freedBytes"], (MIB + 10) as i64);
    assert!(!old.exists(), "archive file must be removed");
    assert!(middle.exists() && newest.exists());

    let (_, listing) = server
        .get(&format!("/api/games/{game_id}/versions"), Some(&admin))
        .await;
    assert_eq!(listing["versions"].as_array().expect("versions").len(), 2);
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&admin))
        .await;
    assert_eq!(detail["title"], "Doom");
    assert_eq!(detail["versionCount"], 2);

    // Deleting the pinned version moves the pin to the newest remaining one.
    let (status, pinned) = server
        .put(
            &format!("/api/games/{game_id}/versions/default"),
            &admin,
            json!({ "versionId": middle_id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{pinned}");
    let (status, deleted) = server
        .delete(
            &format!("/api/games/{game_id}/versions/{middle_id}?confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["defaultVersionId"], newest_id, "{deleted}");
    assert!(!middle.exists());

    // The last available version can never be deleted.
    let (status, body) = server
        .delete(
            &format!("/api/games/{game_id}/versions/{newest_id}?confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "last_version");
    assert!(newest.exists());

    // An already missing archive is reported, not silently dropped.
    std::fs::remove_file(&newest).expect("remove newest archive");
    run_scan(&server, &admin).await;
    let (status, listing) = server
        .get(&format!("/api/games/{game_id}/versions"), Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert_eq!(
        version_by_label(&listing["versions"], "1.2")["missing"],
        true
    );
    let (status, body) = server
        .delete(
            &format!("/api/games/{game_id}/versions/{newest_id}?confirm=true"),
            &admin,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "version_missing");

    // Each deletion is audited; the row itself is gone, the trail is not.
    let mut audits = json!(null);
    for _ in 0..80 {
        let (_, page) = server
            .get(
                "/api/audit?action=game.version_deleted&limit=10",
                Some(&admin),
            )
            .await;
        audits = page;
        if audits["items"]
            .as_array()
            .is_some_and(|items| items.len() == 2)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let items = audits["items"].as_array().expect("audit items");
    assert_eq!(items.len(), 2, "{audits}");
    let target_id = game_id.to_string();
    assert!(items
        .iter()
        .all(|item| item["targetId"].as_str() == Some(target_id.as_str())));
    let deleted_ids: Vec<i64> = items
        .iter()
        .filter_map(|item| item["metadata"]["versionId"].as_i64())
        .collect();
    assert!(deleted_ids.contains(&old_id), "{audits}");
    assert!(deleted_ids.contains(&middle_id), "{audits}");

    server.stop().await;
}
