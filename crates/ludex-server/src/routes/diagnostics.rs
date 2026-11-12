//! Opt-in anonymous diagnostics reports (#171).
//!
//! `POST /api/diagnostics` is public and deliberately narrow: a bounded JSON
//! body, an extra per-address hourly throttle, strict field validation and a
//! second redaction pass before storage. The endpoint stores no account id and
//! no IP address; `install_id` is a random identifier generated on the device.
//!
//! `GET /api/diagnostics` and `DELETE /api/diagnostics/{id}` are admin-only and
//! expose the stored reports plus aggregate version/kind/platform counts.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::routing::{delete, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::config::{DIAGNOSTICS_RETENTION_DAYS, MAX_DIAGNOSTICS_BYTES};
use crate::error::ApiError;
use crate::events::{Actor, Event};
use crate::extract::{ApiPath, ApiQuery};
use crate::models::{
    DiagnosticsCount, DiagnosticsPage, DiagnosticsReportInput, DiagnosticsReportRow,
};
use crate::sql::{bind_query_as, Bind};
use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const MAX_SUMMARY_CHARS: usize = 2_000;
const MAX_DETAIL_BYTES: usize = 16 * 1024;

const REPORT_COLUMNS: &str =
    "id, install_id, app_version, platform, os, locale, channel, error_kind, summary, detail_json, created_at";

/// Diagnostics routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/diagnostics", post(submit).get(list))
        .route("/diagnostics/{id}", delete(remove))
}

/// `POST /api/diagnostics` — accept one explicitly sent report.
async fn submit(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    request: Request,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let declared_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared_length.is_some_and(|length| length > MAX_DIAGNOSTICS_BYTES) {
        return Err(payload_too_large());
    }

    if let Err(seconds) = state.diagnostics_limits.check(&peer.ip().to_string()) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many diagnostics reports; retry later",
        )
        .with_retry_after(seconds));
    }

    let bytes = axum::body::to_bytes(request.into_body(), MAX_DIAGNOSTICS_BYTES)
        .await
        .map_err(|_| payload_too_large())?;
    let input: DiagnosticsReportInput = serde_json::from_slice(&bytes)
        .map_err(|error| ApiError::bad_request("invalid_report", error.to_string()))?;
    let report = validate(input)?;

    let now = Utc::now();
    let cutoff = crate::db::db_time(now - Duration::days(DIAGNOSTICS_RETENTION_DAYS));
    // Retention sweep before the insert: the table stays bounded even when
    // nobody visits the admin page.
    if let Err(error) = crate::db::query("DELETE FROM diagnostics_reports WHERE created_at < ?")
        .bind(cutoff)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(error = %error, "diagnostics retention sweep failed");
    }

    crate::db::query(
        "INSERT INTO diagnostics_reports
            (install_id, app_version, platform, os, locale, channel, error_kind, summary, detail_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&report.install_id)
    .bind(&report.app_version)
    .bind(&report.platform)
    .bind(&report.os)
    .bind(&report.locale)
    .bind(&report.channel)
    .bind(&report.error_kind)
    .bind(&report.summary)
    .bind(&report.detail_json)
    .bind(crate::db::db_time(now))
    .execute(&state.pool)
    .await
    .map_err(ApiError::from)?;

    Ok((StatusCode::ACCEPTED, Json(json!({ "accepted": true }))))
}

/// `GET /api/diagnostics` — admin list with aggregate counts.
async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<DiagnosticsQuery>,
) -> Result<Json<DiagnosticsPage>, ApiError> {
    user.require_admin_access()?;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut filters: Vec<&str> = Vec::new();
    let mut binds: Vec<Bind> = Vec::new();
    if let Some(before) = query.before {
        filters.push("id < ?");
        binds.push(Bind::Int(before));
    }
    if let Some(kind) = query
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        filters.push("error_kind = ?");
        binds.push(Bind::Text(kind.to_string()));
    }
    let where_clause = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };
    binds.push(Bind::Int(limit + 1));

    let sql = format!(
        "SELECT {REPORT_COLUMNS} FROM diagnostics_reports {where_clause} ORDER BY id DESC LIMIT ?"
    );
    let mut rows = bind_query_as::<DiagnosticsReportRow>(crate::db::query_as(&sql), &binds)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)?;
    let has_more = rows.len() > usize::try_from(limit).unwrap_or(0);
    if has_more {
        rows.truncate(usize::try_from(limit).unwrap_or(0));
    }
    let next_before = has_more.then(|| rows.last().map(|row| row.id)).flatten();

    let total = crate::db::query_scalar::<i64>("SELECT COUNT(*) FROM diagnostics_reports")
        .fetch_one(&state.pool)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(DiagnosticsPage {
        items: rows.iter().map(DiagnosticsReportRow::to_public).collect(),
        next_before,
        total,
        by_version: counts(&state, "app_version").await?,
        by_kind: counts(&state, "error_kind").await?,
        by_platform: counts(&state, "platform").await?,
        retention_days: DIAGNOSTICS_RETENTION_DAYS,
    }))
}

/// `DELETE /api/diagnostics/{id}` — admin deletion.
async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<i64>,
) -> Result<StatusCode, ApiError> {
    user.require_admin_access()?;
    let result = crate::db::query("DELETE FROM diagnostics_reports WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(ApiError::from)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found(
            "report_not_found",
            "No diagnostics report with this id",
        ));
    }
    state.events.emit(
        Event::new("diagnostics.deleted", Actor::User { id: user.id })
            .target("diagnostics_report", id.to_string())
            .metadata(json!({ "reportId": id })),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn counts(state: &AppState, column: &str) -> Result<Vec<DiagnosticsCount>, ApiError> {
    // `column` is a hard-coded identifier from the callers above, never input.
    let sql = format!(
        "SELECT {column} AS value, COUNT(*) AS count FROM diagnostics_reports \
         GROUP BY {column} ORDER BY count DESC, value ASC LIMIT 20"
    );
    crate::db::query_as::<DiagnosticsCount>(&sql)
        .fetch_all(&state.pool)
        .await
        .map_err(ApiError::from)
}

/// Validated and redacted report ready for insertion.
#[derive(Debug)]
struct ValidatedReport {
    install_id: String,
    app_version: String,
    platform: String,
    os: String,
    locale: String,
    channel: String,
    error_kind: String,
    summary: String,
    detail_json: String,
}

fn validate(input: DiagnosticsReportInput) -> Result<ValidatedReport, ApiError> {
    let install_id = input.install_id.trim().to_string();
    if !(8..=64).contains(&install_id.len())
        || !install_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(ApiError::bad_request(
            "invalid_install_id",
            "installId must be 8-64 characters of [A-Za-z0-9_-]",
        ));
    }

    let app_version = clean_field(&input.app_version, 64, "invalid_app_version")?;
    let platform = input.platform.trim().to_ascii_lowercase();
    if !matches!(
        platform.as_str(),
        "linux" | "windows" | "macos" | "web" | "android" | "ios" | "other"
    ) {
        return Err(ApiError::bad_request(
            "invalid_platform",
            "platform must be linux, windows, macos, web, android, ios or other",
        ));
    }
    let os = clean_field(&input.os, 120, "invalid_os")?;
    let locale = optional_field(&input.locale, 32);
    let channel = optional_field(&input.channel, 32);
    let error_kind = input.error_kind.trim().to_ascii_lowercase();
    if !matches!(
        error_kind.as_str(),
        "panic" | "startup" | "command" | "api" | "install" | "launch" | "update" | "web" | "other"
    ) {
        return Err(ApiError::bad_request(
            "invalid_error_kind",
            "errorKind must be panic, startup, command, api, install, launch, update, web or other",
        ));
    }

    let summary = input.summary.trim();
    if summary.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_summary",
            "summary is required",
        ));
    }
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(ApiError::bad_request(
            "invalid_summary",
            "summary is too long",
        ));
    }
    let summary = ludex_core::redact::redact(summary, &server_context());
    let summary = truncate_chars(&summary, MAX_SUMMARY_CHARS);

    if !(input.detail.is_null() || input.detail.is_object()) {
        return Err(ApiError::bad_request(
            "invalid_detail",
            "detail must be a JSON object",
        ));
    }
    let raw_detail = serde_json::to_string(&input.detail).unwrap_or_default();
    if raw_detail.len() > MAX_DETAIL_BYTES {
        return Err(ApiError::bad_request(
            "detail_too_large",
            "detail exceeds the 16 KiB limit",
        ));
    }
    let detail = ludex_core::redact::redact_json(&input.detail, &server_context());
    let detail_json = serde_json::to_string(&detail).unwrap_or_else(|_| "{}".to_string());
    if detail_json.len() > MAX_DETAIL_BYTES {
        return Err(ApiError::bad_request(
            "detail_too_large",
            "detail exceeds the 16 KiB limit",
        ));
    }

    Ok(ValidatedReport {
        install_id,
        app_version,
        platform,
        os,
        locale,
        channel,
        error_kind,
        summary,
        detail_json,
    })
}

/// Server-side context for the second redaction pass. There is no account or
/// home directory here; the generic scanner still removes paths, URLs, IPs,
/// e-mails and credentials from a misbehaving client.
fn server_context() -> ludex_core::redact::RedactionContext {
    ludex_core::redact::RedactionContext::new()
}

fn clean_field(value: &str, max: usize, code: &str) -> Result<String, ApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max {
        return Err(ApiError::bad_request(code, "value is missing or too long"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ApiError::bad_request(
            code,
            "value contains control characters",
        ));
    }
    Ok(trimmed.to_string())
}

fn optional_field(value: &str, max: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(max)
        .collect()
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect()
}

fn payload_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "report_too_large",
        format!("Diagnostics reports are limited to {MAX_DIAGNOSTICS_BYTES} bytes"),
    )
}

/// Query parameters for the admin list.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsQuery {
    /// Page size, 1..=200 (default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Return rows with `id < before` (newest-first cursor).
    #[serde(default)]
    pub before: Option<i64>,
    /// Filter by error kind.
    #[serde(default)]
    pub kind: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> DiagnosticsReportInput {
        DiagnosticsReportInput {
            install_id: "abc12345def67890".to_string(),
            app_version: "0.3.2".to_string(),
            platform: "linux".to_string(),
            os: "Linux 6.8".to_string(),
            locale: "fr".to_string(),
            channel: "stable".to_string(),
            error_kind: "panic".to_string(),
            summary: "panic at /home/alice/Games/Hades: token=abcdef123456".to_string(),
            detail: json!({ "path": "/home/alice/Games/Hades", "url": "http://10.1.20.5:8080/api" }),
        }
    }

    #[test]
    fn validation_redacts_paths_and_tokens() {
        let report = validate(input()).expect("valid report");
        assert!(
            !report.summary.contains("/home/alice"),
            "{}",
            report.summary
        );
        assert!(
            !report.summary.contains("abcdef123456"),
            "{}",
            report.summary
        );
        assert!(
            !report.detail_json.contains("10.1.20.5"),
            "{}",
            report.detail_json
        );
        assert!(
            !report.detail_json.contains("/home/alice"),
            "{}",
            report.detail_json
        );
    }

    #[test]
    fn install_id_is_strict() {
        let mut report = input();
        report.install_id = "short".to_string();
        assert!(validate(report).is_err());

        let mut report = input();
        report.install_id = "with/slash/inside".to_string();
        assert!(validate(report).is_err());
    }

    #[test]
    fn unknown_platforms_and_kinds_are_rejected() {
        let mut report = input();
        report.platform = "solaris".to_string();
        assert!(validate(report).is_err());

        let mut report = input();
        report.error_kind = "exploit".to_string();
        assert!(validate(report).is_err());
    }

    #[test]
    fn oversized_detail_is_rejected() {
        let mut report = input();
        report.detail = json!({ "blob": "x".repeat(MAX_DETAIL_BYTES + 1) });
        let error = validate(report).expect_err("too large");
        assert_eq!(error.code, "detail_too_large");
    }

    #[test]
    fn missing_summary_is_rejected() {
        let mut report = input();
        report.summary = "   ".to_string();
        assert!(validate(report).is_err());
    }
}
