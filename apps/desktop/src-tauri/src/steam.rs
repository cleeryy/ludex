//! Steam library import (#13).
//!
//! Ludex does not download Steam games. It reads Steam's own metadata
//! (`steamapps/libraryfolders.vdf` and `appmanifest_*.acf`), lets the user
//! import selected titles into the local index and launches them through
//! `steam://rungameid/<appid>`, so Steam keeps ownership, updates and DRM.
//!
//! Title matching is deliberately conservative: normalised titles must match
//! exactly, then once more after stripping common edition suffixes. Steam app
//! ids are never guessed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

/// A fully installed Steam game discovered on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamGame {
    pub app_id: u32,
    pub name: String,
    /// Absolute path of the game folder (`…/steamapps/common/<installdir>`).
    pub install_dir: String,
    /// Steam library root that owns this game.
    pub library_path: String,
    pub size_bytes: u64,
    /// Steam's own last-played timestamp, Unix seconds.
    pub last_played: Option<i64>,
}

/// Everything a scan found, including the roots it looked at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamScan {
    pub roots: Vec<String>,
    pub games: Vec<SteamGame>,
    pub warnings: Vec<String>,
}

/// Valve KeyValues value: a string or a nested object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VdfValue {
    String(String),
    Object(BTreeMap<String, VdfValue>),
}

impl VdfValue {
    fn as_str(&self) -> Option<&str> {
        match self {
            VdfValue::String(value) => Some(value),
            VdfValue::Object(_) => None,
        }
    }

    fn as_object(&self) -> Option<&BTreeMap<String, VdfValue>> {
        match self {
            VdfValue::Object(value) => Some(value),
            VdfValue::String(_) => None,
        }
    }
}

/// Look up a key case-insensitively (Valve's casing is not stable).
fn get<'a>(object: &'a BTreeMap<String, VdfValue>, key: &str) -> Option<&'a VdfValue> {
    object
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
}

fn get_str<'a>(object: &'a BTreeMap<String, VdfValue>, key: &str) -> Option<&'a str> {
    get(object, key).and_then(VdfValue::as_str)
}

/// Tokenise VDF text. Quoted and bare tokens are both accepted; `//` comments
/// run to the end of the line.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '"' => {
                let mut value = String::new();
                while let Some(next) = chars.next() {
                    match next {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => value.push('\n'),
                            Some('t') => value.push('\t'),
                            Some('\\') => value.push('\\'),
                            Some('"') => value.push('"'),
                            Some(other) => {
                                // Windows paths in VDF use single backslashes.
                                value.push('\\');
                                value.push(other);
                            }
                            None => break,
                        },
                        other => value.push(other),
                    }
                }
                tokens.push(value);
            }
            '{' | '}' => tokens.push(character.to_string()),
            '/' => {
                if chars.peek() == Some(&'/') {
                    for next in chars.by_ref() {
                        if next == '\n' {
                            break;
                        }
                    }
                }
            }
            other if other.is_whitespace() => {}
            other => {
                let mut value = String::new();
                value.push(other);
                while let Some(next) = chars.peek().copied() {
                    if next.is_whitespace() || next == '{' || next == '}' || next == '"' {
                        break;
                    }
                    chars.next();
                    value.push(next);
                }
                tokens.push(value);
            }
        }
    }

    tokens
}

/// Parse Valve KeyValues text into a single root object.
pub fn parse_vdf(text: &str) -> Result<BTreeMap<String, VdfValue>, String> {
    let tokens = tokenize(text);
    let mut index = 0usize;
    let object = parse_object(&tokens, &mut index, 0)?;
    Ok(object)
}

fn parse_object(
    tokens: &[String],
    index: &mut usize,
    depth: usize,
) -> Result<BTreeMap<String, VdfValue>, String> {
    if depth > 64 {
        return Err("VDF nesting is too deep.".to_string());
    }
    let mut object = BTreeMap::new();
    loop {
        if *index >= tokens.len() {
            if depth == 0 {
                return Ok(object);
            }
            return Err("Unclosed VDF object.".to_string());
        }
        let key = tokens[*index].clone();
        if key == "}" {
            *index += 1;
            return Ok(object);
        }
        if key == "{" {
            return Err("Unexpected '{' in VDF.".to_string());
        }
        *index += 1;
        let Some(next) = tokens.get(*index) else {
            return Err(format!("Missing value for VDF key {key}."));
        };
        if next == "{" {
            *index += 1;
            let child = parse_object(tokens, index, depth + 1)?;
            object.insert(key, VdfValue::Object(child));
        } else if next == "}" {
            return Err(format!("Missing value for VDF key {key}."));
        } else {
            object.insert(key, VdfValue::String(next.clone()));
            *index += 1;
        }
    }
}

/// `libraryfolders.vdf` → absolute library paths.
pub fn parse_library_folders(text: &str) -> Result<Vec<String>, String> {
    let root = parse_vdf(text)?;
    let container = get(&root, "libraryfolders")
        .and_then(VdfValue::as_object)
        .unwrap_or(&root);

    let mut paths = Vec::new();
    for (key, value) in container {
        match value {
            VdfValue::Object(child) => {
                if let Some(path) = get_str(child, "path")
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                {
                    paths.push(path.to_string());
                }
            }
            // Pre-2021 format: `"1" "D:\\SteamLibrary"`.
            VdfValue::String(path)
                if key.chars().all(|c| c.is_ascii_digit()) && !path.trim().is_empty() =>
            {
                paths.push(path.trim().to_string());
            }
            _ => {}
        }
    }
    Ok(paths)
}

/// `appmanifest_<appid>.acf` → one game, when the manifest is complete.
pub fn parse_app_manifest(text: &str) -> Result<Option<SteamGame>, String> {
    let root = parse_vdf(text)?;
    let state = get(&root, "AppState")
        .and_then(VdfValue::as_object)
        .unwrap_or(&root);

    let Some(app_id) = get_str(state, "appid").and_then(|value| value.parse::<u32>().ok()) else {
        return Ok(None);
    };
    let Some(name) = get_str(state, "name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
    else {
        return Ok(None);
    };

    // StateFlags bit 2 (value 4) means "fully installed". Older manifests may
    // omit the flag; treat those as installed.
    let flags = get_str(state, "StateFlags").and_then(|value| value.parse::<u64>().ok());
    if let Some(flags) = flags {
        if flags & 4 == 0 {
            return Ok(None);
        }
    }

    let size_bytes = get_str(state, "SizeOnDisk")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let last_played = get_str(state, "LastPlayed").and_then(|value| value.parse::<i64>().ok());
    let install_dir = get_str(state, "installdir")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(name)
        .to_string();

    Ok(Some(SteamGame {
        app_id,
        name: name.to_string(),
        install_dir,
        library_path: String::new(),
        size_bytes,
        last_played,
    }))
}

/// Steam libraries to inspect, in order, when the user did not set a root.
pub fn discover_roots(override_root: Option<&str>) -> Vec<PathBuf> {
    if let Some(root) = override_root
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return vec![PathBuf::from(root)];
    }
    if let Some(root) = std::env::var("LUDEX_STEAM_ROOT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return vec![PathBuf::from(root)];
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    #[cfg(windows)]
    {
        if let Some(path) = windows_steam_path() {
            candidates.push(path);
        }
        if let Ok(program_files) = std::env::var("ProgramFiles(x86)") {
            candidates.push(PathBuf::from(program_files).join("Steam"));
        }
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            candidates.push(PathBuf::from(program_files).join("Steam"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home_dir() {
            candidates.push(home.join("Library/Application Support/Steam"));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(home) = home_dir() {
            candidates.push(home.join(".steam/steam"));
            candidates.push(home.join(".steam/root"));
            candidates.push(home.join(".local/share/Steam"));
            candidates.push(home.join(".var/app/com.valvesoftware.Steam/data/Steam"));
            candidates.push(home.join(".steam/debian-installation"));
        }
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if !candidate.is_dir() {
            continue;
        }
        let canonical = candidate.canonicalize().unwrap_or(candidate);
        if !roots.iter().any(|existing| existing == &canonical) {
            roots.push(canonical);
        }
    }
    roots
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

#[cfg(windows)]
fn windows_steam_path() -> Option<PathBuf> {
    // Reading the registry needs a subprocess; failure is not fatal, the
    // Program Files candidates cover the default install.
    let output = crate::process::capture("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let path = text
        .lines()
        .find(|line| line.contains("SteamPath"))
        .and_then(|line| line.split("REG_SZ").nth(1))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(PathBuf::from(path.replace('/', "\\")))
}

/// Read the Steam libraries reachable from `roots` and every installed game.
pub fn scan(roots: &[PathBuf]) -> SteamScan {
    let mut scan = SteamScan::default();
    let mut libraries: Vec<PathBuf> = Vec::new();

    let add_library = |path: PathBuf, libraries: &mut Vec<PathBuf>| {
        let canonical = path.canonicalize().unwrap_or(path);
        if !libraries.iter().any(|existing| existing == &canonical) {
            libraries.push(canonical);
        }
    };

    for root in roots {
        add_library(root.clone(), &mut libraries);
        for relative in [
            "steamapps/libraryfolders.vdf",
            "libraryfolders.vdf",
            "config/libraryfolders.vdf",
        ] {
            let path = root.join(relative);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match parse_library_folders(&text) {
                Ok(paths) => {
                    for path in paths {
                        let candidate = PathBuf::from(&path);
                        if candidate.is_dir() {
                            add_library(candidate, &mut libraries);
                        }
                    }
                }
                Err(error) => scan
                    .warnings
                    .push(format!("Could not read {}: {error}", path.display())),
            }
            break;
        }
    }

    let mut games: Vec<SteamGame> = Vec::new();
    for library in &libraries {
        scan.roots.push(library.to_string_lossy().to_string());
        let steamapps = library.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&steamapps) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match parse_app_manifest(&text) {
                Ok(Some(mut game)) => {
                    let install = steamapps.join("common").join(&game.install_dir);
                    if !install.is_dir() {
                        scan.warnings.push(format!(
                            "{} is listed by Steam but its folder is missing: {}",
                            game.name,
                            install.display()
                        ));
                        continue;
                    }
                    game.library_path = library.to_string_lossy().to_string();
                    game.install_dir = install.to_string_lossy().to_string();
                    if games.iter().any(|existing| existing.app_id == game.app_id) {
                        continue;
                    }
                    games.push(game);
                }
                Ok(None) => {}
                Err(error) => scan
                    .warnings
                    .push(format!("Could not read {}: {error}", path.display())),
            }
        }
    }

    games.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    scan.games = games;
    scan
}

/// Edition markers stripped by the fallback title match.
const EDITION_SUFFIXES: [&str; 14] = [
    "game of the year edition",
    "goty edition",
    "goty",
    "definitive edition",
    "deluxe edition",
    "ultimate edition",
    "complete edition",
    "enhanced edition",
    "special edition",
    "collectors edition",
    "anniversary edition",
    "remastered",
    "directors cut",
    "digital deluxe",
];

/// Lowercase alphanumeric words, used for matching only.
pub fn normalize_title(title: &str) -> String {
    let mut cleaned = String::with_capacity(title.len());
    let mut last_space = true;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            cleaned.push(character.to_ascii_lowercase());
            last_space = false;
        } else if character == '&' {
            cleaned.push_str("and");
            last_space = false;
        } else if !last_space {
            cleaned.push(' ');
            last_space = true;
        }
    }
    cleaned.trim().to_string()
}

/// Normalised title without a trailing edition marker.
pub fn strip_edition(title: &str) -> String {
    let mut normalized = normalize_title(title);
    loop {
        let mut trimmed = false;
        for suffix in EDITION_SUFFIXES {
            if normalized.len() > suffix.len() && normalized.ends_with(suffix) {
                let cut = normalized.len() - suffix.len();
                normalized = normalized[..cut].trim_end().to_string();
                trimmed = true;
                break;
            }
        }
        if !trimmed {
            break;
        }
    }
    normalized
}

/// Index of server titles used for matching. Two server games sharing a title
/// make that key ambiguous, so it is never matched.
#[derive(Debug, Default)]
pub struct TitleIndex {
    exact: BTreeMap<String, Option<i64>>,
    stripped: BTreeMap<String, Option<i64>>,
}

impl TitleIndex {
    pub fn new(server: &[(i64, String)]) -> Self {
        let mut index = Self::default();
        for (game_id, title) in server {
            for (map, key) in [
                (&mut index.exact, normalize_title(title)),
                (&mut index.stripped, strip_edition(title)),
            ] {
                if key.is_empty() {
                    continue;
                }
                match map.get(&key) {
                    Some(Some(existing)) if existing != game_id => {
                        // Two server games share this title: matching is unsafe.
                        map.insert(key, None);
                    }
                    Some(_) => {}
                    None => {
                        map.insert(key, Some(*game_id));
                    }
                }
            }
        }
        index
    }

    /// Exact normalised title first, then once more with edition suffixes
    /// stripped. Ambiguous titles return `None`.
    pub fn lookup(&self, title: &str) -> Option<i64> {
        if let Some(Some(game_id)) = self.exact.get(&normalize_title(title)) {
            return Some(*game_id);
        }
        match self.stripped.get(&strip_edition(title)) {
            Some(Some(game_id)) => Some(*game_id),
            _ => None,
        }
    }
}

/// Match Steam games to server games by normalised title. Returns
/// `app_id -> game_id`; ambiguous titles are skipped rather than guessed.
pub fn match_titles(steam: &[SteamGame], server: &[(i64, String)]) -> BTreeMap<u32, i64> {
    let index = TitleIndex::new(server);
    let mut matches = BTreeMap::new();
    for game in steam {
        if let Some(game_id) = index.lookup(&game.name) {
            matches.insert(game.app_id, game_id);
        }
    }
    matches
}

/// Steam launch URI used by `launch_game` for imported entries.
pub fn launch_uri(app_id: u32) -> String {
    format!("steam://rungameid/{app_id}")
}

/// Stable local id for a Steam game that has no server game: negative app ids
/// can never collide with a server id (which is always positive).
pub fn local_game_id(app_id: u32) -> i64 {
    -(i64::from(app_id))
}

/// Parse the app id back out of a `steam://rungameid/…` URI.
pub fn app_id_from_uri(uri: &str) -> Option<u32> {
    let rest = uri.strip_prefix("steam://rungameid/")?;
    rest.split(['/', '?']).next()?.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY_FOLDERS: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/player/.local/share/Steam"
		"label"		""
		"contentid"		"123456789"
		"totalsize"		"0"
		"apps"
		{
			"228980"		"0"
			"620"		"4096"
		}
	}
	"1"
	{
		"path"		"/mnt/games/SteamLibrary"
	}
}
"#;

    const LEGACY_LIBRARY_FOLDERS: &str = r#"
"LibraryFolders"
{
	"TimeNextStatsReport"		"1700000000"
	"ContentStatsID"		"-1234"
	"1"		"D:\\SteamLibrary"
	"2"		"E:\\Games\\Steam"
}
"#;

    const APP_MANIFEST: &str = r#"
"AppState"
{
	"appid"		"1145360"
	"Universe"		"1"
	"name"		"Hades"
	"StateFlags"		"4"
	"installdir"		"Hades"
	"LastPlayed"		"1700000000"
	"SizeOnDisk"		"15461882265"
}
"#;

    #[test]
    fn library_folders_are_parsed_from_both_formats() {
        assert_eq!(
            parse_library_folders(LIBRARY_FOLDERS).unwrap(),
            vec![
                "/home/player/.local/share/Steam".to_string(),
                "/mnt/games/SteamLibrary".to_string()
            ]
        );
        assert_eq!(
            parse_library_folders(LEGACY_LIBRARY_FOLDERS).unwrap(),
            vec![
                "D:\\SteamLibrary".to_string(),
                "E:\\Games\\Steam".to_string()
            ]
        );
    }

    #[test]
    fn app_manifest_yields_an_installed_game() {
        let game = parse_app_manifest(APP_MANIFEST).unwrap().unwrap();
        assert_eq!(game.app_id, 1145360);
        assert_eq!(game.name, "Hades");
        assert_eq!(game.install_dir, "Hades");
        assert_eq!(game.size_bytes, 15_461_882_265);
        assert_eq!(game.last_played, Some(1_700_000_000));
    }

    #[test]
    fn incomplete_and_broken_manifests_are_ignored() {
        let not_installed =
            APP_MANIFEST.replace("\"StateFlags\"\t\t\"4\"", "\"StateFlags\"\t\t\"2\"");
        assert!(parse_app_manifest(&not_installed).unwrap().is_none());
        assert!(parse_app_manifest("\"AppState\" { \"name\" \"NoId\" }")
            .unwrap()
            .is_none());
        assert!(parse_app_manifest("\"AppState\" { \"appid\" \"7\"").is_err());
    }

    #[test]
    fn scan_reads_manifests_from_every_library() {
        let root = temp_dir("steam-scan");
        let second = root.join("second-library");
        std::fs::create_dir_all(root.join("steamapps/common/Hades")).unwrap();
        std::fs::create_dir_all(second.join("steamapps/common/Team Fortress 2")).unwrap();
        std::fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!(
                r#""libraryfolders" {{ "0" {{ "path" "{}" }} "1" {{ "path" "{}" }} }}"#,
                root.display(),
                second.display()
            ),
        )
        .unwrap();
        std::fs::write(root.join("steamapps/appmanifest_1145360.acf"), APP_MANIFEST).unwrap();
        std::fs::write(
            second.join("steamapps/appmanifest_440.acf"),
            r#""AppState" { "appid" "440" "name" "Team Fortress 2" "StateFlags" "4" "installdir" "Team Fortress 2" }"#,
        )
        .unwrap();

        let scan = scan(std::slice::from_ref(&root));
        assert_eq!(scan.games.len(), 2);
        assert_eq!(scan.games[0].name, "Hades");
        assert_eq!(scan.games[1].name, "Team Fortress 2");
        assert_eq!(scan.games[1].app_id, 440);
        assert!(scan.games[0].install_dir.ends_with("common/Hades"));
        assert_eq!(scan.roots.len(), 2);
        assert!(scan.warnings.is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_install_folder_is_reported_not_listed() {
        let root = temp_dir("steam-missing");
        std::fs::create_dir_all(root.join("steamapps")).unwrap();
        std::fs::write(root.join("steamapps/appmanifest_1145360.acf"), APP_MANIFEST).unwrap();

        let scan = scan(std::slice::from_ref(&root));
        assert!(scan.games.is_empty());
        assert_eq!(scan.warnings.len(), 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn titles_are_normalised_for_matching() {
        assert_eq!(normalize_title("  Hades  "), "hades");
        assert_eq!(
            normalize_title("Ori & the Blind Forest"),
            "ori and the blind forest"
        );
        assert_eq!(normalize_title("Slay the Spire™"), "slay the spire");
        assert_eq!(strip_edition("Hades Game of the Year Edition"), "hades");
        assert_eq!(strip_edition("Celeste (GOTY)"), "celeste");
    }

    fn game(app_id: u32, name: &str) -> SteamGame {
        SteamGame {
            app_id,
            name: name.into(),
            install_dir: name.into(),
            library_path: "/steam".into(),
            size_bytes: 0,
            last_played: None,
        }
    }

    #[test]
    fn matching_prefers_exact_titles_then_strips_editions() {
        let steam = vec![
            game(1, "Hades"),
            game(2, "Hades II"),
            game(3, "Celeste Game of the Year Edition"),
        ];
        let server = vec![
            (10, "Hades".to_string()),
            (11, "Hades II".to_string()),
            (12, "Celeste".to_string()),
        ];
        let matches = match_titles(&steam, &server);
        assert_eq!(matches.get(&1), Some(&10));
        assert_eq!(matches.get(&2), Some(&11));
        assert_eq!(matches.get(&3), Some(&12));
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn ambiguous_server_titles_are_not_matched() {
        let steam = vec![game(7, "Doom")];
        let server = vec![(1, "Doom".to_string()), (2, "DOOM".to_string())];
        assert!(match_titles(&steam, &server).is_empty());
    }

    #[test]
    fn unmatched_games_get_stable_negative_ids() {
        assert_eq!(local_game_id(1145360), -1_145_360);
        assert_eq!(launch_uri(1145360), "steam://rungameid/1145360");
        assert_eq!(app_id_from_uri("steam://rungameid/1145360"), Some(1145360));
        assert_eq!(app_id_from_uri("steam://run/440"), None);
    }

    #[test]
    fn dedupe_keeps_one_entry_per_app_id() {
        let steam = vec![game(440, "Team Fortress 2"), game(440, "Team Fortress 2")];
        let server = vec![(1, "Team Fortress 2".to_string())];
        let matches = match_titles(&steam, &server);
        assert_eq!(matches.len(), 1);
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
