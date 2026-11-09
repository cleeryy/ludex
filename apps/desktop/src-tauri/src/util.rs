//! Small shared helpers.

use serde_json::Value;

/// RFC 3339 timestamp in UTC, second precision (`2026-11-09T20:00:00Z`).
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Trimmed non-empty string, if any.
pub fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Short stable-looking suffix for temporary paths (first 8 chars of a UUID).
pub fn short_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect()
}

/// Deep-merge `patch` into `base`. Objects merge recursively, everything else
/// replaces the previous value. Used so a partial settings payload from the web
/// UI never resets unrelated preferences.
pub fn merge_json(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                if value.is_null() {
                    base_map.insert(key, value);
                } else {
                    match base_map.get_mut(&key) {
                        Some(existing) if existing.is_object() && value.is_object() => {
                            merge_json(existing, value);
                        }
                        _ => {
                            base_map.insert(key, value);
                        }
                    }
                }
            }
        }
        (base_slot, patch_value) => *base_slot = patch_value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_keeps_untouched_keys() {
        let mut base = json!({ "mode": "local", "token": "abc", "nested": { "a": 1, "b": 2 } });
        merge_json(&mut base, json!({ "token": null, "nested": { "b": 3 } }));
        assert_eq!(base["mode"], "local");
        assert!(base["token"].is_null());
        assert_eq!(base["nested"]["a"], 1);
        assert_eq!(base["nested"]["b"], 3);
    }
}
