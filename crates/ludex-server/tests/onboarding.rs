//! Per-user onboarding preferences (#119).
//!
//! The wizard state belongs to the account, not the browser: first login
//! defaults, self-service updates, validation, isolation between accounts and
//! the fact that the administrator PATCH cannot touch these fields.

mod common;

use common::TestServer;
use reqwest::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn defaults_self_update_validation_and_persistence() {
    let server = TestServer::start().await;
    let (admin_token, created) = server.setup().await;

    assert!(
        created["onboardingCompletedAt"].is_null(),
        "new accounts start un-onboarded: {created}"
    );
    assert_eq!(created["onboardingStep"], 0);

    let (status, me) = server.get("/api/auth/me", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert!(me["user"]["onboardingCompletedAt"].is_null());
    assert_eq!(me["user"]["onboardingStep"], 0);

    // The account can persist a step without completing the wizard.
    let (status, updated) = server
        .put("/api/auth/onboarding", &admin_token, json!({ "step": 3 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["user"]["onboardingStep"], 3);
    assert!(updated["user"]["onboardingCompletedAt"].is_null());

    // Completing sets the timestamp and keeps the step.
    let (status, completed) = server
        .put(
            "/api/auth/onboarding",
            &admin_token,
            json!({ "completed": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    let completed_at = completed["user"]["onboardingCompletedAt"]
        .as_str()
        .expect("completion timestamp")
        .to_string();
    assert_eq!(completed["user"]["onboardingStep"], 3);

    // A fresh login sees the persisted preference, which is how it follows the
    // account across the web UI and the desktop app.
    let (status, login) = server.login("admin", common::ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert_eq!(
        login["user"]["onboardingCompletedAt"].as_str(),
        Some(completed_at.as_str())
    );
    assert_eq!(login["user"]["onboardingStep"], 3);

    // Restarting clears the flag so the wizard reopens.
    let (status, restarted) = server
        .put(
            "/api/auth/onboarding",
            &admin_token,
            json!({ "completed": false, "step": 0 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restarted}");
    assert!(restarted["user"]["onboardingCompletedAt"].is_null());
    assert_eq!(restarted["user"]["onboardingStep"], 0);

    // Steps outside the documented range are rejected and change nothing.
    for step in [-1, 21] {
        let (status, body) = server
            .put(
                "/api/auth/onboarding",
                &admin_token,
                json!({ "step": step }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "step {step}: {body}");
        assert_eq!(body["error"]["code"], "invalid_onboarding_step");
    }
    let (status, me) = server.get("/api/auth/me", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["user"]["onboardingStep"], 0);

    // The route requires an authenticated account.
    let (status, _) = server
        .request(
            Method::PUT,
            "/api/auth/onboarding",
            None,
            Some(json!({ "step": 2 })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    server.stop().await;
}

#[tokio::test]
async fn preferences_are_per_user_and_admin_patch_ignores_them() {
    let server = TestServer::start().await;
    let (admin_token, _) = server.setup().await;
    let bob = server
        .create_user(&admin_token, "bob", common::ADMIN_PASSWORD, "user")
        .await;
    let bob_id = bob["id"].as_i64().expect("bob id");
    assert!(bob["onboardingCompletedAt"].is_null());
    assert_eq!(bob["onboardingStep"], 0);

    let (status, login) = server.login("bob", common::ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let bob_token = login["token"].as_str().expect("bob token");

    let (status, updated) = server
        .put(
            "/api/auth/onboarding",
            bob_token,
            json!({ "completed": true, "step": 4 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["user"]["onboardingStep"], 4);
    assert!(updated["user"]["onboardingCompletedAt"].is_string());

    // One account's preference never leaks into another.
    let (status, me) = server.get("/api/auth/me", Some(&admin_token)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert!(me["user"]["onboardingCompletedAt"].is_null());
    assert_eq!(me["user"]["onboardingStep"], 0);

    // The administrator PATCH silently ignores onboarding fields: it is not a
    // way to reset or advance another account's wizard.
    let (status, after) = server
        .request(
            Method::PATCH,
            &format!("/api/users/{bob_id}"),
            Some(&admin_token),
            Some(json!({
                "onboardingCompletedAt": null,
                "onboardingStep": 9
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["onboardingStep"], 4);
    assert!(after["onboardingCompletedAt"].is_string());

    let (status, login) = server.login("bob", common::ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert_eq!(login["user"]["onboardingStep"], 4);
    assert!(login["user"]["onboardingCompletedAt"].is_string());

    server.stop().await;
}
