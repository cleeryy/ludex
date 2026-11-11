//! Desktop SSO handoff (#157): create, one-time claim and exchange.
//!
//! The desktop app never opens a local port: the web page attaches its session
//! to a random state, the desktop claims it once, and the JWT only ever comes
//! from the existing `/api/auth/oidc/exchange` code redemption.

mod common;

use std::time::Duration;

use common::TestServer;
use ludex_server::desktop_handoff::{random_state, MAX_STATE_LEN, MIN_STATE_LEN};
use reqwest::StatusCode;
use serde_json::{json, Value};

/// Configures SSO without any network call: `OidcClient::from_settings` only
/// validates the issuer, it never runs discovery.
fn with_oidc(config: &mut ludex_server::Config) {
    config.oidc_issuer = Some("https://idp.example.com".to_string());
    config.oidc_client_id = Some("ludex-client".to_string());
}

async fn handoff_server() -> TestServer {
    TestServer::start_with(with_oidc).await
}

async fn create_handoff(server: &TestServer, token: &str, state: &str) -> (StatusCode, Value) {
    server
        .post(
            "/api/auth/desktop-handoff",
            Some(token),
            Some(json!({ "state": state })),
        )
        .await
}

async fn claim_handoff(server: &TestServer, state: &str) -> (StatusCode, Value) {
    server
        .post(
            "/api/auth/desktop-handoff/claim",
            None,
            Some(json!({ "state": state })),
        )
        .await
}

fn assert_no_token(body: &Value, label: &str) {
    assert!(
        body.get("token").is_none(),
        "{label} must not contain a token: {body}"
    );
    assert!(
        !body.to_string().contains("eyJ"),
        "{label} must not contain a JWT: {body}"
    );
}

#[tokio::test]
async fn handoff_is_claimed_once_and_redeemed_through_the_exchange() {
    let server = handoff_server().await;
    let (token, user) = server.setup().await;
    let state = random_state();

    let (status, created) = create_handoff(&server, &token, &state).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert!(
        created["expiresAt"].as_str().is_some(),
        "missing expiresAt: {created}"
    );
    assert!(created.get("code").is_none(), "{created}");
    assert_no_token(&created, "handoff creation");

    let (status, claimed) = claim_handoff(&server, &state).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    let code = claimed["code"].as_str().expect("code").to_string();
    assert!(!code.is_empty());
    assert!(claimed["expiresAt"].as_str().is_some(), "{claimed}");
    assert_no_token(&claimed, "handoff claim");
    assert_ne!(code, state, "the code must not be the state");

    // The one-time code travels through the existing exchange, and that is the
    // only response of the whole flow that carries a JWT.
    let (status, exchanged) = server
        .post(
            "/api/auth/oidc/exchange",
            None,
            Some(json!({ "code": code })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{exchanged}");
    assert!(
        exchanged["token"].as_str().is_some_and(|t| !t.is_empty()),
        "{exchanged}"
    );
    assert_eq!(exchanged["user"]["id"], user["id"]);

    // The code is single use.
    let (status, error) = server
        .post(
            "/api/auth/oidc/exchange",
            None,
            Some(json!({ "code": code })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");
    assert_eq!(error["error"]["code"], "invalid_oidc_code");
}

#[tokio::test]
async fn a_handoff_can_only_be_claimed_once() {
    let server = handoff_server().await;
    let (token, _) = server.setup().await;
    let state = random_state();
    let (status, _) = create_handoff(&server, &token, &state).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = claim_handoff(&server, &state).await;
    assert_eq!(status, StatusCode::OK);

    let (status, error) = claim_handoff(&server, &state).await;
    assert_eq!(status, StatusCode::GONE, "{error}");
    assert_eq!(error["error"]["code"], "handoff_unavailable");
    assert_no_token(&error, "second claim");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.contains("user")),
        "the failure must not mention an account: {error}"
    );
}

#[tokio::test]
async fn only_the_exact_state_can_claim_a_handoff() {
    let server = handoff_server().await;
    let (token, _) = server.setup().await;
    let state = random_state();
    let (status, _) = create_handoff(&server, &token, &state).await;
    assert_eq!(status, StatusCode::OK);

    let (status, error) = claim_handoff(&server, &random_state()).await;
    assert_eq!(status, StatusCode::GONE, "{error}");

    // A failed guess does not consume or invalidate the real handoff.
    let (status, claimed) = claim_handoff(&server, &state).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
}

#[tokio::test]
async fn an_expired_handoff_cannot_be_claimed() {
    let server = TestServer::start_with(|config| {
        with_oidc(config);
        config.desktop_handoff_ttl = Duration::from_millis(150);
    })
    .await;
    let (token, _) = server.setup().await;
    let state = random_state();
    let (status, _) = create_handoff(&server, &token, &state).await;
    assert_eq!(status, StatusCode::OK);

    tokio::time::sleep(Duration::from_millis(400)).await;

    let (status, error) = claim_handoff(&server, &state).await;
    assert_eq!(status, StatusCode::GONE, "{error}");
    assert_eq!(error["error"]["code"], "handoff_unavailable");
}

#[tokio::test]
async fn creating_a_handoff_requires_an_authenticated_session() {
    let server = handoff_server().await;
    let (status, error) = create_handoff(&server, "", &random_state()).await;
    // An empty Bearer value is rejected as no credential at all.
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");
    assert_eq!(error["error"]["code"], "unauthorized");

    let (status, error) = server
        .post(
            "/api/auth/desktop-handoff",
            None,
            Some(json!({ "state": random_state() })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");
}

#[tokio::test]
async fn the_state_must_look_like_high_entropy_randomness() {
    let server = handoff_server().await;
    let (token, _) = server.setup().await;

    for state in [
        "short".to_string(),
        "a".repeat(MIN_STATE_LEN - 1),
        format!("{}!", "a".repeat(42)),
        "a".repeat(MAX_STATE_LEN + 1),
        "state with spaces and a long enough tail aaaaaaaaaaaaaaaa".to_string(),
    ] {
        let (status, error) = create_handoff(&server, &token, &state).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{state}: {error}");
        assert_eq!(error["error"]["code"], "invalid_handoff_state");
    }

    // Hex (desktop) and base64url states both pass.
    for state in ["b".repeat(64), random_state()] {
        let (status, body) = create_handoff(&server, &token, &state).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    // The claim endpoint validates the same way, and never leaks a code.
    let (status, error) = claim_handoff(&server, "too-short").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "invalid_handoff_state");
    assert_no_token(&error, "invalid claim");
}

#[tokio::test]
async fn handoff_claims_are_rate_limited_per_address() {
    let server = TestServer::start_with(|config| {
        with_oidc(config);
        config.handoff_claim_limit = 3;
    })
    .await;
    let (token, _) = server.setup().await;
    let state = random_state();
    let (status, _) = create_handoff(&server, &token, &state).await;
    assert_eq!(status, StatusCode::OK);

    let mut throttled = None;
    for _ in 0..6 {
        let (status, body) = claim_handoff(&server, &random_state()).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            throttled = Some(body);
            break;
        }
        assert_eq!(status, StatusCode::GONE, "{body}");
    }
    let throttled = throttled.expect("the claim endpoint must throttle");
    assert_eq!(throttled["error"]["code"], "handoff_throttled");
}

#[tokio::test]
async fn handoff_creation_is_rate_limited_per_account() {
    let server = TestServer::start_with(|config| {
        with_oidc(config);
        config.handoff_create_limit = 2;
    })
    .await;
    let (token, _) = server.setup().await;

    let mut throttled = None;
    for _ in 0..5 {
        let (status, body) = create_handoff(&server, &token, &random_state()).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            throttled = Some(body);
            break;
        }
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let throttled = throttled.expect("handoff creation must throttle");
    assert_eq!(throttled["error"]["code"], "handoff_throttled");
}

#[tokio::test]
async fn handoff_requires_sso_like_the_exchange_it_feeds() {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let (status, error) = create_handoff(&server, &token, &random_state()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    assert_eq!(error["error"]["code"], "oidc_disabled");
}

#[tokio::test]
async fn a_second_web_login_with_the_same_state_refreshes_the_handoff() {
    let server = handoff_server().await;
    let (token, _) = server.setup().await;
    let state = random_state();
    let (status, first) = create_handoff(&server, &token, &state).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (status, second) = create_handoff(&server, &token, &state).await;
    assert_eq!(status, StatusCode::OK, "{second}");

    let (status, claimed) = claim_handoff(&server, &state).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert!(claimed["code"]
        .as_str()
        .is_some_and(|code| !code.is_empty()));

    // Exactly one entry existed: a third claim fails.
    let (status, _) = claim_handoff(&server, &state).await;
    assert_eq!(status, StatusCode::GONE);
}
