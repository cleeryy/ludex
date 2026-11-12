//! Shareable collections (#173): public token pages, visibility filtering and
//! copy-to-library.
//!
//! These tests drive the real HTTP router and a real SQLite database. They
//! assert the safety properties promised by the issue: a public payload never
//! contains account data, private shelves never leak, revocation is immediate
//! and importing the same share twice does not duplicate the shelf.

mod common;

use common::TestServer;
use ludex_server::config::default_database_url;
use reqwest::StatusCode;
use serde_json::{json, Value};

async fn setup_admin() -> (TestServer, String, i64) {
    let server = TestServer::start().await;
    let (token, user) = server.setup().await;
    let id = user["id"].as_i64().expect("admin id");
    (server, token, id)
}

async fn login_as(server: &TestServer, admin_token: &str, username: &str) -> String {
    let (status, created) = server
        .post(
            "/api/users",
            Some(admin_token),
            Some(json!({"username": username, "password": "password123", "role": "user"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": username, "password": "password123"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    login["token"].as_str().expect("token").to_string()
}

async fn patch(server: &TestServer, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
    server
        .request(reqwest::Method::PATCH, path, Some(token), Some(body))
        .await
}

async fn create_collection(
    server: &TestServer,
    token: &str,
    name: &str,
    visibility: &str,
    game_ids: &[i64],
) -> Value {
    let (status, body) = server
        .post(
            "/api/collections",
            Some(token),
            Some(json!({
                "name": name,
                "visibility": visibility,
                "gameIds": game_ids,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

#[tokio::test]
async fn collection_share_links_are_public_revocable_and_optional_password() {
    let (server, admin_token, _admin_id) = setup_admin().await;
    let hades = server
        .add_game(&admin_token, "Hades (2020).zip", 2 * 1024 * 1024)
        .await;
    let celeste = server
        .add_game(&admin_token, "Celeste (2018).zip", 2 * 1024 * 1024)
        .await;

    let collection = create_collection(
        &server,
        &admin_token,
        "Weekend shelf",
        "unlisted",
        &[hades, celeste],
    )
    .await;
    let collection_id = collection["id"].as_i64().expect("collection id");
    assert_eq!(collection["visibility"], "unlisted");
    assert_eq!(collection["isPublic"], false);

    // Private shelves cannot be shared at all.
    let private =
        create_collection(&server, &admin_token, "Private shelf", "private", &[hades]).await;
    let (status, body) = server
        .post(
            &format!("/api/collections/{}/share", private["id"]),
            Some(&admin_token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "collection_private");

    let (status, link) = server
        .post(
            &format!("/api/collections/{collection_id}/share"),
            Some(&admin_token),
            Some(json!({"label": "for the group chat"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{link}");
    let share_id = link["id"].as_i64().expect("share id");
    let token = link["token"].as_str().expect("token").to_string();
    assert!(token.starts_with("lsc_"));
    assert_eq!(link["hasPassword"], false);
    assert_eq!(link["collectionId"], collection_id);

    // Public page: no auth, metadata only.
    let (status, view) = server
        .get(&format!("/api/share/collection/{token}"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["collection"]["name"], "Weekend shelf");
    assert_eq!(view["collection"]["gameCount"], 2);
    assert_eq!(view["games"].as_array().expect("games").len(), 2);
    assert_eq!(view["hasPassword"], false);
    // The owner has no public profile, so no display name is exposed.
    assert!(view["collection"]["owner"].is_null(), "{view}");

    let serialized = view.to_string();
    for forbidden in [
        "username",
        "ownerId",
        "filePath",
        "file_path",
        "minutesPlayed",
        "bookmarked",
        "progress",
        "download",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "public payload leaked {forbidden}: {serialized}"
        );
    }

    // The management list never repeats the token.
    let (status, links) = server
        .get(
            &format!("/api/collections/{collection_id}/shares"),
            Some(&admin_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{links}");
    assert_eq!(links.as_array().expect("links").len(), 1);
    assert!(links[0]["token"].is_null());

    // Revocation invalidates the public page immediately.
    let (status, _) = server
        .delete(
            &format!("/api/collections/{collection_id}/shares/{share_id}"),
            &admin_token,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = server
        .get(&format!("/api/share/collection/{token}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A password gates the games, not the link's existence.
    let (status, protected) = server
        .post(
            &format!("/api/collections/{collection_id}/share"),
            Some(&admin_token),
            Some(json!({"password": "open sesame", "expiresInHours": 24})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{protected}");
    let protected_token = protected["token"].as_str().expect("token").to_string();
    assert_eq!(protected["hasPassword"], true);

    let (status, preview) = server
        .get(&format!("/api/share/collection/{protected_token}"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["hasPassword"], true);
    assert_eq!(preview["collection"]["gameCount"], 2);
    assert_eq!(preview["games"].as_array().expect("games").len(), 0);

    let (status, body) = server
        .post(
            &format!("/api/share/collection/{protected_token}/access"),
            None,
            Some(json!({"password": "wrong"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid_share_password");

    let (status, unlocked) = server
        .post(
            &format!("/api/share/collection/{protected_token}/access"),
            None,
            Some(json!({"password": "open sesame"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{unlocked}");
    assert_eq!(unlocked["games"].as_array().expect("games").len(), 2);

    // Expiry: move the expiry into the past and the public page disappears.
    let pool = ludex_server::db::connect(&default_database_url(&server.data_dir))
        .await
        .expect("connect");
    ludex_server::db::query("UPDATE collection_shares SET expires_at = ? WHERE id = ?")
        .bind("2020-01-01T00:00:00Z")
        .bind(protected["id"].as_i64().expect("share id"))
        .execute(&pool)
        .await
        .expect("expire link");
    drop(pool);
    let (status, body) = server
        .get(&format!("/api/share/collection/{protected_token}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    server.stop().await;
}

#[tokio::test]
async fn visibility_controls_listings_profiles_and_the_feed() {
    let (server, admin_token, admin_id) = setup_admin().await;
    let hades = server
        .add_game(&admin_token, "Hades (2020).zip", 2 * 1024 * 1024)
        .await;

    let private =
        create_collection(&server, &admin_token, "Private shelf", "private", &[hades]).await;
    let unlisted = create_collection(
        &server,
        &admin_token,
        "Unlisted shelf",
        "unlisted",
        &[hades],
    )
    .await;
    let public = create_collection(&server, &admin_token, "Public shelf", "public", &[hades]).await;
    let private_id = private["id"].as_i64().expect("id");
    let unlisted_id = unlisted["id"].as_i64().expect("id");
    let public_id = public["id"].as_i64().expect("id");

    let bob_token = login_as(&server, &admin_token, "bob").await;

    let (status, list) = server.get("/api/collections", Some(&bob_token)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ids: Vec<i64> = list
        .as_array()
        .expect("collections")
        .iter()
        .filter_map(|entry| entry["id"].as_i64())
        .collect();
    assert_eq!(ids, vec![public_id], "{list}");

    for hidden in [private_id, unlisted_id] {
        let (status, _) = server
            .get(&format!("/api/collections/{hidden}"), Some(&bob_token))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = server
            .get(
                &format!("/api/collections/{hidden}/games"),
                Some(&bob_token),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, detail) = server
        .get(&format!("/api/collections/{public_id}"), Some(&bob_token))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");

    // Make the owner visible, then the profile lists public shelves only.
    let (status, profile) = patch(
        &server,
        "/api/me/profile",
        &admin_token,
        json!({"publicProfile": true, "displayName": "Admin"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{profile}");
    let (status, page) = server
        .get(
            &format!("/api/community/users/{admin_id}"),
            Some(&bob_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let profile_ids: Vec<i64> = page["collections"]
        .as_array()
        .expect("collections")
        .iter()
        .filter_map(|entry| entry["id"].as_i64())
        .collect();
    assert_eq!(profile_ids, vec![public_id], "{page}");
    assert_eq!(page["stats"]["collections"], 1);

    // A share link still exposes the display name (profile is public now).
    let (_, link) = server
        .post(
            &format!("/api/collections/{unlisted_id}/share"),
            Some(&admin_token),
            Some(json!({})),
        )
        .await;
    let token = link["token"].as_str().expect("token");
    let (_, view) = server
        .get(&format!("/api/share/collection/{token}"), None)
        .await;
    assert_eq!(view["collection"]["owner"]["displayName"], "Admin");

    // Private and unlisted creations never reach the public feed.
    let (_, feed) = server
        .get("/api/community/feed?limit=50", Some(&bob_token))
        .await;
    let names: Vec<String> = feed["items"]
        .as_array()
        .expect("feed")
        .iter()
        .filter(|entry| entry["kind"] == "collection_created")
        .filter_map(|entry| entry["metadata"]["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(names, vec!["Public shelf".to_string()], "{feed}");

    // Publishing an existing shelf adds one feed entry.
    let (status, _) = patch(
        &server,
        &format!("/api/collections/{unlisted_id}"),
        &admin_token,
        json!({"visibility": "public"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, feed) = server
        .get("/api/community/feed?limit=50", Some(&bob_token))
        .await;
    let names: Vec<String> = feed["items"]
        .as_array()
        .expect("feed")
        .iter()
        .filter(|entry| entry["kind"] == "collection_created")
        .filter_map(|entry| entry["metadata"]["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        names,
        vec!["Unlisted shelf".to_string(), "Public shelf".to_string()],
        "{feed}"
    );

    // Switching back to private removes it from every other account.
    let (status, _) = patch(
        &server,
        &format!("/api/collections/{unlisted_id}"),
        &admin_token,
        json!({"visibility": "private"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = server
        .get(&format!("/api/collections/{unlisted_id}"), Some(&bob_token))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server
        .get(&format!("/api/share/collection/{token}"), None)
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the old token must die with the visibility switch"
    );

    server.stop().await;
}

#[tokio::test]
async fn copying_a_collection_is_idempotent_and_permissioned() {
    let (server, admin_token, _admin_id) = setup_admin().await;
    let hades = server
        .add_game(&admin_token, "Hades (2020).zip", 2 * 1024 * 1024)
        .await;
    let celeste = server
        .add_game(&admin_token, "Celeste (2018).zip", 2 * 1024 * 1024)
        .await;

    let public = create_collection(
        &server,
        &admin_token,
        "Public shelf",
        "public",
        &[hades, celeste],
    )
    .await;
    let public_id = public["id"].as_i64().expect("id");
    let private =
        create_collection(&server, &admin_token, "Private shelf", "private", &[hades]).await;
    let unlisted = create_collection(
        &server,
        &admin_token,
        "Unlisted shelf",
        "unlisted",
        &[hades],
    )
    .await;
    let private_id = private["id"].as_i64().expect("id");
    let unlisted_id = unlisted["id"].as_i64().expect("id");

    let bob_token = login_as(&server, &admin_token, "bob").await;

    // A public collection can be copied by id.
    let (status, first) = server
        .post(
            "/api/collections/import",
            Some(&bob_token),
            Some(json!({"sourceId": public_id})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["created"], true);
    assert_eq!(first["added"], 2);
    assert_eq!(first["skipped"], 0);
    assert_eq!(first["collection"]["name"], "Public shelf");
    assert_eq!(first["collection"]["owner"]["username"], "bob");
    assert_eq!(first["collection"]["visibility"], "private");
    assert_eq!(first["collection"]["gameCount"], 2);
    let copied_id = first["collection"]["id"].as_i64().expect("copied id");

    // Importing again merges into the same shelf instead of duplicating it.
    let (status, second) = server
        .post(
            "/api/collections/import",
            Some(&bob_token),
            Some(json!({"sourceId": public_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["created"], false);
    assert_eq!(second["added"], 0);
    assert_eq!(second["collection"]["id"], copied_id);
    let (_, bob_list) = server.get("/api/collections", Some(&bob_token)).await;
    assert_eq!(
        bob_list
            .as_array()
            .expect("collections")
            .iter()
            .filter(|entry| entry["owned"] == true)
            .count(),
        1,
        "{bob_list}"
    );

    // A private collection is never importable, even by id.
    let (status, body) = server
        .post(
            "/api/collections/import",
            Some(&bob_token),
            Some(json!({"sourceId": private_id})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Unlisted shelves are only importable through a live token.
    let (status, _) = server
        .post(
            "/api/collections/import",
            Some(&bob_token),
            Some(json!({"sourceId": unlisted_id})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, link) = server
        .post(
            &format!("/api/collections/{unlisted_id}/share"),
            Some(&admin_token),
            Some(json!({"password": "open sesame"})),
        )
        .await;
    let token = link["token"].as_str().expect("token");

    let (status, body) = server
        .post(
            "/api/collections/import",
            Some(&bob_token),
            Some(json!({"token": token})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, body) = server
        .post(
            "/api/collections/import",
            Some(&bob_token),
            Some(json!({"token": token, "password": "wrong"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let (status, imported) = server
        .post(
            "/api/collections/import",
            Some(&bob_token),
            Some(json!({"token": token, "password": "open sesame", "name": "From a link"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{imported}");
    assert_eq!(imported["collection"]["name"], "From a link");
    assert_eq!(imported["added"], 1);

    // A smart source keeps its rules; the target evaluates them for its owner.
    let (status, smart) = server
        .post(
            "/api/collections",
            Some(&admin_token),
            Some(json!({
                "name": "Hades only",
                "kind": "smart",
                "visibility": "public",
                "rules": {"match": "all", "rules": [
                    {"field": "title", "op": "contains", "value": "Hades"}
                ]}
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{smart}");
    let smart_id = smart["id"].as_i64().expect("smart id");
    let (status, copied_smart) = server
        .post(
            "/api/collections/import",
            Some(&bob_token),
            Some(json!({"sourceId": smart_id})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{copied_smart}");
    assert_eq!(copied_smart["collection"]["kind"], "smart");
    assert_eq!(
        copied_smart["collection"]["rules"]["rules"][0]["field"],
        "title"
    );
    assert_eq!(
        copied_smart["collection"]["rules"]["rules"][0]["value"],
        "Hades"
    );

    // Bob cannot create or revoke a share on the admin's shelf.
    let (status, _) = server
        .post(
            &format!("/api/collections/{public_id}/share"),
            Some(&bob_token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = server
        .delete(
            &format!("/api/collections/{public_id}/shares/1"),
            &bob_token,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    server.stop().await;
}
