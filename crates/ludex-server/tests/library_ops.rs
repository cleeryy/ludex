//! HTTP tests for multiple library roots (#45), the folder watcher status
//! (#44), integrity checks (#43) and media cache garbage collection (#18).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ludex_server::{start, Config, ServerHandle};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use tempfile::TempDir;

const ADMIN_PASSWORD: &str = "password123";
const MIB: usize = 1024 * 1024;

struct TestServer {
    url: String,
    client: Client,
    handle: Option<ServerHandle>,
    temp: TempDir,
    games_dir: PathBuf,
    media_dir: PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        let mut config = Config::for_local(games_dir.clone(), data_dir, media_dir.clone());
        config.local_mode = false;
        config.bind = "127.0.0.1:0".parse().expect("bind");
        let handle = start(config).await.expect("server starts");
        Self {
            url: handle.url.clone(),
            client: Client::builder().build().expect("client"),
            handle: Some(handle),
            temp,
            games_dir,
            media_dir,
        }
    }

    async fn setup(&self) -> String {
        let (status, body) = self
            .post(
                "/api/setup",
                None,
                Some(json!({
                    "username": "admin",
                    "password": ADMIN_PASSWORD,
                    "serverName": "Home Arcade"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().expect("token").to_string()
    }

    async fn request(
        &self,
        method: Method,
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
        self.request(Method::GET, path, token, None).await
    }

    async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.request(Method::POST, path, token, body).await
    }

    async fn patch(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::PATCH, path, Some(token), Some(body))
            .await
    }

    async fn delete(&self, path: &str, token: &str) -> (StatusCode, Value) {
        self.request(Method::DELETE, path, Some(token), None).await
    }

    fn write_archive(&self, dir: &Path, name: &str, fill: u8) -> PathBuf {
        std::fs::create_dir_all(dir).expect("mkdir");
        let path = dir.join(name);
        std::fs::write(&path, vec![fill; MIB + 64]).expect("write archive");
        path
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.shutdown.send(());
        }
    }
}

async fn scan_status(server: &TestServer, token: &str) -> Value {
    let (status, body) = server.get("/api/library/scan", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn wait_for_scan(server: &TestServer, token: &str) -> Value {
    for _ in 0..400 {
        let status = scan_status(server, token).await;
        if !status["running"].as_bool().unwrap_or(true) && status["finishedAt"].as_str().is_some() {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("initial scan did not finish");
}

async fn wait_for_scan_after(server: &TestServer, token: &str, before: &str) -> Value {
    for _ in 0..400 {
        let status = scan_status(server, token).await;
        if !status["running"].as_bool().unwrap_or(true)
            && status["finishedAt"]
                .as_str()
                .is_some_and(|finished| finished != before)
        {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("scan did not finish");
}

async fn trigger_full_scan(server: &TestServer, token: &str) -> Value {
    let before = scan_status(server, token).await["finishedAt"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for _ in 0..100 {
        let (status, body) = server.post("/api/library/scan", Some(token), None).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        if body["started"].as_bool().unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    wait_for_scan_after(server, token, &before).await
}

async fn trigger_root_scan(server: &TestServer, token: &str, root_id: i64) -> Value {
    let before = scan_status(server, token).await["finishedAt"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for _ in 0..100 {
        let (status, body) = server
            .post(
                &format!("/api/library/roots/{root_id}/scan"),
                Some(token),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        assert_eq!(body["rootId"], root_id);
        if body["started"].as_bool().unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    wait_for_scan_after(server, token, &before).await
}

async fn roots(server: &TestServer, token: &str) -> Vec<Value> {
    let (status, body) = server.get("/api/library/roots", Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn roots_are_admin_only_and_root_one_is_backfilled() {
    let server = TestServer::start().await;
    let token = server.setup().await;
    wait_for_scan(&server, &token).await;

    let (status, body) = server.get("/api/library/roots", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let roots = roots(&server, &token).await;
    assert_eq!(roots.len(), 1, "{roots:?}");
    assert_eq!(
        roots[0]["path"].as_str(),
        Some(server.games_dir.display().to_string().as_str())
    );
    assert_eq!(roots[0]["enabled"], true);
    assert_eq!(roots[0]["available"], true);
    assert_eq!(roots[0]["versionCount"], 0);
}

#[tokio::test]
async fn roots_can_be_added_scanned_updated_and_removed() {
    let server = TestServer::start().await;
    let token = server.setup().await;
    wait_for_scan(&server, &token).await;

    let extra = server.temp.path().join("retro");
    std::fs::create_dir_all(&extra).expect("mkdir");
    let (status, created) = server
        .post(
            "/api/library/roots",
            Some(&token),
            Some(json!({ "path": extra.display().to_string(), "label": "Retro" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let root_id = created["id"].as_i64().expect("id");
    assert_eq!(created["label"], "Retro");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["versionCount"], 0);

    // Duplicate path and relative paths are rejected.
    let (status, body) = server
        .post(
            "/api/library/roots",
            Some(&token),
            Some(json!({ "path": extra.display().to_string() })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "root_exists");
    let (status, body) = server
        .post(
            "/api/library/roots",
            Some(&token),
            Some(json!({ "path": "relative/games" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_root_path");

    server.write_archive(&extra, "Celeste (2018).zip", 3);
    trigger_root_scan(&server, &token, root_id).await;

    let after = roots(&server, &token).await;
    let root = after
        .iter()
        .find(|root| root["id"].as_i64() == Some(root_id))
        .expect("root");
    assert_eq!(root["versionCount"], 1, "{root}");
    assert!(root["lastScanAt"].as_str().is_some(), "{root}");

    let (_, games) = server.get("/api/games", Some(&token)).await;
    let titles: Vec<&str> = games["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|game| game["title"].as_str())
        .collect();
    assert!(
        titles.iter().any(|title| title.contains("Celeste")),
        "{titles:?}"
    );

    let (status, updated) = server
        .patch(
            &format!("/api/library/roots/{root_id}"),
            &token,
            json!({ "label": "Retro archive", "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["label"], "Retro archive");
    assert_eq!(updated["enabled"], false);

    // A non-empty root requires an explicit force.
    let (status, body) = server
        .delete(&format!("/api/library/roots/{root_id}"), &token)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "root_not_empty");

    let (status, body) = server
        .delete(&format!("/api/library/roots/{root_id}?force=true"), &token)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let after = roots(&server, &token).await;
    assert!(after
        .iter()
        .all(|root| root["id"].as_i64() != Some(root_id)));

    // Removing a root keeps the game rows.
    let (_, games) = server.get("/api/games", Some(&token)).await;
    assert_eq!(games["total"], 1, "{games}");
}

#[tokio::test]
async fn mirrored_archives_are_indexed_once() {
    let server = TestServer::start().await;
    let token = server.setup().await;
    wait_for_scan(&server, &token).await;

    let mirror = server.temp.path().join("mirror");
    std::fs::create_dir_all(&mirror).expect("mkdir");
    let (status, created) = server
        .post(
            "/api/library/roots",
            Some(&token),
            Some(json!({ "path": mirror.display().to_string(), "label": "Mirror" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // Same name, same bytes: the second copy is a mirror and must not create a
    // second version.
    server.write_archive(&server.games_dir, "Halo (2001).zip", 9);
    server.write_archive(&mirror, "Halo (2001).zip", 9);
    trigger_full_scan(&server, &token).await;

    let (_, games) = server.get("/api/games", Some(&token)).await;
    assert_eq!(games["total"], 1, "{games}");
    let game_id = games["items"][0]["id"].as_i64().expect("id");
    let (status, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(
        detail["versions"].as_array().map(Vec::len),
        Some(1),
        "{detail}"
    );
}

#[tokio::test]
async fn missing_root_keeps_games_and_reports_the_root_unavailable() {
    let server = TestServer::start().await;
    let token = server.setup().await;
    wait_for_scan(&server, &token).await;

    let external = server.temp.path().join("usb-disk");
    std::fs::create_dir_all(&external).expect("mkdir");
    let (status, created) = server
        .post(
            "/api/library/roots",
            Some(&token),
            Some(json!({ "path": external.display().to_string(), "label": "USB" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let root_id = created["id"].as_i64().expect("id");
    server.write_archive(&external, "Chrono Trigger (1995).zip", 5);
    trigger_root_scan(&server, &token, root_id).await;

    // Simulate an unplugged disk.
    std::fs::rename(&external, server.temp.path().join("usb-disk-unmounted")).expect("rename");
    trigger_full_scan(&server, &token).await;

    let after = roots(&server, &token).await;
    let root = after
        .iter()
        .find(|root| root["id"].as_i64() == Some(root_id))
        .expect("root");
    assert_eq!(root["available"], false, "{root}");
    assert!(root["lastError"].as_str().is_some(), "{root}");

    // The game and its version are preserved: the disk is gone, not the game.
    let (_, games) = server.get("/api/games", Some(&token)).await;
    assert_eq!(games["total"], 1, "{games}");
    let game_id = games["items"][0]["id"].as_i64().expect("id");
    let (_, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    assert_eq!(
        detail["versions"].as_array().map(Vec::len),
        Some(1),
        "{detail}"
    );

    // Downloads surface a dedicated error instead of "file not found".
    let version_id = detail["versions"][0]["id"].as_i64().expect("version");
    let (status, body) = server
        .get(
            &format!("/api/games/{game_id}/download?versionId={version_id}"),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "root_unavailable");
}

#[tokio::test]
async fn watcher_status_is_reported_when_disabled() {
    let server = TestServer::start().await;
    let token = server.setup().await;
    wait_for_scan(&server, &token).await;

    let (status, body) = server.get("/api/library/watcher", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    assert!(body["roots"].as_array().expect("roots").is_empty());
    assert_eq!(body["pending"], false);

    let (status, _) = server.get("/api/library/watcher", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn integrity_check_runs_and_reports_findings() {
    let server = TestServer::start().await;
    let token = server.setup().await;
    wait_for_scan(&server, &token).await;

    let archive = server.write_archive(&server.games_dir, "Celeste (2018).zip", 1);
    trigger_full_scan(&server, &token).await;

    let (status, created) = server
        .post(
            "/api/integrity/check",
            Some(&token),
            Some(json!({ "scope": "archives" })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let report_id = created["reportId"].as_i64().expect("reportId");
    assert_eq!(created["status"], "running");

    let report = wait_for_report(&server, &token, report_id).await;
    assert_eq!(report["report"]["status"], "completed", "{report}");
    assert!(report["report"]["checked"].as_i64().unwrap_or(0) >= 1);
    assert_eq!(report["report"]["findingsCount"], 0, "{report}");
    assert!(report["findings"].as_array().expect("findings").is_empty());

    // Truncating the archive keeps its indexed size: the next run reports it.
    std::fs::write(&archive, vec![1_u8; 100]).expect("truncate");
    let (status, created) = server
        .post("/api/integrity/check", Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let report_id = created["reportId"].as_i64().expect("reportId");
    let report = wait_for_report(&server, &token, report_id).await;
    assert_eq!(report["report"]["findingsCount"], 1, "{report}");
    assert_eq!(report["findings"][0]["kind"], "size_mismatch", "{report}");
    assert_eq!(report["findings"][0]["targetType"], "archive");

    let (status, list) = server
        .get("/api/integrity/reports?limit=5", Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(!list.as_array().expect("reports").is_empty());
    let (status, _) = server
        .get("/api/integrity/reports/999999", Some(&token))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

async fn wait_for_report(server: &TestServer, token: &str, report_id: i64) -> Value {
    for _ in 0..400 {
        let (status, body) = server
            .get(&format!("/api/integrity/reports/{report_id}"), Some(token))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["report"]["status"].as_str() != Some("running") {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("integrity report did not finish");
}

#[tokio::test]
async fn media_gc_dry_run_then_deletes_orphans() {
    let server = TestServer::start().await;
    let token = server.setup().await;
    wait_for_scan(&server, &token).await;

    let orphan = server.media_dir.join("orphan.jpg");
    std::fs::write(&orphan, vec![0_u8; 256]).expect("write orphan");
    age_file(&orphan, Duration::from_secs(48 * 3_600));

    let (status, dry) = server.post("/api/media/gc", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{dry}");
    assert_eq!(dry["dryRun"], true);
    assert_eq!(dry["orphanFiles"], 1, "{dry}");
    assert_eq!(dry["deletedFiles"], 0, "{dry}");
    assert!(orphan.exists(), "dry run must not delete");

    let (status, real) = server
        .post("/api/media/gc?dryRun=false", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{real}");
    assert_eq!(real["deletedFiles"], 1, "{real}");
    assert_eq!(real["deletedBytes"], 256, "{real}");
    assert!(!orphan.exists(), "orphan should be deleted");

    let (status, _) = server.post("/api/media/gc", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

fn age_file(path: &Path, age: Duration) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open");
    file.set_modified(SystemTime::now() - age)
        .expect("set modification time");
}
