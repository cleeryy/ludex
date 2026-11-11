//! Safe per-game config editing (#105) and the generic file layer used by the
//! engine adapters (#106).
//!
//! Rules:
//! - only files inside the install folder, never binary, never above 1 MiB;
//! - edits are line/span level on the original text, so comments, ordering and
//!   whitespace survive a round trip;
//! - every write copies the original to `{appDataDir}/config-backups/game-<id>/`
//!   first and records the diff in SQLite;
//! - a value is validated against the existing type (string, number, boolean)
//!   before anything touches the disk.
//!
//! Formats: INI/CFG (Unreal, Unity `.prefs`, GOG), TOML (`project.godot`) and
//! JSON (`goggame-*.info`). There is no format-preserving dependency: the
//! parsers below keep byte offsets of the value token and replace only that
//! span.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::ConfigEditRecord;
use crate::util::now_rfc3339;

/// Refuse to open anything larger: game configs are tiny, and a mistaken
/// 500 MiB log file in the install folder must not freeze the UI.
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
/// Upper bound on files returned by one scan.
const MAX_CONFIG_FILES: usize = 200;
/// Directory depth walked from the install folder.
const MAX_SCAN_DEPTH: usize = 5;
/// How many history rows the UI asks for.
pub const MAX_HISTORY: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFormat {
    Ini,
    Cfg,
    Toml,
    Json,
}

impl ConfigFormat {
    pub fn for_path(path: &Path) -> Option<Self> {
        // Unity's Linux/macOS player prefs file is literally named `prefs`,
        // with no extension.
        if path
            .file_name()
            .map(|name| name.to_string_lossy().eq_ignore_ascii_case("prefs"))
            .unwrap_or(false)
        {
            return Some(ConfigFormat::Cfg);
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())?;
        match extension.as_str() {
            "ini" => Some(ConfigFormat::Ini),
            // Godot's project.godot and Unity's .prefs are INI-like
            // (`[section]`, `key=value`, `;`/`#` comments).
            "cfg" | "conf" | "godot" | "prefs" => Some(ConfigFormat::Cfg),
            "toml" => Some(ConfigFormat::Toml),
            "json" | "info" => Some(ConfigFormat::Json),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ConfigFormat::Ini => "ini",
            ConfigFormat::Cfg => "cfg",
            ConfigFormat::Toml => "toml",
            ConfigFormat::Json => "json",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueType {
    String,
    Number,
    Boolean,
}

impl ValueType {
    fn as_str(self) -> &'static str {
        match self {
            ValueType::String => "string",
            ValueType::Number => "number",
            ValueType::Boolean => "boolean",
        }
    }
}

/// One editable scalar inside a config file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntry {
    /// `Section/key` (INI), `section.key` (TOML) or `a.b[0]` (JSON).
    pub key: String,
    pub value: String,
    pub value_type: String,
    pub section: Option<String>,
    /// 1-based line number, for the editor.
    pub line: usize,
    #[serde(skip)]
    span: (usize, usize),
    #[serde(skip)]
    quote: Option<char>,
}

/// Summary shown in the Settings tab before a file is opened.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFileInfo {
    /// Path relative to the install folder, `/`-separated.
    pub path: String,
    pub name: String,
    pub format: String,
    pub size_bytes: u64,
    pub entries: usize,
    pub editable: bool,
    pub note: Option<String>,
}

/// A parsed config file ready for the key/value editor.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigDocument {
    pub path: String,
    pub name: String,
    pub format: String,
    pub size_bytes: u64,
    pub entries: Vec<ConfigEntry>,
    pub note: Option<String>,
}

/// One key/value change from the UI.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEditInput {
    pub key: String,
    pub value: String,
}

pub struct WriteOutcome {
    pub document: ConfigDocument,
    pub diff: String,
    /// History row the command layer inserts into SQLite.
    pub record: ConfigEditRecord,
}

pub struct RestoreOutcome {
    pub document: ConfigDocument,
    pub diff: String,
    /// Security copy of the file as it was before the restore.
    pub record: ConfigEditRecord,
}

fn relative_string(install_root: &Path, path: &Path) -> String {
    path.strip_prefix(install_root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Crack/Steam-emulator directories are never game-owned settings. They are
/// read by other parts of the app for read-only achievement data, but exposing
/// them in the config editor would invite editing DRM/emulation files.
const SKIPPED_DIR_NAMES: &[&str] = &[
    "cracks",
    "steam_settings",
    "mr_goldberg",
    "goldberg",
    "rune",
    "codex",
    "plaza",
    "skidrow",
    "reloaded",
    "empress",
];

/// Emulator config files that live next to a game or under Cracks/.
const SKIPPED_FILE_NAMES: &[&str] = &[
    "steam_emu.ini",
    "steam_api.ini",
    "steam_api64.ini",
    "cream_api.ini",
];

fn is_skipped_dir(name: &str) -> bool {
    // `.prefix` is a Wine prefix: thousands of files, never a game config we
    // want to surface. Hidden folders cover staging directories too.
    let lower = name.to_ascii_lowercase();
    name.starts_with('.') || lower == "drive_c" || SKIPPED_DIR_NAMES.contains(&lower.as_str())
}

fn is_skipped_file(name: &str) -> bool {
    SKIPPED_FILE_NAMES.contains(&name.to_ascii_lowercase().as_str())
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

/// Read a file that is expected to be text: bounded, UTF-8, no NUL bytes.
fn read_text(path: &Path) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file.", path.display()));
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} is {} bytes; config files above {} KiB are not edited.",
            path.display(),
            metadata.len(),
            MAX_CONFIG_BYTES / 1024
        ));
    }
    let bytes = fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if looks_binary(&bytes) {
        return Err(format!(
            "{} looks like a binary file; Ludex only edits text configs.",
            path.display()
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| format!("{} is not UTF-8 text; refusing to edit it.", path.display()))
}

/// Resolve a relative config path and refuse anything outside the install
/// folder, including symlinks that escape it.
pub fn resolve_relative(install_root: &Path, relative: &str) -> Result<PathBuf, String> {
    let cleaned = relative.trim();
    if cleaned.is_empty() {
        return Err("Choose a config file.".to_string());
    }
    let relative_path = Path::new(cleaned);
    if relative_path.is_absolute() {
        return Err("Config paths must be relative to the install folder.".to_string());
    }
    if relative_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!("Refusing an unsafe config path: {cleaned}"));
    }
    let root = install_root.canonicalize().map_err(|e| {
        format!(
            "The install folder is not accessible ({}): {e}",
            install_root.display()
        )
    })?;
    let candidate = install_root.join(relative_path);
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("Could not open {cleaned}: {e}"))?;
    if !canonical.starts_with(&root) {
        return Err(format!("{cleaned} points outside the install folder."));
    }
    if ConfigFormat::for_path(&canonical).is_none() {
        return Err(format!(
            "Ludex only edits ini, cfg, toml and json config files (got {cleaned})."
        ));
    }
    Ok(canonical)
}

fn compose_key(section: Option<&str>, key: &str, separator: char) -> String {
    match section {
        Some(section) if !section.is_empty() => format!("{section}{separator}{key}"),
        _ => key.to_string(),
    }
}

fn infer_type(value: &str) -> ValueType {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return ValueType::String;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off"
    ) {
        return ValueType::Boolean;
    }
    if trimmed.parse::<f64>().is_ok() {
        return ValueType::Number;
    }
    ValueType::String
}

fn quoted_value(value: &str) -> (String, Option<char>) {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.chars().next().unwrap_or('"');
        let last = trimmed.chars().last().unwrap_or('"');
        if (first == '"' || first == '\'') && first == last {
            let inner = &trimmed[1..trimmed.len() - 1];
            return (inner.to_string(), Some(first));
        }
    }
    (trimmed.to_string(), None)
}

#[allow(clippy::too_many_arguments)]
fn make_entry(
    section: Option<String>,
    key: &str,
    raw_value: &str,
    value_start: usize,
    value_len: usize,
    line: usize,
    separator: char,
    allow_composite: bool,
) -> Option<ConfigEntry> {
    let (unquoted, quote) = quoted_value(raw_value);
    if !allow_composite && (unquoted.starts_with('[') || unquoted.starts_with('{')) {
        return None;
    }
    Some(ConfigEntry {
        key: compose_key(section.as_deref(), key.trim(), separator),
        value: unquoted.clone(),
        value_type: infer_type(&unquoted).as_str().to_string(),
        section,
        line,
        span: (value_start, value_start + value_len),
        quote,
    })
}

/// End of an INI value before an inline comment (` ; note` or ` # note`).
/// A `;`/`#` inside quotes or without preceding whitespace stays in the value.
fn ini_value_end(raw: &str) -> usize {
    let mut quote: Option<char> = None;
    let mut previous = '\0';
    for (index, character) in raw.char_indices() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None if matches!(character, ';' | '#') && previous.is_whitespace() => {
                return index;
            }
            None => {}
        }
        previous = character;
    }
    raw.len()
}

/// INI/CFG parser: `[section]` headers, `key = value`, `;`/`#` comments.
fn parse_ini(text: &str) -> Vec<ConfigEntry> {
    let mut entries = Vec::new();
    let mut section: Option<String> = None;
    let mut offset = 0usize;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let body = line.trim_end_matches(['\n', '\r']);
        let leading = body.len() - body.trim_start().len();
        let stripped = body.trim();
        if stripped.starts_with('[') && stripped.ends_with(']') && stripped.len() > 2 {
            section = Some(stripped[1..stripped.len() - 1].trim().to_string());
        } else if !stripped.is_empty() && !stripped.starts_with(';') && !stripped.starts_with('#') {
            if let Some(equals) = stripped.find('=') {
                let key = stripped[..equals].trim();
                if !key.is_empty() && !key.contains(';') {
                    let raw_value = &stripped[equals + 1..];
                    let value_end = ini_value_end(raw_value);
                    let value_part = &raw_value[..value_end];
                    let value_trim = value_part.trim();
                    let value_leading = value_part.len() - value_part.trim_start().len();
                    if let Some(entry) = make_entry(
                        section.clone(),
                        key,
                        value_trim,
                        offset + leading + equals + 1 + value_leading,
                        value_trim.len(),
                        index + 1,
                        '/',
                        true,
                    ) {
                        entries.push(entry);
                    }
                }
            }
        }
        offset += line.len();
    }
    entries
}

/// TOML scanner for `[section]` + `key = scalar` lines. Arrays and inline
/// tables are listed as read-only by not exposing them (editing them raw would
/// need a real TOML parser).
fn parse_toml(text: &str) -> Vec<ConfigEntry> {
    let mut entries = Vec::new();
    let mut section: Option<String> = None;
    let mut in_array_table = false;
    let mut offset = 0usize;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let body = line.trim_end_matches(['\n', '\r']);
        let leading = body.len() - body.trim_start().len();
        let stripped = body.trim();
        if stripped.starts_with("[[") && stripped.ends_with("]]") {
            // Arrays of tables can repeat keys; edits are ambiguous, so skip
            // the whole block until a normal header appears.
            section = None;
            in_array_table = true;
        } else if stripped.starts_with('[') && stripped.ends_with(']') {
            section = Some(stripped[1..stripped.len() - 1].trim().to_string());
            in_array_table = false;
        } else if !in_array_table && !stripped.is_empty() && !stripped.starts_with('#') {
            if let Some(equals) = stripped.find('=') {
                let key = stripped[..equals].trim();
                let raw_value = &stripped[equals + 1..];
                let value_leading = raw_value.len() - raw_value.trim_start().len();
                let value_start = offset + leading + equals + 1 + value_leading;
                if let Some((token, token_len)) = toml_token(raw_value) {
                    if let Some(entry) = make_entry(
                        section.clone(),
                        key,
                        token,
                        value_start,
                        token_len,
                        index + 1,
                        '.',
                        false,
                    ) {
                        entries.push(entry);
                    }
                }
            }
        }
        offset += line.len();
    }
    entries
}

/// Value token of a TOML line, with inline comments and quotes handled.
fn toml_token(raw: &str) -> Option<(&str, usize)> {
    let trimmed = raw.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        let quote = trimmed.chars().next()?;
        let mut escaped = false;
        for (index, character) in trimmed.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if character == '\\' && quote == '"' {
                escaped = true;
                continue;
            }
            if character == quote {
                let token = &trimmed[..index + 1];
                return Some((token, token.len()));
            }
        }
        return None;
    }
    let end = trimmed.find('#').unwrap_or(trimmed.len());
    let token = trimmed[..end].trim_end();
    if token.is_empty() || token.starts_with('[') || token.starts_with('{') {
        return None;
    }
    Some((token, token.len()))
}

#[derive(Debug)]
struct JsonLeaf {
    key: String,
    value: String,
    kind: ValueType,
    span: (usize, usize),
}

struct JsonScanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    position: usize,
    leaves: Vec<JsonLeaf>,
}

impl<'a> JsonScanner<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            position: 0,
            leaves: Vec::new(),
        }
    }

    fn skip_ws(&mut self) {
        while self.position < self.bytes.len()
            && matches!(self.bytes[self.position], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.position += 1;
        }
    }

    fn next_byte(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn parse_document(&mut self) -> Result<(), String> {
        self.skip_ws();
        self.parse_value("")?;
        self.skip_ws();
        if self.position != self.bytes.len() {
            return Err("Unexpected trailing content after the JSON value.".to_string());
        }
        Ok(())
    }

    fn parse_value(&mut self, path: &str) -> Result<(), String> {
        self.skip_ws();
        match self.next_byte() {
            Some(b'{') => self.parse_object(path),
            Some(b'[') => self.parse_array(path),
            Some(b'"') => {
                let start = self.position;
                self.parse_string_token()?;
                let end = self.position;
                let raw = &self.text[start..end];
                let value = serde_json::from_str::<String>(raw)
                    .map_err(|e| format!("Invalid JSON string: {e}"))?;
                self.leaves.push(JsonLeaf {
                    key: path.to_string(),
                    value,
                    kind: ValueType::String,
                    span: (start, end),
                });
                Ok(())
            }
            Some(b't') => self.parse_literal("true", path, ValueType::Boolean, "true"),
            Some(b'f') => self.parse_literal("false", path, ValueType::Boolean, "false"),
            Some(b'n') => {
                self.position += 4;
                Ok(())
            }
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => {
                let start = self.position;
                while let Some(byte) = self.next_byte() {
                    if byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E') {
                        self.position += 1;
                    } else {
                        break;
                    }
                }
                let raw = &self.text[start..self.position];
                let value: f64 = raw
                    .parse()
                    .map_err(|_| format!("Invalid JSON number: {raw}"))?;
                if !value.is_finite() {
                    return Err(format!("Invalid JSON number: {raw}"));
                }
                self.leaves.push(JsonLeaf {
                    key: path.to_string(),
                    value: raw.to_string(),
                    kind: ValueType::Number,
                    span: (start, self.position),
                });
                Ok(())
            }
            other => Err(format!(
                "Unexpected JSON token at byte {}: {:?}",
                self.position, other
            )),
        }
    }

    fn parse_literal(
        &mut self,
        literal: &str,
        path: &str,
        kind: ValueType,
        value: &str,
    ) -> Result<(), String> {
        let start = self.position;
        if !self.text[self.position..].starts_with(literal) {
            return Err(format!("Invalid JSON literal at byte {start}."));
        }
        self.position += literal.len();
        self.leaves.push(JsonLeaf {
            key: path.to_string(),
            value: value.to_string(),
            kind,
            span: (start, self.position),
        });
        Ok(())
    }

    fn parse_object(&mut self, path: &str) -> Result<(), String> {
        self.position += 1; // {
        self.skip_ws();
        if self.next_byte() == Some(b'}') {
            self.position += 1;
            return Ok(());
        }
        loop {
            self.skip_ws();
            if self.next_byte() != Some(b'"') {
                return Err(format!("Expected an object key at byte {}.", self.position));
            }
            let key_start = self.position;
            self.parse_string_token()?;
            let key_raw = &self.text[key_start..self.position];
            let key = serde_json::from_str::<String>(key_raw)
                .map_err(|e| format!("Invalid JSON key: {e}"))?;
            self.skip_ws();
            if self.next_byte() != Some(b':') {
                return Err(format!("Expected ':' at byte {}.", self.position));
            }
            self.position += 1;
            let child = if path.is_empty() {
                key
            } else {
                format!("{path}.{key}")
            };
            self.parse_value(&child)?;
            self.skip_ws();
            match self.next_byte() {
                Some(b',') => {
                    self.position += 1;
                }
                Some(b'}') => {
                    self.position += 1;
                    break;
                }
                _ => return Err(format!("Expected ',' or '}}' at byte {}.", self.position)),
            }
        }
        Ok(())
    }

    fn parse_array(&mut self, path: &str) -> Result<(), String> {
        self.position += 1; // [
        self.skip_ws();
        if self.next_byte() == Some(b']') {
            self.position += 1;
            return Ok(());
        }
        let mut index = 0usize;
        loop {
            self.parse_value(&format!("{path}[{index}]"))?;
            index += 1;
            self.skip_ws();
            match self.next_byte() {
                Some(b',') => {
                    self.position += 1;
                    self.skip_ws();
                }
                Some(b']') => {
                    self.position += 1;
                    break;
                }
                _ => return Err(format!("Expected ',' or ']' at byte {}.", self.position)),
            }
        }
        Ok(())
    }

    fn parse_string_token(&mut self) -> Result<(), String> {
        debug_assert_eq!(self.next_byte(), Some(b'"'));
        self.position += 1;
        let mut escaped = false;
        while self.position < self.bytes.len() {
            let byte = self.bytes[self.position];
            self.position += 1;
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'"' => return Ok(()),
                _ => {}
            }
        }
        Err("Unterminated JSON string.".to_string())
    }
}

fn parse_json(text: &str) -> Result<Vec<JsonLeaf>, String> {
    let mut scanner = JsonScanner::new(text);
    scanner.parse_document()?;
    Ok(scanner.leaves)
}

fn parse_entries(format: ConfigFormat, text: &str) -> Result<Vec<ConfigEntry>, String> {
    match format {
        ConfigFormat::Ini | ConfigFormat::Cfg => Ok(parse_ini(text)),
        ConfigFormat::Toml => Ok(parse_toml(text)),
        ConfigFormat::Json => {
            let leaves = parse_json(text)?;
            Ok(leaves
                .into_iter()
                .map(|leaf| ConfigEntry {
                    key: leaf.key,
                    value: leaf.value,
                    value_type: leaf.kind.as_str().to_string(),
                    section: None,
                    line: 0,
                    span: leaf.span,
                    quote: Some('"'),
                })
                .collect())
        }
    }
}

fn document_for(
    install_root: &Path,
    path: &Path,
    relative: &str,
    format: ConfigFormat,
    text: &str,
    size_bytes: u64,
    note: Option<String>,
) -> Result<ConfigDocument, String> {
    let _ = install_root;
    let entries = parse_entries(format, text)?;
    Ok(ConfigDocument {
        path: relative.to_string(),
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| relative.to_string()),
        format: format.as_str().to_string(),
        size_bytes,
        entries,
        note,
    })
}

/// Parse a config file inside `install_root`.
pub fn read(install_root: &Path, relative: &str) -> Result<ConfigDocument, String> {
    let path = resolve_relative(install_root, relative)?;
    let format =
        ConfigFormat::for_path(&path).ok_or_else(|| "Unsupported config format.".to_string())?;
    let text = read_text(&path)?;
    let size_bytes = text.len() as u64;
    let note = if format == ConfigFormat::Json {
        Some("JSON comments are not supported by the format.".to_string())
    } else {
        None
    };
    document_for(
        install_root,
        &path,
        relative,
        format,
        &text,
        size_bytes,
        note,
    )
}

/// List the config-like files inside an install folder. Unparseable files are
/// still listed (editable: false) so the UI can explain why.
pub fn list(install_root: &Path) -> Vec<ConfigFileInfo> {
    let mut files = Vec::new();
    walk(install_root, install_root, 0, &mut files);
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
}

fn walk(root: &Path, directory: &Path, depth: usize, files: &mut Vec<ConfigFileInfo>) {
    if depth > MAX_SCAN_DEPTH || files.len() >= MAX_CONFIG_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut children: Vec<_> = entries.flatten().collect();
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if files.len() >= MAX_CONFIG_FILES {
            return;
        }
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_skipped_dir(&name) {
                walk(root, &path, depth + 1, files);
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if is_skipped_file(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Some(format) = ConfigFormat::for_path(&path) else {
            continue;
        };
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.len() > MAX_CONFIG_BYTES {
            continue;
        }
        let relative = relative_string(root, &path);
        let (entries, editable, note) = match read(root, &relative) {
            Ok(document) => (document.entries.len(), true, document.note),
            Err(error) => (0, false, Some(error)),
        };
        files.push(ConfigFileInfo {
            path: relative,
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            format: format.as_str().to_string(),
            size_bytes: metadata.len(),
            entries,
            editable,
            note,
        });
    }
}

fn validate_new_value(kind: ValueType, original: &str, next: &str) -> Result<(), String> {
    if next
        .chars()
        .any(|character| character == '\n' || character == '\r')
    {
        return Err("A value cannot contain a line break.".to_string());
    }
    if next.chars().any(char::is_control) {
        return Err("A value cannot contain control characters.".to_string());
    }
    match kind {
        ValueType::String => Ok(()),
        ValueType::Number => {
            let parsed: f64 = next
                .trim()
                .parse()
                .map_err(|_| format!("“{next}” is not a number."))?;
            if !parsed.is_finite() {
                return Err(format!("“{next}” is not a finite number."));
            }
            Ok(())
        }
        ValueType::Boolean => {
            let lower = next.trim().to_ascii_lowercase();
            let numeric = matches!(lower.as_str(), "0" | "1");
            if matches!(
                lower.as_str(),
                "true" | "false" | "yes" | "no" | "on" | "off"
            ) || numeric
            {
                // Keep the 0/1 style an engine used; otherwise keep the casing
                // of the original literal so `True` does not become `true`.
                let _ = original;
                Ok(())
            } else {
                Err(format!(
                    "“{next}” is not a boolean (true/false, yes/no, on/off, 0/1)."
                ))
            }
        }
    }
}

fn format_boolean(original: &str, next: &str, format: ConfigFormat) -> String {
    let lower = next.trim().to_ascii_lowercase();
    if format == ConfigFormat::Json {
        return if matches!(lower.as_str(), "1" | "true" | "yes" | "on") {
            "true".to_string()
        } else {
            "false".to_string()
        };
    }
    let value = matches!(lower.as_str(), "1" | "true" | "yes" | "on");
    let original = original.trim();
    if original == "0" || original == "1" {
        return if value { "1" } else { "0" }.to_string();
    }
    if original == "True" || original == "False" {
        return if value { "True" } else { "False" }.to_string();
    }
    if original == "TRUE" || original == "FALSE" {
        return if value { "TRUE" } else { "FALSE" }.to_string();
    }
    if original.eq_ignore_ascii_case("yes") || original.eq_ignore_ascii_case("no") {
        return if value { "yes" } else { "no" }.to_string();
    }
    if original.eq_ignore_ascii_case("on") || original.eq_ignore_ascii_case("off") {
        return if value { "on" } else { "off" }.to_string();
    }
    if value { "true" } else { "false" }.to_string()
}

fn format_string(next: &str, quote: Option<char>, format: ConfigFormat) -> String {
    match format {
        ConfigFormat::Json => serde_json::to_string(next).unwrap_or_else(|_| "\"\"".to_string()),
        _ => match quote {
            Some('"') => serde_json::to_string(next).unwrap_or_else(|_| format!("\"{next}\"")),
            Some('\'') => format!("'{}'", next.replace('\'', "\\'")),
            _ => next.to_string(),
        },
    }
}

/// Apply edits to `text`, returning the new content and a line diff.
fn apply_edits(
    format: ConfigFormat,
    text: &str,
    edits: &[ConfigEditInput],
) -> Result<(String, String), String> {
    if edits.is_empty() {
        return Err("No changes to apply.".to_string());
    }
    let entries = parse_entries(format, text)?;
    let mut targets: Vec<(&ConfigEntry, String)> = Vec::new();
    for edit in edits {
        let key = edit.key.trim();
        if key.is_empty() {
            return Err("A config key is missing.".to_string());
        }
        let matches: Vec<&ConfigEntry> = entries.iter().filter(|entry| entry.key == key).collect();
        if matches.is_empty() {
            return Err(format!("{key} is not in this file."));
        }
        if matches.len() > 1 {
            return Err(format!(
                "{key} appears more than once; edit it directly in the file."
            ));
        }
        let entry = matches[0];
        validate_new_value(
            match entry.value_type.as_str() {
                "number" => ValueType::Number,
                "boolean" => ValueType::Boolean,
                _ => ValueType::String,
            },
            &entry.value,
            &edit.value,
        )?;
        targets.push((entry, edit.value.clone()));
    }

    // Replace from the end so earlier byte offsets stay valid.
    targets.sort_by_key(|left| std::cmp::Reverse(left.0.span.0));
    let mut updated = text.to_string();
    for (entry, new_value) in targets {
        let replacement = match entry.value_type.as_str() {
            "boolean" => format_boolean(&entry.value, &new_value, format),
            _ => {
                if entry.value_type == "number" {
                    new_value.trim().to_string()
                } else {
                    format_string(&new_value, entry.quote, format)
                }
            }
        };
        updated.replace_range(entry.span.0..entry.span.1, &replacement);
    }

    // Prove the result still parses and carries the requested values.
    let reparsed = parse_entries(format, &updated)?;
    for edit in edits {
        let key = edit.key.trim();
        let found = reparsed
            .iter()
            .find(|entry| entry.key == key)
            .ok_or_else(|| format!("The edit removed {key}; the file was not written."))?;
        if found.value != edit.value.trim()
            && !(found.value_type == "boolean" || found.value_type == "number")
        {
            return Err(format!(
                "The edit of {key} did not round-trip; the file was not written."
            ));
        }
    }
    if format == ConfigFormat::Json {
        serde_json::from_str::<serde_json::Value>(&updated)
            .map_err(|e| format!("The version with changes is not valid JSON: {e}"))?;
    }

    let diff = line_diff(text, &updated);
    Ok((updated, diff))
}

/// Compact line diff for the history panel. Context-free on purpose: config
/// files are short and the UI renders a small Mono block.
pub fn line_diff(before: &str, after: &str) -> String {
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    let mut prefix = 0usize;
    while prefix < before_lines.len()
        && prefix < after_lines.len()
        && before_lines[prefix] == after_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix + prefix < before_lines.len()
        && suffix + prefix < after_lines.len()
        && before_lines[before_lines.len() - 1 - suffix]
            == after_lines[after_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed = &before_lines[prefix..before_lines.len() - suffix];
    let added = &after_lines[prefix..after_lines.len() - suffix];
    if removed.is_empty() && added.is_empty() {
        return String::new();
    }
    let mut output = format!("@@ line {} @@\n", prefix + 1);
    for line in removed.iter().take(200) {
        output.push('-');
        output.push_str(line);
        output.push('\n');
    }
    for line in added.iter().take(200) {
        output.push('+');
        output.push_str(line);
        output.push('\n');
    }
    output
}

fn backup_directory(app_data: &Path, game_id: i64) -> PathBuf {
    app_data
        .join("config-backups")
        .join(format!("game-{game_id}"))
}

fn backup_file_name(name: &str) -> String {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let safe: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("{stamp}-{suffix}-{safe}")
}

/// Copy `path` into the per-game backup folder. Refuses to overwrite.
pub fn backup_file(
    app_data: &Path,
    game_id: i64,
    path: &Path,
    bytes: u64,
) -> Result<PathBuf, String> {
    let directory = backup_directory(app_data, game_id);
    fs::create_dir_all(&directory).map_err(|e| {
        format!(
            "Could not create the config backup folder {}: {e}",
            directory.display()
        )
    })?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());
    let backup = directory.join(backup_file_name(&name));
    fs::copy(path, &backup).map_err(|e| {
        format!(
            "Could not back up {} to {}: {e}. Nothing was written.",
            path.display(),
            backup.display()
        )
    })?;
    // Best effort: the backup holds user data only, but keep it private.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o600));
    }
    let _ = bytes;
    Ok(backup)
}

fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid config path: {}", path.display()))?;
    let tmp = parent.join(format!(
        ".{}.ludex-{}",
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "config".to_string()),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    {
        let mut file = fs::File::create(&tmp)
            .map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
        file.sync_all()
            .map_err(|e| format!("Could not flush {}: {e}", tmp.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let _ = fs::set_permissions(
                &tmp,
                fs::Permissions::from_mode(metadata.permissions().mode()),
            );
        }
    }
    // Unix replaces atomically. Windows refuses when the destination exists,
    // so retry after removing it; the backup already exists at that point.
    if let Err(first) = fs::rename(&tmp, path) {
        if path.exists() && fs::remove_file(path).is_ok() {
            if let Err(second) = fs::rename(&tmp, path) {
                let _ = fs::remove_file(&tmp);
                return Err(format!(
                    "Could not replace {}: {second} (initial rename: {first}). The backup is in the config history.",
                    path.display()
                ));
            }
        } else {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "Could not replace {}: {first}. The original file is unchanged.",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Write edited values after a mandatory backup.
pub fn write(
    install_root: &Path,
    app_data: &Path,
    game_id: i64,
    relative: &str,
    edits: &[ConfigEditInput],
    origin: &str,
) -> Result<WriteOutcome, String> {
    let path = resolve_relative(install_root, relative)?;
    let format =
        ConfigFormat::for_path(&path).ok_or_else(|| "Unsupported config format.".to_string())?;
    let text = read_text(&path)?;
    let bytes = text.len() as u64;
    let (updated, diff) = apply_edits(format, &text, edits)?;
    if updated == text {
        return Err("These values are already set; nothing was written.".to_string());
    }

    // Backup first. Nothing is written if the copy fails.
    let backup_path = backup_file(app_data, game_id, &path, bytes)?;

    // The record is inserted by the command layer before/after the atomic
    // replace; it is a truthful record either way (the backup exists).
    let record = ConfigEditRecord {
        id: uuid::Uuid::new_v4().to_string(),
        game_id,
        file_path: relative.to_string(),
        backup_path: backup_path.to_string_lossy().to_string(),
        diff: diff.clone(),
        bytes: bytes as i64,
        origin: origin.to_string(),
        created_at: now_rfc3339(),
    };

    write_atomic(&path, &updated)?;
    let document = document_for(
        install_root,
        &path,
        relative,
        format,
        &updated,
        updated.len() as u64,
        None,
    )?;
    Ok(WriteOutcome {
        document,
        diff,
        record,
    })
}

/// Restore a backup after taking a safety copy of the current file.
///
/// `backup_path` must be inside this game's backup folder; the command layer
/// additionally checks that the row belongs to the game.
pub fn restore_backup(
    install_root: &Path,
    app_data: &Path,
    game_id: i64,
    relative: &str,
    backup_path: &Path,
) -> Result<RestoreOutcome, String> {
    let path = resolve_relative(install_root, relative)?;
    let format =
        ConfigFormat::for_path(&path).ok_or_else(|| "Unsupported config format.".to_string())?;
    let expected_root = backup_directory(app_data, game_id)
        .canonicalize()
        .map_err(|e| format!("The config backup folder is missing: {e}"))?;
    let backup = backup_path
        .canonicalize()
        .map_err(|e| format!("Could not open the backup {}: {e}", backup_path.display()))?;
    if !backup.starts_with(&expected_root) || !backup.is_file() {
        return Err("That backup does not belong to this game.".to_string());
    }
    let restored = read_text(&backup)?;
    let current = read_text(&path)?;
    let safety_backup_path = backup_file(app_data, game_id, &path, current.len() as u64)?;
    let diff = line_diff(&current, &restored);
    write_atomic(&path, &restored)?;
    let document = document_for(
        install_root,
        &path,
        relative,
        format,
        &restored,
        restored.len() as u64,
        None,
    )?;
    Ok(RestoreOutcome {
        document,
        diff,
        record: ConfigEditRecord {
            id: uuid::Uuid::new_v4().to_string(),
            game_id,
            file_path: relative.to_string(),
            backup_path: safety_backup_path.to_string_lossy().to_string(),
            diff: String::new(),
            bytes: current.len() as i64,
            origin: "restore".to_string(),
            created_at: now_rfc3339(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ludex-config-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn root_with_file(name: &str, content: &str) -> PathBuf {
        let root = temp_dir("inline-root");
        write_file(&root, name, content);
        root
    }

    fn write_file(root: &Path, relative: &str, content: &str) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn crack_and_steam_emu_files_are_not_listed() {
        let root = temp_dir("cracks");
        write_file(&root, "GameUserSettings.ini", "[a]\nvalue=1\n");
        write_file(
            &root,
            "Uncharted4_data/screeninfo.cfg",
            "[screen]\nwidth=1920\n",
        );
        write_file(&root, "steam_emu.ini", "[Settings]\nLanguage=english\n");
        write_file(
            &root,
            "Cracks/RUNE/steam_emu.ini",
            "[Settings]\nLanguage=english\n",
        );
        write_file(
            &root,
            "Cracks/Mr_Goldberg/steam_settings/achievements.json",
            "{}",
        );

        let files = list(&root);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["GameUserSettings.ini", "Uncharted4_data/screeninfo.cfg"]
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ini_edit_preserves_comments_and_order() {
        let root = temp_dir("ini");
        let content = "; keep me\n[/Script/Engine.GameUserSettings]\nbUseVSync=False\nResolutionSizeX=1920\nFullscreenMode=1\n";
        write_file(&root, "GameUserSettings.ini", content);
        let document = read(&root, "GameUserSettings.ini").unwrap();
        assert_eq!(document.entries.len(), 3);
        assert_eq!(
            document.entries[1].key,
            "/Script/Engine.GameUserSettings/ResolutionSizeX"
        );

        let (updated, diff) = apply_edits(
            ConfigFormat::Ini,
            content,
            &[
                ConfigEditInput {
                    key: "/Script/Engine.GameUserSettings/bUseVSync".into(),
                    value: "True".into(),
                },
                ConfigEditInput {
                    key: "/Script/Engine.GameUserSettings/ResolutionSizeX".into(),
                    value: "2560".into(),
                },
            ],
        )
        .unwrap();
        assert!(updated.starts_with("; keep me\n"));
        assert!(updated.contains("bUseVSync=True"));
        assert!(updated.contains("ResolutionSizeX=2560"));
        assert!(updated.contains("FullscreenMode=1"));
        assert!(diff.contains("-bUseVSync=False"));
        assert!(diff.contains("+bUseVSync=True"));
    }

    #[test]
    fn ini_inline_comments_survive_an_edit() {
        let content = "[a]\nsize=1280 ; keep this note\nflag=true # and that\n";
        let document = read(&root_with_file("inline.ini", content), "inline.ini").unwrap();
        assert_eq!(document.entries[0].value, "1280");
        assert_eq!(document.entries[1].value, "true");
        let (updated, _) = apply_edits(
            ConfigFormat::Ini,
            content,
            &[ConfigEditInput {
                key: "a/size".into(),
                value: "1920".into(),
            }],
        )
        .unwrap();
        assert_eq!(
            updated,
            "[a]\nsize=1920 ; keep this note\nflag=true # and that\n"
        );
    }

    #[test]
    fn toml_edit_round_trips_scalars_and_comments() {
        let content =
            "; generated\n[display]\nwindow/size/viewport_width=1280\nwindow/vsync/vsync_mode=1\n";
        let (updated, _) = apply_edits(
            ConfigFormat::Toml,
            content,
            &[ConfigEditInput {
                key: "display.window/size/viewport_width".into(),
                value: "1920".into(),
            }],
        )
        .unwrap();
        assert_eq!(
            updated,
            "; generated\n[display]\nwindow/size/viewport_width=1920\nwindow/vsync/vsync_mode=1\n"
        );
    }

    #[test]
    fn json_edit_only_replaces_the_value_span() {
        let content =
            "{\n  \"language\": \"en\",\n  \"nested\": { \"width\": 1280, \"keep\": true }\n}\n";
        let (updated, _) = apply_edits(
            ConfigFormat::Json,
            content,
            &[
                ConfigEditInput {
                    key: "language".into(),
                    value: "fr".into(),
                },
                ConfigEditInput {
                    key: "nested.width".into(),
                    value: "1920".into(),
                },
            ],
        )
        .unwrap();
        assert_eq!(
            updated,
            "{\n  \"language\": \"fr\",\n  \"nested\": { \"width\": 1920, \"keep\": true }\n}\n"
        );
        // Still valid JSON with the untouched key intact.
        let parsed: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(parsed["nested"]["keep"], true);
    }

    #[test]
    fn type_validation_rejects_wrong_values() {
        let content = "[a]\nsize=1280\nflag=true\nname=hello\n";
        assert!(apply_edits(
            ConfigFormat::Ini,
            content,
            &[ConfigEditInput {
                key: "a/size".into(),
                value: "big".into()
            }]
        )
        .is_err());
        assert!(apply_edits(
            ConfigFormat::Ini,
            content,
            &[ConfigEditInput {
                key: "a/flag".into(),
                value: "maybe".into()
            }]
        )
        .is_err());
        assert!(apply_edits(
            ConfigFormat::Ini,
            content,
            &[ConfigEditInput {
                key: "a/name".into(),
                value: "two\nlines".into()
            }]
        )
        .is_err());
        assert!(apply_edits(
            ConfigFormat::Ini,
            content,
            &[ConfigEditInput {
                key: "a/missing".into(),
                value: "x".into()
            }]
        )
        .is_err());
    }

    #[test]
    fn write_backs_up_before_touching_the_file() {
        let root = temp_dir("backup-root");
        let data = temp_dir("backup-data");
        let original = "[x]\nflag=false\n";
        write_file(&root, "game.cfg", original);

        let outcome = write(
            &root,
            &data,
            9,
            "game.cfg",
            &[ConfigEditInput {
                key: "x/flag".into(),
                value: "true".into(),
            }],
            "manual",
        )
        .unwrap();
        assert!(outcome.record.backup_path.ends_with(".cfg"));
        let backup = PathBuf::from(&outcome.record.backup_path);
        assert!(backup.is_file());
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);
        assert!(fs::read_to_string(root.join("game.cfg"))
            .unwrap()
            .contains("flag=true"));
        assert!(outcome.diff.contains("+flag=true"));

        let restored = restore_backup(&root, &data, 9, "game.cfg", &backup).unwrap();
        assert_eq!(fs::read_to_string(root.join("game.cfg")).unwrap(), original);
        let safety = PathBuf::from(&restored.record.backup_path);
        assert!(safety.is_file());
        assert!(fs::read_to_string(&safety).unwrap().contains("flag=true"));
    }

    #[test]
    fn refuses_unsafe_paths_binary_and_oversized_files() {
        let root = temp_dir("unsafe");
        write_file(&root, "ok.ini", "[a]\nx=1\n");
        assert!(resolve_relative(&root, "../outside.ini").is_err());
        assert!(resolve_relative(&root, "/etc/passwd").is_err());
        assert!(resolve_relative(&root, "notes.txt").is_err());

        write_file(&root, "binary.ini", "ab\0cd");
        assert!(read(&root, "binary.ini").unwrap_err().contains("binary"));

        let big = "x".repeat(MAX_CONFIG_BYTES as usize + 1);
        write_file(&root, "huge.ini", &big);
        assert!(read(&root, "huge.ini").unwrap_err().contains("not edited"));
    }

    #[test]
    fn discovery_lists_supported_files_and_skips_prefix() {
        let root = temp_dir("discover");
        write_file(&root, "project.godot", "[display]\nwidth=1280\n");
        write_file(&root, "sub/game.info", "{\"language\":\"en\"}");
        write_file(&root, "notes.txt", "hello");
        write_file(&root, ".prefix/drive_c/thing.ini", "[a]\nx=1");

        let files = list(&root);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert!(paths.contains(&"project.godot"));
        assert!(paths.contains(&"sub/game.info"));
        assert!(!paths.iter().any(|path| path.contains("notes.txt")));
        assert!(!paths.iter().any(|path| path.contains(".prefix")));
    }

    #[test]
    fn json_scan_rejects_invalid_files() {
        assert!(parse_json("{ \"a\": }").is_err());
        assert!(parse_json("{ \"a\": 1 } trailing").is_err());
        assert!(parse_json("[]").unwrap().is_empty());
    }

    #[test]
    fn backup_restore_refuses_another_games_backup() {
        let root = temp_dir("restore-root");
        let data = temp_dir("restore-data");
        write_file(&root, "game.ini", "[a]\nx=1\n");
        let other = backup_file(&data, 42, &root.join("game.ini"), 12).unwrap();
        assert!(restore_backup(&root, &data, 7, "game.ini", &other).is_err());
    }
}
