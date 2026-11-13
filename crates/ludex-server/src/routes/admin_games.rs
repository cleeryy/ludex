//! Admin Games manager (#209).
//!
//! One administrator-only surface to find, inspect and act on library rows
//! without loading the whole library into the browser: the list is filtered,
//! sorted and paginated in SQL, and destructive operations are previewable,
//! confirmed, audited and — for a large archive deletion — run as a background
//! job with progress.
//!
//! The single-game delete lives on `DELETE /api/games/{id}` (#210); this module
//! reuses [`crate::routes::games::remove_game`] so both entry points share the
//! exact same path checks and cascade rules.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex, MutexGuard};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::audit::actions;
use crate::auth::AuthUser;
use crate::db;
use crate::error::ApiError;
use crate::events::Event;
use crate::extract::{ApiPath, ApiQuery, JsonBody};
use crate::routes::games::{remove_game, DeleteMode};
use crate::sql::{bind_query_as, bind_query_scalar, Bind};
use crate::state::AppState;

/// A bulk archive deletion with more items than this runs as a job instead of
/// holding the request open.
const BULK_JOB_THRESHOLD: usize = 25;
/// Hard cap on one bulk request.
const BULK_MAX_ITEMS: usize = 500;
/// Completed job reports are pruned after this many entries.
const JOB_RETENTION: usize = 32;

const AVAILABLE_COUNT: &str = "(SELECT COUNT(*) FROM game_versions v \
     WHERE v.game_id = g.id AND v.missing = 0 AND v.root_unavailable = 0)";
const MISSING_COUNT: &str = "(SELECT COUNT(*) FROM game_versions v \
     WHERE v.game_id = g.id AND (v.missing = 1 OR v.root_unavailable = 1))";
const TOTAL_SIZE: &str = "(SELECT CAST(COALESCE(SUM(v.size), 0) AS BIGINT) \
     FROM game_versions v WHERE v.game_id = g.id \
     AND v.missing = 0 AND v.root_unavailable = 0)";

/// Admin Games routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/games", get(list))
        .route("/admin/games/bulk", post(bulk))
        .route("/admin/games/jobs/{id}", get(job))
}

/// One row of the admin table.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminGameRow {
    /// Database identifier.
    pub id: i64,
    /// Display title.
    pub title: String,
    /// Release date (RFC 3339), when known.
    pub release_date: Option<String>,
    /// `game` or `rom`.
    pub kind: String,
    /// Platform slug for ROMs.
    pub platform: Option<String>,
    /// Metadata provider slug.
    pub provider_slug: Option<String>,
    /// Cached cover URL.
    pub cover_url: Option<String>,
    /// Row creation timestamp.
    pub created_at: String,
    /// Last metadata/version change.
    pub updated_at: String,
    /// Pinned version, if any.
    pub default_version_id: Option<i64>,
    /// Versions with a usable archive.
    pub available_version_count: i64,
    /// Versions marked missing or on an offline root.
    pub missing_version_count: i64,
    /// `available`, `partially_missing` or `missing`.
    pub status: String,
    /// Bytes of available archives.
    pub total_size: i64,
    /// Desktop `game_installed` reports recorded for this game.
    pub installed_reports: i64,
    /// Most recent desktop install report.
    pub last_installed_at: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct AdminGameRowDb {
    id: i64,
    title: String,
    release_date: Option<String>,
    kind: String,
    platform: Option<String>,
    provider_slug: Option<String>,
    cover_path: Option<String>,
    created_at: String,
    updated_at: String,
    default_version_id: Option<i64>,
    available_version_count: i64,
    missing_version_count: i64,
    total_size: i64,
    installed_reports: i64,
    last_installed_at: Option<String>,
}

/// Per-status counts after the non-status filters.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminGameCounts {
    /// Rows with at least one available archive and none missing.
    pub available: i64,
    /// Rows with available archives and at least one missing one.
    pub partially_missing: i64,
    /// Rows with no usable archive (the file was deleted outside Ludex).
    pub missing: i64,
    /// Every row after the non-status filters.
    pub total: i64,
}

/// Paginated admin list payload.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminGamesResponse {
    /// Rows on this page.
    pub items: Vec<AdminGameRow>,
    /// Rows matching every filter (including `status`).
    pub total: i64,
    /// 1-based page.
    pub page: i64,
    /// Page size.
    pub limit: i64,
    /// Facet counts for the table toolbar.
    pub counts: AdminGameCounts,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminGamesQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    page: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

fn like_pattern(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len() + 2);
    for character in raw.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    format!("%{}%", escaped.to_lowercase())
}

fn where_sql(clauses: &[String]) -> String {
    if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    }
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<AdminGamesQuery>,
) -> Result<Json<AdminGamesResponse>, ApiError> {
    user.require_admin_access()?;

    let mut clauses: Vec<String> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();
    if let Some(search) = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        clauses.push("LOWER(g.title) LIKE ? ESCAPE '\\'".to_string());
        binds.push(Bind::Text(like_pattern(search)));
    }
    if let Some(provider) = query
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        clauses.push("g.provider_slug = ?".to_string());
        binds.push(Bind::Text(provider.to_string()));
    }
    if let Some(kind) = query
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if kind != "game" && kind != "rom" {
            return Err(ApiError::bad_request(
                "invalid_kind",
                "kind must be `game` or `rom`",
            ));
        }
        clauses.push("g.kind = ?".to_string());
        binds.push(Bind::Text(kind.to_string()));
    }

    let status = query
        .status
        .as_deref()
        .map(str::trim)
        .unwrap_or("available");
    if !matches!(status, "" | "available" | "missing" | "partial" | "all") {
        return Err(ApiError::bad_request(
            "invalid_status",
            format!("Unknown status filter: {status}"),
        ));
    }
    let status_clause = match status {
        "" | "available" => Some(format!("{AVAILABLE_COUNT} > 0")),
        "missing" => Some(format!("{AVAILABLE_COUNT} = 0")),
        "partial" => Some(format!("{AVAILABLE_COUNT} > 0 AND {MISSING_COUNT} > 0")),
        "all" => None,
        _ => unreachable!(),
    };

    let sort = query.sort.as_deref().unwrap_or("title");
    let sort_sql = match sort {
        "title" => "g.sort_title",
        "releaseDate" => "g.release_date",
        "addedAt" => "g.created_at",
        "updatedAt" => "g.updated_at",
        "size" => "total_size",
        "versions" => "available_version_count",
        "missing" => "missing_version_count",
        "installed" => "last_installed_at",
        other => {
            return Err(ApiError::bad_request(
                "invalid_sort",
                format!("Unknown sort column: {other}"),
            ));
        }
    };
    let direction = match query.order.as_deref().unwrap_or("asc") {
        "asc" => "ASC",
        "desc" => "DESC",
        other => {
            return Err(ApiError::bad_request(
                "invalid_order",
                format!("Unknown sort order: {other}"),
            ));
        }
    };

    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1).saturating_mul(limit);

    let mut filtered = clauses.clone();
    if let Some(extra) = &status_clause {
        filtered.push(extra.clone());
    }
    let mut page_binds = binds.clone();
    page_binds.push(Bind::Int(limit));
    page_binds.push(Bind::Int(offset));

    let sql = format!(
        "SELECT g.id, g.title, g.release_date, g.kind, g.platform, g.provider_slug,
                g.cover_path, g.created_at, g.updated_at, g.default_version_id,
                {AVAILABLE_COUNT} AS available_version_count,
                {MISSING_COUNT} AS missing_version_count,
                {TOTAL_SIZE} AS total_size,
                (SELECT COUNT(*) FROM activities a
                  WHERE a.game_id = g.id AND a.kind = 'game_installed') AS installed_reports,
                (SELECT MAX(a.created_at) FROM activities a
                  WHERE a.game_id = g.id AND a.kind = 'game_installed') AS last_installed_at
         FROM games g{}{}
         ORDER BY ({} IS NULL) ASC, {} {}, g.id ASC
         LIMIT ? OFFSET ?",
        where_sql(&filtered),
        "",
        sort_sql,
        sort_sql,
        direction
    );
    let rows = bind_query_as::<AdminGameRowDb>(db::query_as(&sql), &page_binds)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;

    let total = bind_query_scalar::<i64>(
        db::query_scalar(format!(
            "SELECT COUNT(*) FROM games g{}",
            where_sql(&filtered)
        )),
        &binds,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let base = where_sql(&clauses);
    let available = facet_count(
        &state,
        &format!(
            "{base}{} {AVAILABLE_COUNT} > 0 AND {MISSING_COUNT} = 0",
            separator(&clauses)
        ),
        &binds,
    )
    .await?;
    let partially_missing = facet_count(
        &state,
        &format!(
            "{base}{} {AVAILABLE_COUNT} > 0 AND {MISSING_COUNT} > 0",
            separator(&clauses)
        ),
        &binds,
    )
    .await?;
    let missing = facet_count(
        &state,
        &format!("{base}{} {AVAILABLE_COUNT} = 0", separator(&clauses)),
        &binds,
    )
    .await?;
    let all = facet_count(&state, &base, &binds).await?;

    Ok(Json(AdminGamesResponse {
        items: rows.into_iter().map(row_view).collect(),
        total,
        page,
        limit,
        counts: AdminGameCounts {
            available,
            partially_missing,
            missing,
            total: all,
        },
    }))
}

fn separator(clauses: &[String]) -> &'static str {
    if clauses.is_empty() {
        " WHERE"
    } else {
        " AND"
    }
}

async fn facet_count(state: &AppState, clause: &str, binds: &[Bind]) -> Result<i64, ApiError> {
    let sql = format!("SELECT COUNT(*) FROM games g{clause}");
    bind_query_scalar::<i64>(db::query_scalar(sql), binds)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)
}

fn row_view(row: AdminGameRowDb) -> AdminGameRow {
    AdminGameRow {
        id: row.id,
        title: row.title,
        release_date: row.release_date,
        kind: row.kind,
        platform: row.platform,
        provider_slug: row.provider_slug,
        cover_url: row.cover_path.as_deref().map(crate::media::media_url),
        created_at: row.created_at,
        updated_at: row.updated_at,
        default_version_id: row.default_version_id,
        available_version_count: row.available_version_count,
        missing_version_count: row.missing_version_count,
        status: crate::routes::games::version_status(
            row.available_version_count,
            row.missing_version_count,
        )
        .to_string(),
        total_size: row.total_size,
        installed_reports: row.installed_reports,
        last_installed_at: row.last_installed_at,
    }
}

/// Bulk request body (`POST /api/admin/games/bulk`).
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkRequest {
    /// Game ids; duplicates are removed, 1..=500.
    #[serde(default)]
    pub ids: Vec<i64>,
    /// `remove_from_library`, `delete_archives`, `delete_missing_versions` or
    /// `refresh_metadata`.
    pub action: String,
    /// Required for every action that removes rows or files.
    #[serde(default)]
    pub confirm: bool,
    /// Return the exact counts and paths without changing anything.
    #[serde(default)]
    pub dry_run: bool,
}

/// One game in a dry-run preview.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkPreviewItem {
    /// Game id.
    pub game_id: i64,
    /// Title, so the confirmation can be checked by a human.
    pub title: String,
    /// Rows this action would touch.
    pub versions: i64,
    /// Available versions.
    pub available_version_count: i64,
    /// Missing or offline versions.
    pub missing_version_count: i64,
    /// Files this action would unlink.
    pub files: i64,
    /// Bytes those files occupy.
    pub bytes: i64,
    /// Library-relative paths, capped at 200 for the response.
    pub paths: Vec<String>,
}

/// Dry-run summary.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkPreview {
    /// One entry per requested game, in request order.
    pub items: Vec<BulkPreviewItem>,
    /// Number of games.
    pub games: i64,
    /// Files that would be unlinked.
    pub files: i64,
    /// Bytes that would be freed.
    pub bytes: i64,
}

/// Per-item outcome.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkItemResult {
    /// Game id.
    pub game_id: i64,
    /// Whether the action completed for this game.
    pub ok: bool,
    /// Error code when `ok` is false.
    pub code: Option<String>,
    /// Human-readable error.
    pub message: Option<String>,
    /// Files unlinked.
    pub deleted_files: i64,
    /// Version rows removed.
    pub deleted_versions: i64,
    /// Bytes reclaimed.
    pub freed_bytes: i64,
}

/// Aggregate outcome.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkTotals {
    /// Requested games.
    pub games: i64,
    /// Games that succeeded.
    pub succeeded: i64,
    /// Games that failed (the rest still ran).
    pub failed: i64,
    /// Files unlinked.
    pub files: i64,
    /// Bytes reclaimed.
    pub bytes: i64,
}

/// Bulk response; `jobId` is set when the deletion runs in the background.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkResponse {
    /// Action that ran or was previewed.
    pub action: String,
    /// True for a preview.
    pub dry_run: bool,
    /// Preview payload when `dryRun` is true.
    pub preview: Option<BulkPreview>,
    /// Background job id when `status` is 202.
    pub job_id: Option<String>,
    /// Per-item results (empty while a job is running).
    pub results: Vec<BulkItemResult>,
    /// Aggregate counts.
    pub totals: BulkTotals,
}

/// Background bulk job state.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkJobView {
    /// Job id.
    pub id: String,
    /// Action being executed.
    pub action: String,
    /// `running`, `completed` or `failed`.
    pub status: String,
    /// Requested games.
    pub total: i64,
    /// Games processed so far.
    pub done: i64,
    /// Games that succeeded.
    pub succeeded: i64,
    /// Games that failed.
    pub failed: i64,
    /// Start timestamp.
    pub started_at: String,
    /// Completion timestamp.
    pub finished_at: Option<String>,
    /// Per-item results, appended as the job advances.
    pub results: Vec<BulkItemResult>,
}

static BULK_JOBS: LazyLock<Mutex<HashMap<String, BulkJobView>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn jobs() -> MutexGuard<'static, HashMap<String, BulkJobView>> {
    BULK_JOBS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// Actions accepted by the bulk endpoint.
const BULK_ACTIONS: [&str; 4] = [
    "remove_from_library",
    "delete_archives",
    "delete_missing_versions",
    "refresh_metadata",
];

fn destructive(action: &str) -> bool {
    matches!(
        action,
        "remove_from_library" | "delete_archives" | "delete_missing_versions"
    )
}

async fn bulk(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(request): JsonBody<BulkRequest>,
) -> Result<(StatusCode, Json<BulkResponse>), ApiError> {
    user.require_admin_access()?;
    let action = request.action.trim();
    if !BULK_ACTIONS.contains(&action) {
        return Err(ApiError::bad_request(
            "unknown_bulk_action",
            format!("Unknown bulk action: {action}"),
        ));
    }
    let mut ids: Vec<i64> = Vec::new();
    for id in &request.ids {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    if ids.is_empty() {
        return Err(ApiError::bad_request(
            "no_games",
            "Select at least one game",
        ));
    }
    if ids.len() > BULK_MAX_ITEMS {
        return Err(ApiError::bad_request(
            "too_many_games",
            format!("At most {BULK_MAX_ITEMS} games per request"),
        ));
    }
    if destructive(action) && !request.dry_run && !request.confirm {
        return Err(ApiError::bad_request(
            "confirmation_required",
            "Pass confirm=true after checking the preview; nothing was changed",
        ));
    }
    if request.dry_run && !destructive(action) {
        return Err(ApiError::bad_request(
            "dry_run_unsupported",
            "This action has no preview; run it directly",
        ));
    }

    if request.dry_run {
        let preview = preview_bulk(&state, action, &ids).await?;
        return Ok((
            StatusCode::OK,
            Json(BulkResponse {
                action: action.to_string(),
                dry_run: true,
                preview: Some(preview),
                job_id: None,
                results: Vec::new(),
                totals: BulkTotals::default(),
            }),
        ));
    }

    if action == "delete_archives" && ids.len() > BULK_JOB_THRESHOLD {
        let job = start_bulk_job(&state, &user, action, ids);
        return Ok((
            StatusCode::ACCEPTED,
            Json(BulkResponse {
                action: action.to_string(),
                dry_run: false,
                preview: None,
                job_id: Some(job.id.clone()),
                results: Vec::new(),
                totals: BulkTotals {
                    games: job.total,
                    ..BulkTotals::default()
                },
            }),
        ));
    }

    let mut results = Vec::new();
    for id in &ids {
        results.push(run_bulk_item(&state, &user, action, *id).await);
    }
    let totals = totals_of(&results);
    state.events.emit(
        Event::new(actions::GAME_BULK_ACTION, user.actor())
            .target("games", "bulk")
            .metadata(serde_json::json!({
                "action": action,
                "games": totals.games,
                "succeeded": totals.succeeded,
                "failed": totals.failed,
                "files": totals.files,
                "bytes": totals.bytes,
                "ids": ids,
            })),
    );
    Ok((
        StatusCode::OK,
        Json(BulkResponse {
            action: action.to_string(),
            dry_run: false,
            preview: None,
            job_id: None,
            results,
            totals,
        }),
    ))
}

fn totals_of(results: &[BulkItemResult]) -> BulkTotals {
    BulkTotals {
        games: results.len() as i64,
        succeeded: results.iter().filter(|item| item.ok).count() as i64,
        failed: results.iter().filter(|item| !item.ok).count() as i64,
        files: results.iter().map(|item| item.deleted_files).sum(),
        bytes: results.iter().map(|item| item.freed_bytes).sum(),
    }
}

async fn preview_bulk(
    state: &AppState,
    action: &str,
    ids: &[i64],
) -> Result<BulkPreview, ApiError> {
    let placeholders = vec!["?"; ids.len()].join(",");
    let binds: Vec<Bind> = ids.iter().copied().map(Bind::Int).collect();
    let sql = format!(
        "SELECT g.id, g.title, g.default_version_id, v.id, v.file_path, v.size,
                v.missing, v.root_unavailable
         FROM games g
         LEFT JOIN game_versions v ON v.game_id = g.id
         WHERE g.id IN ({placeholders})
         ORDER BY g.id, v.id"
    );
    let rows = bind_query_as::<(
        i64,
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    )>(db::query_as(&sql), &binds)
    .fetch_all(&state.pool)
    .await
    .map_err(ApiError::from)?;

    let mut grouped: BTreeMap<i64, BulkPreviewItem> = BTreeMap::new();
    for (game_id, title, _default, _version_id, path, size, missing, root_unavailable) in rows {
        let item = grouped.entry(game_id).or_insert_with(|| BulkPreviewItem {
            game_id,
            title,
            versions: 0,
            available_version_count: 0,
            missing_version_count: 0,
            files: 0,
            bytes: 0,
            paths: Vec::new(),
        });
        let Some(path) = path else { continue };
        item.versions += 1;
        let size = size.unwrap_or(0);
        if missing.unwrap_or(0) != 0 || root_unavailable.unwrap_or(0) != 0 {
            item.missing_version_count += 1;
            continue;
        }
        item.available_version_count += 1;
        if action == "delete_archives" && item.paths.len() < 200 {
            item.paths.push(path);
        }
        item.files += 1;
        item.bytes += size;
    }
    // Report a requested id that no longer exists instead of silently dropping
    // it: the confirmation count must match what the operator selected.
    for id in ids {
        grouped.entry(*id).or_insert_with(|| BulkPreviewItem {
            game_id: *id,
            title: String::new(),
            versions: 0,
            available_version_count: 0,
            missing_version_count: 0,
            files: 0,
            bytes: 0,
            paths: Vec::new(),
        });
    }

    let items: Vec<BulkPreviewItem> = grouped.into_values().collect();
    Ok(BulkPreview {
        games: items.len() as i64,
        files: items.iter().map(|item| item.files).sum(),
        bytes: items.iter().map(|item| item.bytes).sum(),
        items,
    })
}

async fn run_bulk_item(
    state: &AppState,
    user: &AuthUser,
    action: &str,
    game_id: i64,
) -> BulkItemResult {
    match action {
        "remove_from_library" | "delete_archives" => {
            let mode = if action == "delete_archives" {
                DeleteMode::Archives
            } else {
                DeleteMode::Library
            };
            match remove_game(state, user, game_id, mode).await {
                Ok(outcome) => BulkItemResult {
                    game_id,
                    ok: true,
                    code: None,
                    message: None,
                    deleted_files: outcome.deleted_files,
                    deleted_versions: outcome.versions,
                    freed_bytes: outcome.freed_bytes,
                },
                Err(error) => BulkItemResult {
                    game_id,
                    ok: false,
                    code: Some(error.code.clone()),
                    message: Some(error.message.clone()),
                    deleted_files: 0,
                    deleted_versions: 0,
                    freed_bytes: 0,
                },
            }
        }
        "delete_missing_versions" => match delete_missing_versions(state, game_id).await {
            Ok(deleted) => BulkItemResult {
                game_id,
                ok: true,
                code: None,
                message: None,
                deleted_files: 0,
                deleted_versions: deleted,
                freed_bytes: 0,
            },
            Err(error) => BulkItemResult {
                game_id,
                ok: false,
                code: Some(error.code.clone()),
                message: Some(error.message.clone()),
                deleted_files: 0,
                deleted_versions: 0,
                freed_bytes: 0,
            },
        },
        "refresh_metadata" => {
            let exists = db::query_scalar::<i64>("SELECT COUNT(*) FROM games WHERE id = ?")
                .bind(game_id)
                .fetch_one(&state.pool)
                .await
                .unwrap_or(0);
            if exists == 0 {
                return BulkItemResult {
                    game_id,
                    ok: false,
                    code: Some("game_not_found".to_string()),
                    message: Some("Game not found".to_string()),
                    deleted_files: 0,
                    deleted_versions: 0,
                    freed_bytes: 0,
                };
            }
            let state_clone = state.clone();
            tokio::spawn(async move {
                crate::metadata::enrich_game(&state_clone, game_id).await;
            });
            BulkItemResult {
                game_id,
                ok: true,
                code: None,
                message: None,
                deleted_files: 0,
                deleted_versions: 0,
                freed_bytes: 0,
            }
        }
        _ => unreachable!("validated action"),
    }
}

/// Drops version rows whose archive vanished, never the files. The game and
/// every other row stay; a rescan of a restored file adds the version back.
async fn delete_missing_versions(state: &AppState, game_id: i64) -> Result<i64, ApiError> {
    let exists = db::query_scalar::<i64>("SELECT COUNT(*) FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if exists == 0 {
        return Err(ApiError::not_found("game_not_found", "Game not found"));
    }
    let deleted = db::query(
        "DELETE FROM game_versions WHERE game_id = ? AND (missing = 1 OR root_unavailable = 1)",
    )
    .bind(game_id)
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?
    .rows_affected() as i64;
    if deleted > 0 {
        db::query(
            "UPDATE games SET default_version_id = NULL, updated_at = ?
             WHERE id = ? AND default_version_id IS NOT NULL
             AND default_version_id NOT IN (SELECT id FROM game_versions WHERE game_id = ?)",
        )
        .bind(db::db_time(db::now()))
        .bind(game_id)
        .bind(game_id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    }
    Ok(deleted)
}

fn start_bulk_job(state: &AppState, user: &AuthUser, action: &str, ids: Vec<i64>) -> BulkJobView {
    let id = format!("bulk-{}", uuid::Uuid::new_v4());
    let job = BulkJobView {
        id: id.clone(),
        action: action.to_string(),
        status: "running".to_string(),
        total: ids.len() as i64,
        done: 0,
        succeeded: 0,
        failed: 0,
        started_at: db::db_time(db::now()),
        finished_at: None,
        results: Vec::new(),
    };
    {
        let mut registry = jobs();
        while registry.len() >= JOB_RETENTION {
            let Some(oldest) = registry
                .iter()
                .find(|(_, entry)| entry.status != "running")
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            registry.remove(&oldest);
        }
        registry.insert(id.clone(), job.clone());
    }

    let state = state.clone();
    let user = user.clone();
    let action = action.to_string();
    tokio::spawn(async move {
        let mut results = Vec::new();
        for game_id in ids {
            let result = run_bulk_item(&state, &user, &action, game_id).await;
            {
                let mut registry = jobs();
                if let Some(entry) = registry.get_mut(&id) {
                    entry.done += 1;
                    if result.ok {
                        entry.succeeded += 1;
                    } else {
                        entry.failed += 1;
                    }
                    entry.results.push(result.clone());
                }
            }
            results.push(result);
        }
        let totals = totals_of(&results);
        {
            let mut registry = jobs();
            if let Some(entry) = registry.get_mut(&id) {
                entry.status = "completed".to_string();
                entry.finished_at = Some(db::db_time(db::now()));
            }
        }
        state.events.emit(
            Event::new(actions::GAME_BULK_ACTION, user.actor())
                .target("games", "bulk")
                .metadata(serde_json::json!({
                    "action": action,
                    "jobId": id,
                    "games": totals.games,
                    "succeeded": totals.succeeded,
                    "failed": totals.failed,
                    "files": totals.files,
                    "bytes": totals.bytes,
                })),
        );
    });

    job
}

async fn job(
    State(_state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<BulkJobView>, ApiError> {
    user.require_admin_access()?;
    jobs()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("job_not_found", "Unknown bulk job"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern("100%_Hades\\"), "%100\\%\\_hades\\\\%");
    }

    #[test]
    fn only_row_and_file_actions_need_confirmation() {
        assert!(destructive("remove_from_library"));
        assert!(destructive("delete_archives"));
        assert!(destructive("delete_missing_versions"));
        assert!(!destructive("refresh_metadata"));
    }
}
