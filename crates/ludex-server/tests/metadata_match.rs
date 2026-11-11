//! HTTP tests for the manual metadata match dialog (#135).
//!
//! Every provider answer is a local fixture server: no test reaches IGDB or the
//! Steam Store. The suite proves the candidates shape, that applying a record
//! updates fields, provider identity and cached artwork, that a later refresh
//! keeps the selected provider id instead of searching again, manual-field
//! preservation, error handling and the audit event.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{TestServer, ADMIN_PASSWORD};
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Local metadata provider answering `/search`, `/games/{id}` and `/cover.jpg`.
struct MockProvider {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockProvider {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock provider");
        let address = listener.local_addr().expect("address");
        let url = format!("http://{address}");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let base = url.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let seen = Arc::clone(&seen);
                let base = base.clone();
                tokio::spawn(async move {
                    let request = read_request(&mut socket).await;
                    let line = request.lines().next().unwrap_or_default().to_string();
                    seen.lock().expect("requests").push(line.clone());
                    let response = respond(&base, &line);
                    let _ = socket.write_all(&response).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    fn paths(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }

    fn called(&self, needle: &str) -> bool {
        self.paths().iter().any(|line| line.contains(needle))
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    while let Ok(read) = socket.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        let request = String::from_utf8_lossy(&buffer).to_string();
        if let Some(end) = request.find("\r\n\r\n") {
            let length = request[..end]
                .lines()
                .find_map(|header| {
                    header
                        .to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if buffer.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buffer).to_string()
}

fn respond(base: &str, request_line: &str) -> Vec<u8> {
    if request_line.contains("/cover.jpg") {
        let body = b"fake-image-bytes".to_vec();
        let header = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: image/jpeg\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let mut response = header.into_bytes();
        response.extend_from_slice(&body);
        return response;
    }
    if request_line.contains("/games/peak-2025") {
        let body = json!({
            "id": "peak-2025",
            "title": "PEAK",
            "description": "Full provider record for PEAK.",
            "releaseDate": "2025-06-16",
            "coverUrl": format!("{base}/cover.jpg"),
            "backgroundUrl": format!("{base}/cover.jpg"),
            "screenshots": [format!("{base}/cover.jpg")],
            "genres": ["Adventure", "Indie"],
            "developers": ["Aggro Crab", "Landfall"],
            "publishers": ["Aggro Crab"],
            "rating": 88,
            "ageRating": 7,
            "providerUrl": "https://metadata.test/games/peak-2025"
        });
        return json_response(&body.to_string());
    }
    if request_line.contains("/games/peak-2019") {
        let body = json!({
            "id": "peak-2019",
            "title": "Peak Climber",
            "description": "A different 2019 climbing game.",
            "releaseDate": "2019-03-01",
            "developers": ["Wrong Studio"],
            "rating": 40
        });
        return json_response(&body.to_string());
    }
    if request_line.contains("/search") {
        let body = json!({
            "results": [
                {
                    "id": "peak-wrong",
                    "title": "PEAK",
                    "description": "Wrong automatic match.",
                    "releaseDate": "2019-03-01",
                    "developers": ["Wrong Studio"],
                    "rating": 12
                },
                {
                    "id": "peak-2025",
                    "title": "PEAK",
                    "description": "Climb with friends.",
                    "releaseDate": "2025-06-16",
                    "coverUrl": format!("{base}/cover.jpg"),
                    "genres": ["Adventure", "Indie"],
                    "developers": ["Aggro Crab", "Landfall"],
                    "publishers": ["Aggro Crab"],
                    "rating": 88,
                    "ageRating": 7,
                    "providerUrl": "https://metadata.test/games/peak-2025"
                },
                {
                    "id": "peak-2019",
                    "title": "Peak Climber",
                    "description": "A different 2019 climbing game.",
                    "releaseDate": "2019-03-01",
                    "developers": ["Wrong Studio"],
                    "rating": 40
                }
            ]
        });
        return json_response(&body.to_string());
    }
    "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        .to_string()
        .into_bytes()
}

fn json_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Server with setup done, an editor account and providers allowed on private
/// addresses (the fixture server runs on loopback).
async fn editor_server() -> (TestServer, String, String) {
    let server = TestServer::start_with(|config| {
        config.providers_allow_private = true;
    })
    .await;
    let (status, body) = server
        .post(
            "/api/setup",
            None,
            Some(json!({
                "username": "admin",
                "password": ADMIN_PASSWORD,
                "serverName": "Match Test",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let admin = body["token"].as_str().expect("token").to_string();
    server
        .create_user(&admin, "editor", "password123", "editor")
        .await;
    let (status, body) = server.login("editor", "password123").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let editor = body["token"].as_str().expect("token").to_string();
    (server, admin, editor)
}

async fn add_provider(server: &TestServer, admin: &str, name: &str, url: &str, priority: i64) {
    let (status, body) = server
        .post(
            "/api/admin/metadata-providers",
            Some(admin),
            Some(json!({
                "name": name,
                "baseUrl": url,
                "priority": priority,
                "enabled": true,
                "mapping": {},
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn seed_game(server: &TestServer, title: &str, release_date: Option<&str>) -> i64 {
    let pool = ludex_server::db::connect(&format!(
        "sqlite://{}?mode=rwc",
        server.data_dir.join("ludex.db").display()
    ))
    .await
    .expect("open database");
    let now = ludex_server::db::db_time(ludex_server::db::now());
    ludex_server::db::query(
        "INSERT INTO games (title, sort_title, release_date, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(title)
    .bind(title)
    .bind(release_date)
    .bind(&now)
    .bind(&now)
    .returning_id(&pool)
    .await
    .expect("insert game")
}

async fn audit_events(server: &TestServer) -> Vec<Value> {
    let pool = ludex_server::db::connect(&format!(
        "sqlite://{}?mode=rwc",
        server.data_dir.join("ludex.db").display()
    ))
    .await
    .expect("open database");
    ludex_server::db::query_as::<(String, String)>(
        "SELECT action, metadata FROM audit_logs WHERE action = 'game.metadata_matched'",
    )
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
    .collect()
}

#[tokio::test]
async fn candidates_apply_and_refresh_keep_the_selected_record() {
    let (server, admin, editor) = editor_server().await;
    let provider = MockProvider::start().await;
    add_provider(&server, &admin, "Fixture", &provider.url, 10).await;
    let game_id = seed_game(&server, "PEAK", Some("2019-03-01T00:00:00Z")).await;

    // ---------------- candidates ----------------
    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/metadata/candidates?q=PEAK"),
            Some(&editor),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["query"], "PEAK");
    assert_eq!(body["gameId"], game_id);
    assert_eq!(body["manualFields"], json!([]));
    let candidates = body["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 3, "{body}");
    let best = candidates
        .iter()
        .find(|candidate| candidate["providerId"] == "peak-2025")
        .expect("PEAK 2025 candidate");
    assert_eq!(
        candidates[0]["score"], best["score"],
        "scores are sorted: {body}"
    );
    assert_eq!(best["provider"], "fixture");
    assert_eq!(best["providerId"], "peak-2025");
    assert_eq!(best["title"], "PEAK");
    assert_eq!(best["year"], 2025);
    assert_eq!(best["developers"][0], "Aggro Crab");
    assert_eq!(best["providerUrl"], "https://metadata.test/games/peak-2025");
    assert!(best["coverUrl"]
        .as_str()
        .is_some_and(|url| url.ends_with("/cover.jpg")));
    assert!(best["score"].as_i64().expect("score") > 0);
    assert!(
        best["confidence"].as_f64().expect("confidence") > 0.5,
        "{best}"
    );
    assert_eq!(best["selected"], false);
    let providers = body["providers"].as_array().expect("providers");
    assert_eq!(providers.len(), 1, "{body}");
    assert_eq!(providers[0]["slug"], "fixture");
    assert_eq!(providers[0]["ok"], true);
    assert_eq!(providers[0]["candidates"], 3);

    // No Steam call: a configured provider is the fallback chain's first stop.
    assert!(
        !provider.called("steam")
            && !provider
                .paths()
                .iter()
                .any(|line| line.contains("storesearch")),
        "{:?}",
        provider.paths()
    );

    // ---------------- apply ----------------
    let (status, detail) = server
        .post(
            &format!("/api/games/{game_id}/metadata/apply"),
            Some(&editor),
            Some(json!({ "provider": "fixture", "providerId": "peak-2025" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["title"], "PEAK");
    assert_eq!(
        detail["releaseDate"].as_str().map(|v| &v[..10]),
        Some("2025-06-16")
    );
    assert_eq!(detail["description"], "Full provider record for PEAK.");
    assert_eq!(detail["genres"], json!(["Adventure", "Indie"]));
    assert_eq!(detail["developers"], json!(["Aggro Crab", "Landfall"]));
    assert_eq!(detail["publishers"], json!(["Aggro Crab"]));
    assert_eq!(detail["rating"], 88.0);
    assert_eq!(detail["ageRating"], 7);
    assert_eq!(detail["provider"]["slug"], "fixture");
    assert_eq!(detail["provider"]["id"], "peak-2025");
    assert_eq!(
        detail["provider"]["url"],
        "https://metadata.test/games/peak-2025"
    );
    let cover = detail["coverUrl"]
        .as_str()
        .expect("cached cover")
        .to_string();
    assert!(cover.starts_with("/api/media/"), "{cover}");
    let (status, _) = server.get(&cover, Some(&editor)).await;
    assert_eq!(status, StatusCode::OK, "cached cover is not served");
    assert!(!detail["screenshots"]
        .as_array()
        .expect("screenshots")
        .is_empty());

    // ---------------- refresh keeps the selected id ----------------
    let (status, queued) = server
        .post(
            &format!("/api/games/{game_id}/refresh-metadata"),
            Some(&editor),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");

    let mut refreshed = Value::Null;
    for _ in 0..200 {
        if provider.called("GET /games/peak-2025") {
            let (status, body) = server
                .get(&format!("/api/games/{game_id}"), Some(&editor))
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body["description"] == "Full provider record for PEAK."
                && body["provider"]["id"] == "peak-2025"
            {
                refreshed = body;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        refreshed["provider"]["id"], "peak-2025",
        "refresh lost the selected record: {refreshed}"
    );
    assert_eq!(
        refreshed["description"], "Full provider record for PEAK.",
        "refresh fell back to the wrong search result: {refreshed}"
    );

    // ---------------- audit ----------------
    let mut events = Vec::new();
    for _ in 0..100 {
        events = audit_events(&server).await;
        if !events.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["action"], "game.metadata_matched");
    let metadata = &events[0]["metadata"];
    assert_eq!(metadata["newProvider"]["slug"], "fixture");
    assert_eq!(metadata["newProvider"]["id"], "peak-2025");
    assert_eq!(metadata["oldTitle"], "PEAK");
    assert_eq!(metadata["replaceManual"], false);
    assert!(
        !events[0].to_string().contains("secret"),
        "audit metadata leaked a token: {events:?}"
    );
}

#[tokio::test]
async fn manual_fields_are_preserved_unless_explicitly_replaced() {
    let (server, admin, editor) = editor_server().await;
    let provider = MockProvider::start().await;
    add_provider(&server, &admin, "Fixture", &provider.url, 10).await;
    let game_id = seed_game(&server, "PEAK", None).await;

    // A hand-written genre: tracked as a manual edit.
    let (status, patched) = server
        .request(
            reqwest::Method::PATCH,
            &format!("/api/games/{game_id}"),
            Some(&editor),
            Some(json!({ "genres": ["Handwritten"], "players": 2 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{patched}");

    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/metadata/candidates"),
            Some(&editor),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["query"], "PEAK", "title is the default query");
    let manual = body["manualFields"].as_array().expect("manualFields");
    assert!(manual.contains(&json!("genres")), "{body}");
    assert!(manual.contains(&json!("players")), "{body}");

    let (status, preserved) = server
        .post(
            &format!("/api/games/{game_id}/metadata/apply"),
            Some(&editor),
            Some(json!({ "provider": "fixture", "providerId": "peak-2025" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{preserved}");
    assert_eq!(preserved["genres"], json!(["Handwritten"]), "{preserved}");
    assert_eq!(preserved["maxPlayers"], 2);
    assert_eq!(preserved["description"], "Full provider record for PEAK.");
    assert_eq!(preserved["provider"]["id"], "peak-2025");

    let (status, replaced) = server
        .post(
            &format!("/api/games/{game_id}/metadata/apply"),
            Some(&editor),
            Some(json!({
                "provider": "fixture",
                "providerId": "peak-2025",
                "replaceManual": true
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    assert_eq!(replaced["genres"], json!(["Adventure", "Indie"]));
    // The provider publishes no player count: there is nothing to replace, so
    // the current value survives even with `replaceManual`.
    assert_eq!(replaced["maxPlayers"], 2);

    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/metadata/candidates"),
            Some(&editor),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["manualFields"],
        json!([]),
        "replaceManual clears the metadata markers"
    );
}

#[tokio::test]
async fn invalid_input_and_provider_errors_are_isolated() {
    let (server, admin, editor) = editor_server().await;
    let provider = MockProvider::start().await;
    add_provider(&server, &admin, "Broken", "http://127.0.0.1:1", 10).await;
    add_provider(&server, &admin, "Fixture", &provider.url, 20).await;
    let game_id = seed_game(&server, "PEAK", None).await;

    // A failing provider is an error state, not a 500.
    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/metadata/candidates?q=PEAK"),
            Some(&editor),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let providers = body["providers"].as_array().expect("providers");
    assert_eq!(providers.len(), 2, "{body}");
    assert_eq!(providers[0]["slug"], "broken");
    assert_eq!(providers[0]["ok"], false);
    assert!(providers[0]["error"]
        .as_str()
        .is_some_and(|error| !error.is_empty()));
    assert_eq!(providers[1]["slug"], "fixture");
    assert_eq!(providers[1]["ok"], true);
    assert_eq!(
        body["candidates"].as_array().expect("candidates").len(),
        3,
        "{body}"
    );

    // Empty query.
    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/metadata/candidates?q=%20"),
            Some(&editor),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_query");

    // Unknown provider slug.
    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/metadata/candidates?provider=nope"),
            Some(&editor),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_provider");

    // IGDB is not configured in this server.
    let (status, body) = server
        .post(
            &format!("/api/games/{game_id}/metadata/apply"),
            Some(&editor),
            Some(json!({ "provider": "igdb", "providerId": "42" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_provider");

    // Unknown record.
    let (status, body) = server
        .post(
            &format!("/api/games/{game_id}/metadata/apply"),
            Some(&editor),
            Some(json!({ "provider": "fixture", "providerId": "ghost" })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "metadata_candidate_not_found");

    // Malformed ids.
    let (status, body) = server
        .post(
            &format!("/api/games/{game_id}/metadata/apply"),
            Some(&editor),
            Some(json!({ "provider": "fixture", "providerId": "  " })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_provider_id");

    // Members cannot open the dialog or apply anything.
    server
        .create_user(&admin, "member", "password123", "user")
        .await;
    let (status, body) = server.login("member", "password123").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let member = body["token"].as_str().expect("token").to_string();
    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/metadata/candidates"),
            Some(&member),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = server
        .post(
            &format!("/api/games/{game_id}/metadata/apply"),
            Some(&member),
            Some(json!({ "provider": "fixture", "providerId": "peak-2025" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}
