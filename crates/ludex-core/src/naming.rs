//! Filename parser for library files.
//!
//! Ludex follows the documented GameVault naming convention:
//!
//! ```text
//! Title (v1.2.3) (EarlyAccess) (GameType) (NoCache) (ReleaseYear).zip
//! ```
//!
//! Round brackets are reserved for flags and are removed from the title. Square
//! brackets are part of the title and are the supported way to keep same-title
//! files as separate games (`Minecraft [Tekkit Modpack]` vs `Minecraft`).
//! A bare trailing version (`Title v1.2.3.rar`) is also accepted.
//!
//! The parser never invents metadata: it only removes well-known suffixes.

use crate::slug::slugify;

/// A filename decomposed into title, flags and grouping helpers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedName {
    /// Display title, with reserved flags removed.
    pub title: String,
    /// Release year, when a `(YYYY)` flag is present.
    pub year: Option<u16>,
    /// Version label without the leading `v`, when present.
    pub version: Option<String>,
    /// Unknown parenthesised flags, in source order.
    pub tags: Vec<String>,
    /// Title with a leading English article removed, used for sorting.
    pub sort_title: String,
    /// Folder/URL-safe slug derived from the title.
    pub slug: String,
}

/// Extensions removed before parsing. Archive and executable formats only.
const KNOWN_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "exe", "iso", "tar", "gz", "tgz", "bz2", "xz", "cab",
];

/// Flags recognised inside round brackets and dropped from the title.
const KNOWN_FLAGS: &[&str] = &[
    "ea",
    "earlyaccess",
    "nc",
    "nocache",
    "w_s",
    "w_p",
    "w_sw",
    "l_p",
    "l_sw",
];

/// Parses a file name (or path) into a [`ParsedName`].
///
/// ```
/// let parsed = ludex_core::parse_filename("HITMAN 3 (v3.10.1) (2021).7z");
/// assert_eq!(parsed.title, "HITMAN 3");
/// assert_eq!(parsed.version.as_deref(), Some("3.10.1"));
/// assert_eq!(parsed.year, Some(2021));
/// ```
pub fn parse_filename(name: &str) -> ParsedName {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = strip_known_extensions(base);

    // Parentheses are parsed before `_` is turned into a space so that flags
    // such as `(W_S)` keep their exact spelling.
    let (without_parens, groups) = strip_parenthetical_groups(stem);
    let collapsed = normalize_separators(&without_parens);

    let mut year: Option<u16> = None;
    let mut version: Option<String> = None;
    let mut tags: Vec<String> = Vec::new();

    for group in groups {
        let flag = group.trim();
        if flag.is_empty() {
            continue;
        }
        if let Some(parsed_year) = parse_year(flag) {
            if year.is_none() {
                year = Some(parsed_year);
            }
            continue;
        }
        if let Some(inner) = flag.strip_prefix('v') {
            if let Some(parsed_version) = parse_version_components(inner) {
                if version.is_none() {
                    version = Some(parsed_version);
                }
                continue;
            }
        }
        if KNOWN_FLAGS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(flag))
        {
            continue;
        }
        if !tags.iter().any(|tag| tag.eq_ignore_ascii_case(flag)) {
            tags.push(flag.to_string());
        }
    }

    let (title_without_version, version) = extract_trailing_version(&collapsed, version);
    let mut title = cleanup_title(&title_without_version);
    if title.is_empty() {
        title = cleanup_title(&normalize_separators(stem));
        if title.is_empty() {
            title = base.to_string();
        }
    }

    let sort_title = sort_title(&title);
    let slug = slugify(&title);

    ParsedName {
        title,
        year,
        version,
        tags,
        sort_title,
        slug,
    }
}

/// Normalises a title for grouping: lowercase, ASCII-folded, punctuation
/// replaced by single spaces, whitespace collapsed.
///
/// Square brackets are dropped but their content is kept, so
/// `Minecraft [Tekkit Modpack]` normalises to `minecraft tekkit modpack`.
///
/// ```
/// assert_eq!(ludex_core::normalize_title("The Roottrees are Dead"), "the roottrees are dead");
/// assert_eq!(ludex_core::normalize_title("roottrees-are-dead"), "roottrees are dead");
/// ```
pub fn normalize_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        for lower in c.to_lowercase() {
            if lower.is_ascii_alphanumeric() {
                out.push(lower);
            } else if let Some(folded) = crate::slug::fold_to_ascii(lower) {
                out.push_str(folded);
            } else if lower.is_alphanumeric() {
                out.push(lower);
            } else {
                out.push(' ');
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Stable identity key used to group files into one game: normalised sort title
/// plus release-year bucket (`none` when untagged).
///
/// ```
/// let a = ludex_core::parse_filename("The Roottrees are Dead.zip");
/// let b = ludex_core::parse_filename("roottrees-are-dead (v1.1).zip");
/// assert_eq!(ludex_core::grouping_key(&a), ludex_core::grouping_key(&b));
/// ```
pub fn grouping_key(parsed: &ParsedName) -> String {
    let year = parsed
        .year
        .map_or_else(|| "none".to_string(), |year| year.to_string());
    format!("{}::{}", normalize_title(&parsed.sort_title), year)
}

/// Strips a leading English article (`The `, `A `, `An `) from a title.
fn sort_title(title: &str) -> String {
    let trimmed = title.trim_start();
    for article in ["the ", "an ", "a "] {
        let Some(prefix) = trimmed.get(..article.len()) else {
            continue;
        };
        if prefix.eq_ignore_ascii_case(article) {
            let rest = trimmed[article.len()..].trim_start();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    trimmed.to_string()
}

/// Removes known archive/executable extensions, at most twice so that
/// `Game.tar.gz` and `Game.zip` both yield `Game`.
fn strip_known_extensions(file_name: &str) -> &str {
    let mut stem = file_name;
    for _ in 0..2 {
        let Some((head, extension)) = stem.rsplit_once('.') else {
            break;
        };
        if head.is_empty() {
            break;
        }
        if KNOWN_EXTENSIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(extension))
        {
            stem = head;
        } else {
            break;
        }
    }
    stem
}

/// Removes every `(...)` group, returning the remaining text and the group
/// contents in source order.
fn strip_parenthetical_groups(input: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(input.len());
    let mut groups = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '(' {
            out.push(c);
            continue;
        }
        let mut group = String::new();
        let mut closed = false;
        for inner in chars.by_ref() {
            if inner == ')' {
                closed = true;
                break;
            }
            group.push(inner);
        }
        if closed {
            groups.push(group);
            out.push(' ');
        } else {
            // Unbalanced parenthesis: keep it literally rather than guessing.
            out.push('(');
            out.push_str(&group);
        }
    }

    (out, groups)
}

/// Parses a `(YYYY)` flag, accepting only plausible release years.
fn parse_year(flag: &str) -> Option<u16> {
    if flag.len() != 4 || !flag.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year = flag.parse::<u16>().ok()?;
    (1900..=2100).contains(&year).then_some(year)
}

/// Parses the part after an optional `v` into a normalised version string.
///
/// Requires at least two dot-separated components so that titles such as
/// `Formula 1 2024` or `Forza Horizon 6` are never mistaken for versions.
fn parse_version_components(input: &str) -> Option<String> {
    if input.is_empty() || input.len() > 40 || !input.contains('.') {
        return None;
    }
    let parts: Vec<&str> = input.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let last_index = parts.len() - 1;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return None;
        }
        if index == last_index {
            let digits = part.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            if part.len() - digits.len() > 4 {
                return None;
            }
        } else if !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    Some(input.to_string())
}

/// Extracts a trailing bare version (`Title v1.2.3`, `Title 1.2.3`).
///
/// A trailing bracketed version (`Title [v1.2.3]`) yields the version but keeps
/// the bracket in the title, because square brackets are part of the title.
fn extract_trailing_version(input: &str, existing: Option<String>) -> (String, Option<String>) {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return (String::new(), existing);
    }

    if trimmed.ends_with(']') {
        if let Some(open) = trimmed.rfind('[') {
            let inner = &trimmed[open + 1..trimmed.len() - 1];
            if let Some(inner_version) = inner.strip_prefix('v') {
                if let Some(parsed) = parse_version_components(inner_version) {
                    let version = existing.or(Some(parsed));
                    return (trimmed.to_string(), version);
                }
            }
        }
    }

    let Some(split) = trimmed.rfind(char::is_whitespace) else {
        return (trimmed.to_string(), existing);
    };
    let token = &trimmed[split..].trim();
    let candidate = token
        .strip_prefix('v')
        .and_then(parse_version_components)
        .or_else(|| parse_version_components(token));
    let Some(parsed) = candidate else {
        return (trimmed.to_string(), existing);
    };

    let head = trimmed[..split].trim_end();
    if head.is_empty() {
        return (trimmed.to_string(), existing);
    }
    (head.to_string(), existing.or(Some(parsed)))
}

/// Collapses whitespace and trims separators left behind by flag removal.
fn cleanup_title(input: &str) -> String {
    collapse_whitespace(input)
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.'))
        .to_string()
}

/// Replaces `_` and non-breaking spaces with spaces, then collapses runs of
/// whitespace.
fn normalize_separators(input: &str) -> String {
    collapse_whitespace(&input.replace(['_', '\u{00a0}'], " "))
}

/// Replaces runs of whitespace with a single space and trims the result.
fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(name: &str) -> ParsedName {
        parse_filename(name)
    }

    #[test]
    fn real_library_files() {
        let cases = [
            "Blue Prince.zip",
            "Bodycam.zip",
            "Call of Duty Black Ops III.zip",
            "Cat Quest II.zip",
            "Forza Horizon 6.zip",
            "HITMAN 3.zip",
            "It Takes Two.zip",
            "Little Nightmares II.zip",
            "Little Nightmares III.zip",
            "Little Nightmares.rar",
            "Marvels Spider Man 2.zip",
            "Minecraft Dungeons II.zip",
            "Minecraft Dungeons.rar",
            "Outer Wilds.rar",
            "RV There Yet.zip",
            "Sackboy A Big Adventure.zip",
            "over the hill.zip",
        ];
        for case in cases {
            let expected = strip_known_extensions(case);
            let result = parsed(case);
            assert_eq!(result.title, expected, "{case}");
            assert_eq!(result.year, None, "{case}");
            assert_eq!(result.version, None, "{case}");
            assert!(result.tags.is_empty(), "{case}: {:?}", result.tags);
        }
    }

    #[test]
    fn uncharted_bare_version() {
        let result = parsed("UNCHARTED Legacy of Thieves Collection v1.4.21058.rar");
        assert_eq!(result.title, "UNCHARTED Legacy of Thieves Collection");
        assert_eq!(result.version.as_deref(), Some("1.4.21058"));
        assert_eq!(result.year, None);
    }

    #[test]
    fn year_flag() {
        let result = parsed("Game (2016).zip");
        assert_eq!(result.title, "Game");
        assert_eq!(result.year, Some(2016));
        assert_eq!(result.version, None);
    }

    #[test]
    fn square_brackets_stay_in_title() {
        let result = parsed("Game [GOTY] v1.2.3.7z");
        assert_eq!(result.title, "Game [GOTY]");
        assert_eq!(result.version.as_deref(), Some("1.2.3"));
        assert!(result.tags.is_empty(), "{:?}", result.tags);
        assert_eq!(result.slug, "game-goty");
    }

    #[test]
    fn brackets_force_separate_groups() {
        let plain = parsed("Minecraft.zip");
        let modpack = parsed("Minecraft [Tekkit Modpack].zip");
        assert_ne!(grouping_key(&plain), grouping_key(&modpack));
        assert_eq!(modpack.title, "Minecraft [Tekkit Modpack]");
        assert_eq!(modpack.slug, "minecraft-tekkit-modpack");
    }

    #[test]
    fn documented_flags() {
        let result = parsed("The Wandering Village (v0.1.32) (EA) (2022).iso");
        assert_eq!(result.title, "The Wandering Village");
        assert_eq!(result.version.as_deref(), Some("0.1.32"));
        assert_eq!(result.year, Some(2022));
        assert_eq!(result.sort_title, "Wandering Village");
        assert!(result.tags.is_empty(), "{:?}", result.tags);

        let result = parsed("Saints Row (W_S) (2022).zip");
        assert_eq!(result.title, "Saints Row");
        assert_eq!(result.year, Some(2022));
        assert!(result.tags.is_empty(), "{:?}", result.tags);

        let result = parsed("My personal IndieGame (v1.2.9) (NC) (2018).zip");
        assert_eq!(result.title, "My personal IndieGame");
        assert_eq!(result.version.as_deref(), Some("1.2.9"));
        assert_eq!(result.year, Some(2018));
        assert!(result.tags.is_empty(), "{:?}", result.tags);
    }

    #[test]
    fn multiple_versions_same_group() {
        let a = parsed("Minecraft (v1.7.10) (2011).zip");
        let b = parsed("Minecraft (v1.13.2) (2011).zip");
        assert_eq!(grouping_key(&a), grouping_key(&b));
        assert_eq!(a.title, "Minecraft");
        assert_eq!(b.title, "Minecraft");
    }

    #[test]
    fn unknown_parenthesised_flag_becomes_tag() {
        let result = parsed("Game (GOTY) (2019).zip");
        assert_eq!(result.title, "Game");
        assert_eq!(result.year, Some(2019));
        assert_eq!(result.tags, vec!["GOTY".to_string()]);
    }

    #[test]
    fn version_forms() {
        for (name, version) in [
            ("Game v1.2.3.zip", "1.2.3"),
            ("Game 1.2.3.zip", "1.2.3"),
            ("Game v1.2.rar", "1.2"),
            ("Game [v1.2.3].zip", "1.2.3"),
            ("Game (v1.2.3).zip", "1.2.3"),
        ] {
            let result = parsed(name);
            assert_eq!(result.version.as_deref(), Some(version), "{name}");
            assert_eq!(
                result.title,
                if name.contains("[v1.2.3]") {
                    "Game [v1.2.3]"
                } else {
                    "Game"
                }
            );
        }
    }

    #[test]
    fn titles_are_not_mistaken_for_versions() {
        for name in [
            "Formula 1 2024.zip",
            "Forza Horizon 6.zip",
            "HITMAN 3.zip",
            "Marvels Spider Man 2.zip",
            "Clair Obscur Expedition 33.zip",
        ] {
            let result = parsed(name);
            assert_eq!(result.version, None, "{name}");
        }
        assert_eq!(parsed("Formula 1 2024.zip").title, "Formula 1 2024");
    }

    #[test]
    fn apostrophes_and_underscores() {
        let result = parsed("Don't Starve Together.zip");
        assert_eq!(result.title, "Don't Starve Together");
        assert_eq!(result.slug, "don-t-starve-together");

        let result = parsed("Clair Obscur_ Expedition 33.zip");
        assert_eq!(result.title, "Clair Obscur Expedition 33");
        assert_eq!(result.slug, "clair-obscur-expedition-33");
    }

    #[test]
    fn accents_are_preserved_in_title_and_folded_in_slug() {
        let result = parsed("Pokémon Écarlate.zip");
        assert_eq!(result.title, "Pokémon Écarlate");
        assert_eq!(result.slug, "pokemon-ecarlate");
        assert_eq!(normalize_title(&result.title), "pokemon ecarlate");
    }

    #[test]
    fn sort_title_strips_articles() {
        assert_eq!(
            parsed("The Witcher 3 Wild Hunt.zip").sort_title,
            "Witcher 3 Wild Hunt"
        );
        assert_eq!(
            parsed("A Plague Tale Innocence.zip").sort_title,
            "Plague Tale Innocence"
        );
        assert_eq!(parsed("Anodyne.zip").sort_title, "Anodyne");
        assert_eq!(parsed("over the hill.zip").sort_title, "over the hill");
        assert_eq!(parsed("THE LONG DARK.zip").sort_title, "LONG DARK");
    }

    #[test]
    fn grouping_ignores_case_and_punctuation() {
        let a = parsed("The Roottrees are Dead.zip");
        let b = parsed("roottrees-are-dead.zip");
        let c = parsed("Roottrees Are Dead (v1.1).zip");
        assert_eq!(grouping_key(&a), grouping_key(&b));
        assert_eq!(grouping_key(&a), grouping_key(&c));
        assert_eq!(grouping_key(&a), "roottrees are dead::none");
    }

    #[test]
    fn grouping_includes_year() {
        let a = parsed("Doom (1993).zip");
        let b = parsed("Doom (2016).zip");
        assert_ne!(grouping_key(&a), grouping_key(&b));
        assert_eq!(grouping_key(&a), "doom::1993");
    }

    #[test]
    fn directory_paths_are_ignored() {
        let result = parsed("Games/Blue Prince.zip");
        assert_eq!(result.title, "Blue Prince");
    }

    #[test]
    fn handles_unbalanced_parenthesis() {
        let result = parsed("Game (Beta.zip");
        assert_eq!(result.title, "Game (Beta");
    }

    #[test]
    fn empty_and_extension_only_names() {
        assert_eq!(parsed("").title, "");
        // A dotfile keeps its name; cleanup only trims separators.
        assert_eq!(parsed(".zip").title, "zip");
    }
}
