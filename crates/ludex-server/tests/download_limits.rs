//! Job-scoped download admission (#217).
//!
//! A single-stream client keeps the #195 reconnect budget; a client that
//! fetches several ranges of one archive in parallel names its job and gets a
//! budget sized for N parts. These tests drive the real HTTP route.

mod common;

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use common::TestServer;
use ludex_server::download_limits::{
    IP_PARTS_MAX, JOB_PARTS_MAX, JOB_REQUESTS_PER_MINUTE, LEGACY_REQUESTS_PER_MINUTE,
};
use reqwest::header::{RANGE, RETRY_AFTER};
use reqwest::{Response, StatusCode};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context as LayerContext, Layer};
use tracing_subscriber::prelude::*;

/// Two full jobs must fit under the address cap so a legitimate N=8 client and
/// a concurrent second job do not collide.
const _: () = assert!(IP_PARTS_MAX >= JOB_PARTS_MAX * 2);

/// Start a server with one 2 MiB archive and return `(server, token, game,
/// version)`.
async fn fixture() -> (TestServer, String, i64, i64) {
    let server = TestServer::start().await;
    let (token, _) = server.setup().await;
    let game_id = server
        .add_game(&token, "Hades (2020).zip", 2 * 1024 * 1024)
        .await;
    let (status, detail) = server
        .get(&format!("/api/games/{game_id}"), Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let version_id = detail["versions"][0]["id"].as_i64().expect("version id");
    (server, token, game_id, version_id)
}

fn download_url(server: &TestServer, game_id: i64, version_id: i64) -> String {
    format!(
        "{}/api/games/{game_id}/download?versionId={version_id}",
        server.url
    )
}

/// One ranged download, optionally inside a job (#217).
async fn part(
    server: &TestServer,
    token: &str,
    game_id: i64,
    version_id: i64,
    job: Option<&str>,
    part_index: Option<u32>,
    range: &str,
) -> Response {
    let mut request = server
        .client
        .get(download_url(server, game_id, version_id))
        .bearer_auth(token)
        .header(RANGE, range);
    if let Some(job) = job {
        request = request.header("X-Ludex-Job-Id", job);
    }
    if let Some(index) = part_index {
        request = request.query(&[("partIndex", index)]);
    }
    request.send().await.expect("download request")
}

#[tokio::test]
async fn eight_part_client_is_not_rate_limited() {
    let (server, token, game_id, version_id) = fixture().await;

    // The capability is public, so a client can size its parts before it has a
    // session.
    let (status, config) = server.get("/api/config", None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(
        config["parallelPartsMax"].as_u64(),
        Some(JOB_PARTS_MAX as u64),
        "{config}"
    );

    // Eight simultaneous parts, each 64 KiB: the advertised N exactly fits the
    // concurrent-part cap and stays far below the request budget.
    let job = "01930f2e-7d2a-7c3b-9f11-2f0d4f5c6a70";
    let client = server.client.clone();
    let url = download_url(&server, game_id, version_id);
    let bearer = token.clone();
    let concurrent_job = job.to_string();
    let requests = (0..JOB_PARTS_MAX).map(|index| {
        let client = client.clone();
        let url = url.clone();
        let bearer = bearer.clone();
        let job = concurrent_job.clone();
        async move {
            let start = index as u64 * 64 * 1024;
            let range = format!("bytes={start}-{}", start + 64 * 1024 - 1);
            let response = client
                .get(url)
                .bearer_auth(bearer)
                .header(RANGE, range)
                .header("X-Ludex-Job-Id", job)
                .query(&[("partIndex", index as u32)])
                .send()
                .await
                .expect("part request");
            let status = response.status();
            let body = response.bytes().await.expect("part body");
            (status, body.len())
        }
    });
    for (status, length) in futures_util::future::join_all(requests).await {
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(length, 64 * 1024);
    }

    // Retries and Range resumes inside the same job still fit the budget.
    for _ in 0..32 {
        let response = part(
            &server,
            &token,
            game_id,
            version_id,
            Some(job),
            None,
            "bytes=0-0",
        )
        .await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let _ = response.bytes().await.expect("retry body");
    }

    // An invalid job id or part index is a validation error, not a 429.
    let response = part(
        &server,
        &token,
        game_id,
        version_id,
        Some("bad/job"),
        None,
        "bytes=0-0",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.expect("error body");
    assert_eq!(body["error"]["code"], "invalid_job_id");

    let response = part(
        &server,
        &token,
        game_id,
        version_id,
        Some(job),
        Some(999),
        "bytes=0-0",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.expect("error body");
    assert_eq!(body["error"]["code"], "invalid_part_index");

    server.stop().await;
}

#[tokio::test]
async fn a_hot_job_is_limited_with_retry_after_and_other_jobs_are_isolated() {
    let (server, token, game_id, version_id) = fixture().await;

    let mut limited_at = None;
    let mut retry_after = None;
    for attempt in 1..=JOB_REQUESTS_PER_MINUTE + 50 {
        let response = part(
            &server,
            &token,
            game_id,
            version_id,
            Some("hot-job"),
            None,
            "bytes=0-0",
        )
        .await;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            retry_after = response.headers().get(RETRY_AFTER).cloned();
            let body: serde_json::Value = response.json().await.expect("limit body");
            assert_eq!(
                body["error"]["code"], "too_many_download_attempts",
                "{body}"
            );
            limited_at = Some(attempt);
            break;
        }
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let _ = response.bytes().await.expect("part body");
    }

    let limited_at = limited_at.expect("hot loop must be rate limited");
    assert_eq!(
        limited_at,
        JOB_REQUESTS_PER_MINUTE + 1,
        "the budget must admit exactly {JOB_REQUESTS_PER_MINUTE} parts per minute"
    );
    let retry_after = retry_after.expect("Retry-After header");
    let seconds = retry_after
        .to_str()
        .expect("ascii")
        .parse::<u64>()
        .expect("seconds");
    assert!((1..=60).contains(&seconds), "Retry-After: {seconds}");

    // Inventing another job id does not escape the address ceiling, but a
    // second legitimate job gets its own budget.
    let response = part(
        &server,
        &token,
        game_id,
        version_id,
        Some("second-job"),
        None,
        "bytes=0-0",
    )
    .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let _ = response.bytes().await.expect("body");

    server.stop().await;
}

#[tokio::test]
async fn single_stream_reconnect_budget_is_unchanged() {
    let (server, token, game_id, version_id) = fixture().await;

    for _ in 0..LEGACY_REQUESTS_PER_MINUTE {
        let response = part(
            &server,
            &token,
            game_id,
            version_id,
            None,
            None,
            "bytes=0-0",
        )
        .await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let _ = response.bytes().await.expect("body");
    }

    let response = part(
        &server,
        &token,
        game_id,
        version_id,
        None,
        None,
        "bytes=0-0",
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        response.headers().get(RETRY_AFTER).is_some(),
        "429 must carry Retry-After"
    );
    let body: serde_json::Value = response.json().await.expect("limit body");
    assert_eq!(body["error"]["code"], "too_many_download_attempts");

    server.stop().await;
}

/* ----------------------------- stream logging (#217) ---------------------- */

#[derive(Clone, Debug, Default)]
struct Captured {
    message: String,
    fields: Vec<(String, String)>,
}

impl Captured {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }
}

#[derive(Clone)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<Captured>>>,
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _context: LayerContext<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(Captured {
            message: visitor.message,
            fields: visitor.fields,
        });
    }
}

static CAPTURED: OnceLock<Arc<Mutex<Vec<Captured>>>> = OnceLock::new();

fn captured() -> Arc<Mutex<Vec<Captured>>> {
    CAPTURED
        .get_or_init(|| {
            let events = Arc::new(Mutex::new(Vec::new()));
            let layer = CaptureLayer {
                events: Arc::clone(&events),
            };
            let _ = tracing_subscriber::registry()
                .with(tracing_subscriber::filter::LevelFilter::INFO)
                .with(layer)
                .try_init();
            events
        })
        .clone()
}

#[tokio::test]
async fn download_stream_logs_job_part_bytes_and_range() {
    let events = captured();
    let (server, token, game_id, version_id) = fixture().await;
    let job_id = "01930f2e-7d2a-7c3b-9f11-2f0d4f5c6a71";

    let response = part(
        &server,
        &token,
        game_id,
        version_id,
        Some(job_id),
        Some(3),
        "bytes=100-199",
    )
    .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let body = response.bytes().await.expect("body");
    assert_eq!(body.len(), 100);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let logged = loop {
        let found = events
            .lock()
            .unwrap()
            .iter()
            .find(|event| {
                event.message.contains("download stream closed")
                    && event
                        .field("job")
                        .is_some_and(|field| field.contains(job_id))
            })
            .cloned();
        if let Some(event) = found {
            break event;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no download stream log for job {job_id}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert_eq!(logged.field("part"), Some("Some(3)"));
    assert_eq!(logged.field("range_start"), Some("100"));
    assert_eq!(logged.field("range_end"), Some("Some(199)"));
    assert_eq!(logged.field("bytes"), Some("100"));
    // The client read every byte, so this is a completed transfer, not an
    // abort: hyper stops polling the body after the last byte (#211, 7.4).
    assert_eq!(
        logged.field("outcome"),
        Some("\"complete\""),
        "a fully delivered range must not be logged as client_abort"
    );
    assert!(!logged.fields.iter().any(|(name, _)| name.contains("token")));

    server.stop().await;
}

#[tokio::test]
async fn a_full_archive_download_is_logged_complete() {
    let events = captured();
    let (server, token, game_id, version_id) = fixture().await;

    let response = server
        .client
        .get(download_url(&server, game_id, version_id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("download");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.bytes().await.expect("body");
    assert_eq!(body.len(), 2 * 1024 * 1024);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let logged = loop {
        let found = events
            .lock()
            .unwrap()
            .iter()
            .find(|event| {
                event.message.contains("download stream closed")
                    && event.field("version") == Some(&format!("{}", version_id))
                    && event.field("status") == Some("200")
            })
            .cloned();
        if let Some(event) = found {
            break event;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no complete download stream log for version {version_id}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert_eq!(logged.field("range_start"), Some("0"));
    assert_eq!(logged.field("range_end"), Some("Some(2097151)"));
    assert_eq!(logged.field("bytes"), Some("2097152"));
    assert_eq!(logged.field("outcome"), Some("\"complete\""));

    server.stop().await;
}
