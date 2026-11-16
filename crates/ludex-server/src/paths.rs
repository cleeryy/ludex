//! Confined path construction for on-disk storage (#266).
//!
//! Every path the server derives from a request is built here instead of with a
//! bare `Path::join`. Two properties are enforced:
//!
//! * [`component`] accepts one directory entry only — no separators, no `.` or
//!   `..`, no control characters, bounded length — so `join` cannot traverse.
//! * [`ensure_within`] checks the result is `base` or a descendant of it, and
//!   [`canonical_within`] repeats the check after resolving symlinks when the
//!   path (or its deepest existing ancestor) is on disk.
//!
//! The tests at the bottom of this module are the evidence behind every
//! `// codeql[rust/path-injection]` suppression in the upload and save code:
//! a separator-bearing identifier is rejected before it reaches the filesystem.

use std::path::{Path, PathBuf};

/// Longest accepted identifier. A UUID is 36 characters; a sanitised archive
/// name is capped at 200. 255 is the per-entry limit on ext4/APFS.
const MAX_COMPONENT_CHARS: usize = 255;

/// Why a path was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    /// The value is empty, `.`, `..`, overlong, or contains a separator or a
    /// control character.
    NotAComponent,
    /// The joined path does not stay under its root.
    EscapesRoot,
    /// The path exists but could not be canonicalised, or canonicalisation put
    /// it outside the root (a symlink pointing elsewhere).
    Unresolvable,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAComponent => formatter.write_str("path component is not a single entry"),
            Self::EscapesRoot => formatter.write_str("path escapes its storage root"),
            Self::Unresolvable => formatter.write_str("path cannot be resolved inside its root"),
        }
    }
}

impl std::error::Error for PathError {}

impl From<PathError> for crate::error::ApiError {
    fn from(error: PathError) -> Self {
        // A row whose identifier or file name is not a single component can
        // only come from a bug or a tampered database. Refuse it instead of
        // touching the filesystem, and keep the response generic.
        tracing::warn!(error = %error, "rejected an out-of-root storage path");
        Self::bad_request("invalid_path", "The stored path is not valid")
    }
}

/// Returns `raw` when it is exactly one safe path component.
pub fn component(raw: &str) -> Result<&str, PathError> {
    if raw.is_empty() || raw.chars().count() > MAX_COMPONENT_CHARS {
        return Err(PathError::NotAComponent);
    }
    if raw == "." || raw == ".." {
        return Err(PathError::NotAComponent);
    }
    if raw
        .chars()
        .any(|character| character == '/' || character == '\\' || character == '\0')
    {
        return Err(PathError::NotAComponent);
    }
    if raw.chars().any(char::is_control) {
        return Err(PathError::NotAComponent);
    }
    Ok(raw)
}

/// Joins one validated component onto `base`.
pub fn child(base: &Path, raw: &str) -> Result<PathBuf, PathError> {
    let name = component(raw)?;
    let path = base.join(name);
    ensure_within(base, &path)?;
    Ok(path)
}

/// Joins a sequence of validated components onto `base`.
pub fn nested(base: &Path, parts: &[&str]) -> Result<PathBuf, PathError> {
    let mut path = base.to_path_buf();
    for part in parts {
        path = child(&path, part)?;
    }
    Ok(path)
}

/// Lexical containment check: `candidate` must be `base` or below it.
///
/// `..` segments are resolved lexically first so `{base}/a/../b` is compared as
/// `{base}/b` rather than as a string prefix.
pub fn ensure_within(base: &Path, candidate: &Path) -> Result<(), PathError> {
    let base = lexical(base);
    let candidate = lexical(candidate);
    if candidate == base || candidate.starts_with(&base) {
        Ok(())
    } else {
        Err(PathError::EscapesRoot)
    }
}

/// Canonicalises `candidate` (or its deepest existing ancestor) and confirms it
/// is still under `base`.
///
/// Call this before a create/write/rename/remove on a path built from a
/// client-supplied name, so a symlink planted inside the data directory cannot
/// redirect the write outside it.
pub async fn canonical_within(base: &Path, candidate: &Path) -> Result<PathBuf, PathError> {
    ensure_within(base, candidate)?;
    let canonical_base = match tokio::fs::canonicalize(base).await {
        Ok(path) => path,
        // The root is created by the first upload; a missing root cannot
        // contain anything, so the lexical check above is the whole answer.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(candidate.to_path_buf())
        }
        Err(_) => return Err(PathError::Unresolvable),
    };

    let mut existing = candidate;
    let canonical_candidate = loop {
        match tokio::fs::canonicalize(existing).await {
            Ok(path) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Walk up to the first ancestor that exists: a file about to be
                // created lives inside a directory that does exist.
                match existing.parent() {
                    Some(parent) if parent != existing => existing = parent,
                    _ => return Ok(candidate.to_path_buf()),
                }
            }
            Err(_) => return Err(PathError::Unresolvable),
        }
    };

    if canonical_candidate == canonical_base || canonical_candidate.starts_with(&canonical_base) {
        Ok(candidate.to_path_buf())
    } else {
        Err(PathError::EscapesRoot)
    }
}

/// Resolves `.` and `..` without touching the filesystem.
fn lexical(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_reject_traversal() {
        for raw in [
            "",
            ".",
            "..",
            "../etc",
            "a/b",
            "a\\b",
            "/abs",
            "C:\\x",
            "bad\0name",
            "line\nbreak",
        ] {
            assert_eq!(
                component(raw),
                Err(PathError::NotAComponent),
                "{raw:?} must be refused"
            );
        }
        assert_eq!(component(&"a".repeat(256)), Err(PathError::NotAComponent));
    }

    #[test]
    fn components_accept_server_identifiers() {
        let uuid = "2f8d7e5a-6a0e-4c0f-9d1f-3c6e4b0a1f22";
        assert_eq!(component(uuid), Ok(uuid));
        assert_eq!(component("42"), Ok("42"));
        assert_eq!(
            component("Hades save (2020).zip"),
            Ok("Hades save (2020).zip")
        );
    }

    #[test]
    fn child_stays_under_the_root() {
        let root = Path::new("/srv/ludex/data/uploads");
        assert_eq!(child(root, "abc").expect("child"), root.join("abc"));
        assert_eq!(child(root, "../etc"), Err(PathError::NotAComponent));
        assert_eq!(child(root, ".."), Err(PathError::NotAComponent));
        assert_eq!(
            nested(root, &["1", "2"]).expect("nested"),
            root.join("1").join("2")
        );
        assert_eq!(nested(root, &["1", "../2"]), Err(PathError::NotAComponent));
    }

    #[test]
    fn ensure_within_resolves_dot_segments_lexically() {
        let root = Path::new("/srv/ludex/data/games");
        assert!(ensure_within(root, &root.join("game.zip")).is_ok());
        assert!(ensure_within(root, &root.join("a/../game.zip")).is_ok());
        assert_eq!(
            ensure_within(root, &root.join("../../etc/passwd")),
            Err(PathError::EscapesRoot)
        );
        assert_eq!(
            ensure_within(root, Path::new("/srv/ludex/data/other")),
            Err(PathError::EscapesRoot)
        );
    }

    #[tokio::test]
    async fn canonical_within_accepts_fresh_files_and_rejects_escaping_symlinks() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data");
        tokio::fs::create_dir_all(&root).await.expect("mkdir");

        // A destination that does not exist yet, inside an existing root.
        let fresh = root.join("game.zip");
        assert_eq!(canonical_within(&root, &fresh).await.expect("fresh"), fresh);

        // A nested directory that does not exist yet either.
        let nested_dir = root.join("uploads/abc");
        tokio::fs::create_dir_all(&nested_dir).await.expect("mkdir");
        assert_eq!(
            canonical_within(&root, &nested_dir.join("0.part"))
                .await
                .expect("part"),
            nested_dir.join("0.part")
        );

        // A symlink inside the root pointing outside it must be refused.
        #[cfg(unix)]
        {
            let outside = temp.path().join("outside");
            tokio::fs::create_dir_all(&outside).await.expect("mkdir");
            let link = root.join("escape");
            std::os::unix::fs::symlink(&outside, &link).expect("symlink");
            assert_eq!(
                canonical_within(&root, &link.join("payload")).await,
                Err(PathError::EscapesRoot)
            );
        }
    }
}
