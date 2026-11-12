//! Filesystem helpers with path-traversal guards.
//!
//! Every user-controlled name (game slug, version label, executable override)
//! goes through [`sanitize_component`] and [`safe_child`]. Deletions verify that
//! the canonical target stays inside the configured install folder.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const MAX_COMPONENT_CHARS: usize = 80;

/// Win32 verbatim-path prefix.
#[cfg(any(windows, test))]
pub const VERBATIM_PREFIX: &str = r"\\?\";
/// Win32 verbatim UNC prefix; the plain form is `\\server\share`.
#[cfg(any(windows, test))]
pub const VERBATIM_UNC_PREFIX: &str = r"\\?\UNC\";
/// Legacy Win32 `MAX_PATH` (characters, including the terminating NUL in Win32
/// terms; a path at this length needs long-path support to be opened).
#[cfg(any(windows, test))]
pub const LEGACY_MAX_PATH: usize = 260;

/// Normalise a native filesystem path for an OS API (#187).
///
/// Windows: the verbatim `\\?\` form is stripped from drive paths and
/// `\\?\UNC\server\share` becomes `\\server\share`. A path that would no longer
/// fit in the legacy `MAX_PATH` keeps its verbatim prefix, because that prefix
/// is what enables long-path support; other verbatim forms (volumes, devices)
/// are left untouched. Everywhere else this is a no-op.
pub fn normalize_native_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy();
        let normalized = normalize_verbatim_string(&raw);
        if normalized == raw {
            path.to_path_buf()
        } else {
            PathBuf::from(normalized)
        }
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

/// String form of [`normalize_native_path`], platform-independent so the
/// Windows table is unit-tested on every host (#187).
#[cfg(any(windows, test))]
pub(crate) fn normalize_verbatim_string(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix(VERBATIM_PREFIX) else {
        return raw.to_string();
    };

    let plain = if let Some(unc) = strip_unc_prefix(rest) {
        // `\\?\UNC\server\share\dir` -> `\\server\share\dir`. A malformed UNC
        // tail (no server/share) is left alone: stripping it would produce a
        // path the Win32 API cannot open.
        if unc.split('\\').filter(|part| !part.is_empty()).count() < 2 {
            return raw.to_string();
        }
        format!(r"\\{unc}")
    } else if is_drive_path(rest) {
        rest.to_string()
    } else {
        // `\\?\Volume{…}` and other device namespaces are not playable paths.
        return raw.to_string();
    };

    if plain.chars().count() >= LEGACY_MAX_PATH {
        // The prefix is what makes >MAX_PATH paths work: keep it.
        return raw.to_string();
    }
    plain
}

#[cfg(any(windows, test))]
fn strip_unc_prefix(rest: &str) -> Option<&str> {
    let head = rest.get(..4)?;
    if head.eq_ignore_ascii_case(r"UNC\") {
        Some(&rest[4..])
    } else {
        None
    }
}

/// A drive path tail (`C:\…` or `C:`), after the verbatim prefix.
#[cfg(any(windows, test))]
fn is_drive_path(rest: &str) -> bool {
    let mut chars = rest.chars();
    matches!(chars.next(), Some(letter) if letter.is_ascii_alphabetic())
        && matches!(chars.next(), Some(':'))
}

/// Make an arbitrary string usable as a single path component.
pub fn sanitize_component(raw: &str, fallback: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_control()
            || matches!(
                character,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            )
        {
            cleaned.push('-');
        } else {
            cleaned.push(character);
        }
    }

    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    let cleaned = if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        fallback.to_string()
    } else {
        cleaned
    };

    let truncated: String = cleaned.chars().take(MAX_COMPONENT_CHARS).collect();
    let truncated = truncated.trim_end_matches([' ', '.']).to_string();
    let truncated = if truncated.is_empty() {
        fallback.to_string()
    } else {
        truncated
    };

    if is_windows_reserved(&truncated) {
        format!("_{truncated}")
    } else {
        truncated
    }
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem[3..].chars().all(|c| c.is_ascii_digit()))
}

/// Folder name for a game title: core slug, then filesystem sanitisation.
pub fn slug_for_title(title: &str) -> String {
    let slug = crate::bridge::slugify(title);
    let slug = slug.to_lowercase();
    let slug = sanitize_component(&slug, "game");
    let slug = slug.trim_matches('-').trim_matches('_').to_string();
    if slug.is_empty() {
        "game".to_string()
    } else {
        slug
    }
}

/// Directory name for a version: `version` or `default`.
pub fn version_label(version: Option<&str>) -> String {
    let raw = version.map(str::trim).filter(|value| !value.is_empty());
    let label = sanitize_component(raw.unwrap_or("default"), "default");
    let label = label.trim_matches('.').trim().to_string();
    if label.is_empty() || label == "." || label == ".." {
        "default".to_string()
    } else {
        label
    }
}

/// Join a single, already sanitised component to a base directory.
pub fn safe_child(base: &Path, name: &str) -> Result<PathBuf, String> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) if !component.is_empty() => {
            Ok(base.join(component))
        }
        _ => Err(format!("Refusing unsafe path component: {name}")),
    }
}

/// Canonical containment check. Both paths must exist.
pub fn is_inside(base: &Path, candidate: &Path) -> bool {
    match (base.canonicalize(), candidate.canonicalize()) {
        (Ok(base), Ok(candidate)) => candidate != base && candidate.starts_with(&base),
        _ => false,
    }
}

/// Recursively remove an installed game, refusing anything outside `base`.
pub fn remove_installed_dir(base: &Path, target: &Path) -> Result<(), String> {
    let base = base.canonicalize().map_err(|e| {
        format!(
            "The install folder is not accessible ({}): {e}",
            base.display()
        )
    })?;

    let target = match target.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not resolve {}: {error}", target.display())),
    };

    if target == base || !target.starts_with(&base) {
        return Err(format!(
            "Refusing to delete {} because it is outside the install folder {}.",
            target.display(),
            base.display()
        ));
    }

    if target.is_dir() {
        fs::remove_dir_all(&target)
            .map_err(|e| format!("Could not delete {}: {e}", target.display()))
    } else {
        fs::remove_file(&target).map_err(|e| format!("Could not delete {}: {e}", target.display()))
    }
}

/// Total size of files under `path` (symlinks are not followed).
pub fn dir_size(path: &Path) -> u64 {
    dir_stats(path).0
}

/// `(total bytes, regular file count)` of everything under `path`.
///
/// Symlinks are neither followed nor counted: deletion never follows them and
/// the manifest skips them too.
pub fn dir_stats(path: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.is_dir() {
            if let Ok(entries) = fs::read_dir(&current) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        } else if metadata.is_file() {
            bytes = bytes.saturating_add(metadata.len());
            files += 1;
        }
    }
    (bytes, files)
}

fn copy_dir_inner(
    source: &Path,
    destination: &Path,
    done: &mut u64,
    total: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = destination.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_inner(&from, &to, done, total, on_progress)?;
        } else if file_type.is_file() {
            let copied = fs::copy(&from, &to)?;
            *done = done.saturating_add(copied);
            on_progress(*done, total);
        }
        // Symlinks are skipped: archive extraction is expected to materialise
        // real files, and copying links across devices can escape the target.
    }
    Ok(())
}

/// Recursively copy a directory, reporting copied bytes.
pub fn copy_dir_recursive(
    source: &Path,
    destination: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> io::Result<u64> {
    let total = dir_size(source);
    let mut done = 0u64;
    copy_dir_inner(source, destination, &mut done, total, &mut on_progress)?;
    Ok(done)
}

/// Copy a single file with a 1 MiB buffer, reporting copied bytes. Used for
/// standalone game files (`.exe`, `.sh`, `.AppImage`) so they never go through
/// the archive extractor.
pub fn copy_file(
    source: &Path,
    destination: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> io::Result<u64> {
    use std::io::{BufReader, BufWriter, Read, Write};

    let total = fs::metadata(source)?.len();
    let mut reader = BufReader::with_capacity(1024 * 1024, fs::File::open(source)?);
    let mut writer = BufWriter::with_capacity(1024 * 1024, fs::File::create(destination)?);
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut done = 0u64;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        done = done.saturating_add(read as u64);
        on_progress(done, total);
    }

    writer.flush()?;
    Ok(done)
}

fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn backup_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "game".to_string());
    destination.with_file_name(format!(
        ".{name}.old-{}",
        crate::util::short_id(&uuid::Uuid::new_v4().to_string())
    ))
}

/// Move a staged directory into its final location.
///
/// `fs::rename` is used when possible. When the source and destination are on
/// different devices (or the rename is refused), the tree is copied with
/// progress and the staging folder removed. An existing destination is kept as
/// a backup until the move succeeds.
pub fn move_dir(
    source: &Path,
    destination: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Could not create the install folder {}: {e}",
                parent.display()
            )
        })?;
    }

    let backup = backup_path(destination);
    let had_existing = destination.exists();
    if had_existing {
        fs::rename(destination, &backup).map_err(|e| {
            format!(
                "Could not replace the previous install {}: {e}",
                destination.display()
            )
        })?;
    }

    let result = match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            let total = dir_size(source);
            let mut reported = 0u64;
            copy_dir_recursive(source, destination, |done, _| {
                reported = done;
                on_progress(done, total);
            })
            .map_err(|copy_error| {
                format!(
                    "Could not move files to {} (rename failed: {rename_error}; copy failed: {copy_error})",
                    destination.display()
                )
            })?;
            let _ = reported;
            fs::remove_dir_all(source).map_err(|e| {
                format!(
                    "Files were copied to {} but the staging folder could not be removed: {e}",
                    destination.display()
                )
            })?;
            Ok(())
        }
    };

    match result {
        Ok(()) => {
            if had_existing {
                let _ = remove_path(&backup);
            }
            Ok(())
        }
        Err(error) => {
            if destination.exists() {
                let _ = remove_path(destination);
            }
            if had_existing {
                let _ = fs::rename(&backup, destination);
            }
            Err(error)
        }
    }
}

/// Nearest existing ancestor, used for disk-space checks on not-yet-created dirs.
pub fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            return Some(candidate.to_path_buf());
        }
        current = candidate.parent();
    }
    None
}

/// Remove leftover `.slug.staging-*` folders from installs that were
/// interrupted by an app exit. Safe to call at startup: only hidden staging
/// directories inside the install root are touched.
pub fn cleanup_staging_dirs(install_dir: &Path) {
    let Ok(entries) = fs::read_dir(install_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && name.contains(".staging-") && entry.path().is_dir() {
            if let Err(error) = fs::remove_dir_all(entry.path()) {
                tracing::warn!(path = %entry.path().display(), error = %error, "could not remove stale staging folder");
            }
        }
    }
}

/// Locate an executable on `PATH` (adds `.exe` on Windows).
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = directory.join(format!("{name}.exe"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Split a launch argument string into arguments, honouring simple quoting.
///
/// Backslashes are kept literally so Windows paths survive; only single and
/// double quotes group whitespace.
pub fn split_args(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for character in raw.chars() {
        match character {
            '"' | '\'' => match quote {
                Some(active) if active == character => quote = None,
                None => quote = Some(character),
                Some(_) => current.push(character),
            },
            character if character.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_path_components() {
        assert_eq!(sanitize_component("../evil", "game"), "-evil");
        assert_eq!(sanitize_component("..", "game"), "game");
        assert_eq!(sanitize_component("a/b\\c", "game"), "a-b-c");
        assert_eq!(sanitize_component("CON", "game"), "_CON");
        assert_eq!(sanitize_component("   ", "game"), "game");
        assert_eq!(
            sanitize_component("My Game: Deluxe", "game"),
            "My Game- Deluxe"
        );
    }

    #[test]
    fn safe_child_rejects_nested_paths() {
        let base = Path::new("/tmp/games");
        assert!(safe_child(base, "doom").is_ok());
        assert!(safe_child(base, "../doom").is_err());
        assert!(safe_child(base, "a/b").is_err());
        assert!(safe_child(base, "").is_err());
    }

    #[test]
    fn version_labels_are_safe() {
        assert_eq!(version_label(None), "default");
        assert_eq!(version_label(Some("  ")), "default");
        assert_eq!(version_label(Some("1.2.3")), "1.2.3");
        let traversal = version_label(Some("../../x"));
        assert!(!traversal.contains('/'));
        assert_ne!(traversal, "..");
    }

    #[test]
    fn split_args_handles_quotes() {
        assert_eq!(
            split_args(r#"-windowed "C:\Games\My Game" --x"#),
            vec!["-windowed", r"C:\Games\My Game", "--x"]
        );
    }

    /// #187: `Command::new` fail-fasts (0xc0000409) when handed the verbatim
    /// form of a drive path; the plain form must be used instead.
    #[test]
    fn verbatim_drive_paths_lose_their_prefix() {
        assert_eq!(
            normalize_verbatim_string(r"\\?\C:\Games\GameVault\Uncharted\u4.exe"),
            r"C:\Games\GameVault\Uncharted\u4.exe"
        );
        assert_eq!(normalize_verbatim_string(r"\\?\D:\"), r"D:\");
        assert_eq!(normalize_verbatim_string(r"\\?\d:\games"), r"d:\games");
        // Already plain: unchanged.
        assert_eq!(
            normalize_verbatim_string(r"C:\Games\Game\game.exe"),
            r"C:\Games\Game\game.exe"
        );
    }

    #[test]
    fn verbatim_unc_paths_become_plain_unc() {
        assert_eq!(
            normalize_verbatim_string(r"\\?\UNC\srv\share\games\game.exe"),
            r"\\srv\share\games\game.exe"
        );
        assert_eq!(
            normalize_verbatim_string(r"\\?\unc\srv\share"),
            r"\\srv\share"
        );
        // Malformed (no server/share) and plain UNC paths are untouched.
        assert_eq!(normalize_verbatim_string(r"\\?\UNC\srv"), r"\\?\UNC\srv");
        assert_eq!(
            normalize_verbatim_string(r"\\srv\share\game.exe"),
            r"\\srv\share\game.exe"
        );
    }

    #[test]
    fn long_verbatim_paths_keep_the_prefix() {
        let long_plain = format!(r"C:\Games\{}game.exe", "segment\\".repeat(40));
        assert!(long_plain.chars().count() >= LEGACY_MAX_PATH);
        let verbatim = format!("{VERBATIM_PREFIX}{long_plain}");
        assert_eq!(normalize_verbatim_string(&verbatim), verbatim);

        let long_unc_plain = format!(r"\\srv\share\{}game.exe", "segment\\".repeat(40));
        let verbatim_unc = format!("{VERBATIM_UNC_PREFIX}{}", &long_unc_plain[2..]);
        assert_eq!(
            normalize_verbatim_string(&verbatim_unc),
            verbatim_unc,
            "a >MAX_PATH UNC path must keep its prefix"
        );
    }

    #[test]
    fn unrelated_verbatim_forms_are_left_alone() {
        let volume = r"\\?\Volume{4c1b02c1-0000-0000-0000-100000000000}\Games";
        assert_eq!(normalize_verbatim_string(volume), volume);
        assert_eq!(normalize_verbatim_string(r"\\?\"), r"\\?\");
    }

    #[cfg(not(windows))]
    #[test]
    fn normalize_native_path_is_a_noop_off_windows() {
        let path = Path::new(r"\\?\C:\Games\Game\game.exe");
        assert_eq!(normalize_native_path(path), path);
    }

    #[test]
    fn move_dir_replaces_existing() {
        let root = std::env::temp_dir().join(format!(
            "ludex-move-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        let source = root.join("staging");
        let destination = root.join("final");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("game.exe"), b"new").unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("old.exe"), b"old").unwrap();

        move_dir(&source, &destination, |_, _| {}).unwrap();
        assert!(destination.join("game.exe").exists());
        assert!(!destination.join("old.exe").exists());
        assert!(!source.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
