//! Executable discovery for extracted game folders.
//!
//! After extracting an archive, Ludex needs to guess which file starts the
//! game. The heuristic scores file names against the game title, prefers
//! top-level files and larger binaries, and hard-excludes installers,
//! redistributables and crash handlers.

use std::path::{Path, PathBuf};

use crate::naming::normalize_title;

/// How deep [`pick_executable`] walks below the game root.
pub const MAX_DEPTH: usize = 4;

/// Directory names that never contain the game executable.
const EXCLUDED_DIRS: &[&str] = &[
    "redist",
    "_commonredist",
    "support",
    "tools",
    "directx",
    "dotnet",
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
    if !root.is_dir() {
        return None;
    }

    let title_norm = normalize_title(title);
    let title_tokens: Vec<&str> = title_norm.split_whitespace().collect();

    let mut best: Option<(u8, i64, PathBuf)> = None;

    walk(root, 0, &mut |path: &Path, depth: usize| {
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
        let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        let score = score_candidate(stem, &title_norm, &title_tokens, depth, size);

        let replace = match &best {
            None => true,
            Some((best_tier, best_score, best_path)) => {
                tier < *best_tier
                    || (tier == *best_tier
                        && (score > *best_score
                            || (score == *best_score && path < best_path.as_path())))
            }
        };
        if replace {
            best = Some((tier, score, path.to_path_buf()));
        }
    });

    best.map(|(_, _, path)| path)
}

/// Recursively visits files and `.app` bundles below `dir`.
fn walk(dir: &Path, depth: usize, visit: &mut dyn FnMut(&Path, usize)) {
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();

    for path in paths {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') || is_excluded_dir(name) {
                continue;
            }
            if is_bundle(&path) {
                visit(&path, depth + 1);
            } else {
                walk(&path, depth + 1, visit);
            }
        } else if metadata.is_file() {
            visit(&path, depth + 1);
        }
    }
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
    if !title_norm.is_empty() && stem_norm == title_norm {
        score += 150;
    } else if !title_norm.is_empty() && stem_norm.contains(title_norm) {
        score += 60;
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
}
