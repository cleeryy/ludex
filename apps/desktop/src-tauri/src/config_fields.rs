//! Common config fields (#148): recognise and manage the values every game
//! keeps repeating — the UI language and the player name.
//!
//! Detection is a read-only heuristic layer on top of [`crate::game_config`]:
//! it scans the editable config files already listed for a game, matches key
//! (and section) names case-insensitively against a small built-in catalog and
//! reports the exact file + key plus a confidence. Nothing is written during
//! detection, an ambiguous match is never auto-picked, and every apply still
//! goes through the `game_config::write` backup + diff path.
//!
//! A per-game mapping (SQLite, `config_field_mappings`) overrides the catalog:
//! the pinned file/key is reported as a `template` source. The opt-in
//! `config_sync_prefs` row lets a launch re-apply the language and player name
//! without a dialog.

use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::{self, ConfigFieldMapping};
use crate::game_config::{self, ConfigDocument, ConfigEditInput, ConfigEntry};
use crate::util::now_rfc3339;

/// How many editable files one detection pass reads.
const MAX_SCANNED_FILES: usize = 40;
/// Upper bound for the allowed-value list copied from a file (GOG `langs[n]`).
const MAX_ALLOWED_VALUES: usize = 64;
/// Longest player name Ludex writes into a config file.
pub const MAX_PLAYER_NAME_CHARS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Language,
    PlayerName,
}

impl FieldKind {
    fn id(self) -> &'static str {
        match self {
            FieldKind::Language => "language",
            FieldKind::PlayerName => "playerName",
        }
    }

    fn label(self) -> &'static str {
        match self {
            FieldKind::Language => "Language",
            FieldKind::PlayerName => "Player name",
        }
    }

    fn from_id(raw: &str) -> Option<Self> {
        match canonical(raw).as_str() {
            "language" => Some(FieldKind::Language),
            "playername" => Some(FieldKind::PlayerName),
            _ => None,
        }
    }
}

/// One recognisable field of the built-in catalog.
struct FieldSpec {
    kind: FieldKind,
    /// Key names that identify the field anywhere.
    aliases: &'static [&'static str],
    /// Sections that make a short key below identify the field.
    sections: &'static [&'static str],
    /// Short keys accepted inside one of those sections.
    section_keys: &'static [&'static str],
}

const LANGUAGE_SPEC: FieldSpec = FieldSpec {
    kind: FieldKind::Language,
    aliases: &[
        "language",
        "locale",
        "culture",
        "languageid",
        "textlanguage",
        "gamelanguage",
        "uilanguage",
        "currentlanguage",
        "languagecode",
        "lastlanguage",
        "gamelocale",
    ],
    sections: &[
        "language",
        "languages",
        "localization",
        "localisation",
        "internationalization",
        "internationalisation",
        "locale",
        "i18n",
    ],
    section_keys: &[
        "value",
        "current",
        "default",
        "fallback",
        "code",
        "language",
        "locale",
        "culture",
        "languageid",
        "textlanguage",
        "selected",
    ],
};

const PLAYER_SPEC: FieldSpec = FieldSpec {
    kind: FieldKind::PlayerName,
    aliases: &[
        "username",
        "playername",
        "nickname",
        "accountname",
        "profilename",
        "playernick",
        "displayname",
        "playerdisplayname",
    ],
    sections: &[
        "player",
        "players",
        "profile",
        "account",
        "user",
        "identity",
        "playersettings",
        "profilesettings",
    ],
    section_keys: &[
        "name",
        "username",
        "nickname",
        "playername",
        "accountname",
        "profilename",
        "displayname",
        "value",
    ],
};

const SPECS: [&FieldSpec; 2] = [&LANGUAGE_SPEC, &PLAYER_SPEC];

/// Whether `raw` is a field id of the built-in catalog.
pub fn is_known_field(raw: &str) -> bool {
    FieldKind::from_id(raw).is_some()
}

/// One key that may hold a common field.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonFieldCandidate {
    /// Path relative to the install folder.
    pub file: String,
    pub key: String,
    pub value: String,
    pub value_type: String,
    pub line: usize,
    pub section: Option<String>,
    /// True when the key itself (not its section) matched the catalog.
    pub exact: bool,
    /// Values the file lists as accepted, when it has any.
    #[serde(skip)]
    allowed: Vec<String>,
}

/// A detected field, or the candidate list when several keys match.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonFieldMatch {
    /// `language` or `playerName`.
    pub field: String,
    pub label: String,
    /// `certain`, `likely` or `ambiguous`.
    pub confidence: String,
    /// `builtin` (catalog heuristic) or `template` (pinned per-game mapping).
    pub source: String,
    /// `None` when the match is ambiguous: Ludex never picks for the user.
    pub file: Option<String>,
    pub key: Option<String>,
    pub value: Option<String>,
    pub value_type: Option<String>,
    /// Values the file itself accepts, when it lists them (e.g. GOG `langs[n]`).
    pub allowed_values: Vec<String>,
    /// Populated for ambiguous matches only.
    pub candidates: Vec<CommonFieldCandidate>,
    pub note: Option<String>,
}

/// Result of one read-only detection pass.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    pub fields: Vec<CommonFieldMatch>,
    pub notes: Vec<String>,
    pub files: usize,
}

/// Detection report returned to the UI. `fields` empty means "no panel".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonFieldsReport {
    pub game_id: i64,
    pub install_path: String,
    pub fields: Vec<CommonFieldMatch>,
    pub mappings: Vec<ConfigFieldMapping>,
    pub notes: Vec<String>,
}

/// A requested value: the field id plus what the user wants it to say.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonFieldTarget {
    pub field: String,
    pub value: String,
}

/// Before/after preview of one common-field write.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonFieldPreview {
    pub field: String,
    pub file: String,
    pub key: String,
    pub current_value: String,
    pub next_value: String,
    pub unchanged: bool,
    pub diff: String,
}

/// One field applied by the launch-time sync.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedCommonField {
    pub field: String,
    pub file: String,
    pub key: String,
    pub before: String,
    pub after: String,
    pub diff: String,
}

/// Outcome of a keep-in-sync pass; warnings never block a launch.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSyncOutcome {
    pub applied: Vec<AppliedCommonField>,
    pub warnings: Vec<String>,
    pub skipped: usize,
}

fn fold_char(character: char) -> char {
    // Accent-insensitive matching: game configs spell language names both
    // "Français" and "Francais".
    match character {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        'ß' => 's',
        other => other,
    }
}

fn canonical(raw: &str) -> String {
    raw.chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .map(fold_char)
        .collect()
}

/// Unity player prefs suffix keys with a stable hash (`Language_h12345`).
fn strip_unity_suffix(name: &str) -> &str {
    if let Some(index) = name.rfind("_h") {
        let (head, tail) = name.split_at(index);
        let digits = &tail[2..];
        if !head.is_empty() && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return head;
        }
    }
    name
}

/// Last path segment of a config key (`Section/key`, `section.key`, `a.b[0]`).
fn entry_name(key: &str) -> &str {
    key.rsplit(['/', '.']).next().unwrap_or(key)
}

/// `Some(exact)` when the entry belongs to `spec`; `exact` is false when the
/// section turned a short key into a match.
fn match_entry(spec: &FieldSpec, entry: &ConfigEntry) -> Option<bool> {
    let name = canonical(strip_unity_suffix(entry_name(&entry.key)));
    if spec.aliases.iter().any(|alias| canonical(alias) == name) {
        return Some(true);
    }
    let section = entry.section.as_deref().map(canonical)?;
    if spec
        .sections
        .iter()
        .any(|known| canonical(known) == section)
        && spec
            .section_keys
            .iter()
            .any(|known| canonical(known) == name)
    {
        return Some(false);
    }
    None
}

fn usable_value(kind: FieldKind, entry: &ConfigEntry) -> bool {
    let value = entry.value.trim();
    if value.chars().any(char::is_control) || value.chars().count() > 80 {
        return false;
    }
    match kind {
        // Numeric ids are listed so the panel can explain why they cannot be
        // mapped; booleans and composites are never a language/name field.
        FieldKind::Language => matches!(entry.value_type.as_str(), "string" | "number"),
        FieldKind::PlayerName => entry.value_type == "string",
    }
}

/// Languages listed next to the language key (GOG `langs[0]`, `languages[1]`).
fn allowed_values(document: &ConfigDocument) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for entry in &document.entries {
        let name = canonical(entry_name(&entry.key));
        let listed = name.starts_with("langs")
            || name.starts_with("languages")
            || name.starts_with("availablelanguages")
            || name.starts_with("supportedlanguages")
            || name.starts_with("languageoptions")
            || name.starts_with("localizations");
        if !listed || entry.value_type != "string" {
            continue;
        }
        let value = entry.value.trim().to_string();
        if value.is_empty() || value.chars().count() > 40 || value.chars().any(char::is_control) {
            continue;
        }
        if !values
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&value))
        {
            values.push(value);
        }
        if values.len() >= MAX_ALLOWED_VALUES {
            break;
        }
    }
    values
}

fn language_like(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= 24
        && trimmed
            .chars()
            .all(|character| character.is_alphabetic() || matches!(character, '-' | '_'))
}

/// Read every editable config file and match the catalog without writing.
pub fn detect(install_root: &Path, mappings: &[ConfigFieldMapping]) -> Detection {
    let mut detection = Detection::default();
    let mut candidates: Vec<(FieldKind, CommonFieldCandidate)> = Vec::new();
    let files = game_config::list(install_root);
    for info in &files {
        if !info.editable || info.entries == 0 {
            continue;
        }
        if detection.files >= MAX_SCANNED_FILES {
            detection.notes.push(format!(
                "Only the first {MAX_SCANNED_FILES} editable config files were scanned."
            ));
            break;
        }
        let Ok(document) = game_config::read(install_root, &info.path) else {
            continue;
        };
        detection.files += 1;
        let allowed = allowed_values(&document);
        for entry in &document.entries {
            for spec in SPECS {
                if !usable_value(spec.kind, entry) {
                    continue;
                }
                if let Some(exact) = match_entry(spec, entry) {
                    candidates.push((
                        spec.kind,
                        CommonFieldCandidate {
                            file: info.path.clone(),
                            key: entry.key.clone(),
                            value: entry.value.clone(),
                            value_type: entry.value_type.clone(),
                            line: entry.line,
                            section: entry.section.clone(),
                            exact,
                            allowed: allowed.clone(),
                        },
                    ));
                }
            }
        }
    }

    for spec in SPECS {
        let kind = spec.kind;
        let mut own: Vec<&CommonFieldCandidate> = candidates
            .iter()
            .filter(|(candidate_kind, _)| *candidate_kind == kind)
            .map(|(_, candidate)| candidate)
            .collect();

        let mapping = mappings.iter().find(|mapping| mapping.field == kind.id());
        if let Some(mapping) = mapping {
            let resolved = own
                .iter()
                .find(|candidate| {
                    candidate.file == mapping.file_path && candidate.key == mapping.key
                })
                .map(|candidate| (*candidate).clone())
                // A template may point at a game-specific key the catalog does
                // not know; read it directly instead of rejecting the pin.
                .or_else(|| mapped_candidate(install_root, mapping));
            if let Some(candidate) = resolved {
                detection.fields.push(match_from(
                    kind,
                    "certain",
                    "template",
                    &candidate,
                    Vec::new(),
                    None,
                ));
                continue;
            }
            detection.notes.push(format!(
                "The pinned {} key {} · {} is no longer in the install folder; Ludex fell back to detection.",
                kind.label(),
                mapping.file_path,
                mapping.key
            ));
        }

        if own.is_empty() {
            continue;
        }
        own.sort_by(|left, right| {
            right
                .exact
                .cmp(&left.exact)
                .then_with(|| left.file.cmp(&right.file))
                .then_with(|| left.key.cmp(&right.key))
        });

        // A section-derived match is much weaker than a key literally named
        // `Language`; only compare candidates of the strongest kind.
        let exact_count = own.iter().filter(|candidate| candidate.exact).count();
        let pool: Vec<&CommonFieldCandidate> = if exact_count > 0 {
            own.iter()
                .copied()
                .filter(|candidate| candidate.exact)
                .collect()
        } else {
            own.clone()
        };

        if pool.len() > 1 {
            detection.notes.push(format!(
                "{} keys look like {}; pin the one this game reads, or edit the file directly.",
                pool.len(),
                kind.label()
            ));
            detection.fields.push(CommonFieldMatch {
                field: kind.id().to_string(),
                label: kind.label().to_string(),
                confidence: "ambiguous".to_string(),
                source: "builtin".to_string(),
                file: None,
                key: None,
                value: None,
                value_type: None,
                allowed_values: Vec::new(),
                candidates: pool.into_iter().cloned().collect(),
                note: None,
            });
            continue;
        }

        let candidate = pool[0];
        let exact = candidate.exact;
        let mut confidence = if exact { "certain" } else { "likely" }.to_string();
        let mut note = None;
        if candidate.value.trim().is_empty() {
            note = Some("This key is empty; Ludex writes the value you choose.".to_string());
        } else if kind == FieldKind::Language && !language_like(&candidate.value) {
            confidence = "likely".to_string();
            note = Some(
                "The current value is not a language code or name; check the file before applying."
                    .to_string(),
            );
        }
        detection.fields.push(match_from(
            kind,
            &confidence,
            "builtin",
            candidate,
            Vec::new(),
            note,
        ));
    }

    detection
}

/// Read one value the catalog does not know, so a per-game template can point
/// at a game-specific key.
fn mapped_candidate(
    install_root: &Path,
    mapping: &ConfigFieldMapping,
) -> Option<CommonFieldCandidate> {
    let document = game_config::read(install_root, &mapping.file_path).ok()?;
    let entry = document
        .entries
        .iter()
        .find(|entry| entry.key == mapping.key)?;
    Some(CommonFieldCandidate {
        file: mapping.file_path.clone(),
        key: entry.key.clone(),
        value: entry.value.clone(),
        value_type: entry.value_type.clone(),
        line: entry.line,
        section: entry.section.clone(),
        exact: true,
        allowed: allowed_values(&document),
    })
}

fn match_from(
    kind: FieldKind,
    confidence: &str,
    source: &str,
    candidate: &CommonFieldCandidate,
    candidates: Vec<CommonFieldCandidate>,
    note: Option<String>,
) -> CommonFieldMatch {
    CommonFieldMatch {
        field: kind.id().to_string(),
        label: kind.label().to_string(),
        confidence: confidence.to_string(),
        source: source.to_string(),
        file: Some(candidate.file.clone()),
        key: Some(candidate.key.clone()),
        value: Some(candidate.value.clone()),
        value_type: Some(candidate.value_type.clone()),
        allowed_values: candidate.allowed.clone(),
        candidates,
        note,
    }
}

/* ------------------------------- languages ------------------------------- */

struct LanguageName {
    code: &'static str,
    alpha3: &'static str,
    /// ISO 639-2/B codes still used by some games (`fre`, `ger`, …).
    alt3: &'static [&'static str],
    english: &'static str,
    native: &'static str,
}

const LANGUAGES: &[LanguageName] = &[
    LanguageName {
        code: "en",
        alpha3: "eng",
        alt3: &[],
        english: "English",
        native: "English",
    },
    LanguageName {
        code: "fr",
        alpha3: "fra",
        alt3: &["fre"],
        english: "French",
        native: "Français",
    },
    LanguageName {
        code: "de",
        alpha3: "deu",
        alt3: &["ger"],
        english: "German",
        native: "Deutsch",
    },
    LanguageName {
        code: "es",
        alpha3: "spa",
        alt3: &[],
        english: "Spanish",
        native: "Español",
    },
    LanguageName {
        code: "it",
        alpha3: "ita",
        alt3: &[],
        english: "Italian",
        native: "Italiano",
    },
    LanguageName {
        code: "pt",
        alpha3: "por",
        alt3: &[],
        english: "Portuguese",
        native: "Português",
    },
    LanguageName {
        code: "ru",
        alpha3: "rus",
        alt3: &[],
        english: "Russian",
        native: "Русский",
    },
    LanguageName {
        code: "pl",
        alpha3: "pol",
        alt3: &[],
        english: "Polish",
        native: "Polski",
    },
    LanguageName {
        code: "nl",
        alpha3: "nld",
        alt3: &["dut"],
        english: "Dutch",
        native: "Nederlands",
    },
    LanguageName {
        code: "sv",
        alpha3: "swe",
        alt3: &[],
        english: "Swedish",
        native: "Svenska",
    },
    LanguageName {
        code: "no",
        alpha3: "nor",
        alt3: &[],
        english: "Norwegian",
        native: "Norsk",
    },
    LanguageName {
        code: "da",
        alpha3: "dan",
        alt3: &[],
        english: "Danish",
        native: "Dansk",
    },
    LanguageName {
        code: "fi",
        alpha3: "fin",
        alt3: &[],
        english: "Finnish",
        native: "Suomi",
    },
    LanguageName {
        code: "cs",
        alpha3: "ces",
        alt3: &["cze"],
        english: "Czech",
        native: "Čeština",
    },
    LanguageName {
        code: "sk",
        alpha3: "slk",
        alt3: &["slo"],
        english: "Slovak",
        native: "Slovenčina",
    },
    LanguageName {
        code: "hu",
        alpha3: "hun",
        alt3: &[],
        english: "Hungarian",
        native: "Magyar",
    },
    LanguageName {
        code: "ro",
        alpha3: "ron",
        alt3: &["rum"],
        english: "Romanian",
        native: "Română",
    },
    LanguageName {
        code: "bg",
        alpha3: "bul",
        alt3: &[],
        english: "Bulgarian",
        native: "Български",
    },
    LanguageName {
        code: "el",
        alpha3: "ell",
        alt3: &["gre"],
        english: "Greek",
        native: "Ελληνικά",
    },
    LanguageName {
        code: "tr",
        alpha3: "tur",
        alt3: &[],
        english: "Turkish",
        native: "Türkçe",
    },
    LanguageName {
        code: "uk",
        alpha3: "ukr",
        alt3: &[],
        english: "Ukrainian",
        native: "Українська",
    },
    LanguageName {
        code: "ar",
        alpha3: "ara",
        alt3: &[],
        english: "Arabic",
        native: "العربية",
    },
    LanguageName {
        code: "he",
        alpha3: "heb",
        alt3: &[],
        english: "Hebrew",
        native: "עברית",
    },
    LanguageName {
        code: "ja",
        alpha3: "jpn",
        alt3: &[],
        english: "Japanese",
        native: "日本語",
    },
    LanguageName {
        code: "ko",
        alpha3: "kor",
        alt3: &[],
        english: "Korean",
        native: "한국어",
    },
    LanguageName {
        code: "zh",
        alpha3: "zho",
        alt3: &["chi"],
        english: "Chinese",
        native: "中文",
    },
    LanguageName {
        code: "th",
        alpha3: "tha",
        alt3: &[],
        english: "Thai",
        native: "ไทย",
    },
    LanguageName {
        code: "vi",
        alpha3: "vie",
        alt3: &[],
        english: "Vietnamese",
        native: "Tiếng Việt",
    },
    LanguageName {
        code: "id",
        alpha3: "ind",
        alt3: &[],
        english: "Indonesian",
        native: "Bahasa Indonesia",
    },
    LanguageName {
        code: "ms",
        alpha3: "msa",
        alt3: &["may"],
        english: "Malay",
        native: "Bahasa Melayu",
    },
    LanguageName {
        code: "hi",
        alpha3: "hin",
        alt3: &[],
        english: "Hindi",
        native: "हिन्दी",
    },
    LanguageName {
        code: "ca",
        alpha3: "cat",
        alt3: &[],
        english: "Catalan",
        native: "Català",
    },
    LanguageName {
        code: "eu",
        alpha3: "eus",
        alt3: &["baq"],
        english: "Basque",
        native: "Euskara",
    },
    LanguageName {
        code: "gl",
        alpha3: "glg",
        alt3: &[],
        english: "Galician",
        native: "Galego",
    },
    LanguageName {
        code: "sr",
        alpha3: "srp",
        alt3: &[],
        english: "Serbian",
        native: "Српски",
    },
    LanguageName {
        code: "hr",
        alpha3: "hrv",
        alt3: &[],
        english: "Croatian",
        native: "Hrvatski",
    },
    LanguageName {
        code: "sl",
        alpha3: "slv",
        alt3: &[],
        english: "Slovenian",
        native: "Slovenščina",
    },
    LanguageName {
        code: "lt",
        alpha3: "lit",
        alt3: &[],
        english: "Lithuanian",
        native: "Lietuvių",
    },
    LanguageName {
        code: "lv",
        alpha3: "lav",
        alt3: &[],
        english: "Latvian",
        native: "Latviešu",
    },
    LanguageName {
        code: "et",
        alpha3: "est",
        alt3: &[],
        english: "Estonian",
        native: "Eesti",
    },
    LanguageName {
        code: "is",
        alpha3: "isl",
        alt3: &["ice"],
        english: "Icelandic",
        native: "Íslenska",
    },
    LanguageName {
        code: "fa",
        alpha3: "fas",
        alt3: &["per"],
        english: "Persian",
        native: "فارسی",
    },
    LanguageName {
        code: "af",
        alpha3: "afr",
        alt3: &[],
        english: "Afrikaans",
        native: "Afrikaans",
    },
    LanguageName {
        code: "sw",
        alpha3: "swa",
        alt3: &[],
        english: "Swahili",
        native: "Kiswahili",
    },
    LanguageName {
        code: "sq",
        alpha3: "sqi",
        alt3: &["alb"],
        english: "Albanian",
        native: "Shqip",
    },
    LanguageName {
        code: "hy",
        alpha3: "hye",
        alt3: &["arm"],
        english: "Armenian",
        native: "Հայերեն",
    },
    LanguageName {
        code: "ka",
        alpha3: "kat",
        alt3: &["geo"],
        english: "Georgian",
        native: "ქართული",
    },
    LanguageName {
        code: "cy",
        alpha3: "cym",
        alt3: &["wel"],
        english: "Welsh",
        native: "Cymraeg",
    },
    LanguageName {
        code: "az",
        alpha3: "aze",
        alt3: &[],
        english: "Azerbaijani",
        native: "Azərbaycan",
    },
    LanguageName {
        code: "be",
        alpha3: "bel",
        alt3: &[],
        english: "Belarusian",
        native: "Беларуская",
    },
    LanguageName {
        code: "mk",
        alpha3: "mkd",
        alt3: &["mac"],
        english: "Macedonian",
        native: "Македонски",
    },
    LanguageName {
        code: "kk",
        alpha3: "kaz",
        alt3: &[],
        english: "Kazakh",
        native: "Қазақша",
    },
    LanguageName {
        code: "uz",
        alpha3: "uzb",
        alt3: &[],
        english: "Uzbek",
        native: "Oʻzbek",
    },
];

fn language_for(raw: &str) -> Option<&'static LanguageName> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let primary = trimmed.split(['-', '_']).next().unwrap_or(trimmed);
    let keys = [canonical(trimmed), canonical(primary)];
    LANGUAGES.iter().find(|language| {
        let names = [
            language.code,
            language.alpha3,
            language.english,
            language.native,
        ];
        names
            .iter()
            .copied()
            .chain(language.alt3.iter().copied())
            .any(|name| {
                let name = canonical(name);
                keys.contains(&name)
            })
    })
}

enum LanguageStyle {
    Code2 { upper: bool },
    Code3 { upper: bool },
    Locale,
    NameLower,
    NameTitle,
    NameUpper,
    Native,
    Unknown,
}

fn language_style(current: &str, language: Option<&LanguageName>) -> LanguageStyle {
    let trimmed = current.trim();
    if trimmed.contains('-') || trimmed.contains('_') {
        return LanguageStyle::Locale;
    }
    let letters: String = trimmed.chars().filter(|c| c.is_alphabetic()).collect();
    let ascii = letters.chars().all(|c| c.is_ascii_alphabetic());
    if ascii && letters.len() == 2 {
        return LanguageStyle::Code2 {
            upper: letters.chars().all(|c| c.is_ascii_uppercase()),
        };
    }
    if ascii && letters.len() == 3 {
        return LanguageStyle::Code3 {
            upper: letters.chars().all(|c| c.is_ascii_uppercase()),
        };
    }
    if let Some(language) = language {
        let value = canonical(trimmed);
        if value == canonical(language.native)
            && canonical(language.native) != canonical(language.english)
        {
            return LanguageStyle::Native;
        }
        if value == canonical(language.english) {
            if trimmed == language.english {
                return LanguageStyle::NameTitle;
            }
            if trimmed == language.english.to_lowercase() {
                return LanguageStyle::NameLower;
            }
            if trimmed == language.english.to_uppercase() {
                return LanguageStyle::NameUpper;
            }
            return LanguageStyle::NameLower;
        }
    }
    if trimmed.chars().next().is_some_and(char::is_uppercase) {
        LanguageStyle::NameTitle
    } else {
        LanguageStyle::Unknown
    }
}

fn validate_language_value(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("A language value cannot be empty.".to_string());
    }
    if trimmed.chars().any(char::is_control) || trimmed.chars().count() > 40 {
        return Err("That language value cannot be written to a config file.".to_string());
    }
    Ok(trimmed.to_string())
}

/// Map a Ludex language code onto what `current` and the file's allowed values
/// use. Returns a value the file can actually store.
fn normalize_language(
    current: &str,
    desired_code: &str,
    allowed: &[String],
    value_type: &str,
) -> Result<String, String> {
    if value_type == "boolean" {
        return Err("This key is a boolean; it is not a language field.".to_string());
    }
    let desired = language_for(desired_code).ok_or_else(|| {
        format!(
            "Ludex has no language name for “{}”; set this key manually.",
            desired_code.trim()
        )
    })?;
    if !allowed.is_empty() {
        if let Some(value) = allowed
            .iter()
            .find(|value| language_for(value).is_some_and(|language| language.code == desired.code))
        {
            return validate_language_value(value);
        }
        return Err(format!(
            "This file only accepts {} for this key; {} cannot be represented.",
            allowed.join(", "),
            desired.english
        ));
    }
    let trimmed = current.trim();
    if value_type == "number" || trimmed.parse::<i64>().is_ok() {
        return Err(
            "This key stores a numeric language id; Ludex cannot map a language name onto it. Set it manually in the config file."
                .to_string(),
        );
    }
    if trimmed.is_empty() {
        return Ok(desired.code.to_string());
    }
    let current_language = language_for(trimmed);
    let style = language_style(trimmed, current_language);
    let qualified = |code: &str| {
        let raw = code.trim().replace('_', "-");
        let mut parts = raw.split('-');
        let primary = parts.next().unwrap_or(desired.code).to_lowercase();
        match parts.next() {
            Some(region) if !region.is_empty() => {
                format!("{primary}-{}", region.to_uppercase())
            }
            _ => primary,
        }
    };
    let value = match style {
        LanguageStyle::Code2 { upper: true } => desired.code.to_uppercase(),
        LanguageStyle::Code2 { upper: false } => desired.code.to_string(),
        LanguageStyle::Code3 { upper: true } => desired.alpha3.to_uppercase(),
        LanguageStyle::Code3 { upper: false } => desired.alpha3.to_string(),
        LanguageStyle::Locale => qualified(desired_code),
        LanguageStyle::NameLower => desired.english.to_lowercase(),
        LanguageStyle::NameTitle => desired.english.to_string(),
        LanguageStyle::NameUpper => desired.english.to_uppercase(),
        LanguageStyle::Native => desired.native.to_string(),
        LanguageStyle::Unknown => desired.code.to_string(),
    };
    validate_language_value(&value)
}

/// Validate a player name Ludex writes into a config file.
pub fn normalize_player_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(
            "The Ludex profile has no display name yet. Set one in Settings → Profile, or type a name."
                .to_string(),
        );
    }
    if trimmed.chars().count() > MAX_PLAYER_NAME_CHARS {
        return Err(format!(
            "Player names are limited to {MAX_PLAYER_NAME_CHARS} characters."
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err("A player name cannot contain control characters.".to_string());
    }
    if !trimmed.chars().all(|character| {
        character.is_alphanumeric() || matches!(character, ' ' | '_' | '-' | '.' | '\'')
    }) {
        return Err("Player names can only use letters, digits, spaces and _ - . '".to_string());
    }
    Ok(trimmed.to_string())
}

/* --------------------------- preview and apply ---------------------------- */

struct ResolvedEdit {
    kind: FieldKind,
    file: String,
    key: String,
    current: String,
    next: String,
}

fn resolve_edit(
    install_root: &Path,
    file: &str,
    key: &str,
    target: &CommonFieldTarget,
) -> Result<ResolvedEdit, String> {
    let kind = FieldKind::from_id(&target.field)
        .ok_or_else(|| format!("Unknown common field “{}”.", target.field))?;
    let document = game_config::read(install_root, file)?;
    let matches: Vec<&ConfigEntry> = document
        .entries
        .iter()
        .filter(|entry| entry.key == key)
        .collect();
    if matches.is_empty() {
        return Err(format!(
            "{key} is no longer in {file}. Reopen the game settings and try again."
        ));
    }
    if matches.len() > 1 {
        return Err(format!(
            "{key} appears more than once in {file}; edit it directly in the file."
        ));
    }
    let entry = matches[0];
    let next = match kind {
        FieldKind::Language => normalize_language(
            &entry.value,
            &target.value,
            &allowed_values(&document),
            &entry.value_type,
        )?,
        FieldKind::PlayerName => normalize_player_name(&target.value)?,
    };
    Ok(ResolvedEdit {
        kind,
        file: file.to_string(),
        key: entry.key.clone(),
        current: entry.value.clone(),
        next,
    })
}

/// Compute the exact before/after of a common-field write without writing.
pub fn preview(
    install_root: &Path,
    file: &str,
    key: &str,
    target: &CommonFieldTarget,
) -> Result<CommonFieldPreview, String> {
    let resolved = resolve_edit(install_root, file, key, target)?;
    let unchanged = resolved.next == resolved.current;
    let diff = if unchanged {
        String::new()
    } else {
        let (_, diff) = game_config::preview(
            install_root,
            file,
            &[ConfigEditInput {
                key: resolved.key.clone(),
                value: resolved.next.clone(),
            }],
        )?;
        diff
    };
    Ok(CommonFieldPreview {
        field: resolved.kind.id().to_string(),
        file: resolved.file,
        key: resolved.key,
        current_value: resolved.current,
        next_value: resolved.next,
        unchanged,
        diff,
    })
}

/// Apply one common field through the backup + diff write path.
pub fn apply(
    install_root: &Path,
    app_data: &Path,
    game_id: i64,
    file: &str,
    key: &str,
    target: &CommonFieldTarget,
    origin: &str,
) -> Result<game_config::WriteOutcome, String> {
    let resolved = resolve_edit(install_root, file, key, target)?;
    if resolved.next == resolved.current {
        return Err("These values are already set; nothing was written.".to_string());
    }
    game_config::write(
        install_root,
        app_data,
        game_id,
        file,
        &[ConfigEditInput {
            key: resolved.key,
            value: resolved.next,
        }],
        origin,
    )
}

fn first_non_empty<'a>(values: &[&'a str]) -> &'a str {
    values
        .iter()
        .copied()
        .find(|value| !value.trim().is_empty())
        .unwrap_or("")
}

/// Re-apply the values a game opted into (#148). Called before a launch; it
/// never asks a question and every error is a warning instead of a failure.
pub fn apply_sync(
    app_data: &Path,
    install_root: &Path,
    connection: &Connection,
    game_id: i64,
    language: &str,
    player_name: &str,
) -> ConfigSyncOutcome {
    let mut outcome = ConfigSyncOutcome::default();
    let prefs = match db::get_config_sync_prefs(connection, game_id) {
        Ok(Some(prefs)) => prefs,
        Ok(None) => return outcome,
        Err(error) => {
            outcome.warnings.push(error);
            return outcome;
        }
    };
    if !prefs.enabled {
        return outcome;
    }
    let desired_language = first_non_empty(&[language, &prefs.language]).to_string();
    let desired_name = first_non_empty(&[player_name, &prefs.player_name]).to_string();
    let mappings = db::list_config_field_mappings(connection, game_id).unwrap_or_default();
    let detection = detect(install_root, &mappings);

    for field in detection.fields {
        let Some(kind) = FieldKind::from_id(&field.field) else {
            continue;
        };
        if field.confidence == "ambiguous" {
            continue;
        }
        let (Some(file), Some(key)) = (field.file.clone(), field.key.clone()) else {
            continue;
        };
        let target = match kind {
            FieldKind::Language => {
                if !prefs.apply_language || desired_language.is_empty() {
                    continue;
                }
                CommonFieldTarget {
                    field: field.field.clone(),
                    value: desired_language.clone(),
                }
            }
            FieldKind::PlayerName => {
                if !prefs.apply_player_name || desired_name.is_empty() {
                    continue;
                }
                CommonFieldTarget {
                    field: field.field.clone(),
                    value: desired_name.clone(),
                }
            }
        };
        let resolved = match resolve_edit(install_root, &file, &key, &target) {
            Ok(resolved) => resolved,
            Err(error) => {
                outcome.warnings.push(format!("{file} · {key}: {error}"));
                continue;
            }
        };
        if resolved.next == resolved.current {
            outcome.skipped += 1;
            continue;
        }
        match game_config::write(
            install_root,
            app_data,
            game_id,
            &file,
            &[ConfigEditInput {
                key: key.clone(),
                value: resolved.next.clone(),
            }],
            "sync",
        ) {
            Ok(written) => {
                if let Err(error) = db::insert_config_edit(connection, &written.record) {
                    outcome.warnings.push(error);
                }
                outcome.applied.push(AppliedCommonField {
                    field: field.field.clone(),
                    file,
                    key,
                    before: resolved.current,
                    after: resolved.next,
                    diff: written.diff,
                });
            }
            Err(error) => outcome.warnings.push(format!("{file} · {key}: {error}")),
        }
    }

    if !outcome.applied.is_empty() {
        if let Err(error) = db::mark_config_sync_applied(connection, game_id, &now_rfc3339()) {
            outcome.warnings.push(error);
        }
    }
    outcome
}

/// Export the pinned keys as a small JSON template (#148).
pub fn export_mappings(game_id: i64, mappings: &[ConfigFieldMapping]) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Template<'a> {
        game_id: i64,
        exported_at: String,
        mappings: &'a [ConfigFieldMapping],
    }
    serde_json::to_string_pretty(&Template {
        game_id,
        exported_at: now_rfc3339(),
        mappings,
    })
    .unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ludex-fields-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn target(field: &str, value: &str) -> CommonFieldTarget {
        CommonFieldTarget {
            field: field.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn detects_unreal_culture_as_the_language() {
        let root = temp_dir("unreal");
        write_file(&root, "Game.uproject", "{}");
        write_file(
            &root,
            "Saved/Config/Windows/Engine.ini",
            "[Internationalization]\nCulture=fr\n",
        );

        let detection = detect(&root, &[]);
        assert_eq!(detection.fields.len(), 1);
        let language = &detection.fields[0];
        assert_eq!(language.field, "language");
        assert_eq!(language.confidence, "certain");
        assert_eq!(language.source, "builtin");
        assert_eq!(
            language.file.as_deref(),
            Some("Saved/Config/Windows/Engine.ini")
        );
        assert_eq!(
            language.key.as_deref(),
            Some("Internationalization/Culture")
        );
        assert_eq!(language.value.as_deref(), Some("fr"));
        assert!(language.candidates.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_player_name_and_language_in_a_generic_ini() {
        let root = temp_dir("generic");
        write_file(
            &root,
            "steam_emu.ini",
            "[Settings]\nLanguage=english\nUserName=cleeryy\n",
        );

        let detection = detect(&root, &[]);
        assert_eq!(detection.fields.len(), 2);
        // Catalog order: language first, then player name.
        assert_eq!(detection.fields[0].field, "language");
        assert_eq!(detection.fields[0].value.as_deref(), Some("english"));
        assert_eq!(detection.fields[1].field, "playerName");
        assert_eq!(detection.fields[1].value.as_deref(), Some("cleeryy"));
        assert_eq!(
            detection.fields[1].key.as_deref(),
            Some("Settings/UserName")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_gog_json_language_and_its_allowed_values() {
        let root = temp_dir("gog");
        write_file(
            &root,
            "goggame-1234.info",
            r#"{"gameId": "1234", "language": "en", "langs": ["en", "fr", "de"]}"#,
        );

        let detection = detect(&root, &[]);
        assert_eq!(detection.fields.len(), 1);
        let language = &detection.fields[0];
        assert_eq!(language.confidence, "certain");
        assert_eq!(language.allowed_values, vec!["en", "fr", "de"]);

        let preview = preview(
            &root,
            "goggame-1234.info",
            "language",
            &target("language", "fr"),
        )
        .unwrap();
        assert_eq!(preview.next_value, "fr");
        assert!(!preview.unchanged);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_godot_locale_through_its_section() {
        let root = temp_dir("godot");
        write_file(
            &root,
            "project.godot",
            "[internationalization]\nlocale/fallback=\"en\"\n",
        );

        let detection = detect(&root, &[]);
        assert_eq!(detection.fields.len(), 1);
        let language = &detection.fields[0];
        assert_eq!(language.confidence, "likely");
        assert_eq!(
            language.key.as_deref(),
            Some("internationalization/locale/fallback")
        );
        assert_eq!(language.value.as_deref(), Some("en"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_unity_prefs_language_with_a_hashed_suffix() {
        let root = temp_dir("unity");
        write_file(&root, "prefs", "Language_h12345=Français\n");

        let detection = detect(&root, &[]);
        assert_eq!(detection.fields.len(), 1);
        let language = &detection.fields[0];
        assert_eq!(language.confidence, "certain");
        assert_eq!(language.key.as_deref(), Some("Language_h12345"));

        let preview =
            preview(&root, "prefs", "Language_h12345", &target("language", "de")).unwrap();
        assert_eq!(preview.next_value, "Deutsch");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detects_nested_json_language_and_player_name() {
        let root = temp_dir("json");
        write_file(
            &root,
            "settings.json",
            r#"{"Game": {"Settings": {"Language": "fr", "PlayerName": "cleeryy"}}}"#,
        );

        let detection = detect(&root, &[]);
        assert_eq!(detection.fields.len(), 2);
        assert_eq!(
            detection.fields[0].key.as_deref(),
            Some("Game.Settings.Language")
        );
        assert_eq!(
            detection.fields[1].key.as_deref(),
            Some("Game.Settings.PlayerName")
        );

        let preview = preview(
            &root,
            "settings.json",
            "Game.Settings.Language",
            &target("language", "de"),
        )
        .unwrap();
        assert_eq!(preview.current_value, "fr");
        assert_eq!(preview.next_value, "de");
        assert!(preview.diff.contains(
            "-{\"Game\": {\"Settings\": {\"Language\": \"fr\", \"PlayerName\": \"cleeryy\"}}}"
        ));
        assert!(preview.diff.contains(
            "+{\"Game\": {\"Settings\": {\"Language\": \"de\", \"PlayerName\": \"cleeryy\"}}}"
        ));
        assert_eq!(
            fs::read_to_string(root.join("settings.json")).unwrap(),
            r#"{"Game": {"Settings": {"Language": "fr", "PlayerName": "cleeryy"}}}"#
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ambiguous_keys_are_listed_and_never_auto_picked() {
        let root = temp_dir("ambiguous");
        write_file(&root, "steam_emu.ini", "[Settings]\nLanguage=english\n");
        write_file(&root, "RUNE/settings.ini", "[Settings]\nLanguage=english\n");

        let detection = detect(&root, &[]);
        assert_eq!(detection.fields.len(), 1);
        let language = &detection.fields[0];
        assert_eq!(language.confidence, "ambiguous");
        assert_eq!(language.file, None);
        assert_eq!(language.key, None);
        assert_eq!(language.value, None);
        assert_eq!(language.candidates.len(), 2);
        assert!(detection
            .notes
            .iter()
            .any(|note| note.contains("pin the one this game reads")));
        // Detection never writes with an ambiguous match.
        assert!(detection
            .fields
            .iter()
            .filter(|field| field.confidence == "ambiguous")
            .all(|field| field.file.is_none()));

        // A pinned mapping resolves the ambiguity without touching the file.
        let mappings = vec![ConfigFieldMapping {
            game_id: 7,
            field: "language".to_string(),
            file_path: "RUNE/settings.ini".to_string(),
            key: "Settings/Language".to_string(),
            created_at: "2026-11-11T10:00:00Z".to_string(),
        }];
        let detection = detect(&root, &mappings);
        let language = &detection.fields[0];
        assert_eq!(language.confidence, "certain");
        assert_eq!(language.source, "template");
        assert_eq!(language.file.as_deref(), Some("RUNE/settings.ini"));
        assert_eq!(language.value.as_deref(), Some("english"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_template_can_pin_a_key_outside_the_catalog() {
        let root = temp_dir("template");
        write_file(&root, "settings.ini", "[Misc]\nChosenLanguage=fr\n");
        let mappings = vec![ConfigFieldMapping {
            game_id: 1,
            field: "language".to_string(),
            file_path: "settings.ini".to_string(),
            key: "Misc/ChosenLanguage".to_string(),
            created_at: "2026-11-11T10:00:00Z".to_string(),
        }];

        let detection = detect(&root, &mappings);
        assert_eq!(detection.fields.len(), 1);
        assert_eq!(detection.fields[0].source, "template");
        assert_eq!(detection.fields[0].confidence, "certain");
        assert_eq!(
            detection.fields[0].key.as_deref(),
            Some("Misc/ChosenLanguage")
        );

        let preview = preview(
            &root,
            "settings.ini",
            "Misc/ChosenLanguage",
            &target("language", "de"),
        )
        .unwrap();
        assert_eq!(preview.next_value, "de");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_stale_mapping_falls_back_to_detection_with_a_note() {
        let root = temp_dir("stale-mapping");
        write_file(&root, "settings.ini", "[Player]\nUserName=cleeryy\n");
        let mappings = vec![ConfigFieldMapping {
            game_id: 3,
            field: "playerName".to_string(),
            file_path: "settings.ini".to_string(),
            key: "Player/Name".to_string(),
            created_at: "2026-11-11T10:00:00Z".to_string(),
        }];

        let detection = detect(&root, &mappings);
        assert_eq!(detection.fields.len(), 1);
        assert_eq!(detection.fields[0].source, "builtin");
        assert_eq!(detection.fields[0].key.as_deref(), Some("Player/UserName"));
        assert!(detection
            .notes
            .iter()
            .any(|note| note.contains("no longer in the install folder")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_recognisable_key_returns_no_fields() {
        let root = temp_dir("missing");
        write_file(
            &root,
            "GameUserSettings.ini",
            "[/Script/Engine.GameUserSettings]\nResolutionSizeX=1920\nFullscreenMode=1\n",
        );

        let detection = detect(&root, &[]);
        assert!(detection.fields.is_empty());
        assert_eq!(detection.files, 1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn language_values_follow_the_style_the_file_already_uses() {
        let cases = [
            ("english", "fr", "french"),
            ("English", "fr", "French"),
            ("ENGLISH", "fr", "FRENCH"),
            ("fr", "de", "de"),
            ("FR", "de", "DE"),
            ("fra", "de", "deu"),
            ("Français", "de", "Deutsch"),
            ("en-US", "fr-CA", "fr-CA"),
            ("en-US", "fr", "fr"),
        ];
        for (current, desired, expected) in cases {
            assert_eq!(
                normalize_language(current, desired, &[], "string").unwrap(),
                expected,
                "{current} -> {desired}"
            );
        }
    }

    #[test]
    fn allowed_values_win_over_the_current_style() {
        let allowed = vec!["english".to_string(), "german".to_string()];
        assert_eq!(
            normalize_language("english", "de", &allowed, "string").unwrap(),
            "german"
        );
        let error = normalize_language("english", "fr", &allowed, "string").unwrap_err();
        assert!(error.contains("only accepts english, german"), "{error}");
    }

    #[test]
    fn numeric_language_ids_are_refused_with_a_clear_error() {
        let error = normalize_language("1", "fr", &[], "number").unwrap_err();
        assert!(error.contains("numeric language id"), "{error}");
        assert!(normalize_language("en", "xx", &[], "string").is_err());
    }

    #[test]
    fn player_names_are_validated() {
        assert_eq!(normalize_player_name("  cleeryy ").unwrap(), "cleeryy");
        assert_eq!(
            normalize_player_name("Jean-Luc O'Neil").unwrap(),
            "Jean-Luc O'Neil"
        );
        assert!(normalize_player_name("")
            .unwrap_err()
            .contains("display name"));
        assert!(
            normalize_player_name(&"x".repeat(MAX_PLAYER_NAME_CHARS + 1))
                .unwrap_err()
                .contains("limited")
        );
        assert!(normalize_player_name("bad\tname")
            .unwrap_err()
            .contains("control"));
        assert!(normalize_player_name("a;b")
            .unwrap_err()
            .contains("only use"));
    }

    #[test]
    fn preview_reports_the_diff_and_never_touches_the_file() {
        let root = temp_dir("preview");
        let original = "[Settings]\r\nLanguage=english\r\n; keep me\r\n";
        write_file(&root, "steam_emu.ini", original);

        let preview = preview(
            &root,
            "steam_emu.ini",
            "Settings/Language",
            &target("language", "fr"),
        )
        .unwrap();
        assert_eq!(preview.current_value, "english");
        assert_eq!(preview.next_value, "french");
        assert!(preview.diff.contains("-Language=english"));
        assert!(preview.diff.contains("+Language=french"));
        assert_eq!(
            fs::read_to_string(root.join("steam_emu.ini")).unwrap(),
            original
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_preserves_utf16_encoding_and_takes_a_backup() {
        let root = temp_dir("utf16");
        let app_data = temp_dir("utf16-data");
        let text = "[Settings]\r\nLanguage=english\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        fs::write(root.join("steam_emu.ini"), &bytes).unwrap();

        let outcome = apply(
            &root,
            &app_data,
            7,
            "steam_emu.ini",
            "Settings/Language",
            &target("language", "fr"),
            "common-field",
        )
        .unwrap();
        assert!(outcome.diff.contains("+Language=french"));
        assert!(PathBuf::from(&outcome.record.backup_path).is_file());
        assert_eq!(outcome.record.origin, "common-field");

        let saved = fs::read(root.join("steam_emu.ini")).unwrap();
        assert_eq!(&saved[..2], &[0xFF, 0xFE]);
        let (decoded, had_errors) = encoding_rs::UTF_16LE.decode_without_bom_handling(&saved[2..]);
        assert!(!had_errors);
        assert!(decoded.contains("Language=french"));
        assert!(decoded.contains("\r\n"));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&app_data);
    }

    #[test]
    fn a_value_the_encoding_cannot_store_is_refused_by_the_preview() {
        let root = temp_dir("cp1252");
        let mut bytes = b"[Player]\nUserName=caf".to_vec();
        bytes.push(0xE9);
        bytes.push(b'\n');
        fs::write(root.join("settings.ini"), &bytes).unwrap();

        let error = preview(
            &root,
            "settings.ini",
            "Player/UserName",
            &target("playerName", "日本語"),
        )
        .unwrap_err();
        assert!(error.contains("Windows-1252"), "{error}");
        assert_eq!(fs::read(root.join("settings.ini")).unwrap(), bytes);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sync_applies_enabled_fields_and_can_be_switched_off() {
        let root = temp_dir("sync");
        let app_data = temp_dir("sync-data");
        write_file(
            &root,
            "steam_emu.ini",
            "[Settings]\nLanguage=english\nUserName=old\n",
        );
        let connection = db::open_in_memory_for_tests();
        db::save_config_sync_prefs(
            &connection,
            7,
            true,
            "fr",
            "cleeryy",
            true,
            true,
            "2026-11-11T10:00:00Z",
        )
        .unwrap();

        let outcome = apply_sync(&app_data, &root, &connection, 7, "fr", "cleeryy");
        assert_eq!(outcome.applied.len(), 2, "{:?}", outcome.warnings);
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        let saved = fs::read_to_string(root.join("steam_emu.ini")).unwrap();
        assert!(saved.contains("Language=french"), "{saved}");
        assert!(saved.contains("UserName=cleeryy"), "{saved}");
        let history = db::list_config_edits(&connection, 7, 10).unwrap();
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|record| record.origin == "sync"));
        let prefs = db::get_config_sync_prefs(&connection, 7).unwrap().unwrap();
        assert!(prefs.last_applied_at.is_some());

        // A second launch with the same values is a no-op.
        let outcome = apply_sync(&app_data, &root, &connection, 7, "fr", "cleeryy");
        assert!(outcome.applied.is_empty());
        assert_eq!(outcome.skipped, 2);

        // Next launch picks up the new app language.
        let outcome = apply_sync(&app_data, &root, &connection, 7, "de", "cleeryy");
        assert_eq!(outcome.applied.len(), 1);
        assert!(fs::read_to_string(root.join("steam_emu.ini"))
            .unwrap()
            .contains("Language=german"));

        // Toggling the opt-in off stops it.
        db::save_config_sync_prefs(
            &connection,
            7,
            false,
            "fr",
            "cleeryy",
            true,
            true,
            "2026-11-11T10:10:00Z",
        )
        .unwrap();
        let outcome = apply_sync(&app_data, &root, &connection, 7, "es", "cleeryy");
        assert!(outcome.applied.is_empty());
        assert!(fs::read_to_string(root.join("steam_emu.ini"))
            .unwrap()
            .contains("Language=german"));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&app_data);
    }

    #[test]
    fn sync_never_guesses_an_ambiguous_key() {
        let root = temp_dir("sync-ambiguous");
        let app_data = temp_dir("sync-ambiguous-data");
        write_file(&root, "steam_emu.ini", "[Settings]\nLanguage=english\n");
        write_file(&root, "RUNE/settings.ini", "[Settings]\nLanguage=english\n");
        let connection = db::open_in_memory_for_tests();
        db::save_config_sync_prefs(
            &connection,
            9,
            true,
            "fr",
            "",
            true,
            false,
            "2026-11-11T10:00:00Z",
        )
        .unwrap();

        let outcome = apply_sync(&app_data, &root, &connection, 9, "fr", "");
        assert!(outcome.applied.is_empty());
        assert_eq!(
            fs::read_to_string(root.join("steam_emu.ini")).unwrap(),
            "[Settings]\nLanguage=english\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("RUNE/settings.ini")).unwrap(),
            "[Settings]\nLanguage=english\n"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&app_data);
    }

    #[test]
    fn exported_templates_round_trip_as_json() {
        let mappings = vec![ConfigFieldMapping {
            game_id: 12,
            field: "language".to_string(),
            file_path: "settings.ini".to_string(),
            key: "Settings/Language".to_string(),
            created_at: "2026-11-11T10:00:00Z".to_string(),
        }];
        let exported = export_mappings(12, &mappings);
        let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
        assert_eq!(parsed["gameId"], 12);
        assert_eq!(parsed["mappings"][0]["field"], "language");
        assert_eq!(parsed["mappings"][0]["file"], "settings.ini");
        assert_eq!(parsed["mappings"][0]["key"], "Settings/Language");
    }
}
