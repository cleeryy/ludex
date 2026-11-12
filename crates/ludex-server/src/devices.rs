//! Device registry (#184).
//!
//! A desktop registers once with a stable, client-generated id and then
//! heartbeats its presence. The registry is what lets the phone answer "which
//! PC is online?" and target one install at a specific machine instead of
//! racing several desktops for the same request.
//!
//! Security model:
//! - authentication stays the normal session or API key; `client_id` is a
//!   routing key, never a credential, and is not a hardware fingerprint;
//! - every query is scoped by `user_id`, so a phone signed into another account
//!   sees no devices at all;
//! - heartbeats are rate-limited per account through the shared
//!   [`AttemptCounter`](crate::security::AttemptCounter);
//! - revoking is soft: the row stays in the database so the same client id
//!   cannot silently re-register, but it disappears from the phone's list and
//!   can no longer heartbeat or claim;
//! - no token is ever stored or returned here.

use std::time::Duration;

use crate::db::{self, Database};
use crate::error::ApiError;
use crate::models::{Device, DeviceCapabilities, DeviceHeartbeatRequest};

/// A device counts as online while its last heartbeat is younger than this.
pub const ONLINE_WINDOW: Duration = Duration::from_secs(60);
/// Heartbeats allowed per account per minute. A 25 s heartbeat is 2.4/min, so
/// the budget tolerates a restart storm but not a busy loop.
pub const HEARTBEAT_LIMIT_PER_MINUTE: u32 = 30;
/// Longest accepted client id.
const MAX_CLIENT_ID: usize = 128;
/// Longest accepted display name.
const MAX_NAME: usize = 80;
/// Most installed game ids kept from one heartbeat.
const MAX_INSTALLED_IDS: usize = 500;

/// Stable identity of a device resolved from its client id.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    /// Server-side device id used to target installs.
    pub id: String,
}

/// Result of one heartbeat.
pub struct HeartbeatOutcome {
    /// The device row after the upsert.
    pub device: Device,
    /// True when this heartbeat registered a new device (worth auditing).
    pub registered: bool,
}

/// Flat row selected by [`list`] and [`get`], in column order.
type DeviceRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    i64,
    Option<i64>,
    i64,
    String,
    String,
    Option<String>,
);

const DEVICE_COLUMNS: &str = "id, name, platform, app_version, last_seen_at, installed_count,
     free_disk_bytes, streaming_available, capabilities_json, created_at, revoked_at";

fn row_to_device(row: DeviceRow, now: chrono::DateTime<chrono::Utc>) -> Device {
    let (
        id,
        name,
        platform,
        app_version,
        last_seen_at,
        installed_count,
        free_disk_bytes,
        streaming_available,
        capabilities_json,
        created_at,
        revoked_at,
    ) = row;
    let capabilities: DeviceCapabilities =
        serde_json::from_str(&capabilities_json).unwrap_or_default();
    Device {
        id,
        name,
        platform,
        app_version,
        online: revoked_at.is_none() && seen_within(&last_seen_at, now, ONLINE_WINDOW),
        last_seen_at,
        installed_count,
        free_disk_bytes,
        streaming_available: streaming_available != 0,
        capabilities,
        created_at,
        revoked_at,
    }
}

/// Whether `last_seen_at` is no older than `window`. An unparsable timestamp is
/// treated as offline rather than online, so a corrupt row never looks alive.
#[must_use]
pub fn seen_within(
    last_seen_at: &str,
    now: chrono::DateTime<chrono::Utc>,
    window: Duration,
) -> bool {
    let Some(seen) = db::parse_time(last_seen_at) else {
        return false;
    };
    let age = now.signed_duration_since(seen);
    // Accept a small clock skew in either direction: a PC whose clock runs a
    // few seconds fast must not look offline, and one from the far future must
    // not stay online forever.
    let skew = i64::try_from(window.as_millis()).unwrap_or(i64::MAX);
    age.num_milliseconds() >= -skew && age.num_milliseconds() <= skew
}

fn clean_text(value: Option<&str>, max: usize) -> Option<String> {
    let cleaned: String = value?
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(max)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn clean_client_id(value: &str) -> Result<String, ApiError> {
    let cleaned: String = value
        .trim()
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
        .take(MAX_CLIENT_ID)
        .collect();
    if cleaned.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_client_id",
            "clientId must be a non-empty id using letters, digits, '-', '_', '.' or ':'",
        ));
    }
    Ok(cleaned)
}

fn sanitize_capabilities(mut capabilities: DeviceCapabilities) -> DeviceCapabilities {
    capabilities.installed_game_ids = capabilities
        .installed_game_ids
        .into_iter()
        .filter(|id| *id > 0)
        .take(MAX_INSTALLED_IDS)
        .collect();
    capabilities.installed_count = capabilities.installed_count.map(|count| count.max(0));
    capabilities.free_disk_bytes = capabilities.free_disk_bytes.map(|free| free.max(0));
    capabilities
}

/// Lists the caller's active (non-revoked) devices, newest heartbeat first.
pub async fn list(pool: &Database, user_id: i64) -> Result<Vec<Device>, ApiError> {
    let now = db::now();
    let rows = db::query_as::<DeviceRow>(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices
         WHERE user_id = ? AND revoked_at IS NULL
         ORDER BY last_seen_at DESC, id DESC"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows
        .into_iter()
        .map(|row| row_to_device(row, now))
        .collect())
}

/// Reads one active device owned by `user_id`.
pub async fn get(pool: &Database, user_id: i64, id: &str) -> Result<Device, ApiError> {
    let row = db::query_as::<DeviceRow>(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices
         WHERE id = ? AND user_id = ? AND revoked_at IS NULL"
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("device_not_found", "Device not found"))?;
    Ok(row_to_device(row, db::now()))
}

/// Resolves a stable client id to the server device id, rejecting revoked or
/// unknown devices. Used when a desktop claims or reports an install.
pub async fn resolve_client(
    pool: &Database,
    user_id: i64,
    client_id: &str,
) -> Result<DeviceIdentity, ApiError> {
    let client_id = clean_client_id(client_id)?;
    let row = db::query_as::<(String, Option<String>)>(
        "SELECT id, revoked_at FROM devices WHERE user_id = ? AND client_id = ?",
    )
    .bind(user_id)
    .bind(&client_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    match row {
        Some((id, None)) => Ok(DeviceIdentity { id }),
        Some((_, Some(_))) => Err(ApiError::forbidden(
            "device_revoked",
            "This device was revoked; it can no longer claim or report installs",
        )),
        None => Err(ApiError::forbidden(
            "device_not_registered",
            "This device is not registered; send a heartbeat before claiming installs",
        )),
    }
}

/// Resolves a device id chosen as an install target. A device from another
/// account is indistinguishable from a missing one (404).
pub async fn require_target(
    pool: &Database,
    user_id: i64,
    id: &str,
) -> Result<DeviceIdentity, ApiError> {
    let row = db::query_as::<(String, Option<String>)>(
        "SELECT id, revoked_at FROM devices WHERE id = ? AND user_id = ?",
    )
    .bind(id.trim())
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("device_not_found", "Device not found"))?;
    if row.1.is_some() {
        return Err(ApiError::bad_request(
            "device_revoked",
            "That device was revoked; pick another PC or install account-wide",
        ));
    }
    Ok(DeviceIdentity { id: row.0 })
}

/// Upserts a device from a heartbeat.
pub async fn heartbeat(
    pool: &Database,
    user_id: i64,
    request: &DeviceHeartbeatRequest,
) -> Result<HeartbeatOutcome, ApiError> {
    let client_id = clean_client_id(&request.client_id)?;
    let existing = db::query_as::<(String, String, Option<String>)>(
        "SELECT id, name, revoked_at FROM devices WHERE user_id = ? AND client_id = ?",
    )
    .bind(user_id)
    .bind(&client_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    if let Some((_, _, Some(_))) = &existing {
        return Err(ApiError::forbidden(
            "device_revoked",
            "This device was revoked; it can no longer heartbeat",
        ));
    }

    let capabilities = sanitize_capabilities(request.capabilities.clone());
    let installed_count = capabilities
        .installed_count
        .unwrap_or_else(|| i64::try_from(capabilities.installed_game_ids.len()).unwrap_or(0));
    let capabilities_json =
        serde_json::to_string(&capabilities).unwrap_or_else(|_| "{}".to_string());
    let platform = clean_text(request.platform.as_deref(), 40);
    let app_version = clean_text(request.app_version.as_deref(), 40);
    let requested_name = clean_text(request.name.as_deref(), MAX_NAME);
    let now = db::db_time(db::now());
    let streaming = i64::from(capabilities.streaming_available);

    let (id, registered) = match existing {
        Some((id, current_name, None)) => {
            let name = requested_name.unwrap_or(current_name);
            db::query(
                "UPDATE devices
                 SET name = ?, platform = ?, app_version = ?, last_seen_at = ?,
                     capabilities_json = ?, installed_count = ?, free_disk_bytes = ?,
                     streaming_available = ?
                 WHERE id = ? AND user_id = ?",
            )
            .bind(&name)
            .bind(&platform)
            .bind(&app_version)
            .bind(&now)
            .bind(&capabilities_json)
            .bind(installed_count)
            .bind(capabilities.free_disk_bytes)
            .bind(streaming)
            .bind(&id)
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(ApiError::from)?;
            (id, false)
        }
        _ => {
            let id = uuid::Uuid::new_v4().to_string();
            let name = requested_name.unwrap_or_else(|| "Desktop".to_string());
            db::query(
                "INSERT INTO devices
                    (id, user_id, client_id, name, platform, app_version, last_seen_at,
                     capabilities_json, installed_count, free_disk_bytes,
                     streaming_available, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(user_id)
            .bind(&client_id)
            .bind(&name)
            .bind(&platform)
            .bind(&app_version)
            .bind(&now)
            .bind(&capabilities_json)
            .bind(installed_count)
            .bind(capabilities.free_disk_bytes)
            .bind(streaming)
            .bind(&now)
            .execute(pool)
            .await
            .map_err(ApiError::from)?;
            (id, true)
        }
    };

    let device = get(pool, user_id, &id).await?;
    Ok(HeartbeatOutcome { device, registered })
}

/// Revokes a device and fails any active install targeted at it.
///
/// Returns true when this call actually revoked it; a second revoke is a
/// no-op and stays quiet in the audit log.
pub async fn revoke(pool: &Database, user_id: i64, id: &str) -> Result<bool, ApiError> {
    let row = db::query_as::<(String, Option<String>)>(
        "SELECT id, revoked_at FROM devices WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("device_not_found", "Device not found"))?;
    if row.1.is_some() {
        return Ok(false);
    }
    let now = db::db_time(db::now());
    db::query("UPDATE devices SET revoked_at = ? WHERE id = ? AND user_id = ?")
        .bind(&now)
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(ApiError::from)?;
    db::query(
        "UPDATE remote_installs
         SET status = 'failed', progress = 0, message = ?, updated_at = ?, completed_at = ?
         WHERE user_id = ? AND target_device_id = ?
           AND status IN ('queued', 'claimed', 'downloading')",
    )
    .bind("The target device was revoked before the install finished.")
    .bind(&now)
    .bind(&now)
    .bind(user_id)
    .bind(id)
    .execute(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(true)
}

/// Admin listing: every device of one account, including revoked rows.
pub async fn list_for_user(pool: &Database, user_id: i64) -> Result<Vec<Device>, ApiError> {
    let exists = db::query_scalar::<i64>("SELECT id FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    if exists.is_none() {
        return Err(ApiError::not_found("user_not_found", "User not found"));
    }
    let now = db::now();
    let rows = db::query_as::<DeviceRow>(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices WHERE user_id = ?
         ORDER BY (revoked_at IS NULL) DESC, last_seen_at DESC, id DESC"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows
        .into_iter()
        .map(|row| row_to_device(row, now))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(seconds: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .timestamp_opt(1_800_000_000 + seconds, 0)
            .unwrap()
    }

    #[test]
    fn online_window_is_strict_and_sixty_seconds() {
        let now = at(0);
        assert!(seen_within(&db::db_time(at(-59)), now, ONLINE_WINDOW));
        assert!(seen_within(&db::db_time(at(-60)), now, ONLINE_WINDOW));
        assert!(!seen_within(&db::db_time(at(-61)), now, ONLINE_WINDOW));
        // A timestamp in the future is not "online forever", but a small clock
        // skew must not mark a live device offline.
        assert!(seen_within(&db::db_time(at(5)), now, ONLINE_WINDOW));
        assert!(!seen_within(&db::db_time(at(3600)), now, ONLINE_WINDOW));
        assert!(!seen_within("not-a-date", now, ONLINE_WINDOW));
    }

    #[test]
    fn client_ids_are_sanitised_not_trusted() {
        assert_eq!(clean_client_id("  abc-123  ").unwrap(), "abc-123");
        assert_eq!(clean_client_id("a b/c").unwrap(), "abc");
        assert!(clean_client_id("   ").is_err());
        let long = "x".repeat(300);
        assert_eq!(clean_client_id(&long).unwrap().len(), MAX_CLIENT_ID);
    }

    #[test]
    fn capabilities_are_clamped() {
        let capabilities = sanitize_capabilities(DeviceCapabilities {
            installed_game_ids: (0..600).collect(),
            installed_count: Some(-4),
            free_disk_bytes: Some(-1),
            streaming_available: true,
        });
        assert_eq!(capabilities.installed_game_ids.len(), MAX_INSTALLED_IDS);
        assert_eq!(capabilities.installed_count, Some(0));
        assert_eq!(capabilities.free_disk_bytes, Some(0));
        assert!(capabilities.streaming_available);
    }
}
