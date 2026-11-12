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
    /// A desktop SSO handoff was created by an authenticated web session.
    pub const OIDC_HANDOFF_CREATED: &str = "oidc.handoff_created";
    /// The desktop app claimed a pending SSO handoff.
    pub const OIDC_HANDOFF_CLAIMED: &str = "oidc.handoff_claimed";
    /// A handoff claim was rejected (unknown, expired, used or invalid state).
    pub const OIDC_HANDOFF_FAILED: &str = "oidc.handoff_failed";
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
    /// Library Health Center report started (#165).
    pub const HEALTH_SCAN_STARTED: &str = "health.scan_started";
    /// A Library Health Center repair action was applied (#165).
    pub const HEALTH_ACTION_APPLIED: &str = "health.action_applied";
    /// Share link created.
    pub const SHARE_CREATED: &str = "share.created";
    /// Share link revoked.
    pub const SHARE_REVOKED: &str = "share.revoked";
    /// A public share link issued a download ticket.
    pub const SHARE_ACCESS: &str = "share.access";
    /// A public share access attempt was rejected.
    pub const SHARE_ACCESS_DENIED: &str = "share.access_denied";
    /// A collection share link was created (#173).
    pub const COLLECTION_SHARED: &str = "collection.shared";
    /// A collection share link was revoked (#173).
    pub const COLLECTION_SHARE_REVOKED: &str = "collection.share_revoked";
    /// A public collection page was opened (#173).
    pub const COLLECTION_SHARE_ACCESSED: &str = "collection.share_accessed";
    /// A public collection page view was rejected (#173).
    pub const COLLECTION_SHARE_DENIED: &str = "collection.share_denied";
    /// A collection was imported into another account (#173).
    pub const COLLECTION_IMPORTED: &str = "collection.imported";
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
    /// An editor picked a metadata provider record for a game (#135).
    pub const GAME_METADATA_MATCHED: &str = "game.metadata_matched";
    /// An editor pinned another version as the download/update default (#168).
    pub const GAME_VERSION_DEFAULT_CHANGED: &str = "game.version_default_changed";
    /// An editor deleted one version's archive while keeping the game (#168).
    pub const GAME_VERSION_DELETED: &str = "game.version_deleted";
    /// An administrator created a backup archive (#169).
    pub const BACKUP_CREATED: &str = "backup.created";
    /// An administrator restored an archive (#169).
    pub const BACKUP_RESTORED: &str = "backup.restored";
    /// An administrator deleted a backup archive (#169).
    pub const BACKUP_DELETED: &str = "backup.deleted";
    /// A phone queued a remote install for a desktop (#167).
    pub const REMOTE_INSTALL_REQUESTED: &str = "remote_install.requested";
    /// A desktop reported progress or a terminal state for a remote install.
    pub const REMOTE_INSTALL_UPDATED: &str = "remote_install.updated";
    /// A remote install was cancelled or cleared (#167).
    pub const REMOTE_INSTALL_CANCELLED: &str = "remote_install.cancelled";
    /// An invitation link was created (#166).
    pub const INVITATION_CREATED: &str = "invitation.created";
    /// An invitation link was revoked (#166).
    pub const INVITATION_REVOKED: &str = "invitation.revoked";
    /// An invitation link was redeemed and the account created (#166).
    pub const INVITATION_REDEEMED: &str = "invitation.redeemed";
    /// A password was accepted but the second factor is still pending (#166).
    pub const LOGIN_CHALLENGE: &str = "login.challenge";
    /// TOTP enrolment started (the secret was issued once) (#166).
    pub const MFA_ENROLLMENT_STARTED: &str = "mfa.enrollment_started";
    /// Two-factor authentication turned on (#166).
    pub const MFA_ENABLED: &str = "mfa.enabled";
    /// Two-factor authentication turned off (#166).
    pub const MFA_DISABLED: &str = "mfa.disabled";
    /// Recovery codes replaced (#166).
    pub const MFA_RECOVERY_CODES_ROTATED: &str = "mfa.recovery_codes_rotated";
    /// A friend request was sent (#161).
    pub const FRIEND_REQUESTED: &str = "friend.requested";
    /// A friend request was accepted (#161).
    pub const FRIEND_ACCEPTED: &str = "friend.accepted";
    /// A friendship was removed (#161).
    pub const FRIEND_REMOVED: &str = "friend.removed";
    /// An account blocked another (#161).
    pub const FRIEND_BLOCKED: &str = "friend.blocked";
    /// An account lifted a block (#161).
    pub const FRIEND_UNBLOCKED: &str = "friend.unblocked";
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
