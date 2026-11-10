//! OIDC integration tests against an in-process mock provider.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, StatusCode as AxumStatus};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::Utc;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use ludex_core::models::Role;
use ludex_server::{start, Config, ServerHandle};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use url::Url;

const CLIENT_ID: &str = "ludex-client";
const CLIENT_SECRET: &str = "ludex-secret";

// Test-only RSA keys (not secrets). Static fixtures keep the suite fast.
const PRIVATE_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDpH2BZziwf5YB2
02WKPeSURKMJd152L2ikOq4yH65TJ83EzZPZkKLNbGQchdmJeLsAiZWPId0vvzk1
V27ElLKoQm0nSGc0YADs6Yn1tis6t0nA+ju12qdpWfUzqtV33TKQueKdiCTJnXHF
DqwXz4GCf02jSbj2vc1quyzfJ2OMUYQmsencrEkpXsPuPgTmSk6j3oHmqtAw35Ah
L6gDk8IF4qxTQbGoBAdOdW/sgLepGgcIvZzfl5VV2j7kSf15iu1DL9KNGeIpGKNP
kUgRsu94UikpNLGneGpwPaJ01AwbZwM+hMtPQBf9Pb0/pkkGYuk9C0/6CPW4HYf6
0cE3hlSxAgMBAAECggEAFs2JZK9slIAj9TuPWprcQX6HPhVNiZzwjOPqhx+fMyYZ
FA+Rzz29Fv/ruPiEw1QqYo6SsQn8cz+url+Lh7IN2bSJrcaAxdhALuxXINA9YURt
c/FmJs8pAf6ZJAEpWUyrNgngt00KYChE6SQaxEd5YRTnuLms3IHLVo3RgVB+hcqB
nzzkV084zJ4Uv7gnzebLU60CwaUr0VW0M9UyD2caE9Wg5MkRrrHe8N2Chny8mn/H
sYX7oahOfV9qHO5bLNrJ6BJ/rTtB5AFj9HuHKGvgyRXbqbnyNwinSiJIb0rzhLK7
k6Zev2jK2dHFLvWHjx/ASlyhz6cq6Ne/S7oZtxOiAwKBgQD+/TJ51zx7jv0yIkIW
6lFi2CAieMgsd2J+xelg8OBfZxYAW06rFR+ct+IqTVCWCr+K+dXw/Z0joI9jcKuF
4SKP1+TJ8T9Jh9gu6WJ4glZbe9VQurtdgUV6cRwwZ/NvxcgdYYDrPeFJWR8nOXqJ
K7Ad0QO219qaaY1DdpDhLA4EiwKBgQDqC/xUW8rjrFs+x+8/3h4ND/+HUUuMhbuL
NxP25pEP9V6F+W2qbzVrBJ3hAcu2YpOA27OwxON0vGyuq9kWNOmQAG6lLsaGNag0
v6ggSiI+2PC0lkOouzMk6xSkzdNM0aWtAFbc+1MtBZMifAdtpJ1fiNNWRZV3Cn0s
ZbAOGibnMwKBgHS/DLtIVsLjpv2rPcMsKXMuLRhbYJsrVgdhKgdegfxsQGCNbalb
FIw2jPB9UaZA6Y4B8XhTRf0n6D7JOV8kwyRE3tWvBSDvd4WTCajPVCayveomdN7M
7ziuTQ/EqGrGjWcWW2ZZgKqKiX72DCp9vDcKmb6/ebE1Hp6CzLar6wcdAoGBAMnx
9ZNEcNfqDAEruoA9Q2i5+R3lQ27IpX4F1DAh2eEPMrQe+EUrF8hzf1Z6mi+Q9v0k
lPNdr3U2cAB3PyUuGqmp9c6Jgbqj6bM5+/OAg/P+dBMd5NteS077mCBriHqsGmwf
8tNR9/7M5JvIsRAmDbQ090VlXQYOAtugEjzYy+CJAoGAWp3FpqDPF/r46nQ59rf0
0k8svTBUr6XN1Tfya62yBdkNsfMwi7WdYdlJ28bWoAL9/44DrRW/Qw3WVRDtzwNn
jkAEKW2EfQ9TrJKIFv1KIBIaYwphlCBoqjn8+AofUJEY1Qhb2tVks/NLpB1biO/1
p9M8vKq7ursBve147BQi5lU=
-----END PRIVATE KEY-----"#;
const WRONG_PRIVATE_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCcAxTIFzaNQ+nA
a3EbtZGv3mRZ+hLoCtdzS8hY5+VWNGBnstVMgqEfBLofhQfYnRP27+cFLK6IboRD
FNhHUYvqeFuao5DYW80AhLGVQLswCfhku1Tq5lf4ek0zil4Uz1EItxKGUxClviGp
9U1P33fv1C5HVk24tH064yBoz2CUrEnSaZguBUsXjSr9EyG2fNi3EAYMVXA88rIw
HSTyBKY2GWqJeu8bA/F+aXJsRleebfU5Gu3HjTwtJqYOLmJotF/3t7IDGL+oEQoI
9MvAM9AcdnC+jmDJGnHszGOjcCAMP8Q+qxX65WK1q4n6wVCpik3/ZerGkR7b8LFK
QpagyT55AgMBAAECggEAD/SDNrLkC/aw9FIwoT00kuw/LlIERrmcHuEWUlxV8Jbe
8KACDybxYCUZPVG7M5Cd6vRON7HiABcwXNVoCIQrF127fZFChhP+X1ISwME72437
ep7LcJIB12o3lMCu/jkVEBt78COU2WR8WB+7ikKABzWXjYlinbQMth3mEb36oj2i
5y79JzkiKqblVn41uT9VgC+0afJAftuxJbb6Mk8gCqggowtbq8XLg9yd9Lm7sUoF
QYdRXaIuXJrrW/HEqt9OZOeeT26IN2fKYPKnizk67JIOJ2KhFvr+5zD3eGdrBMOX
hPiMd7E4ixotkaaTjswj74svMHQfCYML2y+FsE9zUQKBgQDb6iHq7Yibow1W+UhW
ZMk5jPGauwm+7bbJXSUpgVyo2A6O/t/Vt0S/PPjs/TK3q49cWJ44yk9DRQcavE2v
eWY6FhkIVVzwkpLLqijvD4wvdlE7k66QduVB9sWTTJYJJ1dVgmgQMGrwx7NrNTlT
jnxA+X8J1/3ohz6xIR/GfLkYcQKBgQC1nJ52hisdHN368yHixJsHWydlZXJszAtx
rVO9kleCFLSdjaIOjS0FYSuMGaji1TKnbW9Y73OlhlZFIt0ltOaUmg4vacfL72q0
k3T930kkCNabdjuJRgF+inzU3F5Fu+CHlEEYmj6fPHAiGeUBc1Eh2MsT6uRDZLNm
WTM0JJLKiQKBgFO2LVETOGQeTi6kV7/jP6Hzn+iJeC1Aqcq+9S12e8Id9HTEWoos
6DNvz0XPICZAXSMG7Lurbz6pIQoxUdUQ/jPwQ3kjnWxKLyztygUuQuuyOzvVfKar
S5uvhwgKKDa1zg3TxS8cbyjWq9qMeRTwcSX+kzg5CwMfsGKWvFS/F2pRAoGAIIkv
hnJg9PU/+35lWrf4Qigddbfn+P/RKQtxTiwSi1ZkSTmfh9XY8/bFk3LhY8noKkCb
yyYS5NudRwOI9QFkscirs+jSh4Gb7z/fnL/M3azLrcxPvDcTe3UaChR0uy9+eSuh
QeQYRfTQ7fsPoOPk7BhnwWi6fixjS3m9h6Dpv2kCgYEAw37+aPyATzwkrdVODcqn
ZzBQV1ztWrxZJAZYO4UtVk6CTWcJPGF8/8NWNa8n7EIf1THLMdvUlatxfD9jWDA3
LN+0oupzLXPatkVqqxS/Zdfb/3Ow00ks/9ZvFe4oRwHUQDzv2uo+nrKGqaWz6soI
yYCGYykXVogBehTBALzowLo=
-----END PRIVATE KEY-----"#;
const JWKS_N: &str = "6R9gWc4sH-WAdtNlij3klESjCXdedi9opDquMh-uUyfNxM2T2ZCizWxkHIXZiXi7AImVjyHdL785NVduxJSyqEJtJ0hnNGAA7OmJ9bYrOrdJwPo7tdqnaVn1M6rVd90ykLninYgkyZ1xxQ6sF8-Bgn9No0m49r3Narss3ydjjFGEJrHp3KxJKV7D7j4E5kpOo96B5qrQMN-QIS-oA5PCBeKsU0GxqAQHTnVv7IC3qRoHCL2c35eVVdo-5En9eYrtQy_SjRniKRijT5FIEbLveFIpKTSxp3hqcD2idNQMG2cDPoTLT0AX_T29P6ZJBmLpPQtP-gj1uB2H-tHBN4ZUsQ";

#[derive(Clone, Debug)]
struct MockClaims {
    nonce: Option<String>,
    sub: String,
    email: Option<String>,
    email_verified: bool,
    groups: Vec<String>,
    preferred_username: Option<String>,
}

impl Default for MockClaims {
    fn default() -> Self {
        Self {
            nonce: None,
            sub: "oidc-user".to_string(),
            email: Some("sso@example.com".to_string()),
            email_verified: true,
            groups: Vec::new(),
            preferred_username: Some("sso-user".to_string()),
        }
    }
}

struct MockState {
    issuer: String,
    client_id: String,
    private_pem: String,
    jwks: Value,
    claims: Mutex<MockClaims>,
    wrong_signing_key: Mutex<bool>,
    last_token_form: Mutex<Option<HashMap<String, String>>>,
}

#[derive(Clone)]
struct MockProvider {
    issuer: String,
    state: Arc<MockState>,
}

impl MockProvider {
    async fn start() -> Self {
        let jwks = json!({
            "keys": [{
                "kty": "RSA",
                "kid": "test-key",
                "use": "sig",
                "alg": "RS256",
                "n": JWKS_N,
                "e": "AQAB"
            }]
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock bind");
        let address = listener.local_addr().expect("mock addr");
        let issuer = format!("http://{address}");
        let state = Arc::new(MockState {
            issuer: issuer.clone(),
            client_id: CLIENT_ID.to_string(),
            private_pem: PRIVATE_PEM.to_string(),
            jwks,
            claims: Mutex::new(MockClaims::default()),
            wrong_signing_key: Mutex::new(false),
            last_token_form: Mutex::new(None),
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks_handler))
            .route("/token", post(token_handler))
            .with_state(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self { issuer, state }
    }

    fn set_claims(&self, claims: MockClaims) {
        *self.state.claims.lock().expect("claims lock") = claims;
    }

    fn set_wrong_signing_key(&self, wrong: bool) {
        *self.state.wrong_signing_key.lock().expect("wrong key lock") = wrong;
    }

    fn last_token_form(&self) -> HashMap<String, String> {
        self.state
            .last_token_form
            .lock()
            .expect("form lock")
            .clone()
            .unwrap_or_default()
    }
}

async fn discovery(State(state): State<Arc<MockState>>) -> Json<Value> {
    Json(json!({
        "issuer": state.issuer,
        "authorization_endpoint": format!("{}/authorize", state.issuer),
        "token_endpoint": format!("{}/token", state.issuer),
        "jwks_uri": format!("{}/jwks", state.issuer),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
        "scopes_supported": ["openid", "profile", "email"]
    }))
}

async fn jwks_handler(State(state): State<Arc<MockState>>) -> Json<Value> {
    Json(state.jwks.clone())
}

async fn token_handler(State(state): State<Arc<MockState>>, body: String) -> Response {
    let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    if form.get("grant_type").map(String::as_str) != Some("authorization_code")
        || !form.contains_key("code")
        || !form.contains_key("code_verifier")
        || form.get("client_id").map(String::as_str) != Some(state.client_id.as_str())
    {
        return (
            AxumStatus::BAD_REQUEST,
            Json(json!({"error": "invalid_request"})),
        )
            .into_response();
    }
    *state.last_token_form.lock().expect("form lock") = Some(form);

    let claims_config = state.claims.lock().expect("claims lock").clone();
    let now = Utc::now().timestamp();
    let mut claims = json!({
        "iss": state.issuer,
        "aud": state.client_id,
        "sub": claims_config.sub,
        "exp": now + 300,
        "iat": now,
    });
    if let Some(nonce) = claims_config.nonce {
        claims["nonce"] = json!(nonce);
    }
    if let Some(email) = claims_config.email {
        claims["email"] = json!(email);
        claims["email_verified"] = json!(claims_config.email_verified);
    }
    if !claims_config.groups.is_empty() {
        claims["groups"] = json!(claims_config.groups);
    }
    if let Some(username) = claims_config.preferred_username {
        claims["preferred_username"] = json!(username);
    }

    let signing_pem = if *state.wrong_signing_key.lock().expect("wrong key lock") {
        WRONG_PRIVATE_PEM
    } else {
        state.private_pem.as_str()
    };
    let key = EncodingKey::from_rsa_pem(signing_pem.as_bytes()).expect("signing key");
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-key".to_string());
    let id_token = encode(&header, &claims, &key).expect("id token");
    (
        AxumStatus::OK,
        Json(json!({
            "access_token": "mock-access-token",
            "token_type": "Bearer",
            "expires_in": 300,
            "id_token": id_token
        })),
    )
        .into_response()
}

struct OidcTestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    #[allow(dead_code)]
    temp: TempDir,
}

impl OidcTestServer {
    async fn start(mock: &MockProvider, admin_groups: &[&str], auto_create: bool) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::for_local(
            temp.path().join("games"),
            temp.path().join("data"),
            temp.path().join("media"),
        );
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        config.oidc_issuer = Some(mock.issuer.clone());
        config.oidc_client_id = Some(CLIENT_ID.to_string());
        config.oidc_client_secret = Some(CLIENT_SECRET.to_string());
        config.oidc_display_name = "Authentik".to_string();
        config.oidc_auto_create = auto_create;
        config.oidc_default_role = Role::User;
        config.oidc_admin_groups = admin_groups.iter().map(|group| group.to_string()).collect();
        config.oidc_groups_claim = "groups".to_string();
        config.oidc_scopes = "openid profile email".to_string();

        let handle = start(config).await.expect("oidc server");
        Self {
            url: handle.url.clone(),
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("client"),
            handle: Some(handle),
            temp,
        }
    }

    async fn start_without_oidc() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = Config::for_local(
            temp.path().join("games"),
            temp.path().join("data"),
            temp.path().join("media"),
        );
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        let handle = start(config).await.expect("plain server");
        Self {
            url: handle.url.clone(),
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("client"),
            handle: Some(handle),
            temp,
        }
    }

    async fn stop(mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.url, path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("request");
        let status = response.status();
        let text = response.text().await.expect("body");
        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        (status, value)
    }

    async fn get(&self, path: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.request(reqwest::Method::GET, path, token, None).await
    }

    async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.request(
            reqwest::Method::POST,
            path,
            token,
            Some(body.unwrap_or(Value::Null)),
        )
        .await
    }

    async fn patch(
        &self,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.request(
            reqwest::Method::PATCH,
            path,
            token,
            Some(body.unwrap_or(Value::Null)),
        )
        .await
    }

    async fn setup(&self) -> (String, Value) {
        let (status, body) = self
            .post(
                "/api/setup",
                None,
                Some(json!({"username": "admin", "password": "password123"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let token = body["token"].as_str().expect("token").to_string();
        (token, body["user"].clone())
    }

    async fn start_flow(&self) -> Flow {
        let response = self
            .client
            .get(format!("{}/api/auth/oidc/start", self.url))
            .send()
            .await
            .expect("start");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("location");
        let url = Url::parse(location).expect("authorization url");
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        Flow {
            authorization_url: url,
            state: params.get("state").expect("state").clone(),
            nonce: params.get("nonce").expect("nonce").clone(),
            code_challenge: params.get("code_challenge").expect("challenge").clone(),
            redirect_uri: params.get("redirect_uri").expect("redirect").clone(),
            scope: params.get("scope").expect("scope").clone(),
            params,
        }
    }

    async fn callback(&self, code: &str, state: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/api/auth/oidc/callback?code={code}&state={state}",
                self.url
            ))
            .send()
            .await
            .expect("callback")
    }

    async fn exchange(&self, code: &str) -> (StatusCode, Value) {
        self.post("/api/auth/oidc/exchange", None, Some(json!({"code": code})))
            .await
    }
}

impl Drop for OidcTestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
    }
}

struct Flow {
    #[allow(dead_code)]
    authorization_url: Url,
    state: String,
    nonce: String,
    code_challenge: String,
    redirect_uri: String,
    scope: String,
    params: HashMap<String, String>,
}

fn oidc_code(location: &str) -> String {
    location
        .split_once("oidc_code=")
        .expect("oidc_code in location")
        .1
        .split('&')
        .next()
        .expect("code")
        .to_string()
}

#[tokio::test]
async fn oidc_flow_provisions_user_and_exchanges_code() {
    let mock = MockProvider::start().await;
    let server = OidcTestServer::start(&mock, &[], true).await;

    let (status, config) = server.get("/api/config", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["features"]["oidc"], true);
    assert_eq!(config["oidcDisplayName"], "Authentik");

    let flow = server.start_flow().await;
    assert_eq!(
        flow.params.get("response_type").map(String::as_str),
        Some("code")
    );
    assert_eq!(
        flow.params.get("client_id").map(String::as_str),
        Some(CLIENT_ID)
    );
    assert_eq!(
        flow.params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert_eq!(flow.state.len(), 64);
    assert_eq!(flow.nonce.len(), 64);
    assert_eq!(flow.code_challenge.len(), 43);
    assert!(flow.scope.contains("openid"));
    assert!(flow.redirect_uri.starts_with(&server.url));
    assert!(flow.redirect_uri.ends_with("/api/auth/oidc/callback"));

    mock.set_claims(MockClaims {
        nonce: Some(flow.nonce.clone()),
        sub: "oidc-subject-1".to_string(),
        email: Some("sso@example.com".to_string()),
        email_verified: true,
        groups: Vec::new(),
        preferred_username: Some("sso-user".to_string()),
    });

    let response = server.callback("test-code", &flow.state).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("location")
        .to_string();
    assert!(location.starts_with("/#/login?oidc_code="), "{location}");
    let code = oidc_code(&location);

    let form = mock.last_token_form();
    assert_eq!(
        form.get("redirect_uri").map(String::as_str),
        Some(flow.redirect_uri.as_str())
    );
    let verifier = form.get("code_verifier").expect("code verifier");
    assert_eq!(
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
        flow.code_challenge
    );

    let (status, auth) = server.exchange(&code).await;
    assert_eq!(status, StatusCode::OK, "{auth}");
    let token = auth["token"].as_str().expect("token").to_string();
    assert_eq!(auth["user"]["username"], "sso-user");
    assert_eq!(auth["user"]["email"], "sso@example.com");
    assert_eq!(auth["user"]["role"], "user");
    assert_eq!(auth["user"]["activated"], true);

    let (status, me) = server.get("/api/auth/me", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["user"]["id"], auth["user"]["id"]);

    let (status, _) = server.exchange(&code).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let response = server.callback("test-code", "wrong-state").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    server.stop().await;
}

#[tokio::test]
async fn oidc_links_verified_email_without_changing_password() {
    let mock = MockProvider::start().await;
    let server = OidcTestServer::start(&mock, &[], true).await;
    let (admin_token, _) = server.setup().await;

    let (status, local_user) = server
        .post(
            "/api/users",
            Some(&admin_token),
            Some(json!({
                "username": "local-link",
                "password": "local-password",
                "email": "link@example.com",
                "role": "user"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{local_user}");
    let local_id = local_user["id"].as_i64().expect("local id");

    let flow = server.start_flow().await;
    mock.set_claims(MockClaims {
        nonce: Some(flow.nonce),
        sub: "oidc-email-link".to_string(),
        email: Some("link@example.com".to_string()),
        email_verified: true,
        groups: Vec::new(),
        preferred_username: Some("should-not-replace".to_string()),
    });
    let response = server.callback("test-code", &flow.state).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let code = oidc_code(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("location"),
    );
    let (status, auth) = server.exchange(&code).await;
    assert_eq!(status, StatusCode::OK, "{auth}");
    assert_eq!(auth["user"]["id"].as_i64(), Some(local_id));
    assert_eq!(auth["user"]["username"], "local-link");
    assert_eq!(auth["user"]["role"], "user");

    // The local password still works: OIDC never overwrote it.
    let (status, _) = server
        .post(
            "/api/auth/login",
            None,
            Some(json!({"username": "local-link", "password": "local-password"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // A disabled local account cannot be used through SSO either.
    let (status, _) = server
        .patch(
            &format!("/api/users/{local_id}"),
            Some(&admin_token),
            Some(json!({"activated": false})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let flow = server.start_flow().await;
    mock.set_claims(MockClaims {
        nonce: Some(flow.nonce),
        sub: "oidc-email-link".to_string(),
        email: Some("link@example.com".to_string()),
        email_verified: true,
        groups: Vec::new(),
        preferred_username: Some("should-not-replace".to_string()),
    });
    let response = server.callback("test-code", &flow.state).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = response.json().await.expect("disabled body");
    assert_eq!(body["error"]["code"], "account_disabled");

    server.stop().await;
}

#[tokio::test]
async fn oidc_admin_group_mapping() {
    let mock = MockProvider::start().await;
    let server = OidcTestServer::start(&mock, &["ludex-admins"], true).await;
    let flow = server.start_flow().await;
    mock.set_claims(MockClaims {
        nonce: Some(flow.nonce),
        sub: "oidc-admin".to_string(),
        email: Some("admin-sso@example.com".to_string()),
        email_verified: true,
        groups: vec!["users".to_string(), "ludex-admins".to_string()],
        preferred_username: Some("sso-admin".to_string()),
    });
    let response = server.callback("test-code", &flow.state).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let code = oidc_code(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("location"),
    );
    let (status, auth) = server.exchange(&code).await;
    assert_eq!(status, StatusCode::OK, "{auth}");
    assert_eq!(auth["user"]["role"], "admin");

    server.stop().await;
}

#[tokio::test]
async fn oidc_rejects_bad_nonce_and_unknown_exchange_code() {
    let mock = MockProvider::start().await;
    let server = OidcTestServer::start(&mock, &[], true).await;
    let flow = server.start_flow().await;
    mock.set_claims(MockClaims {
        nonce: Some("wrong-nonce".to_string()),
        sub: "oidc-bad-nonce".to_string(),
        email: Some("bad-nonce@example.com".to_string()),
        email_verified: true,
        groups: Vec::new(),
        preferred_username: Some("bad-nonce".to_string()),
    });
    let response = server.callback("test-code", &flow.state).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let (status, error) = server.exchange("unknown-code").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "invalid_oidc_code");

    server.stop().await;
}

#[tokio::test]
async fn oidc_rejects_bad_signature() {
    let mock = MockProvider::start().await;
    let server = OidcTestServer::start(&mock, &[], true).await;
    let flow = server.start_flow().await;
    mock.set_claims(MockClaims {
        nonce: Some(flow.nonce),
        sub: "oidc-bad-signature".to_string(),
        email: Some("bad-signature@example.com".to_string()),
        email_verified: true,
        groups: Vec::new(),
        preferred_username: Some("bad-signature".to_string()),
    });
    mock.set_wrong_signing_key(true);
    let response = server.callback("test-code", &flow.state).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value = response.json().await.expect("signature error");
    assert_eq!(body["error"]["code"], "oidc_invalid_token");

    server.stop().await;
}

#[tokio::test]
async fn oidc_disabled_routes_return_404() {
    let server = OidcTestServer::start_without_oidc().await;

    let (status, error) = server.get("/api/auth/oidc/start", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "oidc_disabled");

    let (status, error) = server
        .post(
            "/api/auth/oidc/exchange",
            None,
            Some(json!({"code": "anything"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "oidc_disabled");

    let (status, config) = server.get("/api/config", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["features"]["oidc"], false);
    assert_eq!(config["oidcDisplayName"], "SSO");

    server.stop().await;
}

#[tokio::test]
async fn oidc_auto_create_disabled_returns_403() {
    let mock = MockProvider::start().await;
    let server = OidcTestServer::start(&mock, &[], false).await;
    let flow = server.start_flow().await;
    mock.set_claims(MockClaims {
        nonce: Some(flow.nonce),
        sub: "oidc-no-create".to_string(),
        email: Some("no-create@example.com".to_string()),
        email_verified: true,
        groups: Vec::new(),
        preferred_username: Some("no-create".to_string()),
    });
    let response = server.callback("test-code", &flow.state).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = response.json().await.expect("error body");
    assert_eq!(body["error"]["code"], "oidc_user_not_provisioned");

    server.stop().await;
}
