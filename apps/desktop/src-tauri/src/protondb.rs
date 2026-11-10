//! ProtonDB compatibility ratings (#91).
//!
//! The rating is a public summary: `GET
//! https://www.protondb.com/api/v1/reports/summaries/{appid}.json`. Responses
//! are cached in the desktop SQLite index for [`CACHE_TTL_DAYS`] days so the
//! game page never waits on the network twice, and an offline machine keeps
//! showing the last known tier.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

pub const CACHE_TTL_DAYS: i64 = 7;
pub const SUMMARY_BASE: &str = "https://www.protondb.com/api/v1/reports/summaries";

/// The tiers ProtonDB reports, in decreasing order of compatibility.
pub const TIERS: [&str; 6] = ["platinum", "gold", "silver", "bronze", "borked", "pending"];

pub fn summary_url(app_id: u32) -> String {
    format!("{SUMMARY_BASE}/{app_id}.json")
}

pub fn game_url(app_id: u32) -> String {
    format!("https://www.protondb.com/app/{app_id}")
}

/// Resolve the Steam app id used for a rating.
///
/// Priority: explicit request, stored per-game id, then the id of an imported
/// `steam://rungameid/…` entry. No id means the UI asks the user for one.
pub fn resolve_app_id(
    explicit: Option<u32>,
    stored: Option<u32>,
    launch_uri: Option<&str>,
) -> Option<u32> {
    explicit
        .filter(|app_id| *app_id > 0)
        .or(stored.filter(|app_id| *app_id > 0))
        .or_else(|| launch_uri.and_then(crate::steam::app_id_from_uri))
}

/// A parsed ProtonDB summary.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub tier: String,
    pub best_tier: Option<String>,
    pub trending_tier: Option<String>,
    pub confidence: Option<String>,
    pub total: i64,
    pub score: Option<f64>,
}

/// Rating shown in the UI. `status` is one of `ok`, `cached`, `stale`,
/// `unavailable`, `notFound` or `noAppId`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtonDbRating {
    pub app_id: Option<u32>,
    pub status: String,
    pub tier: Option<String>,
    pub best_tier: Option<String>,
    pub trending_tier: Option<String>,
    pub confidence: Option<String>,
    pub total: i64,
    pub score: Option<f64>,
    pub url: Option<String>,
    pub fetched_at: Option<String>,
    pub message: Option<String>,
}

impl ProtonDbRating {
    pub fn no_app_id() -> Self {
        Self {
            app_id: None,
            status: "noAppId".into(),
            tier: None,
            best_tier: None,
            trending_tier: None,
            confidence: None,
            total: 0,
            score: None,
            url: None,
            fetched_at: None,
            message: Some(
                "Set a Steam app id in the launch options to see the ProtonDB rating.".into(),
            ),
        }
    }

    pub fn unavailable(app_id: u32, message: impl Into<String>) -> Self {
        Self {
            app_id: Some(app_id),
            status: "unavailable".into(),
            tier: None,
            best_tier: None,
            trending_tier: None,
            confidence: None,
            total: 0,
            score: None,
            url: Some(game_url(app_id)),
            fetched_at: None,
            message: Some(message.into()),
        }
    }

    pub fn not_found(app_id: u32) -> Self {
        Self {
            app_id: Some(app_id),
            status: "notFound".into(),
            tier: None,
            best_tier: None,
            trending_tier: None,
            confidence: None,
            total: 0,
            score: None,
            url: Some(game_url(app_id)),
            fetched_at: None,
            message: Some("ProtonDB has no reports for this app id.".into()),
        }
    }

    pub fn from_summary(
        app_id: u32,
        summary: &Summary,
        fetched_at: &str,
        stale: bool,
        cached: bool,
    ) -> Self {
        let status = if stale {
            "stale"
        } else if cached {
            "cached"
        } else {
            "ok"
        };
        Self {
            app_id: Some(app_id),
            status: status.into(),
            tier: Some(summary.tier.clone()),
            best_tier: summary.best_tier.clone(),
            trending_tier: summary.trending_tier.clone(),
            confidence: summary.confidence.clone(),
            total: summary.total,
            score: summary.score,
            url: Some(game_url(app_id)),
            fetched_at: Some(fetched_at.to_string()),
            message: None,
        }
    }
}

/// Normalise a tier name; unknown values are dropped rather than guessed.
pub fn normalize_tier(raw: &str) -> Option<String> {
    let tier = raw.trim().to_ascii_lowercase();
    TIERS.contains(&tier.as_str()).then_some(tier)
}

fn opt_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Parse the ProtonDB summary JSON. `tier` is required; the extra fields are
/// best effort because ProtonDB adds and removes them over time.
pub fn parse_summary(body: &str) -> Result<Summary, String> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| format!("Invalid ProtonDB response: {error}"))?;
    if !value.is_object() {
        return Err("ProtonDB returned an unexpected payload.".to_string());
    }

    let tier = opt_string(&value, "tier")
        .or_else(|| opt_string(&value, "bestReportedTier"))
        .or_else(|| opt_string(&value, "trendingTier"))
        .and_then(|raw| normalize_tier(&raw))
        .ok_or_else(|| "ProtonDB returned no compatibility tier.".to_string())?;

    let total = value
        .get("total")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let score = value.get("score").and_then(Value::as_f64);

    Ok(Summary {
        tier,
        best_tier: opt_string(&value, "bestReportedTier").and_then(|raw| normalize_tier(&raw)),
        trending_tier: opt_string(&value, "trendingTier").and_then(|raw| normalize_tier(&raw)),
        confidence: opt_string(&value, "confidence"),
        total,
        score,
    })
}

/// True when a cache row is younger than the TTL.
pub fn cache_is_fresh(fetched_at: &str, now: DateTime<Utc>, ttl_days: i64) -> bool {
    let Ok(fetched) = DateTime::parse_from_rfc3339(fetched_at) else {
        return false;
    };
    let age = now
        .signed_duration_since(fetched.with_timezone(&Utc))
        .num_seconds();
    // A small negative age is clock skew, not a reason to refetch forever.
    age > -3600 && age < ttl_days.max(0) * 86_400
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SUMMARY: &str = r#"{
        "bestReportedTier": "platinum",
        "confidence": "strong",
        "score": 0.82,
        "tier": "gold",
        "total": 412,
        "trendingTier": "platinum"
    }"#;

    #[test]
    fn parses_a_real_summary() {
        let summary = parse_summary(SUMMARY).unwrap();
        assert_eq!(summary.tier, "gold");
        assert_eq!(summary.best_tier.as_deref(), Some("platinum"));
        assert_eq!(summary.trending_tier.as_deref(), Some("platinum"));
        assert_eq!(summary.confidence.as_deref(), Some("strong"));
        assert_eq!(summary.total, 412);
        assert_eq!(summary.score, Some(0.82));
    }

    #[test]
    fn tolerates_missing_optional_fields_and_case() {
        let summary = parse_summary(r#"{"tier":"Gold","total":"12"}"#).unwrap();
        assert_eq!(summary.tier, "gold");
        assert_eq!(summary.total, 0, "wrong types are ignored, not fatal");
        assert!(summary.best_tier.is_none());

        let summary = parse_summary(r#"{"bestReportedTier":"borked"}"#).unwrap();
        assert_eq!(summary.tier, "borked");
    }

    #[test]
    fn rejects_payloads_without_a_known_tier() {
        assert!(parse_summary("{}").is_err());
        assert!(parse_summary(r#"{"tier":"diamond"}"#).is_err());
        assert!(parse_summary("not json").is_err());
        assert!(parse_summary("[]").is_err());
        assert_eq!(normalize_tier(" PLATINUM ").as_deref(), Some("platinum"));
        assert!(normalize_tier("").is_none());
    }

    #[test]
    fn cache_ttl_is_seven_days() {
        let now = Utc.with_ymd_and_hms(2026, 11, 10, 12, 0, 0).unwrap();
        assert!(cache_is_fresh("2026-11-09T12:00:00Z", now, CACHE_TTL_DAYS));
        assert!(cache_is_fresh("2026-11-04T12:00:01Z", now, CACHE_TTL_DAYS));
        assert!(!cache_is_fresh("2026-11-02T12:00:00Z", now, CACHE_TTL_DAYS));
        assert!(!cache_is_fresh("not a date", now, CACHE_TTL_DAYS));
        assert!(cache_is_fresh("2026-11-10T12:30:00Z", now, CACHE_TTL_DAYS));
    }

    #[test]
    fn app_ids_resolve_in_priority_order_and_missing_is_none() {
        assert_eq!(resolve_app_id(Some(440), Some(620), None), Some(440));
        assert_eq!(resolve_app_id(None, Some(620), None), Some(620));
        assert_eq!(
            resolve_app_id(None, None, Some("steam://rungameid/1145360")),
            Some(1145360)
        );
        assert_eq!(resolve_app_id(Some(0), Some(0), None), None);
        assert_eq!(
            resolve_app_id(None, None, Some("https://example.com")),
            None
        );
        assert_eq!(resolve_app_id(None, None, None), None);
    }

    #[test]
    fn urls_point_at_the_public_api_and_page() {
        assert_eq!(
            summary_url(1145360),
            "https://www.protondb.com/api/v1/reports/summaries/1145360.json"
        );
        assert_eq!(game_url(1145360), "https://www.protondb.com/app/1145360");
    }

    #[test]
    fn ratings_serialise_to_the_ui_shape() {
        let summary = parse_summary(SUMMARY).unwrap();
        let rating =
            ProtonDbRating::from_summary(1145360, &summary, "2026-11-10T12:00:00Z", false, true);
        let json = serde_json::to_value(&rating).unwrap();
        assert_eq!(json["status"], "cached");
        assert_eq!(json["tier"], "gold");
        assert_eq!(json["appId"], 1145360);
        assert_eq!(json["url"], "https://www.protondb.com/app/1145360");

        let missing = ProtonDbRating::no_app_id();
        assert_eq!(missing.status, "noAppId");
        assert!(missing.tier.is_none());
        assert_eq!(serde_json::to_value(&missing).unwrap()["status"], "noAppId");
    }
}
