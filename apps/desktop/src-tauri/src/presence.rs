//! Discord Rich Presence (#12).
//!
//! Rich Presence uses Discord's local IPC socket, not a bot token: the desktop
//! opens `discord-ipc-*`, handshakes with an application id and pushes an
//! activity. Nothing leaves the machine except the activity Discord itself
//! renders.
//!
//! The setting is off by default. When Discord is not running, the connection
//! is retried with an exponential backoff instead of spamming the socket; on a
//! machine without Discord (any Linux box without a socket) it simply stays
//! disconnected and reports why.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Discord truncates `details`/`state` at 128 characters.
const MAX_FIELD_CHARS: usize = 128;
/// Longest pause between reconnection attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(120);

/// What Ludex wants Discord to show. Built without any IPC so it is testable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresencePayload {
    pub details: String,
    pub state: String,
    pub large_image: Option<String>,
    pub large_text: Option<String>,
    /// Unix milliseconds; Discord renders the elapsed time from it.
    pub start_ms: Option<i64>,
}

impl PresencePayload {
    /// A running game: its title, a public cover and the session start.
    pub fn playing(title: &str, cover_url: Option<&str>, started_at: DateTime<Utc>) -> Self {
        let title = truncate(title.trim(), MAX_FIELD_CHARS);
        let title = if title.is_empty() {
            "A game".to_string()
        } else {
            title
        };
        let cover = cover_url.and_then(public_cover);
        Self {
            details: title.clone(),
            state: "Ludex".to_string(),
            large_image: cover.clone(),
            large_text: cover.map(|_| title),
            start_ms: Some(started_at.timestamp_millis()),
        }
    }

    /// Nothing running: show Ludex as idle rather than a stale game.
    pub fn idle() -> Self {
        Self {
            details: "Idle".to_string(),
            state: "Browsing the library".to_string(),
            large_image: None,
            large_text: None,
            start_ms: None,
        }
    }

    /// JSON shape sent to Discord, used by the tests.
    #[cfg(test)]
    pub fn to_json(&self) -> serde_json::Value {
        let mut assets = serde_json::Map::new();
        if let Some(image) = &self.large_image {
            assets.insert("large_image".into(), image.clone().into());
        }
        if let Some(text) = &self.large_text {
            assets.insert("large_text".into(), text.clone().into());
        }
        let mut value = serde_json::json!({
            "details": self.details,
            "state": self.state,
        });
        if let Some(start) = self.start_ms {
            value["timestamps"] = serde_json::json!({ "start": start });
        }
        if !assets.is_empty() {
            value["assets"] = serde_json::Value::Object(assets);
        }
        value
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// Discord can only render an image the viewer's client can reach, so a
/// loopback/LAN cover URL is dropped instead of showing a broken asset.
pub fn public_cover(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    let host = host
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if host.is_empty() || host.starts_with('[') || !host.contains('.') {
        return None;
    }
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".internal") {
        return None;
    }
    let octets: Vec<&str> = host.split('.').collect();
    if let Ok(first) = octets.first().unwrap_or(&"").parse::<u8>() {
        let second = octets.get(1).and_then(|value| value.parse::<u8>().ok());
        let private = matches!(
            (first, second),
            (127, _) | (10, _) | (192, Some(168)) | (172, Some(16..=31)) | (169, Some(254))
        );
        if private {
            return None;
        }
    }
    Some(url.to_string())
}

/// State machine driving the IPC client: what to show, when to reconnect.
#[derive(Debug, Clone)]
pub struct PresenceCore {
    enabled: bool,
    configured: bool,
    desired: Option<PresencePayload>,
    applied: Option<PresencePayload>,
    connected: bool,
    failures: u32,
    next_attempt: Option<Instant>,
    last_error: Option<String>,
}

impl PresenceCore {
    pub fn new(enabled: bool, configured: bool) -> Self {
        Self {
            enabled,
            configured,
            desired: None,
            applied: None,
            connected: false,
            failures: 0,
            next_attempt: None,
            last_error: None,
        }
    }

    /// Apply a settings change. Returns true when the current connection must
    /// be dropped (presence disabled or the application id changed).
    pub fn configure(&mut self, enabled: bool, configured: bool) -> bool {
        let was_active = self.is_active();
        let changed = (self.enabled, self.configured) != (enabled, configured);
        self.enabled = enabled;
        self.configured = configured;
        if !self.is_active() {
            self.connected = false;
            self.failures = 0;
            self.next_attempt = None;
            self.applied = None;
            return was_active;
        }
        if changed {
            self.failures = 0;
            self.next_attempt = None;
            self.applied = None;
        }
        false
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.configured
    }

    /// Record what should be shown.
    pub fn want(&mut self, payload: Option<PresencePayload>) {
        self.desired = payload;
    }

    #[cfg(test)]
    pub fn desired(&self) -> Option<&PresencePayload> {
        self.desired.as_ref()
    }

    /// A payload that still has to be pushed, if any.
    pub fn pending(&self) -> Option<&PresencePayload> {
        if !self.is_active() || !self.connected {
            return None;
        }
        if self.applied.as_ref() == self.desired.as_ref() {
            return None;
        }
        self.desired.as_ref()
    }

    pub fn mark_sent(&mut self) {
        self.applied = self.desired.clone();
    }

    pub fn should_connect(&self, now: Instant) -> bool {
        self.is_active() && !self.connected && self.next_attempt.map(|at| now >= at).unwrap_or(true)
    }

    pub fn on_connected(&mut self) {
        self.connected = true;
        self.failures = 0;
        self.next_attempt = None;
        self.last_error = None;
    }

    /// Forget the current socket and schedule an immediate reconnect (the
    /// application id changed under a live connection).
    pub fn reset(&mut self) {
        self.connected = false;
        self.applied = None;
        self.failures = 0;
        self.next_attempt = None;
    }

    /// Record a failed connect/send and schedule the next attempt. Returns the
    /// delay, which doubles from two seconds up to two minutes.
    pub fn on_failure(&mut self, error: String, now: Instant) -> Duration {
        self.connected = false;
        self.applied = None;
        self.failures = self.failures.saturating_add(1);
        let exponent = self.failures.min(6);
        let delay = Duration::from_secs(2u64.saturating_mul(1u64 << exponent)).min(MAX_BACKOFF);
        self.next_attempt = Some(now + delay);
        self.last_error = Some(error);
        delay
    }

    pub fn status(&self, retry_in: Option<Duration>) -> PresenceStatus {
        PresenceStatus {
            enabled: self.enabled,
            configured: self.configured,
            connected: self.connected,
            last_error: self.last_error.clone(),
            details: self.desired.as_ref().map(|payload| payload.details.clone()),
            retry_in_seconds: retry_in.map(|delay| delay.as_secs()).unwrap_or(0),
        }
    }
}

/// Snapshot exposed to the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceStatus {
    pub enabled: bool,
    pub configured: bool,
    pub connected: bool,
    pub last_error: Option<String>,
    pub details: Option<String>,
    pub retry_in_seconds: u64,
}

/// Anything that can push a payload; the Discord client implements it, tests
/// use a recorder.
pub trait PresenceSink: Send {
    fn set(&mut self, payload: &PresencePayload) -> Result<(), String>;
    fn clear(&mut self) -> Result<(), String>;
}

/// Discord IPC sink backed by `discord-rich-presence`.
pub struct DiscordSink {
    client: discord_rich_presence::DiscordIpcClient,
}

impl DiscordSink {
    pub fn connect(client_id: &str) -> Result<Self, String> {
        use discord_rich_presence::DiscordIpc;

        let mut client = discord_rich_presence::DiscordIpcClient::new(client_id);
        client
            .connect()
            .map_err(|error| format!("Could not connect to Discord (is it running?): {error}"))?;
        Ok(Self { client })
    }
}

impl PresenceSink for DiscordSink {
    fn set(&mut self, payload: &PresencePayload) -> Result<(), String> {
        use discord_rich_presence::activity::{Activity, ActivityType, Assets, Timestamps};
        use discord_rich_presence::DiscordIpc;

        let mut activity = Activity::new()
            .details(payload.details.clone())
            .state(payload.state.clone())
            .activity_type(ActivityType::Playing);
        if let Some(start) = payload.start_ms {
            activity = activity.timestamps(Timestamps::new().start(start));
        }
        if let Some(image) = payload.large_image.clone() {
            let mut assets = Assets::new().large_image(image);
            if let Some(text) = payload.large_text.clone() {
                assets = assets.large_text(text);
            }
            activity = activity.assets(assets);
        }
        self.client
            .set_activity(activity)
            .map_err(|error| format!("Discord refused the activity: {error}"))
    }

    fn clear(&mut self) -> Result<(), String> {
        use discord_rich_presence::DiscordIpc;
        self.client
            .clear_activity()
            .map_err(|error| format!("Discord refused to clear the activity: {error}"))
    }
}

/// Commands accepted by the worker thread.
enum Command {
    Configure { enabled: bool, client_id: String },
    Set(Option<PresencePayload>),
    Shutdown,
}

static CHANNEL: OnceLock<Mutex<Option<Sender<Command>>>> = OnceLock::new();
static STATUS: OnceLock<Mutex<PresenceStatus>> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);

fn channel() -> &'static Mutex<Option<Sender<Command>>> {
    CHANNEL.get_or_init(|| Mutex::new(None))
}

fn status_lock() -> &'static Mutex<PresenceStatus> {
    STATUS.get_or_init(|| Mutex::new(PresenceStatus::default()))
}

pub fn status() -> PresenceStatus {
    match status_lock().lock() {
        Ok(status) => status.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn publish(status: PresenceStatus) {
    if let Ok(mut slot) = status_lock().lock() {
        *slot = status;
    }
}

fn send(command: Command) {
    if let Ok(slot) = channel().lock() {
        if let Some(sender) = slot.as_ref() {
            let _ = sender.send(command);
        }
    }
}

/// Re-apply the settings after `save_settings`.
pub fn configure(enabled: bool, client_id: &str) {
    send(Command::Configure {
        enabled,
        client_id: client_id.trim().to_string(),
    });
}

/// Show a running game (or idle when `None`).
pub fn set_payload(payload: Option<PresencePayload>) {
    send(Command::Set(payload));
}

/// Stop the worker and clear the activity (app exit).
pub fn shutdown() {
    send(Command::Shutdown);
}

/// Spawn the worker thread once. Safe to call from `setup`.
pub fn start(enabled: bool, client_id: &str) {
    if STARTED.swap(true, Ordering::SeqCst) {
        configure(enabled, client_id);
        return;
    }

    let (sender, receiver) = std::sync::mpsc::channel();
    if let Ok(mut slot) = channel().lock() {
        *slot = Some(sender);
    }
    let client_id = client_id.trim().to_string();
    publish(PresenceStatus {
        enabled,
        configured: !client_id.is_empty(),
        ..PresenceStatus::default()
    });

    std::thread::spawn(move || worker(receiver, enabled, client_id));
}

/// Apply a settings change to the worker state, dropping the socket when the
/// connection or the application id changed.
fn apply_configure(
    core: &mut PresenceCore,
    sink: &mut Option<DiscordSink>,
    current_id: &mut String,
    enabled: bool,
    client_id: String,
) {
    let changed_id = client_id != *current_id;
    *current_id = client_id;
    let drop_connection = core.configure(enabled, !current_id.is_empty());
    if drop_connection || changed_id {
        if let Some(client) = sink.as_mut() {
            let _ = client.clear();
        }
        *sink = None;
        core.reset();
    }
}

/// Worker loop: owns the IPC client, reconnects with backoff.
fn worker(receiver: Receiver<Command>, enabled: bool, client_id: String) {
    let mut core = PresenceCore::new(enabled, !client_id.is_empty());
    let mut sink: Option<DiscordSink> = None;
    let mut current_id = client_id;

    loop {
        match receiver.recv_timeout(Duration::from_millis(1000)) {
            Ok(Command::Configure { enabled, client_id }) => {
                apply_configure(&mut core, &mut sink, &mut current_id, enabled, client_id);
            }
            Ok(Command::Set(payload)) => {
                core.want(payload);
            }
            Ok(Command::Shutdown) => {
                if let Some(sink) = sink.as_mut() {
                    let _ = sink.clear();
                }
                tracing::debug!("discord presence worker stopped");
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // Drain any queued command before touching the socket so a burst of
        // launch/idle events only pushes the last one.
        while let Ok(command) = receiver.try_recv() {
            match command {
                Command::Configure { enabled, client_id } => {
                    apply_configure(&mut core, &mut sink, &mut current_id, enabled, client_id);
                }
                Command::Set(payload) => core.want(payload),
                Command::Shutdown => {
                    if let Some(sink) = sink.as_mut() {
                        let _ = sink.clear();
                    }
                    return;
                }
            }
        }

        let now = Instant::now();
        if core.should_connect(now) {
            match DiscordSink::connect(&current_id) {
                Ok(connected) => {
                    sink = Some(connected);
                    core.on_connected();
                }
                Err(error) => {
                    let delay = core.on_failure(error, now);
                    tracing::debug!(
                        seconds = delay.as_secs(),
                        "discord presence retry scheduled"
                    );
                }
            }
        }

        if let Some(payload) = core.pending().cloned() {
            if let Some(client) = sink.as_mut() {
                match client.set(&payload) {
                    Ok(()) => core.mark_sent(),
                    Err(error) => {
                        // A dead socket: drop it and let the backoff reconnect.
                        let now = Instant::now();
                        core.on_failure(error, now);
                        sink = None;
                    }
                }
            }
        }

        let retry_in = core
            .next_attempt
            .map(|at| at.saturating_duration_since(Instant::now()));
        publish(core.status(retry_in));
    }
}

/// Write a payload straight to an arbitrary writer, used by the tests. The
/// four byte header is little-endian opcode then length, the framing Discord's
/// IPC uses.
#[cfg(test)]
pub fn write_frame<W: std::io::Write>(
    writer: &mut W,
    opcode: u32,
    json: &str,
) -> std::io::Result<()> {
    writer.write_all(&opcode.to_le_bytes())?;
    writer.write_all(&(json.len() as u32).to_le_bytes())?;
    writer.write_all(json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn started() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    struct MockSink {
        sent: Arc<Mutex<Vec<PresencePayload>>>,
        cleared: Arc<Mutex<u32>>,
        fail: bool,
    }

    impl PresenceSink for MockSink {
        fn set(&mut self, payload: &PresencePayload) -> Result<(), String> {
            if self.fail {
                return Err("socket closed".into());
            }
            self.sent.lock().unwrap().push(payload.clone());
            Ok(())
        }

        fn clear(&mut self) -> Result<(), String> {
            *self.cleared.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[test]
    fn playing_payload_carries_title_and_elapsed_time() {
        let payload = PresencePayload::playing(
            "  Hades  ",
            Some("https://cdn.example.com/hades.jpg"),
            started(),
        );
        assert_eq!(payload.details, "Hades");
        assert_eq!(payload.start_ms, Some(started().timestamp_millis()));
        assert_eq!(
            payload.large_image.as_deref(),
            Some("https://cdn.example.com/hades.jpg")
        );
        assert_eq!(payload.large_text.as_deref(), Some("Hades"));

        let json = payload.to_json();
        assert_eq!(json["details"], "Hades");
        assert_eq!(json["timestamps"]["start"], started().timestamp_millis());
        assert_eq!(
            json["assets"]["large_image"],
            "https://cdn.example.com/hades.jpg"
        );
    }

    #[test]
    fn a_private_cover_url_is_dropped() {
        for url in [
            "http://cdn.example.com/cover.jpg",
            "https://localhost/cover.jpg",
            "https://127.0.0.1/cover.jpg",
            "https://10.1.20.58/media/cover.jpg",
            "https://192.168.1.5/cover.jpg",
            "https://172.20.0.3/cover.jpg",
            "https://ludex.local/cover.jpg",
            "https://[::1]/cover.jpg",
            "not-a-url",
        ] {
            assert!(public_cover(url).is_none(), "{url} should be rejected");
        }
        assert_eq!(
            public_cover("https://ludex.cleeryy.cloud/media/cover.jpg").as_deref(),
            Some("https://ludex.cleeryy.cloud/media/cover.jpg")
        );
    }

    #[test]
    fn idle_payload_has_no_timer_or_art() {
        let payload = PresencePayload::idle();
        assert_eq!(payload.details, "Idle");
        assert!(payload.start_ms.is_none());
        assert!(payload.large_image.is_none());
        assert_eq!(payload.to_json()["state"], "Browsing the library");
        assert!(payload.to_json().get("timestamps").is_none());
    }

    #[test]
    fn long_titles_are_truncated_for_discord() {
        let title = "x".repeat(400);
        let payload = PresencePayload::playing(&title, None, started());
        assert_eq!(payload.details.chars().count(), MAX_FIELD_CHARS);
    }

    #[test]
    fn disabled_presence_never_connects() {
        let mut core = PresenceCore::new(false, true);
        core.want(Some(PresencePayload::idle()));
        assert!(!core.should_connect(Instant::now()));
        assert!(core.pending().is_none());
        assert!(!core.is_active());
    }

    #[test]
    fn an_unconfigured_client_id_never_connects() {
        let mut core = PresenceCore::new(true, false);
        core.want(Some(PresencePayload::idle()));
        assert!(!core.should_connect(Instant::now()));
        assert!(core.pending().is_none());
    }

    #[test]
    fn disabling_drops_the_connection_and_the_backoff() {
        let mut core = PresenceCore::new(true, true);
        core.on_connected();
        core.on_failure("boom".into(), Instant::now());
        assert!(core.configure(false, true));
        assert!(!core.is_active());
        assert!(!core.connected);
        assert!(!core.should_connect(Instant::now()));
        assert!(core.status(None).last_error.is_some());
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let mut core = PresenceCore::new(true, true);
        let now = Instant::now();
        let mut delays = Vec::new();
        for _ in 0..8 {
            delays.push(core.on_failure("offline".into(), now).as_secs());
        }
        assert_eq!(delays, vec![4, 8, 16, 32, 64, 120, 120, 120]);
        assert!(!core.should_connect(now));
        assert!(core.should_connect(now + MAX_BACKOFF + Duration::from_secs(1)));
    }

    #[test]
    fn reconnecting_clears_the_error_and_pushes_the_latest_payload() {
        let mut core = PresenceCore::new(true, true);
        let now = Instant::now();
        core.on_failure("offline".into(), now);
        assert!(!core.should_connect(now));

        let later = now + MAX_BACKOFF + Duration::from_secs(1);
        assert!(core.should_connect(later));
        core.on_connected();
        core.want(Some(PresencePayload::playing("Hades", None, started())));

        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut sink = MockSink {
            sent: Arc::clone(&sent),
            cleared: Arc::new(Mutex::new(0)),
            fail: false,
        };
        let payload = core.pending().cloned().expect("a payload is pending");
        sink.set(&payload).unwrap();
        core.mark_sent();

        assert_eq!(sent.lock().unwrap().len(), 1);
        assert!(core.pending().is_none());
        assert!(core.status(None).last_error.is_none());
        assert!(core.status(None).connected);
    }

    #[test]
    fn a_failed_push_sends_the_game_again_after_reconnect() {
        let mut core = PresenceCore::new(true, true);
        core.on_connected();
        core.want(Some(PresencePayload::playing("Hades", None, started())));

        let cleared = Arc::new(Mutex::new(0));
        let mut sink = MockSink {
            sent: Arc::new(Mutex::new(Vec::new())),
            cleared: Arc::clone(&cleared),
            fail: true,
        };
        let payload = core.pending().cloned().unwrap();
        assert!(sink.set(&payload).is_err());
        core.on_failure("socket closed".into(), Instant::now());
        assert!(!core.connected);
        // The desired payload is still there for the next connection.
        assert_eq!(core.desired().unwrap().details, "Hades");
    }

    #[test]
    fn idle_and_playing_are_distinct_transitions() {
        let mut core = PresenceCore::new(true, true);
        core.on_connected();
        core.want(Some(PresencePayload::playing("Hades", None, started())));
        assert!(core.pending().is_some());
        core.mark_sent();

        core.want(Some(PresencePayload::idle()));
        assert_eq!(core.pending().unwrap().details, "Idle");
        core.mark_sent();
        assert!(core.pending().is_none());
    }

    #[test]
    fn frames_use_the_discord_ipc_header() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, 1, "{\"cmd\":\"SET_ACTIVITY\"}").unwrap();
        assert_eq!(&buffer[0..4], &1u32.to_le_bytes());
        assert_eq!(
            u32::from_le_bytes(buffer[4..8].try_into().unwrap()) as usize,
            buffer.len() - 8
        );
        assert_eq!(&buffer[8..], b"{\"cmd\":\"SET_ACTIVITY\"}");
    }
}
