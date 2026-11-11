//! Executable listing for the Launch options picker (#143).
//!
//! The launch/installer/uninstaller fields used to be free text, so a typo
//! only surfaced when the game failed to start. This module scans the folder of
//! an installed game (installed by Ludex, adopted or imported from a launcher)
//! and lists every program it can find with its kind, size and modification
//! time.
//!
//! The scan is:
//!
//! - **bounded**: depth 4-8 (default 6) and a hard entry cap, so a network
//!   drive or a huge repack cannot stall the UI,
//! - **loop-safe**: directory links are canonicalised and a visited set stops
//!   a junction loop,
//! - **rooted**: only files whose canonical target stays inside the canonical
//!   install folder are returned; a link that escapes it is skipped with a
//!   note,
//! - **cached**: a listing is reused for [`CACHE_TTL`], and the command accepts
//!   an explicit `refresh` that bypasses the cache.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

use crate::bridge;
use crate::db::InstalledGame;
use crate::util::now_rfc3339;

/// How long a scan is reused before the folder is walked again.
pub const CACHE_TTL: Duration = Duration::from_secs(5);

/// Hard cap on returned entries. A game folder with more programs than this is
/// abnormal and the list would not be usable anyway.
const MAX_ENTRIES: usize = 400;

/// Hard cap on directory entries inspected during one walk.
const MAX_SCANNED_ENTRIES: usize = 20_000;

/// What a program found in the install folder can be used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutableKind {
    /// A game binary Ludex can launch.
    Launch,
    /// A `setup*`, `install*` or `gog*` program.
    Installer,
    /// An `unins*` or `uninstall*` program.
    Uninstaller,
    /// A program that is neither: redistributables, crash handlers, helpers.
    Other,
}

/// One program found inside the install folder (`docs/CONTRACT.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameExecutable {
    /// Path relative to the install folder, forward slashes, never `..`.
    pub path: String,
    /// File name only, for the compact list label.
    pub name: String,
    pub size_bytes: u64,
    /// RFC 3339, `None` when the filesystem does not expose it.
    pub modified_at: Option<String>,
    pub kind: ExecutableKind,
    /// True for the file `pick_executable` would launch now.
    pub recommended: bool,
}

/// Result of one listing, including diagnostics (`docs/CONTRACT.md`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameExecutables {
    pub game_id: i64,
    /// Absolute folder that was scanned.
    pub install_path: String,
    pub entries: Vec<GameExecutable>,
    /// Skipped links, unreadable folders, truncation.
    pub notes: Vec<String>,
    pub scanned_at: String,
    /// True when the answer came from the short-lived cache.
    pub cached: bool,
}

#[derive(Debug, Clone)]
struct Listing {
    install_path: String,
    entries: Vec<GameExecutable>,
    notes: Vec<String>,
    scanned_at: String,
}

#[derive(Debug, Clone)]
struct CachedListing {
    listing: Listing,
    at: Instant,
}

/// Short-lived cache of listings, keyed by canonical install folder.
///
/// One entry per folder: a game lives in exactly one folder at a time, so a
/// moved install simply gets a new key.
#[derive(Debug)]
pub struct ExecutableCache {
    ttl: Duration,
    rows: HashMap<String, CachedListing>,
}

impl Default for ExecutableCache {
    fn default() -> Self {
        Self::new(CACHE_TTL)
    }
}

impl ExecutableCache {
    /// Cache with an explicit time-to-live (tests use short values).
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            rows: HashMap::new(),
        }
    }

    fn get(&self, key: &str, now: Instant) -> Option<&Listing> {
        let row = self.rows.get(key)?;
        if now.saturating_duration_since(row.at) <= self.ttl {
            Some(&row.listing)
        } else {
            None
        }
    }

    fn put(&mut self, key: String, listing: Listing, now: Instant) {
        // Listings are tiny, but a long session with many moved folders must
        // not grow the map forever: drop expired rows first.
        if self.rows.len() >= 64 {
            self.rows
                .retain(|_, row| now.saturating_duration_since(row.at) <= self.ttl);
        }
        self.rows.insert(key, CachedListing { listing, at: now });
    }

    /// Drop the cached listing of one install folder.
    ///
    /// Called after an installer run, which may have written new programs into
    /// the folder; a move does not need it because the folder path is the key.
    pub fn invalidate(&mut self, key: &str) {
        self.rows.remove(key);
    }
}

/// Canonical folder of an installed game, whatever put it in the index.
///
/// `ludex` installs, adopted folders (#129) and launcher imports with a known
/// directory all store it in `install_path`. An imported row without a folder
/// cannot be scanned: the UI falls back to the manual field.
pub fn installed_root(installed: &InstalledGame) -> Result<PathBuf, String> {
    let raw = installed.install_path.trim();
    if raw.is_empty() {
        return Err("Ludex has no folder for this game yet. Link or install it first.".to_string());
    }
    let root = PathBuf::from(raw);
    if !root.is_dir() {
        return Err(format!(
            "The install folder is missing: {}. Reinstall the game or link it again.",
            root.display()
        ));
    }
    root.canonicalize()
        .map_err(|error| format!("Could not resolve {}: {error}", root.display()))
}

/// Scan an install folder and list its programs.
fn scan_folder(root: &Path, title: &str, max_depth: usize) -> Result<Listing, String> {
    let root_canonical = root.canonicalize().map_err(|error| {
        format!(
            "Could not read the install folder {}: {error}",
            root.display()
        )
    })?;
    if !root_canonical.is_dir() {
        return Err(format!("The install folder is missing: {}", root.display()));
    }

    let max_depth = bridge::clamp_scan_depth(max_depth);
    let mut notes: Vec<String> = Vec::new();
    let mut buckets = Buckets::default();
    let mut visited_dirs: HashSet<PathBuf> = HashSet::new();
    visited_dirs.insert(root_canonical.clone());
    let mut scanned_entries = 0usize;
    let mut depth_truncated = false;
    let mut entries_truncated = false;

    let mut stack: Vec<(PathBuf, usize, bool)> = vec![(root.to_path_buf(), 0, false)];
    while let Some((dir, depth, excluded)) = stack.pop() {
        if depth >= max_depth {
            // Only report a truncation when the boundary folder actually holds
            // something: an empty folder at the limit is not a missed program.
            if fs::read_dir(&dir)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false)
            {
                depth_truncated = true;
            }
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                notes.push(format!("Could not read {}: {error}", dir.display()));
                continue;
            }
        };

        for entry in entries.flatten() {
            scanned_entries += 1;
            if scanned_entries > MAX_SCANNED_ENTRIES {
                entries_truncated = true;
                break;
            }
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let name = name.to_string();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                notes.push(format!("Could not inspect {}.", path.display()));
                continue;
            };

            if metadata.file_type().is_symlink() {
                let Ok(target) = fs::canonicalize(&path) else {
                    notes.push(format!("{} is a broken link; skipped.", path.display()));
                    continue;
                };
                if !target.starts_with(&root_canonical) {
                    notes.push(format!(
                        "{} points outside the install folder; skipped.",
                        path.display()
                    ));
                    continue;
                }
                if target.is_dir() {
                    if !visited_dirs.insert(target.clone()) {
                        notes.push(format!(
                            "{} links back to {}, which was already scanned; not followed again.",
                            path.display(),
                            target.display()
                        ));
                        continue;
                    }
                    stack.push((
                        path.clone(),
                        depth + 1,
                        excluded || bridge::is_excluded_folder(&name),
                    ));
                    continue;
                }
                if target.is_file() {
                    buckets.push(&path, &name, excluded);
                }
                continue;
            }

            if metadata.is_dir() {
                if is_app_bundle(&name) {
                    // macOS: the bundle directory is the launchable program.
                    buckets.push(&path, &name, excluded);
                    continue;
                }
                if let Ok(canonical) = fs::canonicalize(&path) {
                    visited_dirs.insert(canonical);
                }
                stack.push((
                    path.clone(),
                    depth + 1,
                    excluded || bridge::is_excluded_folder(&name),
                ));
                continue;
            }
            if metadata.is_file() {
                buckets.push(&path, &name, excluded);
            }
        }
        if entries_truncated {
            break;
        }
    }

    if depth_truncated {
        notes.push(format!(
            "Search stopped at {max_depth} folder levels; a deeper program is not listed."
        ));
    }
    if entries_truncated {
        notes.push(format!(
            "The folder holds more than {MAX_SCANNED_ENTRIES} entries; the list was truncated."
        ));
    }

    // The existing #134 heuristic ranks the launch candidates and decides which
    // one Ludex would start now; the listing only annotates and orders it.
    let discovery = bridge::discover_executables(root, title, max_depth);
    let rank: HashMap<&Path, usize> = discovery
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| (candidate.path.as_path(), index))
        .collect();
    let recommended = discovery
        .chosen
        .as_deref()
        .and_then(|chosen| chosen.strip_prefix(root).ok())
        .map(|relative| relative_slash(relative));
    let mut launch = buckets.launch;
    launch.sort_by(|left, right| {
        let left_rank = rank.get(left.path.as_path()).copied().unwrap_or(usize::MAX);
        let right_rank = rank
            .get(right.path.as_path())
            .copied()
            .unwrap_or(usize::MAX);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.path.cmp(&right.path))
    });

    let mut entries: Vec<GameExecutable> = Vec::new();
    for found in launch {
        entries.push(found.into_entry(root, ExecutableKind::Launch, recommended.as_deref()));
    }
    for (mut bucket, kind) in [
        (buckets.installers, ExecutableKind::Installer),
        (buckets.uninstallers, ExecutableKind::Uninstaller),
        (buckets.others, ExecutableKind::Other),
    ] {
        bucket.sort_by(|left, right| left.path.cmp(&right.path));
        for found in bucket {
            entries.push(found.into_entry(root, kind, recommended.as_deref()));
        }
    }
    if entries.len() > MAX_ENTRIES {
        entries.truncate(MAX_ENTRIES);
        notes.push(format!(
            "Only the first {MAX_ENTRIES} programs are listed; narrow the folder if the one you need is missing."
        ));
    }

    Ok(Listing {
        install_path: root.to_string_lossy().to_string(),
        entries,
        notes,
        scanned_at: now_rfc3339(),
    })
}

/// Cached or fresh listing for an installed game.
pub fn list(
    cache: &mut ExecutableCache,
    installed: &InstalledGame,
    refresh: bool,
) -> Result<GameExecutables, String> {
    let root = installed_root(installed)?;
    let key = root.to_string_lossy().to_string();
    let now = Instant::now();
    let cached = if refresh {
        None
    } else {
        cache.get(&key, now).cloned()
    };
    let (listing, from_cache) = match cached {
        Some(listing) => (listing, true),
        None => {
            let listing = scan_folder(&root, &installed.title, bridge::DEFAULT_SCAN_DEPTH)?;
            cache.put(key, listing.clone(), now);
            (listing, false)
        }
    };

    Ok(GameExecutables {
        game_id: installed.game_id,
        install_path: listing.install_path,
        entries: listing.entries,
        notes: listing.notes,
        scanned_at: listing.scanned_at,
        cached: from_cache,
    })
}

/// Normalise a user-chosen executable to a forward-slash relative path.
///
/// Refuses parent traversal, absolute paths outside the install folder and
/// links that resolve outside it. The returned path is the canonical target
/// relative to the canonical folder, so saving it can never escape the root
/// later.
pub fn resolve_relative(root: &Path, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Choose an executable first.".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("The executable path contains control characters.".to_string());
    }
    let root_canonical = root.canonicalize().map_err(|error| {
        format!(
            "Could not read the install folder {}: {error}",
            root.display()
        )
    })?;
    if !root_canonical.is_dir() {
        return Err(format!("The install folder is missing: {}", root.display()));
    }

    let candidate = PathBuf::from(trimmed);
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("The executable path cannot contain “..”.".to_string());
    }

    let full = if candidate.is_absolute() {
        candidate
    } else {
        root.join(&candidate)
    };
    let resolved = match full.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "That file does not exist in the install folder: {trimmed}"
            ));
        }
        Err(error) => {
            return Err(format!("Could not resolve {trimmed}: {error}"));
        }
    };

    if resolved == root_canonical || !crate::paths::is_inside(&root_canonical, &resolved) {
        return Err(format!(
            "Refusing “{trimmed}”: it is outside the install folder."
        ));
    }
    if !resolved.is_file() && !is_app_bundle(&file_name(&resolved)) {
        return Err(format!("“{trimmed}” is not an executable file."));
    }
    if !is_program_name(&file_name(&resolved)) {
        return Err(format!(
            "“{trimmed}” is not a program Ludex can launch (.exe, .sh or .AppImage)."
        ));
    }

    let relative = resolved
        .strip_prefix(&root_canonical)
        .map_err(|_| format!("Refusing “{trimmed}”: it is outside the install folder."))?;
    Ok(relative_slash(relative))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

fn relative_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// True for the file extensions the launch code accepts.
fn is_program_name(name: &str) -> bool {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "exe" => true,
        "bat" | "cmd" => cfg!(windows),
        "sh" | "appimage" => cfg!(all(unix, not(target_os = "macos"))),
        "app" => cfg!(target_os = "macos"),
        _ => false,
    }
}

/// Classify one file; `None` when it is not a program at all.
///
/// Installer and uninstaller detection is name-based, so a `setup.sh` is an
/// installer and a plain `Game.sh` stays a launch candidate (#143).
fn classify(name: &str, excluded_dir: bool) -> Option<ExecutableKind> {
    if !is_program_name(name) {
        return None;
    }
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    };
    let stem = stem.to_ascii_lowercase();
    if stem.starts_with("unins") || stem.starts_with("uninstall") {
        return Some(ExecutableKind::Uninstaller);
    }
    if stem.starts_with("setup") || stem.starts_with("install") || stem.starts_with("gog") {
        return Some(ExecutableKind::Installer);
    }
    if excluded_dir || ludex_core::exe::is_installer(name) {
        return Some(ExecutableKind::Other);
    }
    Some(ExecutableKind::Launch)
}

#[cfg(target_os = "macos")]
fn is_app_bundle(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".app")
}

#[cfg(not(target_os = "macos"))]
fn is_app_bundle(_name: &str) -> bool {
    false
}

fn format_modified(value: SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Utc> = value.into();
    datetime.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[derive(Debug)]
struct Found {
    path: PathBuf,
    name: String,
    size: u64,
    modified: Option<SystemTime>,
}

impl Found {
    fn into_entry(
        self,
        root: &Path,
        kind: ExecutableKind,
        recommended: Option<&str>,
    ) -> GameExecutable {
        let path = relative_slash(self.path.strip_prefix(root).unwrap_or(self.path.as_path()));
        GameExecutable {
            recommended: recommended == Some(path.as_str()),
            path,
            name: self.name,
            size_bytes: self.size,
            modified_at: self.modified.map(format_modified),
            kind,
        }
    }
}

#[derive(Debug, Default)]
struct Buckets {
    launch: Vec<Found>,
    installers: Vec<Found>,
    uninstallers: Vec<Found>,
    others: Vec<Found>,
    /// Canonical identity of every recorded file, so two links to the same
    /// target are listed once.
    seen_files: HashSet<PathBuf>,
}

impl Buckets {
    fn push(&mut self, path: &Path, name: &str, excluded_dir: bool) {
        let Some(kind) = classify(name, excluded_dir) else {
            return;
        };
        let identity = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.seen_files.insert(identity) {
            return;
        }
        // `metadata` follows the link, but only after the walk proved the
        // target stays inside the install folder.
        let metadata = fs::metadata(path).ok();
        let size = metadata.as_ref().map(|meta| meta.len()).unwrap_or(0);
        let modified = metadata.and_then(|meta| meta.modified().ok());
        let found = Found {
            path: path.to_path_buf(),
            name: name.to_string(),
            size,
            modified,
        };
        match kind {
            ExecutableKind::Launch => self.launch.push(found),
            ExecutableKind::Installer => self.installers.push(found),
            ExecutableKind::Uninstaller => self.uninstallers.push(found),
            ExecutableKind::Other => self.others.push(found),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-exe-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_file(path: &Path, size: usize) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(&vec![b'x'; size]).unwrap();
    }

    fn by_path<'a>(entries: &'a [GameExecutable], path: &str) -> &'a GameExecutable {
        entries
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("{path} not listed: {:?}", entries))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn lists_nested_programs_and_classifies_installers() {
        let root = temp_dir("nested");
        write_file(&root.join("Game/Game.sh"), 64);
        write_file(&root.join("Game/bin/helper.sh"), 32);
        write_file(&root.join("Game/bin/GameHelper.exe"), 128);
        write_file(&root.join("setup.sh"), 16);
        write_file(&root.join("unins000.sh"), 8);
        write_file(&root.join("Redist/vcredist_x64.exe"), 1024);
        write_file(&root.join("readme.txt"), 10);

        let listing = scan_folder(&root, "Game", 6).unwrap();
        assert_eq!(
            by_path(&listing.entries, "Game/Game.sh").kind,
            ExecutableKind::Launch
        );
        assert_eq!(
            by_path(&listing.entries, "Game/bin/helper.sh").kind,
            ExecutableKind::Launch
        );
        // A nested Windows `.exe` is a launch candidate on Linux too (#134).
        assert_eq!(
            by_path(&listing.entries, "Game/bin/GameHelper.exe").kind,
            ExecutableKind::Launch
        );
        assert_eq!(
            by_path(&listing.entries, "setup.sh").kind,
            ExecutableKind::Installer
        );
        assert_eq!(
            by_path(&listing.entries, "unins000.sh").kind,
            ExecutableKind::Uninstaller
        );
        assert_eq!(
            by_path(&listing.entries, "Redist/vcredist_x64.exe").kind,
            ExecutableKind::Other
        );
        assert!(listing
            .entries
            .iter()
            .all(|entry| entry.path != "readme.txt"));
        let game = by_path(&listing.entries, "Game/Game.sh");
        assert_eq!(game.name, "Game.sh");
        assert_eq!(game.size_bytes, 64);
        assert!(game.modified_at.is_some());
        assert!(game.recommended);
        assert!(!by_path(&listing.entries, "Game/bin/helper.sh").recommended);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn installer_names_win_over_the_launch_list() {
        let root = temp_dir("installers");
        write_file(&root.join("gogsetup.sh"), 4);
        write_file(&root.join("installer.sh"), 4);
        write_file(&root.join("Game.sh"), 8);

        let listing = scan_folder(&root, "Game", 6).unwrap();
        assert_eq!(
            by_path(&listing.entries, "gogsetup.sh").kind,
            ExecutableKind::Installer
        );
        assert_eq!(
            by_path(&listing.entries, "installer.sh").kind,
            ExecutableKind::Installer
        );
        // The installer entries are never marked recommended.
        assert!(!by_path(&listing.entries, "gogsetup.sh").recommended);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn empty_folder_returns_no_entries_and_no_error() {
        let root = temp_dir("empty");
        let listing = scan_folder(&root, "Game", 6).unwrap();
        assert!(listing.entries.is_empty());
        assert_eq!(
            listing.install_path,
            root.canonicalize().unwrap().to_string_lossy().to_string()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn skips_a_link_that_escapes_the_install_folder() {
        let root = temp_dir("escape");
        let outside = temp_dir("outside");
        write_file(&outside.join("Outside.sh"), 8);
        std::os::unix::fs::symlink(outside.join("Outside.sh"), root.join("Outside.sh")).unwrap();
        write_file(&root.join("Game.sh"), 8);

        let listing = scan_folder(&root, "Game", 6).unwrap();
        assert!(listing
            .entries
            .iter()
            .all(|entry| entry.path != "Outside.sh"));
        assert_eq!(listing.entries.len(), 1);
        assert!(listing
            .notes
            .iter()
            .any(|note| note.contains("outside the install folder")));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_directory_loop_does_not_hang_the_scan() {
        let root = temp_dir("loop");
        write_file(&root.join("Game/Game.sh"), 8);
        std::os::unix::fs::symlink(root.join("Game"), root.join("Game/loop")).unwrap();

        let listing = scan_folder(&root, "Game", 8).unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert!(listing
            .notes
            .iter()
            .any(|note| note.contains("already scanned")));
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn resolve_accepts_files_inside_and_rejects_escapes() {
        let root = temp_dir("resolve");
        let outside = temp_dir("resolve-outside");
        write_file(&root.join("Game/Game.sh"), 8);
        write_file(&outside.join("Evil.sh"), 8);
        std::os::unix::fs::symlink(outside.join("Evil.sh"), root.join("Link.sh")).unwrap();

        assert_eq!(
            resolve_relative(&root, "Game/Game.sh").unwrap(),
            "Game/Game.sh"
        );
        assert_eq!(
            resolve_relative(&root, "./Game/Game.sh").unwrap(),
            "Game/Game.sh"
        );
        // Absolute paths are accepted only while they stay inside the root.
        assert_eq!(
            resolve_relative(&root, &root.join("Game/Game.sh").to_string_lossy()).unwrap(),
            "Game/Game.sh"
        );
        assert!(resolve_relative(&root, "../Game/Game.sh").is_err());
        assert!(resolve_relative(&root, "/etc/passwd").is_err());
        assert!(resolve_relative(&root, "Link.sh").is_err());
        assert!(resolve_relative(&root, "Game/Missing.sh").is_err());
        assert!(resolve_relative(&root, "readme.txt").is_err());
        assert!(resolve_relative(&root, "  ").is_err());

        write_file(&root.join("readme.txt"), 4);
        assert!(resolve_relative(&root, "readme.txt")
            .unwrap_err()
            .contains("not a program"));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn cache_reuses_then_expires_and_refresh_bypasses_it() {
        let root = temp_dir("cache");
        write_file(&root.join("Game.sh"), 8);
        let installed = InstalledGame {
            game_id: 7,
            version_id: 1,
            title: "Game".into(),
            version: None,
            install_path: root.to_string_lossy().to_string(),
            executable: None,
            size_bytes: 8,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: None,
            source: crate::db::SOURCE_LUDEX.into(),
            launch_uri: None,
        };
        let mut cache = ExecutableCache::default();

        let first = list(&mut cache, &installed, false).unwrap();
        assert!(!first.cached);
        assert_eq!(first.entries.len(), 1);

        // A new file is invisible until the TTL expires or a refresh is asked.
        write_file(&root.join("Setup.exe"), 4);
        let second = list(&mut cache, &installed, false).unwrap();
        assert!(second.cached);
        assert_eq!(second.entries.len(), 1);

        let refreshed = list(&mut cache, &installed, true).unwrap();
        assert!(!refreshed.cached);
        assert_eq!(refreshed.entries.len(), 2);

        // Expiry: the same call with a clock past the TTL rescans.
        let key = root.canonicalize().unwrap().to_string_lossy().to_string();
        let later = Instant::now() + CACHE_TTL + Duration::from_secs(1);
        assert!(cache.get(&key, later).is_none());
        cache.invalidate(&key);
        assert!(cache.get(&key, Instant::now()).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_install_folder_is_an_error() {
        let installed = InstalledGame {
            game_id: 1,
            version_id: 0,
            title: "Gone".into(),
            version: None,
            install_path: "/nonexistent/ludex/game".into(),
            executable: None,
            size_bytes: 0,
            installed_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
            archive_hash: None,
            install_root: None,
            source: crate::db::SOURCE_LUDEX.into(),
            launch_uri: None,
        };
        let mut cache = ExecutableCache::default();
        let error = list(&mut cache, &installed, false).unwrap_err();
        assert!(error.contains("install folder is missing"), "{error}");

        let empty = InstalledGame {
            install_path: "   ".into(),
            ..installed
        };
        assert!(list(&mut cache, &empty, false).is_err());
    }
}
