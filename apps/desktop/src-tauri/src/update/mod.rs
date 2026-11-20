//! Updates: application self-update and the update centre (#16, #163), game
//! update detection (#30), the desktop OS notifications (#213) and the Linux
//! install-kind detection behind the portable updater (#312).

use serde::Serialize;

pub(crate) mod app;
pub(crate) mod game;
/// Which Linux install is running (#312). The pure helpers compile and are
/// tested on every platform so the Windows/macOS builds never rot.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod install_kind;
pub(crate) mod notify;
/// Portable Linux self-update (#312).
#[cfg(target_os = "linux")]
pub(crate) mod portable;

/// Download progress emitted as `update://progress` (#16, #312).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateProgress {
    pub(super) downloaded: u64,
    pub(super) total: Option<u64>,
    pub(super) percent: Option<f64>,
    /// Instantaneous download speed since the previous event, in bytes/s.
    pub(super) speed_bps: u64,
}

/// Remove the secret-bearing parts of a URL: userinfo, query and fragment.
///
/// Release URLs carry none of it, but a proxy or a signed CDN URL in a manifest
/// must never reach the log. Used by every update log line (#312).
#[must_use]
pub(crate) fn scrub_url(url: &str) -> String {
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return truncate(trimmed, 300);
    };
    let rest = rest.split(['#', '?']).next().unwrap_or(rest);
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    truncate(&format!("{scheme}://{host}{path}"), 300)
}

/// Scrub every URL embedded in a free-form message.
#[must_use]
pub(crate) fn scrub_text(text: &str) -> String {
    let scrubbed: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            if word.contains("://") {
                scrub_url(word)
            } else {
                word.to_string()
            }
        })
        .collect();
    truncate(&scrubbed.join(" "), 400)
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_scrubbing_keeps_the_path_and_drops_the_secrets() {
        assert_eq!(
            scrub_url("https://user:pw@host.test/a/b?q=1#f"),
            "https://host.test/a/b"
        );
        assert_eq!(scrub_url("https://host.test"), "https://host.test");
        assert_eq!(scrub_url("  https://host.test/x  "), "https://host.test/x");
        assert_eq!(scrub_url("not a url"), "not a url");
        assert_eq!(
            scrub_text("see https://h.test/x?sig=1 now"),
            "see https://h.test/x now"
        );
        assert_eq!(scrub_url(""), "");
        assert!(scrub_url(&"x".repeat(1000)).chars().count() <= 300);
        assert!(scrub_text(&"x ".repeat(1000)).chars().count() <= 400);
    }
}
