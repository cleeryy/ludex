//! End-to-end tests for invitations, TOTP second factors and session
//! management (#166).

mod common;

use chrono::{Duration, Utc};
use common::{TestServer, ADMIN_PASSWORD};
use ludex_server::db;
use ludex_server::totp;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

fn code_for(secret: &str) -> String {
    totp::totp_at(secret, Utc::now().timestamp()).expect("totp code")
}

/// Rewrites an invitation in the test database, for expiry coverage without
/// sleeping for days.
async fn backdate_invitation(server: &TestServer, id: i64) {
    let url = format!("sqlite://{}/ludex.db?mode=rwc", server.data_dir.display());
    let pool = db::connect(&url).await.expect("connect database");
    let past = db::db_time(Utc::now() - Duration::days(1));
    db::query("UPDATE invitations SET expires_at = ? WHERE id = ?")
        .bind(&past)
        .bind(id)
        .execute(&pool)
        .await
        .expect("backdate invitation");
    pool.close().await;
}

#[tokio::test]
async fn invitation_lifecycle_create_preview_accept_and_reuse() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let (status, created) = server
        .post(
            "/api/invitations",
            Some(&admin),
            Some(json!({
                "role": "user",
                "email": "newbie@example.com",
                "note": "Welcome aboard",
                "expiresInDays": 7,
                "maxUses": 1,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["token"].as_str().expect("token").to_string();
    assert_eq!(token.len(), 48, "24 random bytes, hex encoded");
    assert_eq!(created["path"], format!("/register?invite={token}"));
    assert_eq!(created["invitation"]["status"], "pending");
    assert_eq!(created["invitation"]["uses"], 0);
    assert_eq!(created["invitation"]["maxUses"], 1);
    assert_eq!(created["invitation"]["role"], "user");
    let invitation_id = created["invitation"]["id"].as_i64().expect("id");

    // The admin list never returns the plaintext token.
    let (status, list) = server.get("/api/invitations", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert!(list[0].get("token").is_none(), "token must never be listed");

    // Public preview: enough to render the registration page.
    let (status, preview) = server.get(&format!("/api/invitations/{token}"), None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["role"], "user");
    assert_eq!(preview["email"], "newbie@example.com");
    assert_eq!(preview["note"], "Welcome aboard");
    assert_eq!(preview["requiresPassword"], true);

    // Redeeming creates the account with the invited role and signs it in.
    let (status, accepted) = server
        .post(
            &format!("/api/invitations/{token}/accept"),
            None,
            Some(json!({ "username": "newbie", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{accepted}");
    let newbie_token = accepted["token"]
        .as_str()
        .expect("session token")
        .to_string();
    assert_eq!(accepted["user"]["username"], "newbie");
    assert_eq!(accepted["user"]["role"], "user");
    assert_eq!(accepted["user"]["email"], "newbie@example.com");
    assert_eq!(accepted["mfaRequired"], false);

    // The session works immediately and the email carried over.
    let (status, me) = server.get("/api/auth/me", Some(&newbie_token)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["user"]["username"], "newbie");

    // Single use: the same link is refused with a clear reason.
    let (status, reuse) = server
        .post(
            &format!("/api/invitations/{token}/accept"),
            None,
            Some(json!({ "username": "second", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::GONE, "{reuse}");
    assert_eq!(reuse["error"]["code"], "invitation_used");

    let (status, preview) = server.get(&format!("/api/invitations/{token}"), None).await;
    assert_eq!(status, StatusCode::GONE, "{preview}");
    assert_eq!(preview["error"]["code"], "invitation_used");

    // The list reflects the redemption.
    let (status, list) = server.get("/api/invitations", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["uses"], 1);
    assert_eq!(list[0]["status"], "used");
    assert!(list[0]["lastUsedAt"].is_string());

    // Audit trail: creation and redemption, without the token or the password.
    let (status, audit) = server
        .get(
            "/api/audit?action=invitation.redeemed&limit=10",
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{audit}");
    let entries = audit["items"].as_array().expect("items");
    assert_eq!(entries.len(), 1, "{audit}");
    assert_eq!(entries[0]["targetType"], "invitation");
    assert_eq!(entries[0]["targetId"], invitation_id.to_string());
    let metadata = entries[0]["metadata"].to_string();
    assert!(metadata.contains("newbie"));
    assert!(!metadata.contains("password123"));
    assert!(!metadata.contains(&token));

    let (status, audit) = server
        .get(
            "/api/audit?action=invitation.created&limit=10",
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(audit["items"].as_array().map(Vec::len), Some(1));
    assert!(
        !audit.to_string().contains(&token),
        "token leaked into the audit log"
    );

    server.stop().await;
}

#[tokio::test]
async fn invitation_revoke_expiry_and_validation() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    // Revoked links stop working immediately.
    let (_, created) = server
        .post(
            "/api/invitations",
            Some(&admin),
            Some(json!({ "role": "editor" })),
        )
        .await;
    let id = created["invitation"]["id"].as_i64().expect("id");
    let token = created["token"].as_str().expect("token").to_string();
    let (status, _) = server
        .delete(&format!("/api/invitations/{id}"), &admin)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, preview) = server.get(&format!("/api/invitations/{token}"), None).await;
    assert_eq!(status, StatusCode::GONE, "{preview}");
    assert_eq!(preview["error"]["code"], "invitation_revoked");

    // Expired links fail with their own reason.
    let (_, created) = server
        .post(
            "/api/invitations",
            Some(&admin),
            Some(json!({ "expiresInDays": 1 })),
        )
        .await;
    let id = created["invitation"]["id"].as_i64().expect("id");
    let token = created["token"].as_str().expect("token").to_string();
    backdate_invitation(&server, id).await;
    let (status, preview) = server.get(&format!("/api/invitations/{token}"), None).await;
    assert_eq!(status, StatusCode::GONE, "{preview}");
    assert_eq!(preview["error"]["code"], "invitation_expired");

    // Unknown tokens and parameter validation.
    let (status, body) = server.get("/api/invitations/does-not-exist", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "invitation_not_found");

    for (payload, code) in [
        (json!({ "expiresInDays": 0 }), "invalid_expiry"),
        (json!({ "expiresInDays": 91 }), "invalid_expiry"),
        (json!({ "maxUses": 0 }), "invalid_max_uses"),
        (json!({ "maxUses": 101 }), "invalid_max_uses"),
        (json!({ "email": "not-an-email" }), "invalid_email"),
    ] {
        let (status, body) = server
            .post("/api/invitations", Some(&admin), Some(payload))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], code, "{body}");
    }

    // Only administrators manage invitations.
    server
        .create_user(&admin, "editor", "password123", "editor")
        .await;
    let (_, login) = server.login("editor", "password123").await;
    let editor = login["token"].as_str().expect("token").to_string();
    let (status, body) = server
        .post("/api/invitations", Some(&editor), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server.get("/api/invitations", Some(&editor)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    server.stop().await;
}

#[tokio::test]
async fn invitation_redemption_is_rate_limited_per_address() {
    let server = TestServer::start_with(|config| {
        config.invitation_attempt_limit = 1;
    })
    .await;
    let (admin, _) = server.setup().await;
    let (_, created) = server
        .post("/api/invitations", Some(&admin), Some(json!({})))
        .await;
    let token = created["token"].as_str().expect("token").to_string();

    let (status, body) = server
        .post(
            &format!("/api/invitations/{token}/accept"),
            None,
            Some(json!({ "username": "first", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = server
        .post(
            &format!("/api/invitations/{token}/accept"),
            None,
            Some(json!({ "username": "second", "password": "password123" })),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "invitation_rate_limited");

    server.stop().await;
}

#[tokio::test]
async fn totp_enrollment_login_challenge_replay_and_recovery_codes() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;

    let (status, status_body) = server.get("/api/auth/mfa", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{status_body}");
    assert_eq!(status_body["enabled"], false);
    assert_eq!(status_body["recoveryCodesRemaining"], 0);

    // Enrol: the secret is returned once, with the otpauth URI.
    let (status, enrollment) = server
        .post("/api/auth/mfa/totp/enroll", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{enrollment}");
    let secret = enrollment["secret"].as_str().expect("secret").to_string();
    assert!(!secret.is_empty());
    assert!(enrollment["otpauthUri"]
        .as_str()
        .expect("uri")
        .starts_with("otpauth://totp/"));
    assert_eq!(enrollment["digits"], 6);
    assert_eq!(enrollment["period"], 30);

    // Confirming with a wrong code changes nothing.
    let (status, body) = server
        .post(
            "/api/auth/mfa/totp/activate",
            Some(&admin),
            Some(json!({ "code": "000000" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid_mfa_code");

    let code = code_for(&secret);
    let (status, activated) = server
        .post(
            "/api/auth/mfa/totp/activate",
            Some(&admin),
            Some(json!({ "code": code })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{activated}");
    assert_eq!(activated["mfa"]["enabled"], true);
    let recovery: Vec<String> = activated["recoveryCodes"]
        .as_array()
        .expect("recovery codes")
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(recovery.len(), 10);
    assert!(recovery.iter().all(|code| code.len() == 11));

    // The second enrolment is refused while 2FA is on.
    let (status, body) = server
        .post("/api/auth/mfa/totp/enroll", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "mfa_already_enabled");

    // Password alone no longer yields a session: no token, no user.
    let (status, challenge) = server.login("admin", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{challenge}");
    assert_eq!(challenge["mfaRequired"], true);
    assert!(
        challenge.get("token").is_none(),
        "a token leaked: {challenge}"
    );
    assert!(
        challenge.get("user").is_none(),
        "a user leaked: {challenge}"
    );
    assert!(challenge["challengeToken"].is_string());
    let challenge_token = challenge["challengeToken"]
        .as_str()
        .expect("challenge")
        .to_string();

    // A wrong code fails and keeps the challenge alive.
    let (status, body) = server
        .post(
            "/api/auth/login/mfa",
            None,
            Some(json!({ "challengeToken": challenge_token, "code": "123456" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid_mfa_code");

    // The right code issues the session.
    let (status, signed_in) = server
        .post(
            "/api/auth/login/mfa",
            None,
            Some(json!({ "challengeToken": challenge_token, "code": code_for(&secret) })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{signed_in}");
    let mfa_token = signed_in["token"].as_str().expect("token").to_string();
    assert_eq!(signed_in["user"]["mfaEnabled"], true);

    // The challenge is single use.
    let (status, body) = server
        .post(
            "/api/auth/login/mfa",
            None,
            Some(json!({ "challengeToken": challenge_token, "code": code_for(&secret) })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "challenge_used");

    // TOTP codes cannot be replayed inside their window.
    let (_, challenge) = server.login("admin", ADMIN_PASSWORD).await;
    let (status, body) = server
        .post(
            "/api/auth/login/mfa",
            None,
            Some(json!({
                "challengeToken": challenge["challengeToken"],
                "code": code_for(&secret),
            })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "replayed code accepted: {body}"
    );
    assert_eq!(body["error"]["code"], "invalid_mfa_code");

    // A recovery code works once.
    let (_, challenge) = server.login("admin", ADMIN_PASSWORD).await;
    let challenge_token = challenge["challengeToken"]
        .as_str()
        .expect("challenge")
        .to_string();
    let (status, body) = server
        .post(
            "/api/auth/login/mfa",
            None,
            Some(json!({
                "challengeToken": challenge_token,
                "code": recovery[0],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, status_body) = server.get("/api/auth/mfa", Some(&mfa_token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status_body["recoveryCodesRemaining"], 9);

    let (_, challenge) = server.login("admin", ADMIN_PASSWORD).await;
    let (status, body) = server
        .post(
            "/api/auth/login/mfa",
            None,
            Some(json!({
                "challengeToken": challenge["challengeToken"],
                "code": recovery[0],
            })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "recovery code reused: {body}"
    );

    // Unknown challenge tokens are rejected.
    let (status, body) = server
        .post(
            "/api/auth/login/mfa",
            None,
            Some(json!({ "challengeToken": "nope", "code": code_for(&secret) })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid_challenge");

    // Disabling needs the password, then the password alone signs in again.
    let (status, body) = server
        .post(
            "/api/auth/mfa/disable",
            Some(&mfa_token),
            Some(json!({ "password": "wrong-password" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_password");

    let (status, body) = server
        .post(
            "/api/auth/mfa/disable",
            Some(&mfa_token),
            Some(json!({ "password": ADMIN_PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);

    let (status, signed_in) = server.login("admin", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{signed_in}");
    assert!(
        signed_in["token"].is_string(),
        "no session after disabling 2FA"
    );

    // Audit trail records the security events without the secret.
    let (status, audit) = server
        .get("/api/audit?action=mfa.enabled&limit=5", Some(&admin))
        .await;
    assert_eq!(status, StatusCode::OK, "{audit}");
    assert_eq!(audit["items"].as_array().map(Vec::len), Some(1));
    assert!(!audit.to_string().contains(&secret));

    server.stop().await;
}

#[tokio::test]
async fn sessions_can_be_revoked_individually_and_all_at_once() {
    let server = TestServer::start().await;
    let (_admin, _) = server.setup().await;

    let (_, first) = server.login("admin", ADMIN_PASSWORD).await;
    let first_token = first["token"].as_str().expect("first").to_string();
    let (_, second) = server.login("admin", ADMIN_PASSWORD).await;
    let second_token = second["token"].as_str().expect("second").to_string();

    let (status, sessions) = server.get("/api/auth/sessions", Some(&first_token)).await;
    assert_eq!(status, StatusCode::OK, "{sessions}");
    let entries = sessions.as_array().expect("sessions");
    assert_eq!(entries.len(), 2, "{sessions}");
    assert!(entries[0].get("device").is_some(), "device label missing");
    let current: Vec<&Value> = entries
        .iter()
        .filter(|entry| entry["current"] == true)
        .collect();
    assert_eq!(current.len(), 1);

    // Revoke every other session; the caller keeps working.
    let (status, revoked) = server.delete("/api/auth/sessions", &first_token).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["revoked"], 1);

    let (status, body) = server.get("/api/auth/me", Some(&second_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "session_revoked");
    let (status, _) = server.get("/api/auth/me", Some(&first_token)).await;
    assert_eq!(status, StatusCode::OK);

    // Revoking one session by id.
    let (_, third) = server.login("admin", ADMIN_PASSWORD).await;
    let third_token = third["token"].as_str().expect("third").to_string();
    let (_, sessions) = server.get("/api/auth/sessions", Some(&first_token)).await;
    let third_id = sessions
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| {
            entry["current"] != true && entry["revokedAt"].is_null() && entry["id"].is_number()
        })
        .and_then(|entry| entry["id"].as_i64())
        .expect("session id");
    let (status, _) = server
        .delete(&format!("/api/auth/sessions/{third_id}"), &first_token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = server.get("/api/auth/me", Some(&third_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    server.stop().await;
}

#[tokio::test]
async fn admin_password_reset_signs_every_device_out() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let created = server
        .create_user(&admin, "bob", "password123", "user")
        .await;
    let bob_id = created["id"].as_i64().expect("id");

    let (_, first) = server.login("bob", "password123").await;
    let first_token = first["token"].as_str().expect("token").to_string();
    let (_, second) = server.login("bob", "password123").await;
    let second_token = second["token"].as_str().expect("token").to_string();

    // The server generates a temporary password and returns it once.
    let (status, reset) = server
        .post(
            &format!("/api/users/{bob_id}/reset-password"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["mustChangePassword"], true);
    assert_eq!(reset["username"], "bob");
    let temporary = reset["password"]
        .as_str()
        .expect("generated password")
        .to_string();
    assert!(temporary.len() >= 8);
    assert!(reset["revoked"].as_i64().unwrap_or(0) >= 2);

    for token in [&first_token, &second_token] {
        let (status, body) = server.get("/api/auth/me", Some(token)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    }

    // The temporary password works, the old one does not, and the flag is set.
    let (status, _) = server.login("bob", "password123").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, login) = server.login("bob", &temporary).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    assert_eq!(login["user"]["mustChangePassword"], true);
    let bob_token = login["token"].as_str().expect("token").to_string();

    // Choosing a new password clears the flag; other sessions were already dead.
    let (status, body) = server
        .post(
            "/api/auth/change-password",
            Some(&bob_token),
            Some(json!({ "currentPassword": temporary, "newPassword": "newpassword123" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, me) = server.get("/api/auth/me", Some(&bob_token)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["user"]["mustChangePassword"], false);

    // Audited.
    let (status, audit) = server
        .get(
            "/api/audit?action=user.password_reset&limit=5",
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{audit}");
    assert_eq!(audit["items"].as_array().map(Vec::len), Some(1));
    assert!(!audit.to_string().contains(&temporary));

    server.stop().await;
}

#[tokio::test]
async fn disabling_an_account_invalidates_sessions_immediately() {
    let server = TestServer::start().await;
    let (admin, _) = server.setup().await;
    let created = server
        .create_user(&admin, "carol", "password123", "user")
        .await;
    let carol_id = created["id"].as_i64().expect("id");
    let (_, login) = server.login("carol", "password123").await;
    let carol_token = login["token"].as_str().expect("token").to_string();

    let (status, body) = server
        .request(
            Method::PATCH,
            &format!("/api/users/{carol_id}"),
            Some(&admin),
            Some(json!({ "activated": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["activated"], false);

    // Existing JWT: rejected on the next request, not at expiry.
    let (status, body) = server.get("/api/auth/me", Some(&carol_token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "account_disabled");

    // New credentials are refused too.
    let (status, body) = server.login("carol", "password123").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "account_disabled");

    server.stop().await;
}

#[tokio::test]
async fn second_factor_attempts_are_rate_limited() {
    let server = TestServer::start_with(|config| {
        config.mfa_attempt_limit = 2;
    })
    .await;
    let (admin, _) = server.setup().await;
    let (_, enrollment) = server
        .post("/api/auth/mfa/totp/enroll", Some(&admin), None)
        .await;
    let secret = enrollment["secret"].as_str().expect("secret").to_string();
    let (status, body) = server
        .post(
            "/api/auth/mfa/totp/activate",
            Some(&admin),
            Some(json!({ "code": code_for(&secret) })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for attempt in 1..=2 {
        let (_, challenge) = server.login("admin", ADMIN_PASSWORD).await;
        let (status, body) = server
            .post(
                "/api/auth/login/mfa",
                None,
                Some(json!({
                    "challengeToken": challenge["challengeToken"],
                    "code": "000000",
                })),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "attempt {attempt}: {body}"
        );
    }

    // The third attempt in the window is refused before the code is checked.
    let (_, challenge) = server.login("admin", ADMIN_PASSWORD).await;
    let (status, body) = server
        .post(
            "/api/auth/login/mfa",
            None,
            Some(json!({
                "challengeToken": challenge["challengeToken"],
                "code": "000000",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "mfa_rate_limited");

    server.stop().await;
}
