//! Redaction of crash and diagnostic text before it leaves the device (#171).
//!
//! Desktop panic hooks, failed Tauri commands and web error reports all run
//! through [`redact`] before a report is stored for preview or sent to a
//! server. The same implementation runs on the server (`POST /api/diagnostics`)
//! as a second line of defence, so a client that forgets a field cannot ship a
//! home directory, hostname, token or internal URL.
//!
//! The rules are deliberately coarse:
//!
//! * URLs and server origins become `<url>`;
//! * absolute paths become `<path:xxxxxxxx>`, where the suffix is a stable
//!   non-reversible hash so two reports from the same folder still correlate;
//! * IPv4/IPv6 addresses become `<ip>`, e-mail addresses `<email>`;
//! * `Bearer` credentials, `token=…`-style values and long opaque strings
//!   become `<token>`;
//! * the configured home directory, username and hostname become `~`, `<user>`
//!   and `<host>`.
//!
//! Redaction never tries to be clever about natural language: it errs on the
//! side of replacing too much.

use serde_json::Value;

/// Longest string kept in a redacted report field.
pub const MAX_FIELD_CHARS: usize = 2_000;

/// Longest string `redact` returns before truncating with an ellipsis.
pub const MAX_REDACTED_CHARS: usize = 8_000;

/// Values used to recognise the local machine in free-form text.
///
/// All fields are optional; a context with no values still strips URLs, paths,
/// addresses, e-mails and credentials.
#[derive(Debug, Clone, Default)]
pub struct RedactionContext {
    /// Home directory, for example `/home/alice` or `C:\Users\Alice`.
    pub home: Option<String>,
    /// Local account name.
    pub username: Option<String>,
    /// Machine hostname.
    pub hostname: Option<String>,
    /// Server origins configured in the app, replaced verbatim before the
    /// generic URL scanner runs.
    pub server_urls: Vec<String>,
}

impl RedactionContext {
    /// Empty context: only the generic scanners run.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the home directory to strip.
    #[must_use]
    pub fn with_home(mut self, home: impl Into<String>) -> Self {
        self.home = non_empty(Some(home.into()));
        self
    }

    /// Sets the account name to strip.
    #[must_use]
    pub fn with_username(mut self, username: impl Into<String>) -> Self {
        self.username = non_empty(Some(username.into()));
        self
    }

    /// Sets the hostname to strip.
    #[must_use]
    pub fn with_hostname(mut self, hostname: impl Into<String>) -> Self {
        self.hostname = non_empty(Some(hostname.into()));
        self
    }

    /// Adds a server origin to strip.
    #[must_use]
    pub fn with_server_url(mut self, url: impl Into<String>) -> Self {
        if let Some(url) = non_empty(Some(url.into())) {
            self.server_urls.push(url);
        }
        self
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Redacts one free-form string.
#[must_use]
pub fn redact(text: &str, context: &RedactionContext) -> String {
    let mut output = text.replace('\u{0000}', "");
    if let Some(home) = &context.home {
        output = replace_ignore_case(&output, home, "~");
        // Windows paths are case-insensitive and may use either separator.
        let forward = home.replace('\\', "/");
        if forward != *home {
            output = replace_ignore_case(&output, &forward, "~");
        }
        let backward = home.replace('/', "\\");
        if backward != *home {
            output = replace_ignore_case(&output, &backward, "~");
        }
    }
    for url in &context.server_urls {
        output = replace_ignore_case(&output, url, "<url>");
    }

    output = replace_ignore_case(
        &output,
        "authorization: bearer ",
        "authorization: bearer <token>",
    );
    output = replace_bearer(&output);
    if let Some(username) = &context.username {
        if username.chars().count() >= 3 {
            output = replace_ignore_case(&output, username, "<user>");
        }
    }
    if let Some(hostname) = &context.hostname {
        if hostname.chars().count() >= 3 {
            output = replace_ignore_case(&output, hostname, "<host>");
        }
    }

    let mut redacted = scan(&output);
    if redacted.chars().count() > MAX_REDACTED_CHARS {
        redacted = redacted.chars().take(MAX_REDACTED_CHARS).collect();
        redacted.push('…');
    }
    redacted
}

/// Redacts a JSON value recursively, bounding depth, array length, object keys
/// and string length so a hostile or runaway report cannot blow up the store.
#[must_use]
pub fn redact_json(value: &Value, context: &RedactionContext) -> Value {
    redact_json_at(value, context, 0)
}

/// Maximum nesting depth kept by [`redact_json`].
pub const MAX_JSON_DEPTH: usize = 4;
/// Maximum array items kept by [`redact_json`].
pub const MAX_JSON_ITEMS: usize = 32;

fn redact_json_at(value: &Value, context: &RedactionContext, depth: usize) -> Value {
    if depth > MAX_JSON_DEPTH {
        return Value::String("<truncated>".to_string());
    }
    match value {
        Value::String(text) => Value::String(truncate(&redact(text, context), MAX_FIELD_CHARS)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(MAX_JSON_ITEMS)
                .map(|item| redact_json_at(item, context, depth + 1))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map.iter().take(MAX_JSON_ITEMS) {
                let key = truncate(&redact(key, context), 120);
                out.insert(key, redact_json_at(item, context, depth + 1));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut text: String = value.chars().take(max).collect();
    text.push('…');
    text
}

/// Replaces every occurrence of `needle`, ignoring ASCII case.
fn replace_ignore_case(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let lower_haystack = haystack.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    let mut output = String::with_capacity(haystack.len());
    let mut cursor = 0;
    while let Some(offset) = lower_haystack[cursor..].find(&lower_needle) {
        let start = cursor + offset;
        let end = start + needle.len();
        if !haystack.is_char_boundary(start) || !haystack.is_char_boundary(end) {
            // Defensive: a non-ASCII boundary means the lowercased copy shifted.
            break;
        }
        output.push_str(&haystack[cursor..start]);
        output.push_str(replacement);
        cursor = end;
    }
    output.push_str(&haystack[cursor..]);
    output
}

/// Replaces `Bearer <credential>` anywhere in the text.
fn replace_bearer(text: &str) -> String {
    const NEEDLE: &str = "bearer ";
    let lower = text.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(NEEDLE) {
        let start = cursor + offset;
        let value_start = start + NEEDLE.len();
        output.push_str(&text[cursor..value_start]);
        let mut end = value_start;
        for character in text[value_start..].chars() {
            if character.is_whitespace() || matches!(character, '"' | '\'' | ',' | ')' | ']') {
                break;
            }
            end += character.len_utf8();
        }
        if end == value_start {
            output.push_str(&text[value_start..]);
            return output;
        }
        output.push_str("<token>");
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    output
}

/// Character-level scanner for URLs, paths, addresses and credentials.
fn scan(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let characters: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        let current = characters[index];
        if current.is_whitespace() {
            output.push(current);
            index += 1;
            continue;
        }

        // URL schemes.
        if starts_with_ignore_case(&characters[index..], "https://")
            || starts_with_ignore_case(&characters[index..], "http://")
            || starts_with_ignore_case(&characters[index..], "ftp://")
            || starts_with_ignore_case(&characters[index..], "wss://")
            || starts_with_ignore_case(&characters[index..], "file://")
        {
            index = consume_token(&characters, index, &mut output, "<url>");
            continue;
        }

        // UNC paths and Windows drive paths.
        if current == '\\' && characters.get(index + 1) == Some(&'\\') {
            index = consume_token(
                &characters,
                index,
                &mut output,
                &path_marker(&characters[index..]),
            );
            continue;
        }
        if current.is_ascii_alphabetic()
            && characters.get(index + 1) == Some(&':')
            && matches!(characters.get(index + 2), Some('/' | '\\'))
        {
            index = consume_token(
                &characters,
                index,
                &mut output,
                &path_marker(&characters[index..]),
            );
            continue;
        }

        // Home-relative POSIX path (`~/Games/...`).
        if current == '~'
            && characters.get(index + 1) == Some(&'/')
            && characters
                .get(index + 2)
                .is_some_and(|next| !next.is_whitespace())
        {
            index = consume_token(
                &characters,
                index,
                &mut output,
                &path_marker(&characters[index..]),
            );
            continue;
        }

        // Absolute POSIX path: a slash followed by a non-space path character.
        if current == '/'
            && characters
                .get(index + 1)
                .is_some_and(|next| !next.is_whitespace() && *next != '/')
        {
            index = consume_token(
                &characters,
                index,
                &mut output,
                &path_marker(&characters[index..]),
            );
            continue;
        }

        // A run of visible characters: classify it as one token.
        let start = index;
        let mut end = index;
        while end < characters.len() && !characters[end].is_whitespace() {
            end += 1;
        }
        let raw: String = characters[start..end].iter().collect();
        let (core, trailing) = split_trailing_punctuation(&raw);
        let replacement = classify_token(core);
        output.push_str(&replacement);
        output.push_str(trailing);
        index = end;
    }
    output
}

fn starts_with_ignore_case(haystack: &[char], needle: &str) -> bool {
    let needle: Vec<char> = needle.chars().collect();
    haystack.len() >= needle.len()
        && haystack
            .iter()
            .zip(&needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

/// Consumes a token and writes `marker`, returning the next index.
///
/// The marker for paths embeds a stable hash of the raw token, so two reports
/// can be grouped by folder without revealing it.
fn consume_token(characters: &[char], start: usize, output: &mut String, marker: &str) -> usize {
    let mut end = start;
    while end < characters.len()
        && !characters[end].is_whitespace()
        && !matches!(characters[end], '"' | '\'' | '<' | '>')
    {
        end += 1;
    }
    output.push_str(marker);
    end
}

fn path_marker(characters: &[char]) -> String {
    let raw: String = characters
        .iter()
        .take_while(|character| !character.is_whitespace())
        .collect();
    let hash = fnv1a(&raw);
    format!("<path:{hash:08x}>")
}

fn split_trailing_punctuation(raw: &str) -> (&str, &str) {
    let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);
    let split = trimmed.len();
    (&raw[..split], &raw[split..])
}

fn classify_token(token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }
    if is_email(token) {
        return "<email>".to_string();
    }
    if is_ipv4(token) || is_ipv6(token) {
        return "<ip>".to_string();
    }
    if let Some((key, value)) = token.split_once('=') {
        if is_secret_key(key) && !value.is_empty() {
            return format!("{key}=<token>");
        }
    }
    if let Some((key, value)) = token.split_once(':') {
        if is_secret_key(key) && !value.is_empty() && !value.starts_with("//") {
            return format!("{key}:<token>");
        }
    }
    if looks_opaque(token) {
        return "<token>".to_string();
    }
    token.to_string()
}

fn is_secret_key(key: &str) -> bool {
    let key = key
        .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .to_ascii_lowercase();
    matches!(
        key.as_str(),
        "token"
            | "access_token"
            | "refresh_token"
            | "id_token"
            | "password"
            | "passwd"
            | "secret"
            | "client_secret"
            | "api_key"
            | "apikey"
            | "authorization"
            | "cookie"
            | "session"
            | "jwt"
    )
}

fn is_email(token: &str) -> bool {
    let Some((local, domain)) = token.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
}

fn is_ipv4(token: &str) -> bool {
    let cleaned = token.trim_matches(|character: char| {
        !character.is_ascii_digit() && character != '.' && character != ':'
    });
    let mut parts = 0;
    for part in cleaned.split(':') {
        if part.is_empty() {
            continue;
        }
        let mut dots = 0;
        for piece in part.split('.') {
            if piece.is_empty() || piece.len() > 3 || !piece.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            let Ok(number) = piece.parse::<u16>() else {
                return false;
            };
            if number > 255 {
                return false;
            }
            dots += 1;
        }
        if dots == 4 {
            parts += 1;
        }
    }
    parts >= 1 && cleaned.contains('.')
}

fn is_ipv6(token: &str) -> bool {
    let cleaned = token.trim_matches(|character: char| {
        !character.is_ascii_hexdigit() && character != ':' && character != '.'
    });
    let colons = cleaned
        .chars()
        .filter(|character| *character == ':')
        .count();
    colons >= 3
        && cleaned
            .chars()
            .all(|character| character.is_ascii_hexdigit() || matches!(character, ':' | '.'))
}

fn looks_opaque(token: &str) -> bool {
    let cleaned = token.trim_matches(|character: char| {
        !character.is_ascii_alphanumeric()
            && character != '_'
            && character != '-'
            && character != '.'
    });
    if cleaned.len() < 32 {
        return false;
    }
    let has_digit = cleaned.chars().any(|character| character.is_ascii_digit());
    let has_alpha = cleaned
        .chars()
        .any(|character| character.is_ascii_alphabetic());
    has_digit && has_alpha
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Formats a stable short hash for a path, used by callers that need the same
/// marker as [`redact`] without running the scanner.
#[must_use]
pub fn path_hash(path: &str) -> String {
    format!("{:08x}", fnv1a(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> RedactionContext {
        RedactionContext::new()
            .with_home("/home/alice")
            .with_username("alice")
            .with_hostname("gaming-pc")
            .with_server_url("http://10.1.20.5:8080")
    }

    #[test]
    fn strips_home_username_and_hostname() {
        let redacted = redact(
            "panic at /home/alice/Ludex/games/Hades: alice on gaming-pc failed",
            &context(),
        );
        assert!(!redacted.contains("alice"), "{redacted}");
        assert!(!redacted.contains("gaming-pc"), "{redacted}");
        assert!(!redacted.contains("/home/alice"), "{redacted}");
        assert!(redacted.contains("<path:"), "{redacted}");
    }

    #[test]
    fn strips_urls_and_credentials() {
        let redacted = redact(
            "POST http://10.1.20.5:8080/api/games failed: Authorization: Bearer abc.def.ghi token=supersecret",
            &context(),
        );
        assert!(!redacted.contains("10.1.20.5"), "{redacted}");
        assert!(!redacted.contains("abc.def.ghi"), "{redacted}");
        assert!(!redacted.contains("supersecret"), "{redacted}");
        assert!(!redacted.contains("http://"), "{redacted}");
        assert!(redacted.contains("<url>"), "{redacted}");
        assert!(redacted.contains("<token>"), "{redacted}");
    }

    #[test]
    fn strips_addresses_and_emails() {
        let redacted = redact(
            "peer 192.168.1.42 and 2001:db8::1, contact alice@example.com",
            &context(),
        );
        assert!(!redacted.contains("192.168.1.42"), "{redacted}");
        assert!(!redacted.contains("2001:db8::1"), "{redacted}");
        assert!(!redacted.contains("alice@example.com"), "{redacted}");
        assert!(redacted.contains("<ip>"), "{redacted}");
        assert!(redacted.contains("<email>"), "{redacted}");
    }

    #[test]
    fn path_hash_is_stable_and_opaque() {
        let first = redact(
            "open /srv/games/Hades/Hades.exe failed",
            &RedactionContext::new(),
        );
        let again = redact(
            "open /srv/games/Hades/Hades.exe failed",
            &RedactionContext::new(),
        );
        assert_eq!(first, again);
        assert!(!first.contains("/srv/games"), "{first}");
        assert!(first.contains("<path:"), "{first}");
        assert_eq!(path_hash("/srv/games"), path_hash("/srv/games"));
        assert_ne!(path_hash("/srv/games"), path_hash("/srv/other"));
    }

    #[test]
    fn windows_paths_and_unc_paths_are_hashed() {
        let redacted = redact(
            r"install C:\Users\Alice\Games\Hades and \\nas\share\ludex",
            &RedactionContext::new(),
        );
        assert!(!redacted.contains("C:\\Users"), "{redacted}");
        assert!(!redacted.contains("\\\\nas"), "{redacted}");
        assert_eq!(redacted.matches("<path:").count(), 2, "{redacted}");
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        let text = "Install failed: the archive is corrupted (code 12).";
        assert_eq!(redact(text, &RedactionContext::new()), text);
    }

    #[test]
    fn json_is_redacted_and_bounded() {
        let value = serde_json::json!({
            "path": "/home/alice/Games/Hades",
            "url": "https://example.com/private",
            "nested": { "token": "abcdef1234567890abcdef1234567890" },
            "items": (0..100).collect::<Vec<i64>>(),
        });
        let redacted = redact_json(&value, &context());
        let text = redacted.to_string();
        assert!(!text.contains("alice"), "{text}");
        assert!(!text.contains("example.com"), "{text}");
        assert!(
            redacted["items"].as_array().expect("array").len() <= MAX_JSON_ITEMS,
            "{text}"
        );
    }
}
