//! VirusTotal integration contract (#224).
//!
//! A local mock speaks the few VirusTotal endpoints Ludex uses, so the tests
//! exercise the real HTTP transport, quota accounting, cache and verdict
//! mapping without touching the network. The mock is also the proof that a
//! scan never uploads anything: its upload counter must stay at zero unless an
//! administrator explicitly consents.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode as AxumStatus;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use common::{TestServer, ADMIN_PASSWORD};
use ludex_server::db;
use reqwest::StatusCode;
use serde_json::{json, Value};

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// Behaviour the mock returns on `GET /files/{sha256}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Behaviour {
    #[default]
    Clean,
    Malicious,
    Suspicious,
    NotFound,
    Unauthorized,
    RateLimited,
    ServerError,
    Timeout,
}

#[derive(Default)]
struct MockState {
    behaviour: Mutex<Behaviour>,
    lookups: AtomicUsize,
    uploads: AtomicUsize,
    validations: AtomicUsize,
    reject_key: Mutex<bool>,
    /// Hash of every lookup, in call order: proves selection order and
    /// per-hash deduplication (#239).
    order: Mutex<Vec<String>>,
}

impl MockState {
    fn set(&self, behaviour: Behaviour) {
        *self
            .behaviour
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = behaviour;
    }

    fn lookups(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }

    fn uploads(&self) -> usize {
        self.uploads.load(Ordering::SeqCst)
    }
}

async fn file_lookup(
    State(mock): State<Arc<MockState>>,
    AxumPath(sha256): AxumPath<String>,
) -> Response {
    mock.lookups.fetch_add(1, Ordering::SeqCst);
    mock.order
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(sha256.clone());
    let behaviour = *mock
        .behaviour
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let body = |malicious: i64, suspicious: i64| {
        json!({
            "data": {
                "attributes": {
                    "sha256": sha256,
                    "size": 4,
                    "last_analysis_date": 1_767_225_600,
                    "last_analysis_stats": {
                        "malicious": malicious,
                        "suspicious": suspicious,
                        "undetected": 68,
                        "harmless": 2,
                        "timeout": 1,
                        "failure": 1
                    },
                    "last_analysis_results": {
                        "EngineMalicious": {
                            "category": if malicious > 0 { "malicious" } else { "undetected" },
                            "result": if malicious > 0 { Some("Trojan.Test") } else { None }
                        },
                        "EngineSuspicious": {
                            "category": if suspicious > 0 { "suspicious" } else { "undetected" },
                            "result": if suspicious > 0 { Some("Suspicious.Test") } else { None }
                        }
                    }
                }
            }
        })
    };
    match behaviour {
        Behaviour::Clean => (AxumStatus::OK, Json(body(0, 0))).into_response(),
        Behaviour::Malicious => (AxumStatus::OK, Json(body(3, 1))).into_response(),
        Behaviour::Suspicious => (AxumStatus::OK, Json(body(0, 2))).into_response(),
        Behaviour::NotFound => AxumStatus::NOT_FOUND.into_response(),
        Behaviour::Unauthorized => AxumStatus::UNAUTHORIZED.into_response(),
        Behaviour::RateLimited => (
            AxumStatus::TOO_MANY_REQUESTS,
            [(reqwest::header::RETRY_AFTER, "7")],
            Json(json!({ "error": { "code": "QuotaExceededError" } })),
        )
            .into_response(),
        Behaviour::ServerError => (
            AxumStatus::SERVICE_UNAVAILABLE,
            Json(json!({ "error": { "code": "maintenance" } })),
        )
            .into_response(),
        Behaviour::Timeout => {
            tokio::time::sleep(Duration::from_secs(3)).await;
            (AxumStatus::OK, Json(body(0, 0))).into_response()
        }
    }
}

async fn user_lookup(
    State(mock): State<Arc<MockState>>,
    AxumPath(key): AxumPath<String>,
) -> Response {
    mock.validations.fetch_add(1, Ordering::SeqCst);
    if *mock
        .reject_key
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return AxumStatus::UNAUTHORIZED.into_response();
    }
    (
        AxumStatus::OK,
        Json(json!({ "data": { "id": key, "type": "user" } })),
    )
        .into_response()
}

async fn upload(State(mock): State<Arc<MockState>>, _body: axum::body::Bytes) -> Response {
    mock.uploads.fetch_add(1, Ordering::SeqCst);
    (
        AxumStatus::OK,
        Json(json!({ "data": { "id": "analysis-mock", "type": "analysis" } })),
    )
        .into_response()
}

async fn upload_url() -> Response {
    // Files above 32 MB ask for a dedicated upload URL first. The tests stay
    // under the direct limit, so this only keeps the route honest.
    (
        AxumStatus::OK,
        Json(json!({ "data": "http://127.0.0.1:9/upload-target" })),
    )
        .into_response()
}

struct MockVt {
    state: Arc<MockState>,
    url: String,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl MockVt {
    async fn start() -> Self {
        let state = Arc::new(MockState {
            behaviour: Mutex::new(Behaviour::default()),
            ..MockState::default()
        });
        let app = Router::new()
            .route("/files/{sha256}", get(file_lookup))
            .route("/files/upload_url", get(upload_url))
            .route("/files", post(upload))
            .route("/users/{key}", get(user_lookup))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock listener");
        let address = listener.local_addr().expect("mock address");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            state,
            url: format!("http://{address}"),
            handle: Some(handle),
        }
    }
}

impl Drop for MockVt {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

struct Fixture {
    server: TestServer,
    mock: MockVt,
    token: String,
}

impl Fixture {
    async fn start() -> Self {
        Self::start_with(|_| {}).await
    }

    async fn start_with(configure: impl FnOnce(&mut ludex_server::Config)) -> Self {
        let mock = MockVt::start().await;
        let base = mock.url.clone();
        let server = TestServer::start_with(move |config| {
            config.virustotal_api_base = base;
            config.virustotal_per_minute = 100;
            config.virustotal_daily_limit = 100;
            config.virustotal_request_timeout = Duration::from_secs(1);
            configure(config);
        })
        .await;
        let (token, _) = server.setup().await;
        Self {
            server,
            mock,
            token,
        }
    }

    /// Inserts a game and one version directly, so a test never depends on the
    /// scanner's title parsing or on hashing a large file.
    async fn seed(&self, title: &str, file_name: &str, sha256: Option<&str>) -> (i64, i64) {
        seed(&self.server, title, file_name, sha256).await
    }

    async fn save_key(&self, key: &str) -> (StatusCode, Value) {
        self.server
            .put(
                "/api/admin/integrations/virustotal",
                &self.token,
                json!({ "apiKey": key }),
            )
            .await
    }

    async fn rescan(&self) -> (StatusCode, Value) {
        self.server
            .get(
                "/api/admin/integrations/virustotal/rescan",
                Some(&self.token),
            )
            .await
    }

    async fn save_rescan(&self, body: Value) -> (StatusCode, Value) {
        self.server
            .put(
                "/api/admin/integrations/virustotal/rescan",
                &self.token,
                body,
            )
            .await
    }

    async fn run_rescan(&self) -> (StatusCode, Value) {
        self.server
            .post(
                "/api/admin/integrations/virustotal/rescan/run",
                Some(&self.token),
                None,
            )
            .await
    }

    async fn scan(&self, version_id: i64, mode: Option<&str>) -> (StatusCode, Value) {
        let mut body = json!({ "versionId": version_id });
        if let Some(mode) = mode {
            body["mode"] = json!(mode);
        }
        self.server
            .post("/api/admin/virustotal/scan", Some(&self.token), Some(body))
            .await
    }
}

async fn database(server: &TestServer) -> db::Database {
    let url = ludex_server::config::default_database_url(&server.data_dir);
    db::connect(&url).await.expect("test database")
}

async fn seed(
    server: &TestServer,
    title: &str,
    file_name: &str,
    sha256: Option<&str>,
) -> (i64, i64) {
    let pool = database(server).await;
    let now = db::db_time(db::now());
    let game_id = db::query(
        "INSERT INTO games (title, sort_title, created_at, updated_at) VALUES (?, ?, ?, ?)",
    )
    .bind(title)
    .bind(title.to_lowercase())
    .bind(&now)
    .bind(&now)
    .returning_id(&pool)
    .await
    .expect("insert game");
    let version_id = db::query(
        "INSERT INTO game_versions
            (game_id, file_path, file_name, size, missing, indexed_at, content_sha256)
         VALUES (?, ?, ?, 4, 0, ?, ?)",
    )
    .bind(game_id)
    .bind(file_name)
    .bind(file_name)
    .bind(&now)
    .bind(sha256)
    .returning_id(&pool)
    .await
    .expect("insert version");
    pool.close().await;
    (game_id, version_id)
}

fn only_scan(body: &Value) -> &Value {
    body["scans"]
        .as_array()
        .and_then(|scans| scans.first())
        .unwrap_or_else(|| panic!("one scan in {body}"))
}

#[tokio::test]
async fn key_management_never_returns_the_key() {
    let fixture = Fixture::start().await;

    let (status, body) = fixture
        .server
        .get("/api/admin/integrations/virustotal", Some(&fixture.token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["configured"], false);
    assert_eq!(body["source"], "none");
    assert_eq!(body["fingerprint"], Value::Null);
    assert_eq!(body["quota"]["dailyLimit"], 100);
    assert_eq!(body["quota"]["remaining"], 100);
    assert!(body["quota"]["resetAt"]
        .as_str()
        .is_some_and(|value| value.contains('T')));

    let (status, body) = fixture.save_key("vt-secret-abcd").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["configured"], true);
    assert_eq!(body["source"], "database");
    assert_eq!(body["fingerprint"], "...abcd");
    assert!(
        !body.to_string().contains("vt-secret-abcd"),
        "the key leaked in {body}"
    );

    // Encrypted at rest, not plaintext.
    let pool = database(&fixture.server).await;
    let (stored,): (String,) =
        db::query_as::<(String,)>("SELECT api_key_enc FROM virustotal_integration WHERE id = 1")
            .fetch_one(&pool)
            .await
            .expect("stored key");
    assert!(stored.starts_with("v1."));
    assert!(!stored.contains("vt-secret-abcd"));
    pool.close().await;

    let (status, body) = fixture
        .server
        .delete("/api/admin/integrations/virustotal", &fixture.token)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["configured"], false);
    assert_eq!(body["source"], "none");
}

#[tokio::test]
async fn environment_key_wins_and_cannot_be_changed() {
    let fixture = Fixture::start_with(|config| {
        config.virustotal_api_key = Some("env-key-wxyz".to_string());
    })
    .await;

    let (status, body) = fixture
        .server
        .get("/api/admin/integrations/virustotal", Some(&fixture.token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "env");
    assert_eq!(body["configured"], true);
    assert_eq!(body["fingerprint"], "...wxyz");
    assert!(!body.to_string().contains("env-key-wxyz"));

    let (status, body) = fixture.save_key("database-key-1234").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "env_managed");

    let (status, body) = fixture
        .server
        .delete("/api/admin/integrations/virustotal", &fixture.token)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "env_managed");
}

#[tokio::test]
async fn test_endpoint_reports_acceptance_and_rejection() {
    let fixture = Fixture::start().await;

    // An unsaved key can be validated before saving it.
    let (status, body) = fixture
        .server
        .post(
            "/api/admin/integrations/virustotal/test",
            Some(&fixture.token),
            Some(json!({ "apiKey": "transient-key-9999" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    let (_, state) = fixture
        .server
        .get("/api/admin/integrations/virustotal", Some(&fixture.token))
        .await;
    assert_eq!(state["source"], "none", "testing must not store the key");
    assert_eq!(state["lastError"], Value::Null);

    // Without `apiKey` the stored key is used.
    fixture.save_key("vt-secret-abcd").await;
    let (status, body) = fixture
        .server
        .post(
            "/api/admin/integrations/virustotal/test",
            Some(&fixture.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["message"], "VirusTotal accepted the API key.");

    *fixture
        .mock
        .state
        .reject_key
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;

    // A rejected unsaved key never becomes the stored integration's error.
    let (status, body) = fixture
        .server
        .post(
            "/api/admin/integrations/virustotal/test",
            Some(&fixture.token),
            Some(json!({ "apiKey": "transient-key-9999" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false);
    let (_, state) = fixture
        .server
        .get("/api/admin/integrations/virustotal", Some(&fixture.token))
        .await;
    assert_eq!(state["lastError"], Value::Null, "{state}");

    let (status, body) = fixture
        .server
        .post(
            "/api/admin/integrations/virustotal/test",
            Some(&fixture.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("invalid_key:")),
        "{body}"
    );

    // The stored-key failure is surfaced as lastError, never as a fake clean.
    let (_, status) = fixture
        .server
        .get("/api/admin/integrations/virustotal", Some(&fixture.token))
        .await;
    assert!(
        status["lastError"]
            .as_str()
            .is_some_and(|error| error.starts_with("invalid_key:")),
        "{status}"
    );
}

#[tokio::test]
async fn scan_verdicts_cache_and_player_read() {
    let fixture = Fixture::start().await;
    fixture.save_key("vt-secret-abcd").await;
    let (game_id, version_id) = fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;

    fixture.mock.state.set(Behaviour::Malicious);
    let (status, body) = fixture.scan(version_id, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let scan = only_scan(&body);
    assert_eq!(scan["verdict"], "malicious", "{body}");
    assert_eq!(scan["versionId"], version_id);
    assert_eq!(scan["source"], "api");
    assert_eq!(scan["stats"]["malicious"], 3);
    assert_eq!(scan["stats"]["suspicious"], 1);
    assert_eq!(scan["stats"]["totalEngines"], 76);
    assert_eq!(scan["stats"]["undetected"], 68);
    assert!(scan["sha256"].as_str().is_some_and(|hash| hash.len() == 64));
    assert!(scan["permalink"]
        .as_str()
        .is_some_and(|link| link.starts_with("https://www.virustotal.com/gui/file/")));
    assert_eq!(fixture.mock.state.lookups(), 1);
    assert_eq!(fixture.mock.state.uploads(), 0, "a scan must never upload");

    // The cache answers the second scan without an API call.
    let (status, body) = fixture.scan(version_id, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(only_scan(&body)["source"], "cache");
    assert_eq!(fixture.mock.state.lookups(), 1);

    // Players read the cached report without consuming quota.
    let (status, body) = fixture
        .server
        .get(
            &format!("/api/games/{game_id}/virustotal"),
            Some(&fixture.token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["gameId"], game_id);
    assert_eq!(body["versions"][0]["verdict"], "malicious");
    assert_eq!(fixture.mock.state.lookups(), 1);

    // force re-checks, now suspicious only.
    fixture.mock.state.set(Behaviour::Suspicious);
    let (status, body) = fixture
        .server
        .post(
            "/api/admin/virustotal/scan",
            Some(&fixture.token),
            Some(json!({ "versionId": version_id, "force": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(only_scan(&body)["verdict"], "suspicious");
    assert_eq!(only_scan(&body)["source"], "api");
    assert_eq!(fixture.mock.state.lookups(), 2);

    // Clean looks clean.
    fixture.mock.state.set(Behaviour::Clean);
    let (status, body) = fixture
        .server
        .post(
            "/api/admin/virustotal/scan",
            Some(&fixture.token),
            Some(json!({ "versionId": version_id, "force": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(only_scan(&body)["verdict"], "clean");
}

#[tokio::test]
async fn every_failure_keeps_an_honest_verdict() {
    let fixture = Fixture::start().await;
    fixture.save_key("vt-secret-abcd").await;
    let (_, version_id) = fixture.seed("Hades", "Hades.zip", Some(SHA_B)).await;

    let cases = [
        (Behaviour::NotFound, "not_found", "not_found"),
        (Behaviour::Unauthorized, "error", "invalid_key"),
        (Behaviour::RateLimited, "quota_exceeded", "quota_exceeded"),
        (Behaviour::ServerError, "unknown", "outage"),
        (Behaviour::Timeout, "unknown", "outage"),
    ];
    for (behaviour, verdict, error_prefix) in cases {
        fixture.mock.state.set(behaviour);
        let (status, body) = fixture
            .server
            .post(
                "/api/admin/virustotal/scan",
                Some(&fixture.token),
                Some(json!({ "versionId": version_id, "force": true })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            only_scan(&body)["verdict"],
            verdict,
            "{behaviour:?}: {body}"
        );
        assert_ne!(only_scan(&body)["verdict"], "clean");

        let (_, state) = fixture
            .server
            .get("/api/admin/integrations/virustotal", Some(&fixture.token))
            .await;
        if behaviour == Behaviour::NotFound {
            // A 404 is a normal answer, not an integration error.
            assert_eq!(state["lastError"], Value::Null, "{state}");
        } else {
            assert!(
                state["lastError"]
                    .as_str()
                    .is_some_and(|error| error.starts_with(error_prefix)),
                "{behaviour:?}: {state}"
            );
            if behaviour == Behaviour::RateLimited {
                // The mock sends `Retry-After: 7`; the hint must survive.
                assert!(
                    state["lastError"]
                        .as_str()
                        .is_some_and(|error| error.contains("retry after 7s")),
                    "{state}"
                );
            }
        }
    }
}

#[tokio::test]
async fn local_budget_is_enforced_and_surfaced() {
    let fixture = Fixture::start_with(|config| {
        config.virustotal_daily_limit = 1;
    })
    .await;
    fixture.save_key("vt-secret-abcd").await;
    let (_, first) = fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;
    let (_, second) = fixture.seed("Hades", "Hades.zip", Some(SHA_B)).await;

    let (status, body) = fixture.scan(first, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(only_scan(&body)["verdict"], "clean");
    assert_eq!(fixture.mock.state.lookups(), 1);

    let (status, body) = fixture.scan(second, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(only_scan(&body)["verdict"], "quota_exceeded", "{body}");
    assert_eq!(
        fixture.mock.state.lookups(),
        1,
        "no request past the budget"
    );

    let (_, state) = fixture
        .server
        .get("/api/admin/integrations/virustotal", Some(&fixture.token))
        .await;
    assert_eq!(state["quota"]["usedToday"], 1);
    assert_eq!(state["quota"]["remaining"], 0);
    assert!(state["lastError"]
        .as_str()
        .is_some_and(|error| error.starts_with("quota_exceeded")));
}

#[tokio::test]
async fn upload_requires_consent_and_is_size_gated() {
    let fixture = Fixture::start().await;
    fixture.save_key("vt-secret-abcd").await;
    let (_, version_id) = fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;
    std::fs::write(fixture.server.games_dir.join("Celeste.zip"), b"ludex").expect("archive");

    // Hash mode never uploads.
    let (status, body) = fixture.scan(version_id, Some("hash_and_binaries")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(fixture.mock.state.uploads(), 0);

    // No consent: refused before any upload.
    let (status, body) = fixture.scan(version_id, Some("upload")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "upload_consent_required");
    assert_eq!(fixture.mock.state.uploads(), 0);

    let (status, body) = fixture
        .server
        .post(
            "/api/admin/virustotal/scan",
            Some(&fixture.token),
            Some(json!({
                "versionId": version_id,
                "mode": "upload",
                "confirmUpload": true,
                "iUnderstand": "public-sharing"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(only_scan(&body)["verdict"], "unknown");
    assert_eq!(fixture.mock.state.uploads(), 1);

    // `consent` is the legacy name for the same acknowledgement.
    let (status, body) = fixture
        .server
        .post(
            "/api/admin/virustotal/scan",
            Some(&fixture.token),
            Some(json!({
                "versionId": version_id,
                "mode": "upload",
                "confirmUpload": true,
                "consent": "public-sharing"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(fixture.mock.state.uploads(), 2);

    // 651 MB sparse file: refused with 413 and a clear message.
    let big = "Big Repack.zip";
    let big_path = fixture.server.games_dir.join(big);
    let file = std::fs::File::create(&big_path).expect("sparse file");
    file.set_len(651 * 1024 * 1024).expect("sparse length");
    drop(file);
    let (_, big_version) = fixture.seed("Big Repack", big, Some(SHA_B)).await;
    let (status, body) = fixture
        .server
        .post(
            "/api/admin/virustotal/scan",
            Some(&fixture.token),
            Some(json!({
                "versionId": big_version,
                "mode": "upload",
                "confirmUpload": true,
                "iUnderstand": "public-sharing"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["error"]["code"], "upload_too_large");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("650 MB")),
        "{body}"
    );
    assert_eq!(fixture.mock.state.uploads(), 2, "nothing else was uploaded");
}

#[tokio::test]
async fn disabled_without_key_and_forbidden_for_players() {
    let fixture = Fixture::start().await;
    let (game_id, version_id) = fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;

    let (status, body) = fixture.scan(version_id, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(only_scan(&body)["verdict"], "disabled");
    assert_eq!(fixture.mock.state.lookups(), 0);

    fixture
        .server
        .create_user(&fixture.token, "player", ADMIN_PASSWORD, "user")
        .await;
    let (status, login) = fixture.server.login("player", ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let player = login["token"].as_str().expect("token").to_string();

    let (status, body) = fixture
        .server
        .post(
            "/api/admin/virustotal/scan",
            Some(&player),
            Some(json!({ "versionId": version_id })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // A player can still read the (empty) cached report.
    let (status, body) = fixture
        .server
        .get(&format!("/api/games/{game_id}/virustotal"), Some(&player))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["versions"], json!([]));
}

#[tokio::test]
async fn hash_and_binaries_hashes_beside_the_archive_only() {
    let fixture = Fixture::start().await;
    fixture.save_key("vt-secret-abcd").await;
    let (_, version_id) = fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;
    std::fs::write(fixture.server.games_dir.join("Celeste.zip"), b"archive").expect("archive");
    std::fs::write(fixture.server.games_dir.join("setup.exe"), b"installer").expect("installer");
    std::fs::write(fixture.server.games_dir.join("notes.txt"), b"ignored").expect("notes");

    let (status, body) = fixture.scan(version_id, Some("hash_and_binaries")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let scans = body["scans"].as_array().expect("scans");
    assert_eq!(scans.len(), 2, "archive plus one installer: {body}");
    assert_eq!(fixture.mock.state.lookups(), 2);
    assert_eq!(fixture.mock.state.uploads(), 0);
    let shas: Vec<&str> = scans
        .iter()
        .filter_map(|scan| scan["sha256"].as_str())
        .collect();
    assert!(shas.iter().all(|sha| sha.len() == 64));
    assert_ne!(shas[0], shas[1], "the installer has its own hash");
}

/* ------------------- periodic re-check (#239) ---------------------------- */

/// Inserts or updates one cache row, so a test can age a report without
/// waiting for a TTL.
async fn cache_report(
    server: &TestServer,
    sha256: &str,
    verdict: &str,
    checked_at: &str,
    ttl_seconds: i64,
) {
    let pool = database(server).await;
    db::query(
        "INSERT INTO virustotal_cache
            (sha256, verdict, stats_json, engine_hits_json, checked_at, ttl_seconds)
         VALUES (?, ?, '{}', '[]', ?, ?)
         ON CONFLICT (sha256) DO UPDATE SET
            verdict = excluded.verdict,
            checked_at = excluded.checked_at,
            ttl_seconds = excluded.ttl_seconds",
    )
    .bind(sha256)
    .bind(verdict)
    .bind(checked_at)
    .bind(ttl_seconds)
    .execute(&pool)
    .await
    .expect("cache row");
    pool.close().await;
}

async fn set_next_run(server: &TestServer, value: &str) {
    let pool = database(server).await;
    db::query("UPDATE virustotal_rescan SET next_run_at = ? WHERE id = 1")
        .bind(value)
        .execute(&pool)
        .await
        .expect("next run");
    pool.close().await;
}

async fn audit_rows(server: &TestServer, action: &str) -> Vec<Value> {
    let pool = database(server).await;
    let rows = db::query_as::<(String, String)>(
        "SELECT action, metadata FROM audit_logs WHERE action = ? ORDER BY id",
    )
    .bind(action)
    .fetch_all(&pool)
    .await
    .expect("audit rows")
    .into_iter()
    .map(|(action, metadata)| {
        json!({
            "action": action,
            "metadata": serde_json::from_str::<Value>(&metadata).unwrap_or(Value::Null),
        })
    })
    .collect();
    pool.close().await;
    rows
}

async fn wait_for_audit(server: &TestServer, action: &str) -> Vec<Value> {
    for _ in 0..80 {
        let rows = audit_rows(server, action).await;
        if !rows.is_empty() {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no {action} audit row");
}

fn hours_ago(hours: i64) -> String {
    db::db_time(db::now() - chrono::Duration::hours(hours))
}

#[tokio::test]
async fn rescan_policy_defaults_validation_and_audit() {
    let fixture = Fixture::start().await;

    let (status, body) = fixture.rescan().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false, "off by default: {body}");
    assert_eq!(body["intervalHours"], 168, "{body}");
    assert_eq!(body["includeMalicious"], false, "{body}");
    assert_eq!(body["nextRunAt"], Value::Null, "{body}");
    assert_eq!(body["lastRunAt"], Value::Null, "{body}");
    assert_eq!(body["remaining"], 0, "{body}");
    assert_eq!(body["due"], false, "{body}");

    for interval in [json!(12), json!(-24), json!(365)] {
        let (status, body) = fixture
            .save_rescan(json!({ "enabled": true, "intervalHours": interval }))
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "interval {interval}: {body}"
        );
        assert_eq!(body["error"]["code"], "invalid_interval", "{body}");
    }

    let (status, body) = fixture
        .save_rescan(json!({ "enabled": true, "intervalHours": 24 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true, "{body}");
    assert_eq!(body["intervalHours"], 24, "{body}");
    assert_eq!(body["due"], false, "{body}");
    let next = body["nextRunAt"].as_str().expect("next run").to_string();
    let next = db::parse_time(&next).expect("rfc3339");
    let expected = db::now() + chrono::Duration::hours(24);
    assert!(
        (next - expected).num_seconds().abs() < 60,
        "next run ~24 h away, got {next} vs {expected}"
    );

    // Disabling clears the schedule but keeps the history fields.
    let (status, body) = fixture
        .save_rescan(json!({ "enabled": false, "intervalHours": 24 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["nextRunAt"], Value::Null, "{body}");

    // Manual only is a valid policy and never schedules a run.
    let (status, body) = fixture
        .save_rescan(json!({ "enabled": true, "intervalHours": 0 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["nextRunAt"], Value::Null, "{body}");
    assert_eq!(body["due"], false, "{body}");

    let rows = wait_for_audit(&fixture.server, "virustotal.rescan_policy").await;
    assert_eq!(rows.len(), 3, "one audit row per accepted save: {rows:?}");
    assert_eq!(rows[0]["metadata"]["intervalHours"], 24);
    assert_eq!(rows[0]["metadata"]["interval"], "daily");
    assert_eq!(rows[0]["metadata"]["enabled"], true);
    assert_eq!(rows[2]["metadata"]["interval"], "manual");
    assert!(
        !rows.iter().any(|row| row.to_string().contains("vt-secret")),
        "the audit trail never carries a key: {rows:?}"
    );

    // Player accounts cannot read or change the policy.
    fixture
        .server
        .create_user(&fixture.token, "player", ADMIN_PASSWORD, "user")
        .await;
    let (_, login) = fixture.server.login("player", ADMIN_PASSWORD).await;
    let player = login["token"].as_str().expect("token").to_string();
    let (status, _) = fixture
        .server
        .get("/api/admin/integrations/virustotal/rescan", Some(&player))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = fixture
        .server
        .put(
            "/api/admin/integrations/virustotal/rescan",
            &player,
            json!({ "enabled": true, "intervalHours": 24 }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = fixture
        .server
        .post(
            "/api/admin/integrations/virustotal/rescan/run",
            Some(&player),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rescan_selects_missing_then_oldest_and_deduplicates() {
    let fixture = Fixture::start_with(|config| config.virustotal_per_minute = 0).await;
    fixture.save_key("vt-secret-abcd").await;
    // Never checked: must run first.
    let (_, missing) = fixture.seed("Hollow", "Hollow.zip", Some(SHA_B)).await;
    // Old report: second.
    let (_, old) = fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;
    cache_report(
        &fixture.server,
        SHA_A,
        "clean",
        &hours_ago(24 * 10),
        7 * 24 * 3_600,
    )
    .await;
    // Same archive under a second version: one lookup in total.
    fixture
        .seed("Celeste Deluxe", "Celeste Deluxe.zip", Some(SHA_A))
        .await;
    // Fresh report: never selected at a 24 h interval.
    let sha_c = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    fixture.seed("Tunic", "Tunic.zip", Some(sha_c)).await;
    cache_report(
        &fixture.server,
        sha_c,
        "clean",
        &hours_ago(1),
        7 * 24 * 3_600,
    )
    .await;

    let (status, body) = fixture
        .save_rescan(json!({ "enabled": true, "intervalHours": 24 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["remaining"], 2, "one missing plus one stale: {body}");

    let (status, body) = fixture.run_rescan().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scannedLastRun"], 2, "{body}");
    assert_eq!(body["remaining"], 0, "{body}");
    assert_eq!(body["lastError"], Value::Null, "{body}");
    assert!(body["lastRunAt"].is_string(), "{body}");
    let order = fixture
        .mock
        .state
        .order
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        order,
        vec![SHA_B.to_string(), SHA_A.to_string()],
        "{order:?}"
    );
    assert_eq!(fixture.mock.state.uploads(), 0, "a rescan never uploads");
    assert_eq!(
        body["cursor"], SHA_A,
        "the cursor is the last processed hash: {body}"
    );

    // The next scheduled run is one interval after this pass.
    let next = db::parse_time(body["nextRunAt"].as_str().expect("next run")).expect("rfc3339");
    let last = db::parse_time(body["lastRunAt"].as_str().expect("last run")).expect("rfc3339");
    assert_eq!((next - last).num_hours(), 24, "{body}");

    // A second run has nothing left to do.
    let (status, body) = fixture.run_rescan().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scannedLastRun"], 0, "{body}");
    assert_eq!(fixture.mock.state.lookups(), 2, "no duplicate lookups");
    let _ = (missing, old);
}

#[tokio::test]
async fn rescan_skips_malicious_until_opted_in() {
    let fixture = Fixture::start_with(|config| config.virustotal_per_minute = 0).await;
    fixture.save_key("vt-secret-abcd").await;
    let (_, version_id) = fixture.seed("Hollow", "Hollow.zip", Some(SHA_A)).await;
    cache_report(
        &fixture.server,
        SHA_A,
        "malicious",
        &hours_ago(24 * 20),
        100 * 365 * 24 * 3_600,
    )
    .await;

    let (status, body) = fixture
        .save_rescan(json!({ "enabled": true, "intervalHours": 24 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["remaining"], 0,
        "a malicious verdict is skipped: {body}"
    );

    let (status, body) = fixture.run_rescan().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scannedLastRun"], 0, "{body}");
    assert_eq!(fixture.mock.state.lookups(), 0, "nothing left the machine");

    // Opting in keeps the existing schedule and re-checks the archive.
    let scheduled = body["nextRunAt"].as_str().expect("next run").to_string();
    let (status, body) = fixture
        .save_rescan(json!({
            "enabled": true,
            "intervalHours": 24,
            "includeMalicious": true
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["nextRunAt"].as_str(),
        Some(scheduled.as_str()),
        "editing the policy keeps the run already scheduled"
    );
    assert_eq!(body["remaining"], 1, "{body}");
    let (status, body) = fixture.run_rescan().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["scannedLastRun"], 1, "{body}");
    assert_eq!(fixture.mock.state.lookups(), 1, "{body}");
    let _ = version_id;
}

#[tokio::test]
async fn rescan_fails_soft_on_outage_invalid_key_and_quota() {
    // Outage: one request, no loop, the schedule keeps moving.
    let fixture = Fixture::start_with(|config| {
        config.virustotal_per_minute = 0;
        config.virustotal_daily_limit = 100;
    })
    .await;
    fixture.save_key("vt-secret-abcd").await;
    fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;
    fixture.seed("Hades", "Hades.zip", Some(SHA_B)).await;
    fixture.mock.state.set(Behaviour::ServerError);
    fixture
        .save_rescan(json!({ "enabled": true, "intervalHours": 24 }))
        .await;
    let (status, body) = fixture.run_rescan().await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an outage never fails the request: {body}"
    );
    assert!(
        body["lastError"]
            .as_str()
            .is_some_and(|error| error.starts_with("outage:")),
        "{body}"
    );
    assert_eq!(body["scannedLastRun"], 0, "{body}");
    assert_eq!(
        body["remaining"], 2,
        "both archives stay outstanding: {body}"
    );
    assert!(body["nextRunAt"].is_string(), "retry next interval: {body}");
    assert_eq!(
        fixture.mock.state.lookups(),
        2,
        "two attempts, then the pass stops"
    );

    // A rejected key stops the pass immediately.
    fixture.mock.state.set(Behaviour::Unauthorized);
    let pool = database(&fixture.server).await;
    db::query("DELETE FROM virustotal_cache")
        .execute(&pool)
        .await
        .expect("clear cache");
    pool.close().await;
    let (status, body) = fixture.run_rescan().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["lastError"]
            .as_str()
            .is_some_and(|error| error.starts_with("invalid_key:")),
        "{body}"
    );
    assert_eq!(
        fixture.mock.state.lookups(),
        3,
        "one rejected request per pass, never a loop"
    );

    // Daily budget: one archive refreshed, the rest resumes later.
    let fixture = Fixture::start_with(|config| {
        config.virustotal_per_minute = 0;
        config.virustotal_daily_limit = 1;
    })
    .await;
    fixture.save_key("vt-secret-abcd").await;
    fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;
    fixture.seed("Hades", "Hades.zip", Some(SHA_B)).await;
    fixture
        .save_rescan(json!({ "enabled": true, "intervalHours": 24 }))
        .await;
    let (status, body) = fixture.run_rescan().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        fixture.mock.state.lookups(),
        1,
        "the daily budget is honoured"
    );
    assert_eq!(body["scannedLastRun"], 1, "{body}");
    assert_eq!(body["remaining"], 1, "{body}");
    assert!(
        body["lastError"]
            .as_str()
            .is_some_and(|error| error.starts_with("quota_exceeded:")),
        "{body}"
    );

    let rows = wait_for_audit(&fixture.server, "virustotal.rescan_run").await;
    let last = rows.last().expect("run row");
    assert_eq!(last["metadata"]["scanned"], 1);
    assert_eq!(last["metadata"]["remaining"], 1);
    assert_eq!(last["metadata"]["error"], "quota_exceeded");
    assert!(
        !last.to_string().contains("vt-secret"),
        "no key in the audit trail: {last}"
    );
}

#[tokio::test]
async fn rescan_worker_honours_the_schedule_and_disabling_stops_it() {
    let fixture = Fixture::start_with(|config| {
        config.virustotal_per_minute = 0;
        config.virustotal_rescan_tick = Duration::from_millis(40);
    })
    .await;
    fixture.save_key("vt-secret-abcd").await;
    fixture.seed("Celeste", "Celeste.zip", Some(SHA_A)).await;

    // Enabled weekly: the persisted next run is a week away, so nothing fires.
    let (status, body) = fixture
        .save_rescan(json!({ "enabled": true, "intervalHours": 168 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fixture.mock.state.lookups(),
        0,
        "the worker respects a future nextRunAt"
    );

    // Force the schedule into the past, as a restart after a long downtime
    // would find it: the worker picks it up and persists a fresh next run.
    set_next_run(&fixture.server, &hours_ago(1)).await;
    let mut scanned = false;
    for _ in 0..100 {
        if fixture.mock.state.lookups() > 0 {
            scanned = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(scanned, "the worker runs a due pass");
    let mut finished = None;
    for _ in 0..100 {
        let (status, body) = fixture.rescan().await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["lastRunAt"].is_string() {
            finished = Some(body);
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let body = finished.expect("the pass persisted its outcome");
    assert_eq!(body["scannedLastRun"], 1, "{body}");
    assert_eq!(body["remaining"], 0, "{body}");
    let next = db::parse_time(body["nextRunAt"].as_str().expect("next run")).expect("rfc3339");
    let last = db::parse_time(body["lastRunAt"].as_str().expect("last run")).expect("rfc3339");
    assert_eq!((next - last).num_hours(), 168, "{body}");
    assert!(
        next > db::now() - chrono::Duration::minutes(1),
        "the schedule advanced to the future: {body}"
    );

    // Disabling stops future passes even with an overdue schedule.
    let (status, body) = fixture
        .save_rescan(json!({ "enabled": false, "intervalHours": 168 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    set_next_run(&fixture.server, &hours_ago(1)).await;
    let before = fixture.mock.state.lookups();
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        fixture.mock.state.lookups(),
        before,
        "a disabled policy does nothing"
    );
    let (status, body) = fixture.run_rescan().await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "rescan_disabled", "{body}");
}
