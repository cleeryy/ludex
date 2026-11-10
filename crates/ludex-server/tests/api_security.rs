//! Auth hardening tests (#19): throttling, lockout, revocation, sessions and
//! password changes.

mod common;

use std::time::Duration;

use common::{TestServer, ADMIN_PASSWORD};
use reqwest::StatusCode;
use serde_json::{json, Value};

fn count_sessions(body: &Value) -> usize {
    body.as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn login_is_throttled_then_locked_out() {
    let server = TestServer::start_with(|config| {
        config.login_max_attempts = 3;
        config.login_lockout = Duration::from_secs(60);
        config.login_backoff = Duration::from_millis(1);
    })
    .await;
    let _ = server.setup().await;

    for attempt in 1..=2 {
        let (status, body) = server.login("admin", "wrong-password").await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "attempt {attempt}: {body}"
        );
        assert_eq!(body["error"]["code"], "invalid_credentials");
    }

    let (status, body) = server.login("admin", "wrong-password").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "login_throttled");

    // Even the right password is refused while the key is locked out.
    let (status, body) = server.login("admin", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("seconds")));

    let response = server
        .client
        .post(format!("{}/api/auth/login", server.url))
        .json(&json!({ "username": "admin", "password": ADMIN_PASSWORD }))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .expect("Retry-After header");
    assert!(retry >= 1);
    server.stop().await;
}

#[tokio::test]
async fn logout_revokes_the_token_and_lists_sessions() {
    let server = TestServer::start().await;
    let _ = server.setup().await;

    let (status, first) = server.login("admin", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let first_token = first["token"].as_str().expect("token").to_string();
    assert!(first["expiresAt"].as_str().is_some());

    let (status, second) = server.login("admin", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let second_token = second["token"].as_str().expect("token").to_string();

    let (status, sessions) = server.get("/api/auth/sessions", Some(&first_token)).await;
    assert_eq!(status, StatusCode::OK, "{sessions}");
    assert_eq!(count_sessions(&sessions), 2);
    let current = sessions
        .as_array()
        .expect("sessions")
        .iter()
        .filter(|session| session["current"] == true)
        .count();
    assert_eq!(current, 1, "exactly one session is current");

    let (status, _) = server
        .post("/api/auth/logout", Some(&second_token), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = server.get("/api/auth/me", Some(&second_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "session_revoked");

    let (status, sessions) = server.get("/api/auth/sessions", Some(&first_token)).await;
    assert_eq!(status, StatusCode::OK);
    let revoked = sessions
        .as_array()
        .expect("sessions")
        .iter()
        .filter(|session| session["revokedAt"].is_string())
        .count();
    assert_eq!(revoked, 1);

    // Revoking the session used by the caller signs it out too.
    let current_id = sessions
        .as_array()
        .expect("sessions")
        .iter()
        .find(|session| session["current"] == true)
        .and_then(|session| session["id"].as_i64())
        .expect("current session");
    let (status, _) = server
        .delete(&format!("/api/auth/sessions/{current_id}"), &first_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = server.get("/api/auth/me", Some(&first_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    server.stop().await;
}

#[tokio::test]
async fn password_change_keeps_the_calling_session_and_kills_the_others() {
    let server = TestServer::start().await;
    let _ = server.setup().await;

    let (_, first) = server.login("admin", ADMIN_PASSWORD).await;
    let first_token = first["token"].as_str().expect("token").to_string();
    let (_, second) = server.login("admin", ADMIN_PASSWORD).await;
    let second_token = second["token"].as_str().expect("token").to_string();

    let (status, body) = server
        .post(
            "/api/me/password",
            Some(&first_token),
            Some(json!({
                "currentPassword": ADMIN_PASSWORD,
                "newPassword": "password456"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, _) = server.get("/api/auth/me", Some(&first_token)).await;
    assert_eq!(status, StatusCode::OK, "the caller keeps its token");
    let (status, body) = server.get("/api/auth/me", Some(&second_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "session_revoked");

    let (status, _) = server.login("admin", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, login) = server.login("admin", "password456").await;
    assert_eq!(status, StatusCode::OK, "{login}");
    server.stop().await;
}

#[tokio::test]
async fn admin_can_revoke_every_session_of_a_user() {
    let server = TestServer::start().await;
    let (admin_token, _) = server.setup().await;
    let user = server
        .create_user(&admin_token, "editor1", "password123", "editor")
        .await;
    let user_id = user["id"].as_i64().expect("id");

    let (status, login) = server.login("editor1", "password123").await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let editor_token = login["token"].as_str().expect("token").to_string();
    let (status, _) = server.get("/api/auth/me", Some(&editor_token)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = server
        .delete(&format!("/api/users/{user_id}/sessions"), &admin_token)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revoked"], 1);

    let (status, body) = server.get("/api/auth/me", Some(&editor_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "session_revoked");

    // A fresh login works again.
    let (status, _) = server.login("editor1", "password123").await;
    assert_eq!(status, StatusCode::OK);

    // Only administrators may revoke another account's sessions.
    let (_, editor_login) = server.login("editor1", "password123").await;
    let editor_token = editor_login["token"].as_str().expect("token").to_string();
    let (status, _) = server
        .delete(&format!("/api/users/{user_id}/sessions"), &editor_token)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    server.stop().await;
}

#[tokio::test]
async fn global_rate_limit_returns_retry_after() {
    let server = TestServer::start_with(|config| {
        config.rate_limit_rps = 1;
        config.rate_limit_burst = 2;
    })
    .await;
    let mut limited = 0;
    let mut retry_after = false;
    for _ in 0..6 {
        let response = server
            .client
            .get(format!("{}/api/config", server.url))
            .send()
            .await
            .expect("request");
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            limited += 1;
            retry_after |= response.headers().get("retry-after").is_some();
            let body = response.text().await.unwrap_or_default();
            assert!(body.contains("rate_limited"), "{body}");
        }
    }
    assert!(limited >= 3, "rate limiter let too many requests through");
    assert!(retry_after, "429 responses must carry Retry-After");

    // Health checks are exempt so probes never page anyone.
    for _ in 0..5 {
        let (status, _) = server.get("/api/health", None).await;
        assert_eq!(status, StatusCode::OK);
    }
    server.stop().await;
}
