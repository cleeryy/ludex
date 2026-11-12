//! Outbound webhook delivery.
//!
//! Events are queued as `webhook_deliveries` rows and delivered by a
//! background worker. Each request is signed with HMAC-SHA256 over the exact
//! stored payload, retried up to [`MAX_ATTEMPTS`] times with exponential
//! backoff, and guarded against SSRF: loopback/private/link-local destinations
//! are rejected unless `LUDEX_WEBHOOKS_ALLOW_PRIVATE=true` (tests).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use chrono::Utc;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::Sha256;
use url::Url;

use crate::audit::actions;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::state::AppState;

/// Event names a webhook can subscribe to.
pub const EVENTS: &[&str] = &[
    actions::GAME_INGESTED,
    actions::GAME_DELETED,
    actions::DOWNLOAD_COMPLETED,
    actions::USER_CREATED,
    actions::SAVE_UPLOADED,
    actions::ACHIEVEMENT_UNLOCKED,
    actions::FRIEND_REQUESTED,
    actions::FRIEND_ACCEPTED,
    actions::FRIEND_REMOVED,
];

/// Event name used by the test-delivery endpoint.
pub const TEST_EVENT: &str = "webhook.test";

/// Maximum delivery attempts before a delivery is abandoned.
pub const MAX_ATTEMPTS: i64 = 5;

/// Per-request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Deliveries picked up per worker pass.
const DELIVERY_BATCH: i64 = 20;
/// Idle polling interval; a queued delivery wakes the worker immediately.
const IDLE_POLL: Duration = Duration::from_millis(500);
/// Upper bound for the exponential backoff.
const MAX_BACKOFF_MS: i64 = 60 * 60 * 1000;

/// Whether an action name is subscribable through webhooks.
#[must_use]
pub fn is_webhook_event(action: &str) -> bool {
    EVENTS.contains(&action)
}

/// Computes the `X-Ludex-Signature` header value for a payload.
#[must_use]
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Generates a new random 32-byte HMAC secret, hex encoded.
#[must_use]
pub fn generate_secret() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Validates a webhook URL and resolves it, rejecting non-public targets.
///
/// Resolution happens here and again before every attempt so a DNS record
/// changed after creation cannot silently point at the metadata service.
pub async fn resolve_target(
    raw: &str,
    allow_private: bool,
) -> Result<(Url, Vec<SocketAddr>), String> {
    let url = Url::parse(raw).map_err(|error| format!("invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("webhook URLs must use http or https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("webhook URLs must not contain credentials".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "webhook URL must include a host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("could not resolve {host}: {error}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("could not resolve {host}"));
    }
    if !allow_private && addrs.iter().any(|addr| is_private_ip(addr.ip())) {
        return Err("webhook targets must be public addresses".to_string());
    }
    Ok((url, addrs))
}

/// Whether an address is loopback, private, link-local or otherwise local.
#[must_use]
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_private_ipv4(ip),
        IpAddr::V6(ip) => is_private_ipv6(ip),
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.to_ipv4_mapped().is_some_and(is_private_ipv4)
}

/// Builds the signed payload body for one event.
#[must_use]
pub fn payload_for(event: &Event) -> String {
    json!({
        "event": event.action,
        "timestamp": db::db_time(Utc::now()),
        "data": event.metadata,
    })
    .to_string()
}

/// Queues a delivery for every enabled webhook subscribed to the event.
pub async fn dispatch(state: &AppState, event: &Event) {
    let hooks =
        match db::query_as::<(i64, String)>("SELECT id, events FROM webhooks WHERE enabled = 1")
            .fetch_all(&state.pool)
            .await
        {
            Ok(hooks) => hooks,
            Err(error) => {
                tracing::error!(error = %error, event = %event.action, "could not load webhooks");
                return;
            }
        };
    if hooks.is_empty() {
        return;
    }

    let payload = payload_for(event);
    let now = db::db_time(db::now());
    let mut queued = false;
    for (webhook_id, events) in hooks {
        let subscribed = db::parse_json_list(&events);
        if !subscribed.iter().any(|name| name == &event.action) {
            continue;
        }
        let result = db::query(
            "INSERT INTO webhook_deliveries
                (webhook_id, event, payload, status_code, attempts, next_attempt_at, delivered_at, last_error)
             VALUES (?, ?, ?, NULL, 0, ?, NULL, NULL)",
        )
        .bind(webhook_id)
        .bind(&event.action)
        .bind(&payload)
        .bind(&now)
        .execute(&state.pool)
        .await;
        match result {
            Ok(_) => queued = true,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    webhook = webhook_id,
                    event = %event.action,
                    "could not queue webhook delivery"
                );
            }
        }
    }
    if queued {
        state.webhooks_notify.notify_one();
    }
}

/// Queues a one-off test delivery for a webhook.
pub async fn queue_test(state: &AppState, webhook_id: i64) -> Result<i64, ApiError> {
    let exists =
        db::query_scalar::<i64>("SELECT COUNT(*) FROM webhooks WHERE id = ? AND enabled = 1")
            .bind(webhook_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from)?;
    if exists == 0 {
        return Err(ApiError::not_found(
            "webhook_not_found",
            "Webhook not found or disabled",
        ));
    }
    let payload = json!({
        "event": TEST_EVENT,
        "timestamp": db::db_time(db::now()),
        "data": { "message": "Ludex webhook test delivery" },
    })
    .to_string();
    let delivery_id = db::query(
        "INSERT INTO webhook_deliveries
            (webhook_id, event, payload, status_code, attempts, next_attempt_at, delivered_at, last_error)
         VALUES (?, ?, ?, NULL, 0, ?, NULL, NULL)",
    )
    .bind(webhook_id)
    .bind(TEST_EVENT)
    .bind(&payload)
    .bind(db::db_time(db::now()))
    .returning_id(&state.pool)
    .await
    .map_err(ApiError::from)?;
    state.webhooks_notify.notify_one();
    Ok(delivery_id)
}

/// Background delivery worker: waits for a wake-up, then drains due rows.
pub async fn worker(state: AppState) {
    loop {
        tokio::select! {
            () = state.webhooks_notify.notified() => {}
            () = tokio::time::sleep(IDLE_POLL) => {}
        }
        process_due(&state).await;
    }
}

#[derive(Debug, sqlx::FromRow)]
struct DueDelivery {
    id: i64,
    webhook_id: i64,
    event: String,
    payload: String,
    attempts: i64,
    url: String,
    secret: String,
}

async fn process_due(state: &AppState) {
    loop {
        let rows = match db::query_as::<DueDelivery>(
            "SELECT d.id, d.webhook_id, d.event, d.payload, d.attempts, w.url, w.secret
             FROM webhook_deliveries d
             JOIN webhooks w ON w.id = d.webhook_id
             WHERE d.delivered_at IS NULL
               AND d.next_attempt_at IS NOT NULL
               AND d.next_attempt_at <= ?
               AND w.enabled = 1
             ORDER BY d.next_attempt_at, d.id
             LIMIT ?",
        )
        .bind(db::db_time(db::now()))
        .bind(DELIVERY_BATCH)
        .fetch_all(&state.pool)
        .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(error = %error, "could not load due webhook deliveries");
                return;
            }
        };
        let count = rows.len();
        for row in rows {
            deliver_one(state, row).await;
        }
        if count < usize::try_from(DELIVERY_BATCH).unwrap_or(20) {
            break;
        }
    }
}

async fn deliver_one(state: &AppState, delivery: DueDelivery) {
    let attempts = delivery.attempts + 1;
    let (outcome, status_code) = send(state, &delivery).await;
    let now = Utc::now();

    let (next_attempt, error) = match outcome {
        Ok(()) => (None, None),
        Err(message) => {
            let next = if attempts >= MAX_ATTEMPTS {
                None
            } else {
                Some(db::db_time(
                    now + chrono::Duration::milliseconds(backoff_ms(
                        state.config.webhooks_backoff_ms,
                        attempts,
                    )),
                ))
            };
            (next, Some(message))
        }
    };

    let delivered = if error.is_none() {
        Some(db::db_time(now))
    } else {
        None
    };
    let result = db::query(
        "UPDATE webhook_deliveries SET
            status_code = ?, attempts = ?, next_attempt_at = ?, delivered_at = ?, last_error = ?
         WHERE id = ?",
    )
    .bind(status_code)
    .bind(attempts)
    .bind(next_attempt)
    .bind(delivered)
    .bind(&error)
    .bind(delivery.id)
    .execute(&state.pool)
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, delivery = delivery.id, "could not update webhook delivery");
        return;
    }

    if let Some(error) = error {
        tracing::warn!(
            delivery = delivery.id,
            webhook = delivery.webhook_id,
            event = %delivery.event,
            attempts,
            %error,
            "webhook delivery failed"
        );
    } else {
        tracing::info!(
            delivery = delivery.id,
            webhook = delivery.webhook_id,
            event = %delivery.event,
            attempts,
            "webhook delivered"
        );
    }
}

/// Sends one request. `Ok(())` means a 2xx response; the second value is the
/// HTTP status when a response was received.
async fn send(state: &AppState, delivery: &DueDelivery) -> (Result<(), String>, Option<i64>) {
    let (url, addrs) =
        match resolve_target(&delivery.url, state.config.webhooks_allow_private).await {
            Ok(target) => target,
            Err(error) => return (Err(error), None),
        };
    let Some(host) = url.host_str() else {
        return (Err("webhook URL has no host".to_string()), None);
    };
    // Prefer IPv4: it is reachable on more deployments than IPv6.
    let target = addrs
        .iter()
        .find(|address| address.is_ipv4())
        .unwrap_or(&addrs[0]);
    // Pin the connection to the address that was just validated so a second
    // DNS answer cannot redirect the request into a private network.
    let client = match reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Ludex/", env!("CARGO_PKG_VERSION")))
        .resolve(host, *target)
        .build()
    {
        Ok(client) => client,
        Err(error) => return (Err(format!("could not build HTTP client: {error}")), None),
    };

    let signature = sign(&delivery.secret, delivery.payload.as_bytes());
    let request = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-ludex-signature", signature)
        .header("x-ludex-event", &delivery.event)
        .header("x-ludex-delivery", delivery.id.to_string())
        .body(delivery.payload.clone());

    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let code = i64::from(status.as_u16());
            if status.is_success() {
                (Ok(()), Some(code))
            } else {
                let body = response.text().await.unwrap_or_default();
                let body = body.chars().take(200).collect::<String>();
                (Err(format!("HTTP {code}: {body}")), Some(code))
            }
        }
        Err(error) => (Err(format!("request failed: {error}")), None),
    }
}

/// Exponential backoff for attempt `attempts` (1-based), in milliseconds.
#[must_use]
pub fn backoff_ms(base_ms: u64, attempts: i64) -> i64 {
    let base = i64::try_from(base_ms).unwrap_or(1_000).max(1);
    let exponent = u32::try_from(attempts.saturating_sub(1))
        .unwrap_or(0)
        .min(20);
    base.saturating_mul(1_i64 << exponent).min(MAX_BACKOFF_MS)
}

/// Parses the `events` column of a webhook row.
#[must_use]
pub fn parse_events(raw: &str) -> Vec<String> {
    db::parse_json_list(raw)
        .into_iter()
        .filter(|name| EVENTS.contains(&name.as_str()) || name == TEST_EVENT)
        .collect()
}

/// Serialises a validated event list for storage.
#[must_use]
pub fn events_json(events: &[String]) -> String {
    db::dump_json_list(events)
}

/// Validates a requested subscription list.
pub fn validate_events(events: &[String]) -> Result<Vec<String>, ApiError> {
    if events.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_events",
            "At least one webhook event is required",
        ));
    }
    let mut cleaned = Vec::new();
    for event in events {
        let event = event.trim();
        if !EVENTS.contains(&event) {
            return Err(ApiError::bad_request(
                "invalid_events",
                format!("Unknown webhook event: {event}"),
            ));
        }
        if !cleaned.iter().any(|existing| existing == event) {
            cleaned.push(event.to_string());
        }
    }
    Ok(cleaned)
}

/// Validates a webhook URL at creation/update time.
pub async fn validate_url(raw: &str, allow_private: bool) -> Result<String, ApiError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_url",
            "Webhook URL is required",
        ));
    }
    if raw.len() > 2048 {
        return Err(ApiError::bad_request(
            "invalid_url",
            "Webhook URL is limited to 2048 characters",
        ));
    }
    resolve_target(raw, allow_private)
        .await
        .map(|_| raw.to_string())
        .map_err(|error| ApiError::bad_request("invalid_url", error))
}

/// Reads the JSON value of a delivery payload for tests and logs.
#[must_use]
pub fn payload_value(payload: &str) -> Value {
    serde_json::from_str(payload).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_stable_and_key_dependent() {
        let body = br#"{"event":"game.ingested","data":{}}"#;
        let first = sign("secret-a", body);
        let second = sign("secret-a", body);
        let other = sign("secret-b", body);
        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(first.starts_with("sha256="));
        assert_eq!(first.len(), "sha256=".len() + 64);
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_ms(1_000, 1), 1_000);
        assert_eq!(backoff_ms(1_000, 2), 2_000);
        assert_eq!(backoff_ms(1_000, 3), 4_000);
        assert_eq!(backoff_ms(1_000, 5), 16_000);
        assert_eq!(backoff_ms(1_000, 40), MAX_BACKOFF_MS);
        assert_eq!(backoff_ms(0, 1), 1);
    }

    #[test]
    fn private_addresses_are_detected() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.1.1",
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            let ip: IpAddr = address.parse().expect("ip");
            assert!(is_private_ip(ip), "{address}");
        }
        for address in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            let ip: IpAddr = address.parse().expect("ip");
            assert!(!is_private_ip(ip), "{address}");
        }
    }

    #[test]
    fn event_validation_rejects_unknown_names() {
        assert!(validate_events(&[]).is_err());
        assert!(validate_events(&["nope".to_string()]).is_err());
        let events = validate_events(&[
            "game.ingested".to_string(),
            "game.ingested".to_string(),
            "user.created".to_string(),
        ])
        .expect("events");
        assert_eq!(events, vec!["game.ingested", "user.created"]);
    }

    #[tokio::test]
    async fn url_validation_rejects_private_targets_by_default() {
        assert!(resolve_target("http://127.0.0.1:9/hook", false)
            .await
            .is_err());
        assert!(resolve_target("http://127.0.0.1:9/hook", true)
            .await
            .is_ok());
        assert!(resolve_target("ftp://example.com/hook", true)
            .await
            .is_err());
        assert!(resolve_target("https://user:pass@example.com/hook", true)
            .await
            .is_err());
    }
}
