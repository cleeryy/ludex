//! ROM library support (#52): platform detection, `gamelist.xml` parsing and
//! safe emulator command construction.
//!
//! Emulator binaries are never bundled. The user registers a command template
//! containing `%ROM%` (plus optional `%ROM_DIR%`, `%ROM_NAME%`, `%ROM_STEM%`);
//! [`build_launch_command`] turns it into a program and an argument vector.
//! Ludex never passes the template to a shell, so a ROM path containing quotes
//! or `&&` stays a single argument.
//!
//! ```
//! use ludex_core::rom::{detect_platform, build_launch_command};
//! use std::path::Path;
//!
//! let platform = detect_platform("Chrono Trigger (USA).sfc").unwrap();
//! assert_eq!(platform.id, "snes");
//! let command = build_launch_command("retroarch -L snes9x_libretro.so %ROM%", Path::new("/roms/Chrono Trigger.sfc")).unwrap();
//! assert_eq!(command.args.len(), 3);
//! ```

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One supported platform, with the extensions recognised by the scanner and
/// the RetroArch core suggested for its default emulator preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// Stable slug stored in the database (`snes`, `gba`, …).
    pub id: &'static str,
    /// Human-readable label (`Super Nintendo`).
    pub label: &'static str,
    /// Lowercase file extensions, without the dot.
    pub extensions: &'static [&'static str],
    /// RetroArch core directory name, when one exists.
    pub retroarch_core: Option<&'static str>,
}

/// Every platform the scanner recognises.
pub const PLATFORMS: &[Platform] = &[
    Platform {
        id: "nes",
        label: "Nintendo Entertainment System",
        extensions: &["nes", "unf", "unif", "fds"],
        retroarch_core: Some("mesen"),
    },
    Platform {
        id: "snes",
        label: "Super Nintendo",
        extensions: &["sfc", "smc", "swc", "fig"],
        retroarch_core: Some("snes9x"),
    },
    Platform {
        id: "n64",
        label: "Nintendo 64",
        extensions: &["n64", "z64", "v64"],
        retroarch_core: Some("mupen64plus_next"),
    },
    Platform {
        id: "gb",
        label: "Game Boy",
        extensions: &["gb"],
        retroarch_core: Some("gambatte"),
    },
    Platform {
        id: "gbc",
        label: "Game Boy Color",
        extensions: &["gbc"],
        retroarch_core: Some("gambatte"),
    },
    Platform {
        id: "gba",
        label: "Game Boy Advance",
        extensions: &["gba"],
        retroarch_core: Some("mgba"),
    },
    Platform {
        id: "nds",
        label: "Nintendo DS",
        extensions: &["nds"],
        retroarch_core: Some("melonds"),
    },
    Platform {
        id: "3ds",
        label: "Nintendo 3DS",
        extensions: &["3ds", "cci", "cia"],
        retroarch_core: None,
    },
    Platform {
        id: "gamecube",
        label: "GameCube",
        extensions: &["gcm", "rvz", "wia"],
        retroarch_core: Some("dolphin"),
    },
    Platform {
        id: "wii",
        label: "Wii",
        extensions: &["wbfs", "wia"],
        retroarch_core: Some("dolphin"),
    },
    Platform {
        id: "switch",
        label: "Nintendo Switch",
        extensions: &["nsp", "xci"],
        retroarch_core: None,
    },
    Platform {
        id: "genesis",
        label: "Sega Genesis / Mega Drive",
        extensions: &["md", "gen", "smd"],
        retroarch_core: Some("genesis_plus_gx"),
    },
    Platform {
        id: "mastersystem",
        label: "Master System",
        extensions: &["sms"],
        retroarch_core: Some("genesis_plus_gx"),
    },
    Platform {
        id: "gamegear",
        label: "Game Gear",
        extensions: &["gg"],
        retroarch_core: Some("genesis_plus_gx"),
    },
    Platform {
        id: "saturn",
        label: "Sega Saturn",
        extensions: &[],
        retroarch_core: Some("mednafen_saturn"),
    },
    Platform {
        id: "dreamcast",
        label: "Dreamcast",
        extensions: &["gdi", "cdi"],
        retroarch_core: Some("flycast"),
    },
    Platform {
        id: "psx",
        label: "PlayStation",
        extensions: &["cue", "pbp", "chd", "iso", "bin"],
        retroarch_core: Some("swanstation"),
    },
    Platform {
        id: "ps2",
        label: "PlayStation 2",
        extensions: &["iso", "chd"],
        retroarch_core: Some("pcsx2"),
    },
    Platform {
        id: "psp",
        label: "PlayStation Portable",
        extensions: &["cso", "pbp"],
        retroarch_core: Some("ppsspp"),
    },
    Platform {
        id: "arcade",
        label: "Arcade",
        extensions: &["zip"],
        retroarch_core: Some("fbneo"),
    },
    Platform {
        id: "neogeo",
        label: "Neo Geo",
        extensions: &["neo"],
        retroarch_core: Some("fbneo"),
    },
    Platform {
        id: "atari2600",
        label: "Atari 2600",
        extensions: &["a26"],
        retroarch_core: Some("stella"),
    },
    Platform {
        id: "pcengine",
        label: "PC Engine / TurboGrafx-16",
        extensions: &["pce"],
        retroarch_core: Some("mednafen_pce"),
    },
    Platform {
        id: "wonderswan",
        label: "WonderSwan",
        extensions: &["ws", "wsc"],
        retroarch_core: Some("mednafen_wswan"),
    },
];

/// Looks up a platform by its stable id.
#[must_use]
pub fn platform(id: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|platform| platform.id == id)
}

/// Detects the platform of a ROM from its file name.
///
/// `.iso` is deliberately attributed to PlayStation (the most common use);
/// a `.cue`/`.chd` file next to an `.iso` wins when the scanner groups files.
#[must_use]
pub fn detect_platform(file_name: &str) -> Option<&'static Platform> {
    let lower = file_name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, extension)| extension)?;
    PLATFORMS
        .iter()
        .find(|platform| platform.extensions.contains(&extension))
}

/// Whether the path looks like a ROM the scanner should index.
#[must_use]
pub fn is_rom_path(file_name: &str) -> bool {
    detect_platform(file_name).is_some()
}

/// Priority used to pick one file when a ROM is split across several files
/// (`.cue` + `.bin`, `.gdi` + tracks). Lower is preferred.
#[must_use]
pub fn rom_rank(file_name: &str) -> u8 {
    let lower = file_name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, extension)| extension);
    match extension {
        Some("cue" | "gdi" | "chd" | "rvz" | "wbfs" | "nsp" | "xci") => 0,
        Some("iso" | "pbp" | "cso" | "3ds" | "cia" | "nds") => 1,
        Some("sfc" | "smc" | "nes" | "gba" | "gb" | "gbc" | "n64" | "z64" | "md" | "gen") => 1,
        Some("bin") => 3,
        _ => 2,
    }
}

/// Removes region, revision and dump tags from a ROM title:
/// `Chrono Trigger (USA) (Rev 1) [!]` → `Chrono Trigger`.
#[must_use]
pub fn clean_rom_title(file_name: &str) -> String {
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    let mut title = String::with_capacity(stem.len());
    let mut depth = 0_u32;
    for character in stem.chars() {
        match character {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            other if depth == 0 => title.push(other),
            _ => {}
        }
    }
    let normalised = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalised.is_empty() {
        stem.to_string()
    } else {
        normalised
    }
}

/// Stable grouping key for a ROM: region/revision tags and the extension are
/// ignored so `Game (USA).sfc` and `Game (Japan).sfc` collapse into one game.
#[must_use]
pub fn rom_group_key(file_name: &str) -> String {
    let platform = detect_platform(file_name).map_or("unknown", |platform| platform.id);
    format!(
        "rom::{}::{}",
        platform,
        crate::naming::normalize_title(&clean_rom_title(file_name))
    )
}

/// One `<game>` entry of a `gamelist.xml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GamelistEntry {
    /// Relative or absolute ROM path, as written by EmulationStation.
    pub path: String,
    /// Display name.
    pub name: Option<String>,
    /// Description (`<desc>`).
    pub description: Option<String>,
    /// Box art path (`<image>`).
    pub image: Option<String>,
    /// Release date, verbatim (`YYYYMMDDT000000` in most gamelists).
    pub release_date: Option<String>,
    /// Developer.
    pub developer: Option<String>,
    /// Publisher.
    pub publisher: Option<String>,
    /// Genre label.
    pub genre: Option<String>,
    /// Player count, verbatim.
    pub players: Option<String>,
}

/// Parses an EmulationStation `gamelist.xml` document.
///
/// The parser is intentionally small: it extracts `<game>` blocks and the
/// handful of tags Ludex can store. It tolerates CDATA, entities, comments and
/// unknown tags, which is what real gamelists contain. It never evaluates
/// anything and malformed tags are skipped rather than failing the whole file.
#[must_use]
pub fn parse_gamelist(xml: &str) -> Vec<GamelistEntry> {
    let mut entries = Vec::new();
    for block in tags(xml, "game") {
        let path = tag_text(block, "path").unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        entries.push(GamelistEntry {
            path,
            name: tag_text(block, "name"),
            description: tag_text(block, "desc"),
            image: tag_text(block, "image"),
            release_date: tag_text(block, "releasedate"),
            developer: tag_text(block, "developer"),
            publisher: tag_text(block, "publisher"),
            genre: tag_text(block, "genre"),
            players: tag_text(block, "players"),
        });
    }
    entries
}

/// Returns the inner text of every `<tag>` element (case-insensitive).
fn tags<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find(&open) {
        let start = cursor + found;
        let after = start + open.len();
        // `<game>` or `<game attr="…">`, never `<games>`.
        let is_element = lower
            .as_bytes()
            .get(after)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>' || *byte == b'/');
        let Some(content_start) = lower[after..].find('>').map(|offset| after + offset + 1) else {
            break;
        };
        if !is_element {
            cursor = content_start;
            continue;
        }
        if lower.as_bytes().get(content_start - 2) == Some(&b'/') {
            cursor = content_start;
            continue;
        }
        let Some(end) = lower[content_start..].find(&close) else {
            break;
        };
        result.push(&xml[content_start..content_start + end]);
        cursor = content_start + end + close.len();
    }
    result
}

/// Extracts the text of the first `<tag>` child, decoding entities and CDATA.
fn tag_text(block: &str, tag: &str) -> Option<String> {
    let lower = block.to_ascii_lowercase();
    let open = format!("<{tag}");
    let start = lower.find(&open)?;
    let after = start + open.len();
    let is_element = lower
        .as_bytes()
        .get(after)
        .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>');
    if !is_element {
        return None;
    }
    let content_start = after + lower[after..].find('>')? + 1;
    let close = format!("</{tag}>");
    let end = lower[content_start..].find(&close)? + content_start;
    let raw = &block[content_start..end];
    let decoded = decode_entities(raw.trim());
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

fn decode_entities(value: &str) -> String {
    let cdata = value
        .strip_prefix("<![CDATA[")
        .and_then(|rest| rest.strip_suffix("]]>"))
        .map_or(value, str::trim);
    let mut output = String::with_capacity(cdata.len());
    let mut rest = cdata;
    while let Some(index) = rest.find('&') {
        output.push_str(&rest[..index]);
        let tail = &rest[index..];
        let (replacement, consumed) = match tail {
            _ if tail.starts_with("&amp;") => ("&", 5),
            _ if tail.starts_with("&lt;") => ("<", 4),
            _ if tail.starts_with("&gt;") => (">", 4),
            _ if tail.starts_with("&quot;") => ("\"", 6),
            _ if tail.starts_with("&apos;") => ("'", 6),
            _ => ("&", 1),
        };
        output.push_str(replacement);
        rest = &tail[consumed..];
    }
    output.push_str(rest);
    output
}

/// A command ready to be spawned without a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchCommand {
    /// Emulator executable (first template token).
    pub program: String,
    /// Arguments, with every placeholder substituted.
    pub args: Vec<String>,
}

/// Errors returned by [`build_launch_command`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RomError {
    /// The template is empty.
    #[error("the emulator command is empty")]
    Empty,
    /// The template does not contain `%ROM%`.
    #[error("the emulator command must contain %ROM%")]
    MissingRomPlaceholder,
    /// A placeholder was unterminated in the template.
    #[error("unterminated quote in the emulator command")]
    UnterminatedQuote,
    /// A token contains a character that only makes sense in a shell.
    #[error("the emulator command must not contain shell syntax (`{0}`)")]
    ShellSyntax(String),
    /// The ROM path contains a control character or NUL.
    #[error("the ROM path is not a valid argument")]
    InvalidRomPath,
}

/// Characters that only make sense when a command line is interpreted by a
/// shell. Ludex spawns directly, but rejecting them surfaces copy-pasted shell
/// one-liners instead of silently doing something different.
const SHELL_SYNTAX: &[&str] = &[";", "|", "&&", "$(", "`", ">", "<"];

/// Splits a command template into tokens, honouring single and double quotes
/// and backslash escapes. No shell expansion is performed.
fn tokenize(template: &str) -> Result<Vec<String>, RomError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut quote: Option<char> = None;
    let mut characters = template.chars();
    while let Some(character) = characters.next() {
        match quote {
            Some(active) if character == active => {
                quote = None;
                has_token = true;
            }
            Some(_) => current.push(character),
            None => match character {
                '\'' | '"' => {
                    quote = Some(character);
                    has_token = true;
                }
                '\\' => {
                    // Escapes are only meaningful before a quote, a backslash
                    // or whitespace; Windows paths keep their separators.
                    let mut lookahead = characters.clone();
                    match lookahead.next() {
                        Some(next)
                            if next == '"'
                                || next == '\''
                                || next == '\\'
                                || next.is_whitespace() =>
                        {
                            current.push(next);
                            characters = lookahead;
                        }
                        _ => current.push('\\'),
                    }
                    has_token = true;
                }
                character if character.is_whitespace() => {
                    if has_token {
                        tokens.push(std::mem::take(&mut current));
                        has_token = false;
                    }
                }
                other => {
                    current.push(other);
                    has_token = true;
                }
            },
        }
    }
    if quote.is_some() {
        return Err(RomError::UnterminatedQuote);
    }
    if has_token {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Builds a [`LaunchCommand`] from a user-configured template and a ROM path.
///
/// The template must contain `%ROM%`. `%ROM_DIR%`, `%ROM_NAME%` and
/// `%ROM_STEM%` are optional. Shell metacharacters are rejected instead of
/// interpreted, and the ROM path is substituted as a whole argument.
pub fn build_launch_command(template: &str, rom: &Path) -> Result<LaunchCommand, RomError> {
    if template.trim().is_empty() {
        return Err(RomError::Empty);
    }
    let rom_text = rom.to_string_lossy();
    if rom_text.chars().any(|character| character.is_control()) {
        return Err(RomError::InvalidRomPath);
    }
    let tokens = tokenize(template)?;
    if tokens.is_empty() {
        return Err(RomError::Empty);
    }
    let mut contains_rom = false;
    let mut args = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        for syntax in SHELL_SYNTAX {
            if token.contains(syntax) {
                return Err(RomError::ShellSyntax((*syntax).to_string()));
            }
        }
        let substituted = substitute(token, rom, &mut contains_rom);
        if index == 0 {
            if substituted.is_empty() {
                return Err(RomError::Empty);
            }
            args.push(substituted);
        } else {
            args.push(substituted);
        }
    }
    if !contains_rom {
        return Err(RomError::MissingRomPlaceholder);
    }
    let program = args.remove(0);
    Ok(LaunchCommand { program, args })
}

fn substitute(token: &str, rom: &Path, contains_rom: &mut bool) -> String {
    let mut output = token.to_string();
    if output.contains("%ROM%") {
        *contains_rom = true;
        output = output.replace("%ROM%", &rom.to_string_lossy());
    }
    if output.contains("%ROM_DIR%") {
        let parent = rom.parent().unwrap_or_else(|| Path::new(""));
        output = output.replace("%ROM_DIR%", &parent.to_string_lossy());
    }
    if output.contains("%ROM_NAME%") {
        let name = rom
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().to_string());
        output = output.replace("%ROM_NAME%", &name);
    }
    if output.contains("%ROM_STEM%") {
        let stem = rom
            .file_stem()
            .map_or_else(String::new, |name| name.to_string_lossy().to_string());
        output = output.replace("%ROM_STEM%", &stem);
    }
    output
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_rom_extensions() {
        assert_eq!(
            detect_platform("Chrono Trigger (USA).sfc").unwrap().id,
            "snes"
        );
        assert_eq!(detect_platform("Super Mario 64.z64").unwrap().id, "n64");
        assert_eq!(detect_platform("Pokemon Emerald.GBA").unwrap().id, "gba");
        assert_eq!(detect_platform("Metal Gear Solid.cue").unwrap().id, "psx");
        assert_eq!(detect_platform("Ico.chd").unwrap().id, "psx");
        assert_eq!(detect_platform("Sonic.md").unwrap().id, "genesis");
        assert_eq!(detect_platform("Metal Slug.zip").unwrap().id, "arcade");
        // `.zip` is only a ROM when the containing root is a ROM root; the
        // scanner decides that, not the extension alone.
        assert!(detect_platform("readme.txt").is_none());
        assert!(is_rom_path("Zelda.nsp"));
    }

    #[test]
    fn cleans_titles_and_groups_regions_together() {
        assert_eq!(
            clean_rom_title("Chrono Trigger (USA) (Rev 1) [!].sfc"),
            "Chrono Trigger"
        );
        assert_eq!(clean_rom_title("Game (Japan).gba"), "Game");
        assert_eq!(clean_rom_title("No Tags.smc"), "No Tags");
        assert_eq!(
            rom_group_key("Chrono Trigger (USA).sfc"),
            rom_group_key("Chrono Trigger (Japan).sfc")
        );
        assert_ne!(
            rom_group_key("Chrono Trigger (USA).sfc"),
            rom_group_key("Chrono Trigger (USA).gba")
        );
        assert!(rom_rank("game.cue") < rom_rank("game.bin"));
    }

    #[test]
    fn parses_gamelist_xml() {
        let xml = r#"<?xml version="1.0"?>
        <gameList>
          <game id="1">
            <path>./Chrono Trigger (USA).sfc</path>
            <name>Chrono Trigger</name>
            <desc><![CDATA[A time travel RPG &amp; classic.]]></desc>
            <image>./images/chrono.png</image>
            <releasedate>19950311T000000</releasedate>
            <developer>Square</developer>
            <publisher>Square Soft</publisher>
            <genre>Role-Playing</genre>
            <players>1</players>
          </game>
          <game>
            <path>./Other.gba</path>
            <name>Other &amp; More</name>
          </game>
          <folder><path>./ignored</path></folder>
        </gameList>"#;
        let entries = parse_gamelist(xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "./Chrono Trigger (USA).sfc");
        assert_eq!(entries[0].name.as_deref(), Some("Chrono Trigger"));
        assert_eq!(
            entries[0].description.as_deref(),
            Some("A time travel RPG & classic.")
        );
        assert_eq!(entries[0].release_date.as_deref(), Some("19950311T000000"));
        assert_eq!(entries[1].name.as_deref(), Some("Other & More"));
        assert!(parse_gamelist("<gameList></gameList>").is_empty());
    }

    #[test]
    fn builds_commands_without_a_shell() {
        let command = build_launch_command(
            "retroarch -L \"snes9x libretro.so\" %ROM%",
            Path::new("/roms/Chrono Trigger.sfc"),
        )
        .expect("valid template");
        assert_eq!(command.program, "retroarch");
        assert_eq!(
            command.args,
            vec!["-L", "snes9x libretro.so", "/roms/Chrono Trigger.sfc"]
        );

        let command = build_launch_command(
            "\"C:\\Emulators\\RetroArch\\retroarch.exe\" --rom-dir=%ROM_DIR% %ROM%",
            Path::new("/games/roms/Zelda.n64"),
        )
        .expect("windows template");
        assert_eq!(command.program, "C:\\Emulators\\RetroArch\\retroarch.exe");
        assert_eq!(command.args[0], "--rom-dir=/games/roms");
        assert_eq!(command.args[1], "/games/roms/Zelda.n64");

        assert_eq!(
            build_launch_command("", Path::new("/x.nes")),
            Err(RomError::Empty)
        );
        assert!(build_launch_command("retroarch %ROM%", Path::new("/x.nes")).is_ok());
        assert_eq!(
            build_launch_command("retroarch -L core.so", Path::new("/x.nes")),
            Err(RomError::MissingRomPlaceholder)
        );
        assert_eq!(
            build_launch_command("retroarch %ROM%; rm -rf /", Path::new("/x.nes")),
            Err(RomError::ShellSyntax(";".to_string()))
        );
        assert_eq!(
            build_launch_command("\"retroarch %ROM%", Path::new("/x.nes")),
            Err(RomError::UnterminatedQuote)
        );
        // A quote in the ROM path is a literal argument, not a syntax error.
        let command =
            build_launch_command("retroarch %ROM%", Path::new("/roms/it's fine.nes")).unwrap();
        assert_eq!(command.args, vec!["/roms/it's fine.nes"]);
    }
}
