//! Executable discovery for extracted game folders.
//!
//! After extracting an archive, Ludex needs to guess which file starts the
//! game. The heuristic scores file names against the game title, prefers
//! top-level files and larger binaries, and hard-excludes installers,
//! redistributables and crash handlers.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::naming::normalize_title;

/// How deep [`pick_executable`] walks below the game root.
pub const MAX_DEPTH: usize = 4;

/// Smallest accepted depth for a folder scan (#134).
pub const MIN_SCAN_DEPTH: usize = 4;

/// Default depth for a GameVault-style folder scan (#134).
pub const DEFAULT_SCAN_DEPTH: usize = 6;

/// Largest accepted depth for a folder scan (#134). Deeper trees are rare and
/// scanning them gets slow on network drives.
pub const MAX_SCAN_DEPTH: usize = 8;

/// Clamp a user-configured scan depth to the documented range.
pub fn clamp_scan_depth(depth: usize) -> usize {
    depth.clamp(MIN_SCAN_DEPTH, MAX_SCAN_DEPTH)
}

/// Directory names that never contain the game executable.
const EXCLUDED_DIRS: &[&str] = &[
    "redist",
    "_commonredist",
    "support",
    "tools",
    "directx",
    "dotnet",
    "installer",
    "uninstaller",
    "vcredist",
    "steamworks",
];

/// File stem prefixes that identify installers, redistributables and helpers.
const INSTALLER_PREFIXES: &[&str] = &[
    "unins",
    "uninstall",
    "setup",
    "install",
    "vcredist",
    "vc_redist",
    "dxsetup",
    "dxwebsetup",
    "dotnet",
    "oalinst",
    "redist",
    "crashhandler",
    "unitycrashhandler",
    "notification_helper",
    "ue4prereq",
    "prereq",
];

/// Returns `true` when a file name looks like an installer, uninstaller,
/// redistributable or crash handler rather than a game executable.
///
/// The comparison is case-insensitive and ignores the extension.
///
/// ```
/// assert!(ludex_core::is_installer("unins000.exe"));
/// assert!(ludex_core::is_installer("UnityCrashHandler64.exe"));
/// assert!(!ludex_core::is_installer("HITMAN3.exe"));
/// ```
pub fn is_installer(name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = match base.rsplit_once('.') {
        Some((head, _)) if !head.is_empty() => head,
        _ => base,
    };
    let lower = stem.to_ascii_lowercase();
    INSTALLER_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// One plausible executable found while scanning a folder (#134).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableCandidate {
    /// Absolute-or-root-relative path exactly as discovered.
    pub path: PathBuf,
    /// Folder depth below the scan root; files directly inside it are at 1.
    pub depth: usize,
    /// `0` for the platform's native format, `1` for a Windows `.exe` fallback.
    pub tier: u8,
    /// Rank derived from the title overlap, depth and file size; higher is better.
    pub score: i64,
    /// File size in bytes, used by the ranking and shown in the UI.
    pub size: u64,
    /// True when the file name shares at least one word with the game title.
    pub title_match: bool,
}

/// Read-only result of scanning a game folder for executables (#134).
#[derive(Debug, Clone, Default)]
pub struct Discovery {
    /// Ranked best first, never empty when at least one candidate was found.
    pub candidates: Vec<ExecutableCandidate>,
    /// The executable Ludex would use now. `None` when nothing was found or
    /// when several plausible files exist and none matches the title.
    pub chosen: Option<PathBuf>,
    /// Diagnostics with the affected path: unreadable folders, broken links,
    /// link loops, depth truncation. Never a bare "no executable".
    pub notes: Vec<String>,
}

impl Discovery {
    /// Candidate paths as strings, best first.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect()
    }
}

/// Picks the most likely game executable inside `root`.
///
/// Walks up to [`MAX_DEPTH`] directories, skipping hidden and non-game
/// directories (`Redist`, `_CommonRedist`, `Support`, `Tools`, `DirectX`,
/// `DotNet`). Returns `None` when no candidate exists.
///
/// Platform rules: `.exe` on Windows, `.sh`/`.AppImage` on Linux, and `.app`
/// bundles on macOS (the bundle directory itself is returned). On non-Windows
/// platforms a Windows `.exe` is used as a fallback when no native candidate
/// exists, so repacks can still be identified (and run through Wine/Proton).
pub fn pick_executable(root: &Path, title: &str) -> Option<PathBuf> {
    pick_executable_with_depth(root, title, MAX_DEPTH)
}

/// [`pick_executable`] with an explicit depth, clamped to 4..=8 (#134).
///
/// This keeps the historical behaviour of accepting the best-ranked file even
/// when it does not share a word with the title, which is what launcher and
/// install detection already relied on. Folder scans use
/// [`discover_executables`] instead so an unrelated executable is never picked
/// silently among several candidates.
pub fn pick_executable_with_depth(root: &Path, title: &str, max_depth: usize) -> Option<PathBuf> {
    discover_executables(root, title, max_depth)
        .candidates
        .first()
        .map(|candidate| candidate.path.clone())
}

/// Scan `root` for plausible executables, with diagnostics (#134).
///
/// The scan follows directory links/junctions (with canonicalisation and a
/// visited set, so a loop cannot hang it), skips redistributables, support,
/// tools, installers and uninstallers, and records every folder it could not
/// read instead of returning a bare "no executable".
///
/// `max_depth` is clamped with [`clamp_scan_depth`]. Files directly inside
/// `root` are depth 1.
pub fn discover_executables(root: &Path, title: &str, max_depth: usize) -> Discovery {
    let mut discovery = Discovery::default();
    if !root.is_dir() {
        discovery
            .notes
            .push(format!("{} is not a readable folder.", root.display()));
        return discovery;
    }

    let max_depth = clamp_scan_depth(max_depth);
    let title_norm = normalize_title(title);
    let title_tokens: Vec<&str> = title_norm.split_whitespace().collect();
    let root_canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

    let mut candidates: Vec<ExecutableCandidate> = Vec::new();
    let mut seen_files: HashSet<PathBuf> = HashSet::new();
    let mut visited_dirs: HashSet<PathBuf> = HashSet::new();
    visited_dirs.insert(root_canonical.clone());
    let mut depth_truncated: Option<PathBuf> = None;

    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth >= max_depth {
            depth_truncated.get_or_insert(dir);
            continue;
        }
        let entries = match read_dir_long(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                discovery
                    .notes
                    .push(format!("Could not read {}: {error}", dir.display()));
                continue;
            }
        };

        let mut child_dirs: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                discovery
                    .notes
                    .push(format!("Could not read an entry in {}.", dir.display()));
                continue;
            };
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                discovery
                    .notes
                    .push(format!("Could not inspect {}.", path.display()));
                continue;
            };

            if metadata.file_type().is_symlink() {
                // A junction/symlink: resolve it, guard against loops and keep
                // walking the logical path so candidates stay inside the game
                // folder the user picked.
                let Ok(target) = std::fs::canonicalize(&path) else {
                    discovery
                        .notes
                        .push(format!("{} is a broken link; skipped.", path.display()));
                    continue;
                };
                if target.is_dir() {
                    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    if name.starts_with('.') || is_excluded_dir(name) {
                        continue;
                    }
                    if !visited_dirs.insert(target.clone()) {
                        discovery.notes.push(format!(
                            "{} links back to {}, which was already scanned; not followed again.",
                            path.display(),
                            target.display()
                        ));
                        continue;
                    }
                    if !target.starts_with(&root_canonical) {
                        discovery.notes.push(format!(
                            "{} is a link to {}, outside the game folder. It was followed for detection, but linking the folder may be refused.",
                            path.display(),
                            target.display()
                        ));
                    }
                    child_dirs.push(path);
                } else if target.is_file() {
                    // A link to an executable is a candidate like any other.
                    visit_file(
                        &path,
                        depth + 1,
                        &title_norm,
                        &title_tokens,
                        &mut seen_files,
                        &mut candidates,
                    );
                }
                continue;
            }

            if metadata.is_dir() {
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || is_excluded_dir(name) {
                    continue;
                }
                if is_bundle(&path) {
                    visit_file(
                        &path,
                        depth + 1,
                        &title_norm,
                        &title_tokens,
                        &mut seen_files,
                        &mut candidates,
                    );
                } else {
                    child_dirs.push(path);
                }
            } else if metadata.is_file() {
                visit_file(
                    &path,
                    depth + 1,
                    &title_norm,
                    &title_tokens,
                    &mut seen_files,
                    &mut candidates,
                );
            }
        }

        // Depth-first order with reversed sorting keeps the walk deterministic
        // and cheap: paths are already sorted by `read_dir` on most platforms.
        child_dirs.sort();
        for child in child_dirs.into_iter().rev() {
            stack.push((child, depth + 1));
        }
    }

    if let Some(path) = depth_truncated {
        discovery.notes.push(format!(
            "Search stopped at {max_depth} folder levels; {} was not searched. Raise the scan depth if the executable is deeper.",
            path.display()
        ));
    }

    candidates.sort_by(|left, right| {
        left.tier
            .cmp(&right.tier)
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| left.path.cmp(&right.path))
    });
    discovery.chosen = choose_candidate(&candidates, &title_norm);
    if discovery.chosen.is_none() && !candidates.is_empty() {
        let listed = candidates
            .iter()
            .take(3)
            .map(|candidate| candidate.path.display().to_string())
            .collect::<Vec<String>>()
            .join(", ");
        discovery.notes.push(format!(
            "Several executables were found and none matches the title; choose one manually: {listed}"
        ));
    }
    discovery.candidates = candidates;
    discovery
}

/// Visit one file, score it and record it when it looks like a program.
fn visit_file(
    path: &Path,
    depth: usize,
    title_norm: &str,
    title_tokens: &[&str],
    seen_files: &mut HashSet<PathBuf>,
    candidates: &mut Vec<ExecutableCandidate>,
) {
    let Some(tier) = candidate_tier(path) else {
        return;
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if is_installer(file_name) {
        return;
    }
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return;
    };
    let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !seen_files.insert(identity) {
        return;
    }
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let score = score_candidate(stem, title_norm, title_tokens, depth, size);
    candidates.push(ExecutableCandidate {
        path: path.to_path_buf(),
        depth,
        tier,
        score,
        size,
        title_match: shares_title(stem, title_norm, title_tokens),
    });
}

/// True when the file stem shares a word with the normalised title.
fn shares_title(stem: &str, title_norm: &str, title_tokens: &[&str]) -> bool {
    if title_norm.is_empty() {
        return false;
    }
    let stem_norm = normalize_title(stem);
    if stem_norm == title_norm || stem_norm.contains(title_norm) {
        return true;
    }
    // Spacing is not a signal: `ForzaHorizon6` matches "Forza Horizon 6".
    let stem_compact: String = stem_norm.replace(' ', "");
    let title_compact: String = title_norm.replace(' ', "");
    if !title_compact.is_empty()
        && (stem_compact == title_compact || stem_compact.contains(&title_compact))
    {
        return true;
    }
    let stem_tokens: Vec<&str> = stem_norm.split_whitespace().collect();
    title_tokens.iter().any(|token| stem_tokens.contains(token))
}

/// The executable Ludex can use without guessing: the best ranked file when it
/// relates to the title, or when it is the only plausible file in the tree.
fn choose_candidate(candidates: &[ExecutableCandidate], title_norm: &str) -> Option<PathBuf> {
    let best = candidates.first()?;
    if title_norm.is_empty() {
        return Some(best.path.clone());
    }
    if best.title_match || candidates.len() == 1 {
        return Some(best.path.clone());
    }
    // A lone tier-0 candidate beats a pile of unrelated `.exe` fallbacks: on
    // Linux, a repack whose only native launcher is unrelated to the title is
    // still the only thing that can run.
    if candidates
        .iter()
        .filter(|candidate| candidate.tier == 0)
        .count()
        == 1
        && best.tier == 0
    {
        return Some(best.path.clone());
    }
    None
}

/// `read_dir` retried with a Windows verbatim prefix for long paths.
fn read_dir_long(dir: &Path) -> std::io::Result<std::fs::ReadDir> {
    #[cfg(windows)]
    {
        match std::fs::read_dir(dir) {
            Ok(entries) => Ok(entries),
            Err(_) if long_path_needs_prefix(dir) => std::fs::read_dir(verbatim_path(dir)),
            Err(error) => Err(error),
        }
    }
    #[cfg(not(windows))]
    {
        std::fs::read_dir(dir)
    }
}

#[cfg(windows)]
fn long_path_needs_prefix(path: &Path) -> bool {
    use std::path::Component;
    let raw = path.to_string_lossy();
    raw.len() >= 240
        && path.is_absolute()
        && !raw.starts_with(r"\\?\")
        && matches!(path.components().next(), Some(Component::Prefix(_)))
}

#[cfg(windows)]
fn verbatim_path(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw.starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    // UNC paths keep both leading separators after `\\?\UNC\`.
    if let Some(rest) = raw.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{rest}"));
    }
    PathBuf::from(format!(r"\\?\{raw}"))
}

/// Computes the ranking score for one candidate file.
fn score_candidate(
    stem: &str,
    title_norm: &str,
    title_tokens: &[&str],
    depth: usize,
    size: u64,
) -> i64 {
    let stem_norm = normalize_title(stem);
    let stem_tokens: Vec<&str> = stem_norm.split_whitespace().collect();
    let overlap = title_tokens
        .iter()
        .filter(|token| stem_tokens.contains(token))
        .count() as i64;
    let similarity = if title_tokens.is_empty() {
        0
    } else {
        overlap * 100 / title_tokens.len() as i64
    };

    let mut score = similarity * 2;
    let stem_compact: String = stem_norm.replace(' ', "");
    let title_compact: String = title_norm.replace(' ', "");
    if !title_norm.is_empty() && stem_norm == title_norm {
        score += 150;
    } else if !title_norm.is_empty() && stem_norm.contains(title_norm) {
        score += 60;
    } else if !title_compact.is_empty()
        && (stem_compact == title_compact || stem_compact.contains(&title_compact))
    {
        // `ForzaHorizon6.exe` for "Forza Horizon 6": spacing is not a signal.
        score += 120;
    }
    score += (5_i64 - depth as i64).max(0) * 10;
    score += (size.max(1).ilog2() as i64).min(40);

    // Generic launchers are allowed but should lose against a title match.
    if stem_norm.contains("launcher") {
        score -= 80;
    }

    score
}

/// Returns the preference tier of a candidate: `0` for the platform's native
/// executable format, `1` for a Windows `.exe` fallback on non-Windows hosts.
fn candidate_tier(path: &Path) -> Option<u8> {
    if is_executable_candidate(path) {
        return Some(0);
    }
    if !cfg!(windows) && path.is_file() && extension_eq(path, "exe") {
        return Some(1);
    }
    None
}

/// Case-insensitive extension comparison.
fn extension_eq(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn is_excluded_dir(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    EXCLUDED_DIRS
        .iter()
        .any(|excluded| lower.contains(excluded))
}

#[cfg(target_os = "macos")]
fn is_bundle(path: &Path) -> bool {
    path.is_dir() && extension_eq(path, "app")
}

#[cfg(not(target_os = "macos"))]
fn is_bundle(_path: &Path) -> bool {
    false
}

#[cfg(windows)]
fn is_executable_candidate(path: &Path) -> bool {
    path.is_file() && extension_eq(path, "exe")
}

#[cfg(target_os = "macos")]
fn is_executable_candidate(path: &Path) -> bool {
    is_bundle(path)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_executable_candidate(path: &Path) -> bool {
    path.is_file() && (extension_eq(path, "sh") || extension_eq(path, "appimage"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_file(path: &Path, size: usize) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, vec![b'x'; size]).expect("write file");
    }

    #[test]
    fn installer_detection() {
        for name in [
            "unins000.exe",
            "Uninstall.exe",
            "setup.exe",
            "Installer.exe",
            "vcredist_x64.exe",
            "vc_redist.x64.exe",
            "dxsetup.exe",
            "DXWebSetup.exe",
            "dotnet-runtime.exe",
            "oalinst.exe",
            "redist.exe",
            "CrashHandler.exe",
            "UnityCrashHandler64.exe",
            "notification_helper.exe",
            "UE4PrereqSetup_x64.exe",
        ] {
            assert!(is_installer(name), "{name}");
        }
        for name in [
            "HITMAN3.exe",
            "Game.exe",
            "launcher.exe",
            "Blue Prince.exe",
            "MinecraftLauncher.exe",
            "Game.sh",
            "Game.AppImage",
        ] {
            assert!(!is_installer(name), "{name}");
        }
    }

    #[test]
    fn missing_or_empty_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert_eq!(pick_executable(&temp.path().join("nope"), "Game"), None);
        assert_eq!(pick_executable(temp.path(), "Game"), None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn picks_best_title_match() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Blue Prince.sh"), 128);
        write_file(&root.join("readme.txt"), 10);
        write_file(&root.join("other.sh"), 4096);

        let picked = pick_executable(root, "Blue Prince").expect("candidate");
        assert_eq!(picked, root.join("Blue Prince.sh"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn skips_installers_and_support_folders() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("unins000.sh"), 10);
        write_file(&root.join("Game/Support/Game.sh"), 1024);
        write_file(&root.join("Game/Redist/dotnet.sh"), 2048);
        write_file(&root.join("Game/Tools/helper.sh"), 2048);
        write_file(&root.join("Game/Game.sh"), 64);

        let picked = pick_executable(root, "Game").expect("candidate");
        assert_eq!(picked, root.join("Game/Game.sh"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn deprioritises_launchers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Game/launcher.sh"), 4096);
        write_file(&root.join("Game/Game.sh"), 32);

        let picked = pick_executable(root, "Game").expect("candidate");
        assert_eq!(picked, root.join("Game/Game.sh"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn prefers_top_level_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Game.sh"), 10);
        write_file(&root.join("nested/Game.sh"), 4096);

        let picked = pick_executable(root, "Game").expect("candidate");
        assert_eq!(picked, root.join("Game.sh"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn respects_depth_limit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("a/b/c/d/Game.sh"), 10);
        assert_eq!(pick_executable(root, "Game"), None);

        write_file(&root.join("a/b/c/Game.sh"), 10);
        assert_eq!(
            pick_executable(root, "Game"),
            Some(root.join("a/b/c/Game.sh"))
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn accepts_appimage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Blue Prince.AppImage"), 10);
        assert_eq!(
            pick_executable(root, "Blue Prince"),
            Some(root.join("Blue Prince.AppImage"))
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn installer_only_folder_returns_none() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("unins000.sh"), 10);
        write_file(&root.join("setup.sh"), 10);
        assert_eq!(pick_executable(root, "Game"), None);
    }

    #[cfg(windows)]
    #[test]
    fn picks_windows_executable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("unins000.exe"), 10);
        write_file(&root.join("Game/Game.exe"), 64);

        let picked = pick_executable(root, "Game").expect("candidate");
        assert_eq!(picked, root.join("Game/Game.exe"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn windows_exe_is_a_fallback_on_linux() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(
            &root.join("AnkerGames - Free Pre-installed PC Games.url"),
            10,
        );
        write_file(&root.join("Cat Quest II/Cat Quest II.exe"), 641_536);

        let picked = pick_executable(root, "Cat Quest II").expect("fallback candidate");
        assert_eq!(picked, root.join("Cat Quest II/Cat Quest II.exe"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn native_candidate_wins_over_exe_fallback() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Game/Game.sh"), 32);
        write_file(&root.join("Game/Game.exe"), 4_000_000);

        let picked = pick_executable(root, "Game").expect("candidate");
        assert_eq!(picked, root.join("Game/Game.sh"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn picks_app_bundle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let bundle = root.join("Blue Prince.app");
        fs::create_dir_all(bundle.join("Contents/MacOS")).expect("bundle");
        fs::write(bundle.join("Contents/MacOS/Blue Prince"), b"bin").expect("binary");

        let picked = pick_executable(root, "Blue Prince").expect("candidate");
        assert_eq!(picked, bundle);
    }

    #[test]
    fn scan_depth_is_clamped_to_the_documented_range() {
        assert_eq!(clamp_scan_depth(0), MIN_SCAN_DEPTH);
        assert_eq!(clamp_scan_depth(3), MIN_SCAN_DEPTH);
        assert_eq!(clamp_scan_depth(6), 6);
        assert_eq!(clamp_scan_depth(50), MAX_SCAN_DEPTH);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn discovery_finds_a_deeply_nested_executable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Game/a/b/c/d/Game.sh"), 10);

        // Depth 4 stops before `d`; the default scan depth reaches it.
        assert_eq!(discover_executables(root, "Game", 4).candidates.len(), 0);
        let discovery = discover_executables(root, "Game", 6);
        assert_eq!(discovery.candidates.len(), 1);
        assert_eq!(
            discovery.chosen.as_deref(),
            Some(root.join("Game/a/b/c/d/Game.sh").as_path())
        );
        assert!(discovery.candidates[0].depth <= 6);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn discovery_does_not_follow_a_loop_forever() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let game = root.join("Game");
        write_file(&game.join("Game.sh"), 10);
        std::os::unix::fs::symlink(&game, game.join("loop")).expect("loop link");

        let discovery = discover_executables(root, "Game", 8);
        assert_eq!(discovery.candidates.len(), 1);
        assert!(
            discovery
                .notes
                .iter()
                .any(|note| note.contains("already scanned")),
            "{:?}",
            discovery.notes
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn discovery_returns_every_candidate_ranked() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Game/Other.sh"), 4096);
        write_file(&root.join("Game/Game.sh"), 32);

        let discovery = discover_executables(root, "Game", 6);
        assert_eq!(discovery.candidates.len(), 2);
        assert_eq!(discovery.candidates[0].path, root.join("Game/Game.sh"));
        assert!(discovery.candidates[0].title_match);
        assert_eq!(
            discovery.chosen.as_deref(),
            Some(root.join("Game/Game.sh").as_path())
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn discovery_never_picks_an_unrelated_executable_among_several() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Game/Other.sh"), 4096);
        write_file(&root.join("Game/Helper.sh"), 2048);

        let discovery = discover_executables(root, "Forza Horizon", 6);
        assert_eq!(discovery.candidates.len(), 2);
        assert_eq!(discovery.chosen, None);
        assert!(discovery
            .notes
            .iter()
            .any(|note| note.contains("choose one manually")));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn discovery_matches_a_camel_case_executable_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Game/ForzaHorizon6.sh"), 2048);
        write_file(&root.join("Game/Helper.sh"), 4096);

        let discovery = discover_executables(root, "Forza Horizon 6", 6);
        assert_eq!(discovery.candidates.len(), 2);
        assert_eq!(
            discovery.chosen.as_deref(),
            Some(root.join("Game/ForzaHorizon6.sh").as_path())
        );
        assert!(discovery.candidates[0].title_match);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn discovery_skips_installer_and_support_directories() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_file(&root.join("Game/_CommonRedist/dxsetup.sh"), 10);
        write_file(&root.join("Game/Support/Game.sh"), 10);
        write_file(&root.join("Game/Installers/setup.sh"), 10);
        write_file(&root.join("Game/Game.sh"), 10);

        let discovery = discover_executables(root, "Game", 6);
        assert_eq!(discovery.candidates.len(), 1);
        assert_eq!(discovery.candidates[0].path, root.join("Game/Game.sh"));
    }
}
