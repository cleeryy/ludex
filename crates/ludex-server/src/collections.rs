//! Smart collection rule evaluation.
//!
//! Rules are stored as JSON and evaluated in Rust against the rows the viewer
//! can see. The library is small enough (thousands of rows) that this is
//! simpler and safer than building dynamic SQL, and it keeps the same semantics
//! in the API and in tests.

use ludex_core::naming::normalize_title;

use crate::models::{SmartRule, SmartRules};

/// Fields a rule may target.
const FIELDS: [&str; 8] = [
    "genre",
    "year",
    "players",
    "playtime",
    "rating",
    "installed",
    "provider",
    "title",
];

/// Game facts used by the evaluator.
#[derive(Debug, Clone, Default)]
pub struct Candidate {
    /// Display title.
    pub title: String,
    /// Release date (`YYYY-MM-DD...`), when known.
    pub release_date: Option<String>,
    /// Genre labels.
    pub genres: Vec<String>,
    /// Rating 0-100.
    pub rating: Option<f64>,
    /// Highest supported player count.
    pub max_players: Option<u32>,
    /// Minutes played by the viewer.
    pub minutes_played: i64,
    /// Metadata provider slug.
    pub provider_slug: Option<String>,
    /// Number of available versions. The server cannot see the desktop's
    /// installed state, so `installed` means "a downloadable version exists".
    pub version_count: i64,
}

impl Candidate {
    /// Release year, when the date is parseable.
    #[must_use]
    pub fn year(&self) -> Option<i64> {
        self.release_date
            .as_deref()
            .and_then(|date| date.get(0..4))
            .and_then(|year| year.parse::<i64>().ok())
    }
}

/// Validates a rule tree, returning a human-readable reason on failure.
///
/// Unknown fields, operators and value types are rejected when a collection is
/// created or updated so a stored tree always evaluates predictably.
pub fn validate(rules: &SmartRules) -> Result<(), String> {
    if !matches!(rules.match_mode.as_str(), "all" | "any") {
        return Err("match must be 'all' or 'any'".to_string());
    }
    if rules.rules.len() > 32 {
        return Err("a smart collection accepts at most 32 rules".to_string());
    }
    for (index, rule) in rules.rules.iter().enumerate() {
        validate_rule(rule).map_err(|reason| format!("rule {}: {reason}", index + 1))?;
    }
    Ok(())
}

fn validate_rule(rule: &SmartRule) -> Result<(), String> {
    if !FIELDS.contains(&rule.field.as_str()) {
        return Err(format!("unknown field '{}'", rule.field));
    }
    let op = rule.op.as_deref().unwrap_or(default_op(&rule.field));
    let allowed = allowed_ops(&rule.field);
    if !allowed.contains(&op) {
        return Err(format!("operator '{op}' is not valid for {}", rule.field));
    }
    match rule.field.as_str() {
        "genre" | "provider" => {
            if as_text(&rule.value).is_none() {
                return Err("value must be a string".to_string());
            }
        }
        "title" => {
            if as_text(&rule.value).is_none() {
                return Err("value must be a string".to_string());
            }
        }
        "installed" => {
            if as_bool(&rule.value).is_none() {
                return Err("value must be a boolean".to_string());
            }
        }
        "year" | "players" | "playtime" | "rating" => {
            if as_number(&rule.value).is_none() {
                return Err("value must be a number".to_string());
            }
            if op == "between" && rule.value2.as_ref().and_then(as_number).is_none() {
                return Err("value2 must be a number for 'between'".to_string());
            }
        }
        _ => {}
    }
    Ok(())
}

fn default_op(field: &str) -> &'static str {
    match field {
        "title" => "contains",
        "players" | "playtime" | "rating" => "gte",
        _ => "is",
    }
}

fn allowed_ops(field: &str) -> &'static [&'static str] {
    match field {
        "genre" => &["is", "contains"],
        "title" => &["contains", "is"],
        "year" => &["is", "gte", "lte", "between"],
        "players" => &["is", "gte", "lte"],
        "playtime" | "rating" => &["gte", "lte", "between"],
        "installed" | "provider" => &["is"],
        _ => &["is"],
    }
}

/// Evaluates a rule tree against one game. An empty tree matches nothing: a
/// smart collection with no rules should read as empty, not as the whole
/// library.
#[must_use]
pub fn evaluate(rules: &SmartRules, game: &Candidate) -> bool {
    if rules.rules.is_empty() {
        return false;
    }
    let matched = |rule: &SmartRule| evaluate_rule(rule, game);
    if rules.match_mode == "any" {
        rules.rules.iter().any(matched)
    } else {
        rules.rules.iter().all(matched)
    }
}

fn evaluate_rule(rule: &SmartRule, game: &Candidate) -> bool {
    let op = rule.op.as_deref().unwrap_or(default_op(&rule.field));
    match rule.field.as_str() {
        "genre" => {
            let Some(wanted) = as_text(&rule.value) else {
                return false;
            };
            game.genres.iter().any(|genre| {
                if op == "contains" {
                    normalize_title(genre).contains(&normalize_title(&wanted))
                } else {
                    genre.eq_ignore_ascii_case(&wanted)
                }
            })
        }
        "provider" => {
            let Some(wanted) = as_text(&rule.value) else {
                return false;
            };
            game.provider_slug
                .as_deref()
                .is_some_and(|slug| slug.eq_ignore_ascii_case(&wanted))
        }
        "title" => {
            let Some(wanted) = as_text(&rule.value) else {
                return false;
            };
            if op == "is" {
                normalize_title(&game.title) == normalize_title(&wanted)
            } else {
                normalize_title(&game.title).contains(&normalize_title(&wanted))
            }
        }
        "installed" => {
            let Some(wanted) = as_bool(&rule.value) else {
                return false;
            };
            (game.version_count > 0) == wanted
        }
        "year" => {
            let Some(year) = game.year() else {
                return false;
            };
            let Some(value) = as_number(&rule.value) else {
                return false;
            };
            compare(
                year as f64,
                op,
                value,
                rule.value2.as_ref().and_then(as_number),
            )
        }
        "players" => {
            let Some(value) = game.max_players else {
                return false;
            };
            let Some(target) = as_number(&rule.value) else {
                return false;
            };
            compare(
                f64::from(value),
                op,
                target,
                rule.value2.as_ref().and_then(as_number),
            )
        }
        "playtime" => {
            let Some(target) = as_number(&rule.value) else {
                return false;
            };
            compare(
                game.minutes_played as f64,
                op,
                target,
                rule.value2.as_ref().and_then(as_number),
            )
        }
        "rating" => {
            let Some(rating) = game.rating else {
                return false;
            };
            let Some(target) = as_number(&rule.value) else {
                return false;
            };
            compare(rating, op, target, rule.value2.as_ref().and_then(as_number))
        }
        _ => false,
    }
}

fn compare(actual: f64, op: &str, value: f64, value2: Option<f64>) -> bool {
    match op {
        "gte" => actual >= value,
        "lte" => actual <= value,
        "between" => {
            value2.is_some_and(|upper| actual >= value.min(upper) && actual <= value.max(upper))
        }
        _ => (actual - value).abs() < f64::EPSILON,
    }
}

fn as_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        _ => None,
    }
}

fn as_number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn as_bool(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(flag) => Some(*flag),
        serde_json::Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
        serde_json::Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(mode: &str, entries: Vec<serde_json::Value>) -> SmartRules {
        serde_json::from_value(json!({ "match": mode, "rules": entries })).expect("rules")
    }

    fn game() -> Candidate {
        Candidate {
            title: "Pokémon Écarlate".to_string(),
            release_date: Some("2022-11-18T00:00:00Z".to_string()),
            genres: vec!["RPG".to_string(), "Adventure".to_string()],
            rating: Some(71.0),
            max_players: Some(4),
            minutes_played: 320,
            provider_slug: Some("igdb".to_string()),
            version_count: 2,
        }
    }

    #[test]
    fn and_or_combine_rules() {
        let all = rules(
            "all",
            vec![
                json!({"field": "genre", "value": "RPG"}),
                json!({"field": "year", "op": "gte", "value": 2020}),
            ],
        );
        assert!(evaluate(&all, &game()));

        let any = rules(
            "any",
            vec![
                json!({"field": "genre", "value": "Shooter"}),
                json!({"field": "rating", "op": "gte", "value": 70}),
            ],
        );
        assert!(evaluate(&any, &game()));

        let none = rules(
            "all",
            vec![
                json!({"field": "genre", "value": "Shooter"}),
                json!({"field": "rating", "op": "gte", "value": 70}),
            ],
        );
        assert!(!evaluate(&none, &game()));
    }

    #[test]
    fn empty_rules_match_nothing() {
        assert!(!evaluate(&SmartRules::default(), &game()));
    }

    #[test]
    fn numeric_ranges_and_booleans() {
        let between = rules(
            "all",
            vec![json!({"field": "year", "op": "between", "value": 2021, "value2": 2023})],
        );
        assert!(evaluate(&between, &game()));

        let installed = rules("all", vec![json!({"field": "installed", "value": true})]);
        assert!(evaluate(&installed, &game()));
        let not_installed = rules("all", vec![json!({"field": "installed", "value": false})]);
        assert!(!evaluate(&not_installed, &game()));

        let playtime = rules(
            "all",
            vec![json!({"field": "playtime", "op": "gte", "value": 300})],
        );
        assert!(evaluate(&playtime, &game()));
    }

    #[test]
    fn title_matching_is_accent_and_case_insensitive() {
        let contains = rules(
            "all",
            vec![json!({"field": "title", "op": "contains", "value": "pokemon ecarlate"})],
        );
        assert!(evaluate(&contains, &game()));
        let exact = rules(
            "all",
            vec![json!({"field": "title", "op": "is", "value": "pokémon écarlate"})],
        );
        assert!(evaluate(&exact, &game()));
    }

    #[test]
    fn provider_and_genre_contains() {
        let provider = rules("all", vec![json!({"field": "provider", "value": "IGDB"})]);
        assert!(evaluate(&provider, &game()));
        let genre = rules(
            "all",
            vec![json!({"field": "genre", "op": "contains", "value": "advent"})],
        );
        assert!(evaluate(&genre, &game()));
    }

    #[test]
    fn validation_rejects_unknown_fields_and_operators() {
        assert!(validate(&rules(
            "all",
            vec![json!({"field": "genre", "value": "RPG"})]
        ))
        .is_ok());
        assert!(validate(&rules("all", vec![json!({"field": "bogus", "value": 1})])).is_err());
        assert!(validate(&rules(
            "all",
            vec![json!({"field": "genre", "op": "gte", "value": "RPG"})]
        ))
        .is_err());
        assert!(validate(&rules("maybe", vec![])).is_err());
        assert!(validate(&rules(
            "all",
            vec![json!({"field": "year", "op": "between", "value": 2020})]
        ))
        .is_err());
        assert!(validate(&rules(
            "all",
            vec![json!({"field": "rating", "value": "high"})]
        ))
        .is_err());
    }
}
