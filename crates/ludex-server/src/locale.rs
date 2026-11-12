//! Interface-locale registry shared with the web client (#188).
//!
//! The server validates account preferences and the administrator default
//! against the same `apps/web/src/locales/manifest.json` and `coverage.json`
//! the picker reads, so a locale the UI hides can never be stored. Both files
//! are embedded at compile time: a locale added to the manifest is accepted by
//! the very next build, with no second list to keep in sync.
//!
//! A stored locale is a preference, not a translation: `None` (or an empty
//! string) means "inherit" — the account follows the server default, the
//! server default follows the browser, and English is the final fallback.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::ApiError;

const MANIFEST: &str = include_str!("../../../apps/web/src/locales/manifest.json");
const COVERAGE: &str = include_str!("../../../apps/web/src/locales/coverage.json");
/// Must match `SELECTABLE_PERCENT` in `apps/web/src/lib/i18n.tsx`.
const SELECTABLE_PERCENT: f64 = 95.0;
const FALLBACK_LOCALE: &str = "en";

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    code: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct CoverageEntry {
    percent: f64,
}

#[derive(Debug)]
struct LocaleEntry {
    code: String,
    selectable: bool,
}

fn registry() -> &'static [LocaleEntry] {
    static REGISTRY: OnceLock<Vec<LocaleEntry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let manifest: Vec<ManifestEntry> = serde_json::from_str(MANIFEST).unwrap_or_else(|_| {
            vec![ManifestEntry {
                code: FALLBACK_LOCALE.to_string(),
                status: "official".to_string(),
            }]
        });
        let coverage: HashMap<String, CoverageEntry> =
            serde_json::from_str(COVERAGE).unwrap_or_default();
        manifest
            .into_iter()
            .map(|entry| {
                // The pseudo-locale is a translator tool, never a stored value.
                let selectable = entry.status == "official"
                    || (entry.status == "preview"
                        && coverage
                            .get(&entry.code)
                            .is_some_and(|data| data.percent >= SELECTABLE_PERCENT));
                LocaleEntry {
                    code: entry.code,
                    selectable,
                }
            })
            .collect()
    })
}

/// Canonical casing for a registered locale, case-insensitive.
pub fn canonical(value: &str) -> Option<&'static str> {
    let wanted = value.trim().to_ascii_lowercase();
    registry()
        .iter()
        .find(|entry| entry.code.to_ascii_lowercase() == wanted)
        .map(|entry| entry.code.as_str())
}

/// Whether the picker offers the locale (official or above the coverage gate).
pub fn is_selectable(code: &str) -> bool {
    canonical(code).is_some_and(|code| {
        registry()
            .iter()
            .any(|entry| entry.code == code && entry.selectable)
    })
}

/// Every selectable locale, in manifest order.
pub fn selectable_codes() -> Vec<&'static str> {
    registry()
        .iter()
        .filter(|entry| entry.selectable)
        .map(|entry| entry.code.as_str())
        .collect()
}

/// Validates a client-supplied locale.
///
/// Empty/whitespace means "inherit" and returns `Ok(None)`. An unregistered
/// code is a 400 `invalid_locale`; a registered locale below the coverage gate
/// (including the pseudo-locale) is a 400 `locale_unavailable`, matching the
/// disabled entries the picker shows.
pub fn validate(value: &str) -> Result<Option<String>, ApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let Some(code) = canonical(trimmed) else {
        return Err(ApiError::bad_request(
            "invalid_locale",
            format!("Unknown locale: {trimmed}"),
        ));
    };
    if !is_selectable(code) {
        return Err(ApiError::bad_request(
            "locale_unavailable",
            format!("Locale {code} is not available yet"),
        ));
    }
    Ok(Some(code.to_string()))
}

/// Reads a stored preference, dropping values that are no longer selectable
/// (a locale can fall under the gate after a catalog change).
pub fn normalize_stored(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .and_then(|value| validate(value).ok().flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalises_known_codes() {
        assert_eq!(canonical("fr"), Some("fr"));
        assert_eq!(canonical("PT-br"), Some("pt-BR"));
        assert_eq!(canonical("zh-cn"), Some("zh-CN"));
        assert_eq!(canonical("zz"), None);
        assert_eq!(canonical(""), None);
    }

    #[test]
    fn empty_means_inherit() {
        assert_eq!(validate("").expect("inherit"), None);
        assert_eq!(validate("   ").expect("inherit"), None);
        assert_eq!(validate("fr").expect("known"), Some("fr".to_string()));
    }

    #[test]
    fn rejects_unknown_and_gated_locales() {
        let unknown = validate("zz").expect_err("unknown");
        assert_eq!(unknown.code, "invalid_locale");
        assert_eq!(unknown.status, axum::http::StatusCode::BAD_REQUEST);

        // Arabic is registered but its catalog is far below the 95 % gate.
        let gated = validate("ar").expect_err("gated");
        assert_eq!(gated.code, "locale_unavailable");

        let pseudo = validate("en-XA").expect_err("pseudo");
        assert_eq!(pseudo.code, "locale_unavailable");
    }

    #[test]
    fn every_selectable_locale_round_trips() {
        for code in selectable_codes() {
            assert!(is_selectable(code), "{code} should be selectable");
            assert_eq!(validate(code).expect("selectable"), Some(code.to_string()));
        }
    }

    #[test]
    fn stored_values_that_dropped_under_the_gate_are_ignored() {
        assert_eq!(
            normalize_stored(Some("fr".to_string())),
            Some("fr".to_string())
        );
        assert_eq!(normalize_stored(Some("ar".to_string())), None);
        assert_eq!(normalize_stored(None), None);
    }
}
