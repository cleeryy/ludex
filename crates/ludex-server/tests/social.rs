//! Community phase 2 (#161): friend lifecycle, presence privacy and
//! notification behaviour.

mod common;

use common::TestServer;
use reqwest::StatusCode;
use serde_json::{json, Value};

async fn make_user(server: &TestServer, admin: &str, username: &str) -> (i64, String) {
    let user = server
        .create_user(admin, username, "password123", "user")
        .await;
    let (status, login) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": username, "password": "password123"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    (
        user["id"].as_i64().expect("user id"),
        login["token"].as_str().expect("token").to_string(),
    )
}

async fn unlock_for(
    server: &TestServer,
    token: &str,
    game: i64,
    key: &str,
    user_id: i64,
) -> (StatusCode, Value) {
    server
        .post(
            &format!("/api/games/{game}/achievements/{key}/unlock"),
            Some(token),
            Some(json!({"userId": user_id, "source": "test"})),
        )
        .await
}

async fn befriend(server: &TestServer, requester: &str, username: &str, addressee: &str) -> i64 {
    let (status, body) = server
        .post(
            "/api/friends/requests",
            Some(requester),
            Some(json!({ "username": username })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let request_id = body["id"].as_i64().expect("request id");
    let (status, accepted) = server
        .post(
            &format!("/api/friends/requests/{request_id}/accept"),
            Some(addressee),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    request_id
}

#[tokio::test]
async fn friendships_respect_privacy_and_presence() {
    let server = TestServer::start().await;
    let (admin_token, admin) = server.setup().await;
    let admin_id = admin["id"].as_i64().expect("admin id");
    let (bob_id, bob_token) = make_user(&server, &admin_token, "bob").await;
    let (_, ines_token) = make_user(&server, &admin_token, "ines").await;

    let game = server
        .add_game(&admin_token, "Hades (2020).zip", 3 * 1024 * 1024)
        .await;

    // Presence and friends activity are opt-in, independent from the profile.
    let (status, profile) = server
        .patch(
            "/api/me/profile",
            &admin_token,
            json!({"presenceEnabled": true, "friendsActivity": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{profile}");
    assert_eq!(profile["presenceEnabled"], true);
    assert_eq!(profile["friendsActivity"], true);
    server
        .patch(
            "/api/me/profile",
            &bob_token,
            json!({"presenceEnabled": false, "friendsActivity": true}),
        )
        .await;
    server
        .patch(
            "/api/me/profile",
            &ines_token,
            json!({"presenceEnabled": true}),
        )
        .await;

    // Exact-username request lifecycle.
    let (status, request) = server
        .post(
            "/api/friends/requests",
            Some(&admin_token),
            Some(json!({ "username": "bob" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{request}");
    let request_id = request["id"].as_i64().expect("request id");
    assert_eq!(request["user"]["username"], "bob");

    let (status, own) = server.get("/api/friends", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(own["friends"].as_array().expect("friends").len(), 0);
    assert_eq!(own["outgoing"].as_array().expect("outgoing").len(), 1);
    let body = own.to_string();
    assert!(
        !body.contains('@'),
        "friend payloads must never carry an email: {body}"
    );

    // A second request in either direction is rejected.
    let (status, _) = server
        .post(
            "/api/friends/requests",
            Some(&admin_token),
            Some(json!({ "userId": bob_id })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, inbox) = server.get("/api/friends", Some(&bob_token)).await;
    assert_eq!(status, StatusCode::OK, "{inbox}");
    assert_eq!(inbox["incoming"].as_array().expect("incoming").len(), 1);

    let (status, accepted) = server
        .post(
            &format!("/api/friends/requests/{request_id}/accept"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["user"]["username"], "admin");

    let (_, admin_friends) = server.get("/api/friends", Some(&admin_token)).await;
    assert_eq!(admin_friends["friends"][0]["user"]["id"], bob_id);
    // Bob has presence off: friends see no presence at all.
    assert_eq!(admin_friends["friends"][0]["presence"], Value::Null);

    let (_, bob_friends) = server.get("/api/friends", Some(&bob_token)).await;
    assert_eq!(bob_friends["friends"][0]["user"]["id"], admin_id);

    // Non-friends see nothing: no friend, no presence.
    let (_, ines_friends) = server.get("/api/friends", Some(&ines_token)).await;
    assert_eq!(
        ines_friends["friends"].as_array().expect("friends").len(),
        0
    );
    let (_, ines_presence) = server.get("/api/presence/friends", Some(&ines_token)).await;
    assert_eq!(
        ines_presence["friends"].as_array().expect("presence").len(),
        0
    );

    // Heartbeat shows in-game to friends only.
    let (status, heartbeat) = server
        .post(
            "/api/presence",
            Some(&admin_token),
            Some(json!({"state": "in_game", "gameId": game})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{heartbeat}");
    assert_eq!(heartbeat["enabled"], true);

    let (_, bob_presence) = server.get("/api/presence/friends", Some(&bob_token)).await;
    let entry = &bob_presence["friends"][0];
    assert_eq!(entry["userId"], admin_id);
    assert_eq!(entry["state"], "in_game");
    assert_eq!(entry["game"]["title"], "Hades");
    assert_eq!(bob_presence["pollAfterSeconds"], 30);

    let (_, ines_presence) = server.get("/api/presence/friends", Some(&ines_token)).await;
    assert_eq!(
        ines_presence["friends"].as_array().expect("presence").len(),
        0,
        "a non-friend never sees presence"
    );

    // Enabling presence exposes at most online/offline; unknown states fail.
    let (status, _) = server
        .post(
            "/api/presence",
            Some(&admin_token),
            Some(json!({"state": "dancing"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = server
        .post(
            "/api/presence",
            Some(&admin_token),
            Some(json!({"state": "in_game"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Opting in later shows the last presence to friends.
    server
        .patch(
            "/api/me/profile",
            &bob_token,
            json!({"presenceEnabled": true}),
        )
        .await;
    server
        .post(
            "/api/presence",
            Some(&bob_token),
            Some(json!({"state": "online"})),
        )
        .await;
    let (_, admin_presence) = server
        .get("/api/presence/friends", Some(&admin_token))
        .await;
    let entry = &admin_presence["friends"][0];
    assert_eq!(entry["userId"], bob_id);
    assert_eq!(entry["state"], "online");

    // Friends-only feed: bob shares his activity, ines has no friends.
    server
        .put(
            &format!("/api/games/{game}/progress"),
            &bob_token,
            json!({"state": "playing"}),
        )
        .await;
    let (_, admin_feed) = server
        .get("/api/friends/activity?limit=20", Some(&admin_token))
        .await;
    let items = admin_feed["items"].as_array().expect("items");
    assert!(!items.is_empty(), "{admin_feed}");
    assert_eq!(items[0]["user"]["id"], bob_id, "{admin_feed}");
    let (_, ines_feed) = server
        .get("/api/friends/activity?limit=20", Some(&ines_token))
        .await;
    assert_eq!(ines_feed["items"].as_array().expect("items").len(), 0);

    // Removal is immediate on both sides.
    let (status, _) = server
        .delete(&format!("/api/friends/{bob_id}"), &admin_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, bob_friends) = server.get("/api/friends", Some(&bob_token)).await;
    assert_eq!(bob_friends["friends"].as_array().expect("friends").len(), 0);
    let (_, bob_presence) = server.get("/api/presence/friends", Some(&bob_token)).await;
    assert_eq!(
        bob_presence["friends"].as_array().expect("presence").len(),
        0
    );

    // Blocking removes pending requests and stops new ones.
    let (status, _) = server
        .post(
            "/api/friends/requests",
            Some(&bob_token),
            Some(json!({ "userId": admin_id })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = server
        .post(&format!("/api/blocks/{bob_id}"), Some(&admin_token), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, admin_friends) = server.get("/api/friends", Some(&admin_token)).await;
    assert_eq!(
        admin_friends["incoming"]
            .as_array()
            .expect("incoming")
            .len(),
        0
    );
    assert_eq!(
        admin_friends["blocked"].as_array().expect("blocked").len(),
        1
    );
    let (status, _) = server
        .post(
            "/api/friends/requests",
            Some(&bob_token),
            Some(json!({ "userId": admin_id })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a blocked account cannot tell it was blocked"
    );
    let (status, _) = server
        .post(
            "/api/friends/requests",
            Some(&admin_token),
            Some(json!({ "userId": bob_id })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = server
        .delete(&format!("/api/blocks/{bob_id}"), &admin_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Unknown usernames are not enumerable by search: only exact matches work.
    let (status, _) = server
        .post(
            "/api/friends/requests",
            Some(&ines_token),
            Some(json!({ "username": "bo" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    server.stop().await;
}

#[tokio::test]
async fn notifications_are_scoped_and_follow_preferences() {
    let server = TestServer::start().await;
    let (admin_token, admin) = server.setup().await;
    let admin_id = admin["id"].as_i64().expect("admin id");
    let (bob_id, bob_token) = make_user(&server, &admin_token, "bob").await;
    let (_ines_id, ines_token) = make_user(&server, &admin_token, "ines").await;

    let game = server
        .add_game(&admin_token, "Celeste (2018).zip", 2 * 1024 * 1024)
        .await;

    // A friend request lands in the recipient's list, not the actor's.
    let (status, request) = server
        .post(
            "/api/friends/requests",
            Some(&admin_token),
            Some(json!({"username": "bob"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{request}");
    let request_id = request["id"].as_i64().expect("request id");
    let kinds = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|item| item["kind"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let (_, admin_notifications) = server.get("/api/notifications", Some(&admin_token)).await;
    assert!(
        !kinds(&admin_notifications).contains(&"friend_request".to_string()),
        "the requester is never notified about their own request"
    );
    let (_, bob_notifications) = server.get("/api/notifications", Some(&bob_token)).await;
    assert!(kinds(&bob_notifications).contains(&"friend_request".to_string()));
    let request_notification = bob_notifications["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["kind"] == "friend_request")
        .expect("friend request notification");
    assert_eq!(request_notification["actor"]["username"], "admin");
    assert_eq!(request_notification["readAt"], Value::Null);
    assert!(bob_notifications["unreadCount"].as_i64().unwrap_or(0) >= 1);

    let (status, _) = server
        .post(
            &format!("/api/friends/requests/{request_id}/accept"),
            Some(&bob_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // Accepting retires the request notification; the acceptance notice goes
    // to the other account.
    let (_, bob_notifications) = server.get("/api/notifications", Some(&bob_token)).await;
    assert!(!kinds(&bob_notifications).contains(&"friend_request".to_string()));
    let (_, admin_notifications) = server.get("/api/notifications", Some(&admin_token)).await;
    assert!(kinds(&admin_notifications).contains(&"friend_accepted".to_string()));

    befriend(&server, &admin_token, "ines", &ines_token).await;
    // Bob shares friend activity; only achievements still need an opt-in.
    server
        .patch(
            "/api/me/profile",
            &bob_token,
            json!({"friendsActivity": true}),
        )
        .await;

    // Achievement unlocks only notify friends after the recipient opts in.
    let (status, _) = server
        .post(
            &format!("/api/games/{game}/achievements"),
            Some(&admin_token),
            Some(json!({"key": "first_steps", "name": "First Steps", "points": 10})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = server
        .post(
            &format!("/api/games/{game}/achievements"),
            Some(&admin_token),
            Some(json!({"key": "archivist", "name": "Archivist", "points": 30})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = unlock_for(&server, &admin_token, game, "first_steps", bob_id).await;
    assert_eq!(status, StatusCode::OK);
    let (_, admin_notifications) = server.get("/api/notifications", Some(&admin_token)).await;
    assert!(
        !kinds(&admin_notifications).contains(&"friend_achievement".to_string()),
        "friendAchievements is opt-in and off by default"
    );

    server
        .patch(
            "/api/me/notification-prefs",
            &admin_token,
            json!({"friendAchievements": true}),
        )
        .await;
    let (status, _) = unlock_for(&server, &admin_token, game, "archivist", bob_id).await;
    assert_eq!(status, StatusCode::OK);

    let (_, admin_notifications) = server.get("/api/notifications", Some(&admin_token)).await;
    let achievement = admin_notifications["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["kind"] == "friend_achievement")
        .expect("friend achievement notification");
    assert_eq!(achievement["actor"]["username"], "bob");
    assert_eq!(achievement["game"]["title"], "Celeste");
    assert_eq!(achievement["achievement"]["key"], "archivist");
    assert_eq!(achievement["payload"]["achievementName"], "Archivist");

    // Own unlocks never notify the actor.
    let (status, _) = unlock_for(&server, &admin_token, game, "first_steps", admin_id).await;
    assert_eq!(status, StatusCode::OK);
    let (_, admin_notifications) = server.get("/api/notifications", Some(&admin_token)).await;
    assert!(
        admin_notifications["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["kind"] != "friend_achievement" || item["actor"]["id"] != admin_id),
        "no self notification"
    );

    // Publishing a collection notifies friends but not the owner.
    let (status, _) = server
        .post(
            "/api/collections",
            Some(&bob_token),
            Some(json!({"name": "Night shelf", "isPublic": true})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, admin_notifications) = server.get("/api/notifications", Some(&admin_token)).await;
    let share = admin_notifications["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["kind"] == "collection_share")
        .expect("collection share notification");
    assert_eq!(share["payload"]["collectionName"], "Night shelf");
    assert_eq!(share["actor"]["username"], "bob");

    // Read one, then all.
    let (status, read) = server
        .post(
            &format!("/api/notifications/{}/read", achievement["id"]),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["updated"], 1);
    let remaining = read["unreadCount"].as_i64().expect("unread count");
    assert!(remaining >= 1);

    let (status, all) = server
        .post("/api/notifications/read-all", Some(&admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(all["unreadCount"], 0);
    let (_, admin_notifications) = server.get("/api/notifications", Some(&admin_token)).await;
    assert_eq!(admin_notifications["unreadCount"], 0);
    assert!(admin_notifications["items"]
        .as_array()
        .expect("items")
        .iter()
        .all(|item| item["readAt"].is_string()));

    // Marking another account's notification is a 404, never a silent read.
    let (status, _) = server
        .post(
            &format!("/api/notifications/{}/read", achievement["id"]),
            Some(&ines_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Preferences round-trip.
    let (status, prefs) = server
        .patch(
            "/api/me/notification-prefs",
            &admin_token,
            json!({"friendRequests": false, "collectionShares": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(prefs["friendRequests"], false);
    assert_eq!(prefs["friendAchievements"], true);
    assert_eq!(prefs["friendAccepted"], true);

    // A private profile never appears to a non-friend, and notifications stay
    // scoped to their recipient.
    let (status, _) = server
        .get(&format!("/api/community/users/{bob_id}"), Some(&ines_token))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, ines_notifications) = server.get("/api/notifications", Some(&ines_token)).await;
    assert!(
        !kinds(&ines_notifications).contains(&"friend_achievement".to_string()),
        "non-friends receive no achievement notifications: {ines_notifications}"
    );

    server.stop().await;
}
