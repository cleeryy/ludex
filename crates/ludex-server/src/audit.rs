//! Persistent audit trail.
//!
//! Every [`Event`](crate::events::Event) is written to `audit_logs` by the
//! event worker. Secrets (passwords, tokens, webhook secrets) must never be
//! placed in an event's metadata.

use crate::db;
use crate::db::Database;

use crate::events::Event;

/// Stable action names recorded in `audit_logs.action` and used as webhook
/// event names where the two overlap.
pub mod actions {
    /// Successful password login.
    pub const LOGIN_SUCCEEDED: &str = "login.succeeded";
    /// Rejected password login.
    pub const LOGIN_FAILED: &str = "login.failed";
    /// Account created by an administrator, setup or OIDC provisioning.
    pub const USER_CREATED: &str = "user.created";
    /// Account role, email or activation changed.
    pub const USER_UPDATED: &str = "user.updated";
    /// Account deleted.
    pub const USER_DELETED: &str = "user.deleted";
    /// Account password changed.
    pub const USER_PASSWORD_CHANGED: &str = "user.password_changed";
    /// Account password reset by an administrator.
    pub const USER_PASSWORD_RESET: &str = "user.password_reset";
    /// API key created.
    pub const API_KEY_CREATED: &str = "api_key.created";
    /// API key revoked.
    pub const API_KEY_REVOKED: &str = "api_key.revoked";
    /// A new game entered the library.
    pub const GAME_INGESTED: &str = "game.ingested";
    /// A library file was deleted.
    pub const GAME_DELETED: &str = "game.deleted";
    /// Custom cover/background uploaded or reset.
    pub const GAME_ARTWORK_UPDATED: &str = "game.artwork_updated";
    /// Access denied by parental controls (age limit).
    pub const GAME_ACCESS_DENIED: &str = "game.access_denied";
    /// A download job completed.
    pub const DOWNLOAD_COMPLETED: &str = "download.completed";
    /// Server settings written (initial setup).
    pub const SETTINGS_UPDATED: &str = "settings.updated";
    /// Webhook created.
    pub const WEBHOOK_CREATED: &str = "webhook.created";
    /// Webhook updated.
    pub const WEBHOOK_UPDATED: &str = "webhook.updated";
    /// Webhook deleted.
    pub const WEBHOOK_DELETED: &str = "webhook.deleted";
    /// Cloud save snapshot uploaded.
    pub const SAVE_UPLOADED: &str = "save.uploaded";
    /// Cloud save snapshot deleted.
    pub const SAVE_DELETED: &str = "save.deleted";
    /// Successful OIDC login on an already-linked account.
    pub const OIDC_LOGIN: &str = "oidc.login";
    /// OIDC identity linked to an existing local account.
    pub const OIDC_LINKED: &str = "oidc.linked";
    /// Library root added.
    pub const LIBRARY_ROOT_CREATED: &str = "library_root.created";
    /// Library root label, path, ordering or enabled flag changed.
    pub const LIBRARY_ROOT_UPDATED: &str = "library_root.updated";
    /// Library root removed.
    pub const LIBRARY_ROOT_DELETED: &str = "library_root.deleted";
    /// Integrity check started.
    pub const INTEGRITY_CHECK_STARTED: &str = "integrity.check_started";
    /// Media cache garbage collection removed files.
    pub const MEDIA_GC_RUN: &str = "media.gc";
    /// Share link created.
    pub const SHARE_CREATED: &str = "share.created";
    /// Share link revoked.
    pub const SHARE_REVOKED: &str = "share.revoked";
    /// A public share link issued a download ticket.
    pub const SHARE_ACCESS: &str = "share.access";
    /// A public share access attempt was rejected.
    pub const SHARE_ACCESS_DENIED: &str = "share.access_denied";
    /// Session revoked (logout or admin action).
    pub const SESSION_REVOKED: &str = "session.revoked";
    /// Resumable archive upload completed and indexed.
    pub const UPLOAD_COMPLETED: &str = "upload.completed";
    /// An achievement was unlocked for an account.
    pub const ACHIEVEMENT_UNLOCKED: &str = "achievement.unlocked";
    /// A local achievement provider scan finished.
    pub const ACHIEVEMENT_SCANNED: &str = "achievement.scanned";
    /// A manual achievement definition was created.
    pub const ACHIEVEMENT_DEFINITION_CREATED: &str = "achievement.definition_created";
    /// A manual achievement definition was updated.
    pub const ACHIEVEMENT_DEFINITION_UPDATED: &str = "achievement.definition_updated";
    /// A manual achievement definition was deleted.
    pub const ACHIEVEMENT_DEFINITION_DELETED: &str = "achievement.definition_deleted";
    /// A community metadata provider was created, updated, reordered or deleted.
    pub const METADATA_PROVIDER_UPDATED: &str = "settings.metadata_provider_updated";
}

/// Writes one event to `audit_logs`. Failures are logged, never propagated:
/// auditing must not turn a successful operation into an HTTP error.
pub async fn record(pool: &Database, event: &Event) {
    let metadata = serde_json::to_string(&event.metadata).unwrap_or_else(|_| "{}".to_string());
    let result = db::query(
        "INSERT INTO audit_logs
            (actor_id, actor_kind, action, target_type, target_id, metadata, ip, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(event.actor.id())
    .bind(event.actor.kind())
    .bind(&event.action)
    .bind(event.target_type.as_deref())
    .bind(event.target_id.as_deref())
    .bind(metadata)
    .bind(event.ip.map(|ip| ip.to_string()))
    .bind(crate::db::db_time(crate::db::now()))
    .execute(pool)
    .await;
    if let Err(error) = result {
        tracing::error!(error = %error, action = %event.action, "could not write audit log");
    }
}
