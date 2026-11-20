//! Per-user cloud save snapshots.
//!
//! A snapshot is an opaque archive uploaded by one user for one game. Users
//! only ever see their own snapshots; administrators may download and delete
//! any snapshot (support/cleanup), but listing always stays per-user.
//!
//! Automatic uploads go through the multi-device conflict policy (#296), shared
//! with the desktop in [`ludex_core::save_conflict`]. A conflict never deletes
//! a save: the losing side is stored with `conflict_of` set and a
//! `save (conflict <device> <date>)` label, and the open row in
//! `save_conflicts` is what the UI offers to resolve.

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use ludex_core::save_conflict::{
    conflict_label, decide, ConflictReason, SaveDecision, SaveSide, SaveSideKind,
};
use tokio_util::io::ReaderStream;
use utoipa::openapi::HttpMethod;

use crate::api_keys::Scope;
use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, MultipartBody};
use crate::media::content_disposition;
use crate::models::{
    PruneSavesRequest, RestoreSnapshotRequest, RestoreSnapshotResponse, SaveConflictView,
    SaveHistory, SavePolicyView, SavePruneReport, SaveRestoreReceipt, SaveRestoreReceiptRow,
    SaveSnapshot, SaveSnapshotRow,
};
use crate::openapi::{body, ok, route, Auth, Payload, RouteDoc};
use crate::saves::{self, PendingSave, SaveError};
use crate::state::AppState;

/// Longest accepted snapshot label, in characters.
const MAX_LABEL_CHARS: usize = 200;
/// Auto snapshots kept per user and game; manual snapshots are never pruned.
const AUTO_SNAPSHOT_RETENTION: i64 = 10;
/// Longest accepted device identifier or label, in characters.
const MAX_DEVICE_ID_CHARS: usize = 100;
/// Bounds for an explicit retention request (#295).
const MIN_PRUNE_KEEP: i64 = 1;
const MAX_PRUNE_KEEP: i64 = 50;
/// History page sizes (#295); snapshots per game stay small in practice.
const HISTORY_SNAPSHOT_LIMIT: i64 = 100;
const HISTORY_RECEIPT_LIMIT: i64 = 50;

/// A device clock may not be more than one day off; larger reports are
/// clamped rather than trusted.
const MAX_CLOCK_OFFSET_SECS: i64 = 86_400;
/// `prefer_device` or `ask`.
const POLICY_PREFER: &str = "prefer_device";
const POLICY_ASK: &str = "ask";

/// Columns of `save_snapshots` read by this module, in `SaveSnapshotRow` order:
/// `pinned` is the restore-point flag (#295), the four trailing columns are the
/// conflict-policy metadata (#296).
const SNAPSHOT_COLUMNS: &str =
    "id, user_id, game_id, version_id, label, file_name, size, sha256, created_at, source,
     device_id, pinned, client_mtime, clock_offset_secs, save_seq, conflict_of";

/// Save routes, relative to `/api`.
///
/// The upload method gets its own body limit: the global limit is sized for
/// JSON (10 MiB) while save archives may be much larger.
pub fn router(max_upload_bytes: u64) -> Router<AppState> {
    let upload_limit = DefaultBodyLimit::max(saves::body_limit(max_upload_bytes));
    Router::new()
        .route(
            "/games/{id}/saves",
            get(list).post(upload).layer(upload_limit),
        )
        .route("/games/{id}/saves/history", get(history))
        .route("/games/{id}/saves/prune", post(prune))
        .route("/games/{id}/saves/policy", get(get_policy).put(put_policy))
        .route("/games/{id}/saves/conflicts", get(list_conflicts))
        .route(
            "/games/{id}/saves/conflicts/{conflict_id}/resolve",
            post(resolve_conflict),
        )
        .route("/saves/{id}/download", get(download))
        .route("/saves/{id}/restore", post(restore))
        .route("/saves/{id}", delete(delete_snapshot))
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<Vec<SaveSnapshot>>, ApiError> {
    ensure_game(&state, game_id).await?;
    let rows = db::query_as::<SaveSnapshotRow>(&format!(
        "SELECT {SNAPSHOT_COLUMNS}
         FROM save_snapshots
         WHERE user_id = ? AND game_id = ?
         ORDER BY created_at DESC, id DESC"
    ))
    .bind(user.id)
    .bind(game_id)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(
        rows.into_iter().map(SaveSnapshotRow::to_public).collect(),
    ))
}

/// Multipart fields of an automatic upload. Everything here is optional except
/// the file itself, so an older client keeps working (offset 0, server time).
#[derive(Debug, Default)]
struct UploadMetadata {
    label: String,
    source: String,
    device_id: Option<String>,
    device_label: Option<String>,
    /// Newest save file mtime on the device, RFC 3339.
    client_mtime: Option<DateTime<Utc>>,
    /// Device clock at request time, RFC 3339.
    client_sent_at: Option<DateTime<Utc>>,
    /// The game's own save counter, when the client knows it.
    save_seq: Option<i64>,
}

async fn upload(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    MultipartBody(mut multipart): MultipartBody,
) -> Result<(StatusCode, Json<SaveSnapshot>), ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;

    let mut pending: Option<PendingSave> = None;
    let mut meta = UploadMetadata {
        source: "manual".to_string(),
        ..UploadMetadata::default()
    };

    while let Some(field) = multipart.next_field().await.map_err(map_multipart_error)? {
        match field.name().unwrap_or_default() {
            "file" => {
                if pending.is_some() {
                    return Err(ApiError::bad_request(
                        "invalid_file",
                        "Only one file field is allowed",
                    ));
                }
                let raw_name = field.file_name().unwrap_or_default().to_string();
                if raw_name.trim().is_empty() {
                    return Err(ApiError::bad_request(
                        "invalid_file",
                        "The file field must include a file name",
                    ));
                }
                let file_name = saves::sanitize_file_name(&raw_name);
                let dir = saves::game_dir(&state.core.config.data_dir, user.id, game_id);
                pending = Some(
                    saves::receive(&dir, file_name, field, state.core.config.saves_max_bytes)
                        .await
                        .map_err(|error| {
                            map_save_error(error, state.core.config.saves_max_bytes)
                        })?,
                );
            }
            "label" => {
                let value = field.text().await.map_err(map_multipart_error)?;
                let value = value.trim().to_string();
                if value.chars().count() > MAX_LABEL_CHARS {
                    return Err(ApiError::bad_request(
                        "invalid_label",
                        format!("Label must be at most {MAX_LABEL_CHARS} characters"),
                    ));
                }
                meta.label = value;
            }
            "source" => {
                let value = field.text().await.map_err(map_multipart_error)?;
                let value = value.trim().to_string();
                if value != "manual" && value != "auto" {
                    return Err(ApiError::bad_request(
                        "invalid_source",
                        "source must be 'manual' or 'auto'",
                    ));
                }
                meta.source = value;
            }
            "deviceId" => {
                meta.device_id = parse_device_value(field, "invalid_device_id").await?;
            }
            "deviceLabel" => {
                meta.device_label = parse_device_value(field, "invalid_device_label").await?;
            }
            "clientMtime" => {
                meta.client_mtime =
                    Some(parse_timestamp_field(field, "invalid_client_mtime").await?);
            }
            "clientSentAt" => {
                meta.client_sent_at =
                    Some(parse_timestamp_field(field, "invalid_client_sent_at").await?);
            }
            "saveSeq" => {
                let value = field.text().await.map_err(map_multipart_error)?;
                let value = value.trim();
                if value.is_empty() {
                    meta.save_seq = None;
                } else {
                    meta.save_seq = Some(value.parse::<i64>().map_err(|_| {
                        ApiError::bad_request("invalid_save_seq", "saveSeq must be an integer")
                    })?);
                }
            }
            _ => {
                // Unknown fields are drained and ignored for forward compatibility.
                let mut field = field;
                while field.chunk().await.map_err(map_multipart_error)?.is_some() {}
            }
        }
    }

    let pending = pending.ok_or_else(|| {
        ApiError::bad_request("invalid_file", "Multipart field 'file' is required")
    })?;

    let file_name = pending.file_name().to_string();
    let size = i64::try_from(pending.size()).unwrap_or(i64::MAX);
    let sha256 = pending.sha256().to_string();
    let now_dt = db::now();
    let now = db::db_time(now_dt);
    let clock_offset_secs = clock_offset(now_dt, meta.client_sent_at);

    record_device(&state, user.id, &meta, clock_offset_secs, &now).await;

    // Automatic uploads are idempotent by content hash: an unchanged save
    // returns the snapshot that already holds those bytes (#108).
    if meta.source == "auto" {
        let existing = db::query_as::<SaveSnapshotRow>(&format!(
            "SELECT {SNAPSHOT_COLUMNS}
             FROM save_snapshots
             WHERE user_id = ? AND game_id = ? AND sha256 = ?
             ORDER BY created_at DESC, id DESC LIMIT 1"
        ))
        .bind(user.id)
        .bind(game_id)
        .bind(&sha256)
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
        if let Some(existing) = existing {
            pending.discard().await;
            return Ok((StatusCode::OK, Json(existing.to_public())));
        }
    }

    // The conflict policy only arbitrates automatic syncs. A manual upload is
    // an explicit user action and keeps its previous behaviour.
    let latest = if meta.source == "auto" {
        latest_snapshot(&state, user.id, game_id).await?
    } else {
        None
    };
    let prefer_device = if meta.source == "auto" {
        policy_device(&state, user.id, game_id).await?
    } else {
        None
    };
    let decision = match latest.as_ref() {
        Some(existing) => decide(
            &side_of(existing),
            &candidate_side(&sha256, size, clock_offset_secs, &meta, now_dt),
            prefer_device.as_deref(),
        ),
        None => SaveDecision::FastForward(SaveSideKind::Candidate),
    };

    let mut conflict_id: Option<i64> = None;
    let (label, conflict_of) = match decision {
        SaveDecision::Unchanged => {
            // Only reachable for a manual upload racing an identical auto row;
            // keep the existing behaviour of storing it.
            (meta.label.clone(), None)
        }
        SaveDecision::Conflict(_) => (
            conflict_label(device_name(&meta), now_dt),
            latest.as_ref().map(|row| row.id),
        ),
        SaveDecision::FastForward(SaveSideKind::Existing) => (
            conflict_label(device_name(&meta), now_dt),
            latest.as_ref().map(|row| row.id),
        ),
        SaveDecision::FastForward(SaveSideKind::Candidate) => (meta.label.clone(), None),
    };

    let inserted = db::query(
        "INSERT INTO save_snapshots
            (user_id, game_id, version_id, label, file_name, size, sha256, created_at,
             source, device_id, client_mtime, clock_offset_secs, save_seq, conflict_of)
         VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(&label)
    .bind(&file_name)
    .bind(size)
    .bind(&sha256)
    .bind(&now)
    .bind(&meta.source)
    .bind(meta.device_id.as_deref())
    .bind(meta.client_mtime.map(db::db_time))
    .bind(clock_offset_secs)
    .bind(meta.save_seq)
    .bind(conflict_of)
    .returning_id(&state.core.pool)
    .await;

    let snapshot_id = match inserted {
        Ok(id) => id,
        Err(error) => {
            pending.discard().await;
            return Err(ApiError::from(error));
        }
    };

    // A real conflict (both sides changed) is offered to the user. A stale
    // ancestor is only kept aside: the lineage already decided, so there is
    // nothing to ask.
    if matches!(decision, SaveDecision::Conflict(_)) {
        let reason = match decision {
            SaveDecision::Conflict(reason) => reason,
            _ => ConflictReason::BothChanged,
        };
        let existing_id = match latest.as_ref() {
            Some(row) => row.id,
            None => 0,
        };
        let inserted_conflict = db::query(
            "INSERT INTO save_conflicts
                (user_id, game_id, existing_snapshot_id, candidate_snapshot_id, reason,
                 status, detected_at)
             VALUES (?, ?, ?, ?, ?, 'open', ?)",
        )
        .bind(user.id)
        .bind(game_id)
        .bind(existing_id)
        .bind(snapshot_id)
        .bind(reason.as_str())
        .bind(&now)
        .returning_id(&state.core.pool)
        .await;
        match inserted_conflict {
            Ok(id) => conflict_id = Some(id),
            Err(error) => {
                // Keep the candidate as a restore point, but do not leave a
                // half-written choice: the upload itself is harmless.
                tracing::warn!(error = %error, snapshot = snapshot_id, "could not record a save conflict");
            }
        }
    }

    let dir = saves::game_dir(&state.core.config.data_dir, user.id, game_id);
    if let Err(error) = pending.commit(&dir, snapshot_id).await {
        tracing::error!(error = %error, "could not publish save archive");
        let _ = db::query("DELETE FROM save_snapshots WHERE id = ?")
            .bind(snapshot_id)
            .execute(&state.core.pool)
            .await;
        return Err(ApiError::internal());
    }

    state.core.events.emit(
        Event::new(actions::SAVE_UPLOADED, user.actor())
            .target("save", snapshot_id.to_string())
            .metadata(serde_json::json!({
                "snapshotId": snapshot_id,
                "gameId": game_id,
                "userId": user.id,
                "size": size,
                "sha256": sha256,
                "label": label,
                "source": meta.source,
                "deviceId": meta.device_id,
                "conflictId": conflict_id,
            })),
    );

    if meta.source == "auto" {
        prune_auto_snapshots(&state, user.id, game_id, AUTO_SNAPSHOT_RETENTION).await;
    }

    Ok((
        StatusCode::CREATED,
        Json(SaveSnapshot {
            id: snapshot_id,
            game_id,
            version_id: None,
            label,
            file_name,
            size,
            sha256,
            created_at: db::parse_time(&now).unwrap_or_else(chrono::Utc::now),
            pinned: false,
            source: meta.source,
            device_id: meta.device_id,
            client_mtime: meta.client_mtime,
            clock_offset_secs,
            save_seq: meta.save_seq,
            conflict_of,
        }),
    ))
}

async fn get_policy(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<SavePolicyView>, ApiError> {
    ensure_game(&state, game_id).await?;
    let device = policy_device(&state, user.id, game_id).await?;
    Ok(Json(SavePolicyView {
        policy: if device.is_some() {
            POLICY_PREFER.to_string()
        } else {
            POLICY_ASK.to_string()
        },
        device_id: device,
    }))
}

/// Body of `PUT /games/{id}/saves/policy`.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct SavePolicyUpdate {
    /// `ask` (default) or `prefer_device`.
    policy: String,
    /// Required for `prefer_device`; ignored for `ask`.
    device_id: Option<String>,
}

async fn put_policy(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    Json(body): Json<SavePolicyUpdate>,
) -> Result<Json<SavePolicyView>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let policy = body.policy.trim();
    let prefer = match policy {
        POLICY_ASK => None,
        POLICY_PREFER => {
            let device = body
                .device_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_policy",
                        "deviceId is required when policy is 'prefer_device'",
                    )
                })?;
            validate_device_value(device, "invalid_device_id")?;
            Some(device.to_string())
        }
        other => {
            return Err(ApiError::bad_request(
                "invalid_policy",
                format!("Unknown save policy '{other}'; use 'ask' or 'prefer_device'"),
            ))
        }
    };
    let now = db::db_time(db::now());
    db::query(
        "INSERT INTO save_sync_policies (user_id, game_id, prefer_device_id, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id, game_id) DO UPDATE SET
             prefer_device_id = excluded.prefer_device_id,
             updated_at = excluded.updated_at",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(prefer.as_deref())
    .bind(&now)
    .execute(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    Ok(Json(SavePolicyView {
        policy: if prefer.is_some() {
            POLICY_PREFER.to_string()
        } else {
            POLICY_ASK.to_string()
        },
        device_id: prefer,
    }))
}

async fn list_conflicts(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<Vec<SaveConflictView>>, ApiError> {
    ensure_game(&state, game_id).await?;
    let rows = db::query_as::<ConflictRow>(
        "SELECT id, game_id, reason, status, detected_at, resolved_at, strategy,
                existing_snapshot_id, candidate_snapshot_id
         FROM save_conflicts
         WHERE user_id = ? AND game_id = ? AND status = 'open'
         ORDER BY detected_at DESC, id DESC",
    )
    .bind(user.id)
    .bind(game_id)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    let mut views = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(view) = conflict_view(&state, &row).await? {
            views.push(view);
        }
    }
    Ok(Json(views))
}

/// Body of the conflict resolution route.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct ConflictResolution {
    /// `keep_local` keeps the uploaded candidate, `keep_remote` keeps the
    /// snapshot that was already on the server. Either way both rows stay.
    strategy: String,
}

async fn resolve_conflict(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath((game_id, conflict_id)): ApiPath<(i64, i64)>,
    Json(body): Json<ConflictResolution>,
) -> Result<Json<SaveConflictView>, ApiError> {
    user.require_scope(Scope::Write)?;
    let row = db::query_as::<ConflictRow>(
        "SELECT id, game_id, reason, status, detected_at, resolved_at, strategy,
                existing_snapshot_id, candidate_snapshot_id
         FROM save_conflicts
         WHERE id = ? AND user_id = ? AND game_id = ?",
    )
    .bind(conflict_id)
    .bind(user.id)
    .bind(game_id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)?
    .ok_or_else(|| ApiError::not_found("conflict_not_found", "Save conflict not found"))?;
    if row.status != "open" {
        return Err(ApiError::conflict(
            "conflict_resolved",
            "This save conflict was already resolved",
        ));
    }
    let strategy = match body.strategy.trim() {
        "keep_local" => "keep_local",
        "keep_remote" => "keep_remote",
        other => {
            return Err(ApiError::bad_request(
                "invalid_strategy",
                format!("Unknown strategy '{other}'; use 'keep_local' or 'keep_remote'"),
            ))
        }
    };
    let (winner_id, loser_id) = if strategy == "keep_local" {
        (row.candidate_snapshot_id, row.existing_snapshot_id)
    } else {
        (row.existing_snapshot_id, row.candidate_snapshot_id)
    };
    let loser = fetch_snapshot_by_id(&state, loser_id)
        .await?
        .ok_or_else(|| ApiError::not_found("save_not_found", "Save snapshot not found"))?;
    let now_dt = db::now();
    let now = db::db_time(now_dt);
    let loser_device = device_label(&state, user.id, loser.device_id.as_deref()).await;
    let loser_label = conflict_label(loser_device.as_deref(), now_dt);

    state
        .core
        .pool
        .with_tx(async |transaction| {
            if strategy == "keep_local" {
                // The candidate becomes the canonical save again: clearing the
                // conflict marker is what makes it "the latest save".
                db::query(
                    "UPDATE save_snapshots SET conflict_of = NULL, label = 'save' WHERE id = ?",
                )
                .bind(winner_id)
                .execute(&mut *transaction)
                .await
                .map_err(ApiError::from)?;
            }
            db::query("UPDATE save_snapshots SET conflict_of = ?, label = ? WHERE id = ?")
                .bind(winner_id)
                .bind(&loser_label)
                .bind(loser_id)
                .execute(&mut *transaction)
                .await
                .map_err(ApiError::from)?;
            db::query(
                "UPDATE save_conflicts SET status = 'resolved', resolved_at = ?, strategy = ?
                 WHERE id = ?",
            )
            .bind(&now)
            .bind(strategy)
            .bind(conflict_id)
            .execute(&mut *transaction)
            .await
            .map_err(ApiError::from)?;
            Ok::<(), ApiError>(())
        })
        .await?;

    state.core.events.emit(
        Event::new(actions::SAVE_UPLOADED, user.actor())
            .target("save_conflict", conflict_id.to_string())
            .metadata(serde_json::json!({
                "conflictId": conflict_id,
                "gameId": game_id,
                "strategy": strategy,
                "winnerId": winner_id,
                "loserId": loser_id,
            })),
    );

    let resolved = ConflictRow {
        id: row.id,
        game_id: row.game_id,
        reason: row.reason,
        status: "resolved".to_string(),
        detected_at: row.detected_at,
        resolved_at: Some(now),
        strategy: Some(strategy.to_string()),
        existing_snapshot_id: row.existing_snapshot_id,
        candidate_snapshot_id: row.candidate_snapshot_id,
    };
    conflict_view(&state, &resolved)
        .await?
        .ok_or_else(|| ApiError::not_found("save_not_found", "Save snapshot not found"))
        .map(Json)
}

async fn download(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<Response, ApiError> {
    let row = fetch_snapshot(&state, id).await?;
    ensure_access(&user, &row)?;

    let path = saves::snapshot_path(
        &state.core.config.data_dir,
        row.user_id,
        row.game_id,
        row.id,
        &row.file_name,
    )?;
    let file = tokio::fs::File::open(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found("file_not_found", "The save archive is missing from disk")
        } else {
            ApiError::from(error)
        }
    })?;
    let metadata = file.metadata().await.map_err(ApiError::from)?;
    let content_type = mime_guess::from_path(&row.file_name)
        .first_or_octet_stream()
        .to_string();
    let disposition = HeaderValue::from_str(&content_disposition(&row.file_name))
        .map_err(|_| ApiError::internal())?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|_| ApiError::internal())
}

async fn delete_snapshot(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_scope(Scope::Write)?;
    let row = fetch_snapshot(&state, id).await?;
    ensure_access(&user, &row)?;

    let deleted = db::query("DELETE FROM save_snapshots WHERE id = ?")
        .bind(id)
        .execute(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "save_not_found",
            "Save snapshot not found",
        ));
    }

    saves::delete_file(
        &state.core.config.data_dir,
        row.user_id,
        row.game_id,
        row.id,
        &row.file_name,
    )
    .await;
    state.core.events.emit(
        Event::new(actions::SAVE_DELETED, user.actor())
            .target("save", row.id.to_string())
            .metadata(serde_json::json!({
                "snapshotId": row.id,
                "gameId": row.game_id,
                "ownerId": row.user_id,
                "fileName": row.file_name,
            })),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Retention pass shared by the post-upload cleanup and the explicit prune
/// endpoint (#295). Returns the report described by [`SavePruneReport`].
///
/// Rules, in order:
/// - pinned snapshots are restore points and are never removed;
/// - the newest snapshot of the game is the last known-good copy and stays
///   even when it is neither pinned nor inside the keep window;
/// - only automatic snapshots are candidates, so manual uploads are safe;
/// - row deletions happen before the file deletions so a crash leaves an
///   orphaned file (removed by the media GC rules) instead of a row pointing
///   at a missing archive.
async fn prune_snapshots(
    state: &AppState,
    user_id: i64,
    game_id: i64,
    keep: i64,
) -> SavePruneReport {
    let keep = keep.clamp(MIN_PRUNE_KEEP, MAX_PRUNE_KEEP);
    // Conflict copies (#296) are the losing side of a choice the user still
    // has to make, and a snapshot named in an open conflict is one of the two
    // candidates: neither may be pruned.
    let rows = db::query_as::<(i64, String, String, i64)>(
        "SELECT id, file_name, source, pinned FROM save_snapshots
         WHERE user_id = ? AND game_id = ? AND conflict_of IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM save_conflicts c
               WHERE c.status = 'open'
                 AND (c.existing_snapshot_id = save_snapshots.id
                      OR c.candidate_snapshot_id = save_snapshots.id)
           )
         ORDER BY created_at DESC, id DESC",
    )
    .bind(user_id)
    .bind(game_id)
    .fetch_all(&state.core.pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "could not list save snapshots for pruning");
            return SavePruneReport {
                removed: 0,
                kept: 0,
                protected: 0,
            };
        }
    };

    let total = i64::try_from(rows.len()).unwrap_or(i64::MAX);
    let protected = i64::try_from(rows.iter().filter(|(_, _, _, pinned)| *pinned != 0).count())
        .unwrap_or(i64::MAX);

    let mut auto_seen = 0i64;
    let mut stale = Vec::new();
    for (id, file_name, source, pinned) in rows {
        if source != "auto" || pinned != 0 {
            continue;
        }
        // The newest automatic snapshot counts toward the window first, so a
        // game whose latest session is also its only copy never loses it.
        if auto_seen < keep {
            auto_seen += 1;
            continue;
        }
        stale.push((id, file_name));
    }

    let mut removed = 0i64;
    for (id, file_name) in stale {
        match db::query("DELETE FROM save_snapshots WHERE id = ?")
            .bind(id)
            .execute(&state.core.pool)
            .await
        {
            Ok(_) => removed += 1,
            Err(error) => {
                tracing::warn!(error = %error, snapshot = id, "could not prune a save snapshot row");
                continue;
            }
        }
        saves::delete_file(
            &state.core.config.data_dir,
            user_id,
            game_id,
            id,
            &file_name,
        )
        .await;
    }

    SavePruneReport {
        removed,
        kept: total - removed,
        protected,
    }
}

/* ------------------------------------------------------------------ policy */

/// Server-clock minus device-clock estimate, clamped to one day.
///
/// `offset = device_clock - server_clock`: a device two hours ahead reports a
/// `clientSentAt` two hours in the future, so the offset is +7200 and
/// `SaveSide::effective_mtime` subtracts it.
fn clock_offset(now: DateTime<Utc>, client_sent_at: Option<DateTime<Utc>>) -> i64 {
    client_sent_at
        .map(|sent| {
            sent.signed_duration_since(now)
                .num_seconds()
                .clamp(-MAX_CLOCK_OFFSET_SECS, MAX_CLOCK_OFFSET_SECS)
        })
        .unwrap_or(0)
}

fn side_of(row: &SaveSnapshotRow) -> SaveSide {
    SaveSide {
        sha256: row.sha256.clone(),
        size: row.size,
        client_mtime: row
            .client_mtime
            .as_deref()
            .and_then(db::parse_time)
            .or_else(|| db::parse_time(&row.created_at)),
        clock_offset_secs: row.clock_offset_secs,
        save_seq: row.save_seq,
        device_id: row.device_id.clone(),
        recorded_at: db::parse_time(&row.created_at).unwrap_or_else(Utc::now),
    }
}

fn candidate_side(
    sha256: &str,
    size: i64,
    clock_offset_secs: i64,
    meta: &UploadMetadata,
    recorded_at: DateTime<Utc>,
) -> SaveSide {
    SaveSide {
        sha256: sha256.to_string(),
        size,
        client_mtime: meta.client_mtime,
        clock_offset_secs,
        save_seq: meta.save_seq,
        device_id: meta.device_id.clone(),
        recorded_at,
    }
}

fn device_name(meta: &UploadMetadata) -> Option<&str> {
    meta.device_label.as_deref().or(meta.device_id.as_deref())
}

async fn latest_snapshot(
    state: &AppState,
    user_id: i64,
    game_id: i64,
) -> Result<Option<SaveSnapshotRow>, ApiError> {
    db::query_as::<SaveSnapshotRow>(&format!(
        "SELECT {SNAPSHOT_COLUMNS}
         FROM save_snapshots
         WHERE user_id = ? AND game_id = ? AND conflict_of IS NULL
         ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(user_id)
    .bind(game_id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)
}

async fn policy_device(
    state: &AppState,
    user_id: i64,
    game_id: i64,
) -> Result<Option<String>, ApiError> {
    db::query_scalar::<Option<String>>(
        "SELECT prefer_device_id FROM save_sync_policies WHERE user_id = ? AND game_id = ?",
    )
    .bind(user_id)
    .bind(game_id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)
    .map(Option::flatten)
}

async fn record_device(
    state: &AppState,
    user_id: i64,
    meta: &UploadMetadata,
    clock_offset_secs: i64,
    now: &str,
) {
    let Some(device_id) = meta.device_id.as_deref() else {
        return;
    };
    if let Err(error) = db::query(
        "INSERT INTO save_devices
             (user_id, device_id, label, clock_offset_secs, first_seen_at, last_seen_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, device_id) DO UPDATE SET
             label = COALESCE(excluded.label, save_devices.label),
             clock_offset_secs = excluded.clock_offset_secs,
             last_seen_at = excluded.last_seen_at",
    )
    .bind(user_id)
    .bind(device_id)
    .bind(meta.device_label.as_deref())
    .bind(clock_offset_secs)
    .bind(now)
    .bind(now)
    .execute(&state.core.pool)
    .await
    {
        // Clock metadata is advisory: a failed write must not fail the upload.
        tracing::warn!(error = %error, device = device_id, "could not record a save device");
    }
}

/// Last known human label for a device, falling back to its id.
async fn device_label(state: &AppState, user_id: i64, device_id: Option<&str>) -> Option<String> {
    let device_id = device_id?;
    let label = db::query_scalar::<Option<String>>(
        "SELECT label FROM save_devices WHERE user_id = ? AND device_id = ?",
    )
    .bind(user_id)
    .bind(device_id)
    .fetch_optional(&state.core.pool)
    .await
    .ok()
    .flatten()
    .flatten();
    Some(label.unwrap_or_else(|| device_id.to_string()))
}

#[derive(Debug, sqlx::FromRow)]
struct ConflictRow {
    id: i64,
    game_id: i64,
    reason: String,
    status: String,
    detected_at: String,
    resolved_at: Option<String>,
    strategy: Option<String>,
    existing_snapshot_id: i64,
    candidate_snapshot_id: i64,
}

async fn conflict_view(
    state: &AppState,
    row: &ConflictRow,
) -> Result<Option<SaveConflictView>, ApiError> {
    let Some(existing) = fetch_snapshot_by_id(state, row.existing_snapshot_id).await? else {
        return Ok(None);
    };
    let Some(candidate) = fetch_snapshot_by_id(state, row.candidate_snapshot_id).await? else {
        return Ok(None);
    };
    Ok(Some(SaveConflictView {
        id: row.id,
        game_id: row.game_id,
        status: row.status.clone(),
        reason: row.reason.clone(),
        detected_at: db::parse_time(&row.detected_at).unwrap_or_else(Utc::now),
        resolved_at: row.resolved_at.as_deref().and_then(db::parse_time),
        strategy: row.strategy.clone(),
        existing: existing.to_public(),
        candidate: candidate.to_public(),
    }))
}

async fn fetch_snapshot_by_id(
    state: &AppState,
    id: i64,
) -> Result<Option<SaveSnapshotRow>, ApiError> {
    db::query_as::<SaveSnapshotRow>(&format!(
        "SELECT {SNAPSHOT_COLUMNS} FROM save_snapshots WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(&state.core.pool)
    .await
    .map_err(ApiError::from)
}

async fn parse_device_value(
    field: axum::extract::multipart::Field<'_>,
    code: &'static str,
) -> Result<Option<String>, ApiError> {
    let value = field.text().await.map_err(map_multipart_error)?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    validate_device_value(&value, code)?;
    Ok(Some(value))
}

fn validate_device_value(value: &str, code: &'static str) -> Result<(), ApiError> {
    if value.chars().count() > MAX_DEVICE_ID_CHARS || value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(code, "The device value is invalid"));
    }
    Ok(())
}

async fn parse_timestamp_field(
    field: axum::extract::multipart::Field<'_>,
    code: &'static str,
) -> Result<DateTime<Utc>, ApiError> {
    let value = field.text().await.map_err(map_multipart_error)?;
    let value = value.trim();
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| ApiError::bad_request(code, "The timestamp must be an RFC 3339 instant"))
}

/// Promotes a cloud snapshot to a restore point and records the restore in the
/// game history (#295). The device that writes the local files reports itself
/// here, so the panel can answer "which machine wrote this save last".
async fn restore(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
    Json(request): Json<RestoreSnapshotRequest>,
) -> Result<(StatusCode, Json<RestoreSnapshotResponse>), ApiError> {
    user.require_scope(Scope::Write)?;
    let row = fetch_snapshot(&state, id).await?;
    ensure_access(&user, &row)?;

    let device_id = request
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(device_id) = device_id.as_deref() {
        if device_id.chars().count() > MAX_DEVICE_ID_CHARS
            || device_id.chars().any(char::is_control)
        {
            return Err(ApiError::bad_request(
                "invalid_device_id",
                "The device identifier is invalid",
            ));
        }
    }
    let kind = request.kind.as_deref().unwrap_or("server").trim();
    if kind != "server" && kind != "local" {
        return Err(ApiError::bad_request(
            "invalid_restore_kind",
            "kind must be 'server' or 'local'",
        ));
    }

    db::query("UPDATE save_snapshots SET pinned = 1 WHERE id = ?")
        .bind(id)
        .execute(&state.core.pool)
        .await
        .map_err(ApiError::from)?;

    let now = db::db_time(db::now());
    let receipt_id = db::query(
        "INSERT INTO save_restore_receipts
            (user_id, game_id, snapshot_id, device_id, kind, restored_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(row.game_id)
    .bind(id)
    .bind(device_id.as_deref())
    .bind(kind)
    .bind(&now)
    .returning_id(&state.core.pool)
    .await
    .map_err(ApiError::from)?;

    state.core.events.emit(
        Event::new(actions::SAVE_RESTORED, user.actor())
            .target("save", id.to_string())
            .metadata(serde_json::json!({
                "snapshotId": id,
                "gameId": row.game_id,
                "userId": user.id,
                "deviceId": device_id,
                "kind": kind,
            })),
    );

    Ok((
        StatusCode::CREATED,
        Json(RestoreSnapshotResponse {
            snapshot: fetch_snapshot(&state, id).await?.to_public(),
            receipt: SaveRestoreReceipt {
                id: receipt_id,
                game_id: row.game_id,
                snapshot_id: id,
                device_id,
                kind: kind.to_string(),
                restored_at: db::parse_time(&now).unwrap_or_else(chrono::Utc::now),
            },
        }),
    ))
}

/// Explicit retention pass (#295). `keep` defaults to the automatic retention;
/// pinned restore points and the newest snapshot are always kept.
async fn prune(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
    Json(request): Json<PruneSavesRequest>,
) -> Result<Json<SavePruneReport>, ApiError> {
    user.require_scope(Scope::Write)?;
    ensure_game(&state, game_id).await?;
    let keep = request.keep.unwrap_or(AUTO_SNAPSHOT_RETENTION);
    let report = prune_snapshots(&state, user.id, game_id, keep).await;
    state.core.events.emit(
        Event::new(actions::SAVE_PRUNED, user.actor())
            .target("game", game_id.to_string())
            .metadata(serde_json::json!({
                "gameId": game_id,
                "userId": user.id,
                "removed": report.removed,
                "kept": report.kept,
                "protected": report.protected,
            })),
    );
    Ok(Json(report))
}

/// Snapshot history and restore receipts for the game page (#295).
async fn history(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(game_id): ApiPath<i64>,
) -> Result<Json<SaveHistory>, ApiError> {
    ensure_game(&state, game_id).await?;
    let snapshots = db::query_as::<SaveSnapshotRow>(&format!(
        "SELECT {SNAPSHOT_COLUMNS}
         FROM save_snapshots
         WHERE user_id = ? AND game_id = ?
         ORDER BY created_at DESC, id DESC
         LIMIT ?"
    ))
    .bind(user.id)
    .bind(game_id)
    .bind(HISTORY_SNAPSHOT_LIMIT)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;
    let receipts = db::query_as::<SaveRestoreReceiptRow>(
        "SELECT id, game_id, snapshot_id, device_id, kind, restored_at
         FROM save_restore_receipts
         WHERE user_id = ? AND game_id = ?
         ORDER BY restored_at DESC, id DESC
         LIMIT ?",
    )
    .bind(user.id)
    .bind(game_id)
    .bind(HISTORY_RECEIPT_LIMIT)
    .fetch_all(&state.core.pool)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(SaveHistory {
        snapshots: snapshots
            .into_iter()
            .map(SaveSnapshotRow::to_public)
            .collect(),
        receipts: receipts
            .into_iter()
            .map(SaveRestoreReceiptRow::to_public)
            .collect(),
        auto_retention: AUTO_SNAPSHOT_RETENTION,
    }))
}

/// Keeps only the newest `keep` automatic snapshots for one user and game.
///
/// Same rules as [`prune_snapshots`]: pinned restore points, manual uploads and
/// the newest snapshot are never touched.
async fn prune_auto_snapshots(state: &AppState, user_id: i64, game_id: i64, keep: i64) {
    let _ = prune_snapshots(state, user_id, game_id, keep).await;
}

async fn ensure_game(state: &AppState, game_id: i64) -> Result<(), ApiError> {
    let exists = db::query_scalar::<i64>("SELECT id FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&state.core.pool)
        .await
        .map_err(ApiError::from)?;
    exists
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("game_not_found", "Game not found"))
}

async fn fetch_snapshot(state: &AppState, id: i64) -> Result<SaveSnapshotRow, ApiError> {
    fetch_snapshot_by_id(state, id)
        .await?
        .ok_or_else(|| ApiError::not_found("save_not_found", "Save snapshot not found"))
}

fn ensure_access(user: &AuthUser, row: &SaveSnapshotRow) -> Result<(), ApiError> {
    if row.user_id == user.id || user.is_admin() {
        Ok(())
    } else {
        // 404 rather than 403 so another user's snapshot never leaks existence.
        Err(ApiError::not_found(
            "save_not_found",
            "Save snapshot not found",
        ))
    }
}

fn map_multipart_error(error: axum::extract::multipart::MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "save_too_large",
            "The save archive exceeds the server limit",
        )
    } else {
        ApiError::bad_request("invalid_multipart", error.body_text())
    }
}

fn map_save_error(error: SaveError, max_bytes: u64) -> ApiError {
    match error {
        SaveError::TooLarge => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "save_too_large",
            format!("Save archives are limited to {max_bytes} bytes"),
        ),
        SaveError::Empty => {
            ApiError::bad_request("invalid_file", "The uploaded save archive is empty")
        }
        SaveError::Stream(message) => ApiError::bad_request("invalid_multipart", message),
        SaveError::Io(error) => {
            tracing::error!(error = %error, "could not store save archive");
            ApiError::internal()
        }
    }
}

/// OpenAPI operations served by this module (#237).
pub(crate) const DOCS: &[RouteDoc] = &[
    // Cloud saves
    route(
        HttpMethod::Get,
        "/games/{id}/saves",
        "Cloud saves",
        "List your save snapshots",
        Auth::Session,
        Payload::Array("SaveSnapshot"),
    ),
    route(
        HttpMethod::Post,
        "/games/{id}/saves",
        "Cloud saves",
        "Upload a save archive (multipart)",
        Auth::Session,
        Payload::Schema("SaveSnapshot"),
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/saves/history",
        "Cloud saves",
        "List snapshot history and restore receipts",
        Auth::Session,
        Payload::Schema("SaveHistory"),
    ),
    body(
        ok(
            route(
                HttpMethod::Post,
                "/games/{id}/saves/prune",
                "Cloud saves",
                "Prune old automatic snapshots (pinned and newest are kept)",
                Auth::Session,
                Payload::Schema("SavePruneReport"),
            ),
            200,
        ),
        "PruneSavesRequest",
    ),
    body(
        route(
            HttpMethod::Post,
            "/saves/{id}/restore",
            "Cloud saves",
            "Promote a snapshot to a restore point and record the restore",
            Auth::Session,
            Payload::Schema("RestoreSnapshotResponse"),
        ),
        "RestoreSnapshotRequest",
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/saves/policy",
        "Cloud saves",
        "Read the per-game conflict policy",
        Auth::Session,
        Payload::Schema("SavePolicyView"),
    ),
    route(
        HttpMethod::Put,
        "/games/{id}/saves/policy",
        "Cloud saves",
        "Set the per-game conflict policy ('ask' or 'prefer_device')",
        Auth::Session,
        Payload::Schema("SavePolicyView"),
    ),
    route(
        HttpMethod::Get,
        "/games/{id}/saves/conflicts",
        "Cloud saves",
        "List open save conflicts",
        Auth::Session,
        Payload::Array("SaveConflictView"),
    ),
    route(
        HttpMethod::Post,
        "/games/{id}/saves/conflicts/{conflict_id}/resolve",
        "Cloud saves",
        "Resolve a save conflict ('keep_local' or 'keep_remote'); both saves stay",
        Auth::Session,
        Payload::Schema("SaveConflictView"),
    ),
    route(
        HttpMethod::Get,
        "/saves/{id}/download",
        "Cloud saves",
        "Download a save archive",
        Auth::Session,
        Payload::Binary,
    ),
    ok(
        route(
            HttpMethod::Delete,
            "/saves/{id}",
            "Cloud saves",
            "Delete a save snapshot",
            Auth::Session,
            Payload::None,
        ),
        204,
    ),
];
