//! In-process event bus: audit trail and webhook fan-out.
//!
//! Routes and background workers emit a single [`Event`]; the worker started by
//! [`crate::prepare`] writes the audit row and, for subscribed event names,
//! queues webhook deliveries. Emitting never blocks the caller: the channel is
//! unbounded and a closed channel is only logged.

use std::net::IpAddr;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::state::AppState;

/// Who caused an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// A signed-in account.
    User {
        /// User id.
        id: i64,
    },
    /// A request authenticated with an API key.
    ApiKey {
        /// API key row id.
        id: i64,
    },
    /// The server itself (scanner, bootstrap, OIDC provisioning).
    System,
}

impl Actor {
    /// `actor_kind` value stored in `audit_logs`.
    #[must_use]
    pub fn kind(self) -> &'static str {
        match self {
            Actor::User { .. } => "user",
            Actor::ApiKey { .. } => "api_key",
            Actor::System => "system",
        }
    }

    /// `actor_id` value stored in `audit_logs`.
    #[must_use]
    pub fn id(self) -> Option<i64> {
        match self {
            Actor::User { id } | Actor::ApiKey { id } => Some(id),
            Actor::System => None,
        }
    }
}

/// One auditable, optionally webhook-visible occurrence.
#[derive(Debug, Clone)]
pub struct Event {
    /// Stable action name, e.g. `game.ingested`.
    pub action: String,
    /// Who triggered it.
    pub actor: Actor,
    /// Target entity family (`user`, `game`, `webhook`, ...).
    pub target_type: Option<String>,
    /// Target identifier, as text so ids and file names share a column.
    pub target_id: Option<String>,
    /// Structured, secret-free detail.
    pub metadata: Value,
    /// Originating address when the event comes from a request.
    pub ip: Option<IpAddr>,
}

impl Event {
    /// Creates an event with empty detail.
    #[must_use]
    pub fn new(action: impl Into<String>, actor: Actor) -> Self {
        Self {
            action: action.into(),
            actor,
            target_type: None,
            target_id: None,
            metadata: json!({}),
            ip: None,
        }
    }

    /// Sets the target entity.
    #[must_use]
    pub fn target(mut self, target_type: impl Into<String>, target_id: impl Into<String>) -> Self {
        self.target_type = Some(target_type.into());
        self.target_id = Some(target_id.into());
        self
    }

    /// Sets structured metadata.
    #[must_use]
    pub fn metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// Sets the request IP when one is known.
    #[must_use]
    pub fn ip(mut self, ip: Option<IpAddr>) -> Self {
        self.ip = ip;
        self
    }
}

/// Cheap cloneable handle used to emit events.
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::UnboundedSender<Event>,
}

impl EventSink {
    /// Wraps a sender.
    #[must_use]
    pub fn new(tx: mpsc::UnboundedSender<Event>) -> Self {
        Self { tx }
    }

    /// Emits an event without blocking. Dropped events are logged.
    pub fn emit(&self, event: Event) {
        if let Err(error) = self.tx.send(event) {
            tracing::warn!(error = %error, "event bus is closed; event dropped");
        }
    }
}

/// Creates an event channel.
#[must_use]
pub fn channel() -> (EventSink, mpsc::UnboundedReceiver<Event>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (EventSink::new(tx), rx)
}

/// Consumes events until every sender is dropped.
pub async fn worker(state: AppState, mut rx: mpsc::UnboundedReceiver<Event>) {
    while let Some(event) = rx.recv().await {
        crate::audit::record(&state.pool, &event).await;
        if crate::webhooks::is_webhook_event(&event.action) {
            crate::webhooks::dispatch(&state, &event).await;
        }
    }
    tracing::debug!("event bus stopped");
}
