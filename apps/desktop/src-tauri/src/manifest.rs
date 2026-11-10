//! Installed-file manifests: build, persist and verify.
//!
//! A manifest records every file of an installed game (relative path, size,
//! SHA-256). `verify_install` walks the install folder and compares it with the
//! manifest to report missing, corrupt and extra files; `repair_install` uses
//! the same paths to restore only what is broken.
//!
//! The manifest is stored in the desktop SQLite index, not on disk, so it never
//! shows up as an extra file and moving an install only updates one row.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::util::now_rfc3339;

/// One file in an install. `path` is relative to the install folder and uses
/// `/` separators on every platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileManifest {
    pub entries: Vec<ManifestEntry>,
    pub total_bytes: u64,
    pub created_at: String,
}

/// Progress for manifest builds and verification. `filesTotal`/`bytesTotal`
/// are known up front; `bytesDone` only counts bytes actually hashed.
#[derive(Debug, Clone, Copy, Default)]
pub struct ManifestProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyIssue {
    pub path: String,
    pub expected_size: u64,
    pub actual_size: Option<u64>,
    pub expected_sha256: String,
    pub actual_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyExtra {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyStatus {
    Ok,
    Issues,
    NoManifest,
}

/// Mirrors `VerifyReport` from `docs/CONTRACT.md`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub game_id: i64,
    pub status: VerifyStatus,
    pub files_checked: u64,
    pub bytes_checked: u64,
    pub missing: Vec<VerifyIssue>,
    pub corrupt: Vec<VerifyIssue>,
    pub extra: Vec<VerifyExtra>,
    pub checked_at: String,
    pub message: Option<String>,
}

/// Result of comparing an install folder with its manifest (no Tauri types, so
/// the command wrapper and the tests share the same code).
#[derive(Debug, Clone, Default)]
pub struct VerifyOutcome {
    pub missing: Vec<VerifyIssue>,
    pub corrupt: Vec<VerifyIssue>,
    pub extra: Vec<VerifyExtra>,
    pub files_checked: u64,
    pub bytes_checked: u64,
}

impl VerifyOutcome {
    pub fn has_damage(&self) -> bool {
        !self.missing.is_empty() || !self.corrupt.is_empty()
    }

    pub fn into_report(self, game_id: i64, message: Option<String>) -> VerifyReport {
        let status = if self.has_damage() {
            VerifyStatus::Issues
        } else {
            VerifyStatus::Ok
        };
        VerifyReport {
            game_id,
            status,
            files_checked: self.files_checked,
            bytes_checked: self.bytes_checked,
            missing: self.missing,
            corrupt: self.corrupt,
            extra: self.extra,
            checked_at: now_rfc3339(),
            message,
        }
    }
}

/// SHA-256 of a file, read in 1 MiB chunks so multi-gigabyte files never land
/// in memory. Returns `(size, lowercase hex digest)`.
pub fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut size = 0u64;

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }

    Ok((size, hex::encode(hasher.finalize())))
}

/// Convert a path under `root` to a `/`-separated relative path.
pub fn relative_slash(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Join a manifest-relative path to a base folder, rejecting absolute paths,
/// `..`, drive letters and Windows separators.
pub fn join_relative(base: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains('\\') || relative.contains(':') {
        return None;
    }
    let mut result = base.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        result.push(part);
    }
    Some(result)
}

/// Walk every regular file under `root` as `(relative path, size)`.
///
/// Symlinks are skipped: the extractor materialises real files, and following
/// links could escape the install folder.
pub fn walk_files(root: &Path) -> Result<Vec<(String, u64)>, String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(current) = stack.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && current == root => {
                return Ok(files)
            }
            Err(error) => {
                return Err(format!(
                    "Could not read the install folder {}: {error}",
                    current.display()
                ))
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                if let Some(relative) = relative_slash(root, &path) {
                    files.push((relative, metadata.len()));
                }
            }
        }
    }

    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Hash every file of an install folder and build its manifest.
pub fn build_manifest(
    root: &Path,
    progress: &mut dyn FnMut(ManifestProgress),
) -> Result<FileManifest, String> {
    let files = walk_files(root)?;
    let files_total = files.len() as u64;
    let bytes_total = files.iter().map(|(_, size)| *size).sum::<u64>();
    let mut entries = Vec::with_capacity(files.len());
    let mut bytes_done = 0u64;

    for (index, (path, size)) in files.iter().enumerate() {
        let full = join_relative(root, path)
            .ok_or_else(|| format!("Refusing an unsafe manifest path: {path}"))?;
        let (hashed_size, sha256) = hash_file(&full)?;
        bytes_done = bytes_done.saturating_add(hashed_size);
        entries.push(ManifestEntry {
            path: path.clone(),
            size: hashed_size,
            sha256,
        });
        progress(ManifestProgress {
            files_done: index as u64 + 1,
            files_total,
            bytes_done,
            bytes_total,
        });
        // A file that changed while hashing is not a stable manifest entry.
        if hashed_size != *size {
            tracing::warn!(path = %path, "file changed while building the manifest");
        }
    }

    Ok(FileManifest {
        entries,
        total_bytes: bytes_done,
        created_at: now_rfc3339(),
    })
}

/// Compare an install folder with its manifest.
///
/// Sizes are checked first; only files with a matching size are hashed. Extra
/// files are reported but do not make the install "damaged" on their own.
pub fn verify_tree(
    root: &Path,
    manifest: &FileManifest,
    progress: &mut dyn FnMut(ManifestProgress),
) -> Result<VerifyOutcome, String> {
    let on_disk = walk_files(root)?;
    let mut disk: std::collections::HashMap<String, u64> = on_disk.into_iter().collect();

    let files_total = manifest.entries.len() as u64;
    let bytes_total = manifest.total_bytes;
    let mut outcome = VerifyOutcome::default();
    let mut bytes_done = 0u64;

    for (index, entry) in manifest.entries.iter().enumerate() {
        let actual_size = disk.remove(&entry.path);
        match actual_size {
            None => outcome.missing.push(VerifyIssue {
                path: entry.path.clone(),
                expected_size: entry.size,
                actual_size: None,
                expected_sha256: entry.sha256.clone(),
                actual_sha256: None,
            }),
            Some(size) if size != entry.size => outcome.corrupt.push(VerifyIssue {
                path: entry.path.clone(),
                expected_size: entry.size,
                actual_size: Some(size),
                expected_sha256: entry.sha256.clone(),
                actual_sha256: None,
            }),
            Some(size) => {
                let full = join_relative(root, &entry.path)
                    .ok_or_else(|| format!("Refusing an unsafe manifest path: {}", entry.path))?;
                match hash_file(&full) {
                    Ok((hashed_size, sha256)) => {
                        bytes_done = bytes_done.saturating_add(hashed_size);
                        outcome.files_checked += 1;
                        outcome.bytes_checked = bytes_done;
                        if hashed_size != entry.size || sha256 != entry.sha256 {
                            outcome.corrupt.push(VerifyIssue {
                                path: entry.path.clone(),
                                expected_size: entry.size,
                                actual_size: Some(hashed_size),
                                expected_sha256: entry.sha256.clone(),
                                actual_sha256: Some(sha256),
                            });
                        }
                    }
                    Err(error) => {
                        // A file that cannot be read is corrupt from the user's
                        // point of view; keep verifying the rest of the install.
                        tracing::warn!(error = %error, path = %entry.path, "could not read an installed file");
                        outcome.corrupt.push(VerifyIssue {
                            path: entry.path.clone(),
                            expected_size: entry.size,
                            actual_size: Some(size),
                            expected_sha256: entry.sha256.clone(),
                            actual_sha256: None,
                        });
                    }
                }
            }
        }

        progress(ManifestProgress {
            files_done: index as u64 + 1,
            files_total,
            bytes_done,
            bytes_total,
        });
    }

    // Whatever the manifest did not list is extra.
    let mut extra: Vec<VerifyExtra> = disk
        .into_iter()
        .map(|(path, size)| VerifyExtra { path, size })
        .collect();
    extra.sort_by(|left, right| left.path.cmp(&right.path));
    outcome.extra = extra;

    Ok(outcome)
}

/// Read-only plan used by repair: the manifest entries that must be restored.
pub fn damaged_entries(outcome: &VerifyOutcome) -> Vec<ManifestEntry> {
    let mut entries: Vec<ManifestEntry> = outcome
        .missing
        .iter()
        .chain(outcome.corrupt.iter())
        .map(|issue| ManifestEntry {
            path: issue.path.clone(),
            size: issue.expected_size,
            sha256: issue.expected_sha256.clone(),
        })
        .collect();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-manifest-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let root = temp_root("json");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("game.exe"), b"MZ-game").unwrap();
        fs::write(root.join("bin/data.bin"), vec![7u8; 4096]).unwrap();

        let manifest = build_manifest(&root, &mut |_| {}).unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(manifest.total_bytes, 7 + 4096);
        assert_eq!(manifest.entries[0].path, "bin/data.bin");

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: FileManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.entries, manifest.entries);
        assert_eq!(parsed.total_bytes, manifest.total_bytes);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_status_serialises_for_the_ui() {
        let ok = VerifyOutcome::default().into_report(1, None);
        assert_eq!(serde_json::to_value(&ok).unwrap()["status"], "ok");

        let mut damaged = VerifyOutcome::default();
        damaged.missing.push(VerifyIssue {
            path: "a.bin".into(),
            expected_size: 1,
            actual_size: None,
            expected_sha256: "0".into(),
            actual_sha256: None,
        });
        let issues = damaged.into_report(1, None);
        assert_eq!(serde_json::to_value(&issues).unwrap()["status"], "issues");

        let no_manifest = VerifyReport {
            game_id: 1,
            status: VerifyStatus::NoManifest,
            files_checked: 0,
            bytes_checked: 0,
            missing: Vec::new(),
            corrupt: Vec::new(),
            extra: Vec::new(),
            checked_at: "2026-01-01T00:00:00Z".into(),
            message: None,
        };
        assert_eq!(
            serde_json::to_value(&no_manifest).unwrap()["status"],
            "noManifest"
        );
    }

    #[test]
    fn verification_detects_missing_corrupt_and_extra_files() {
        let root = temp_root("verify");
        fs::write(root.join("a.bin"), vec![1u8; 1024]).unwrap();
        fs::write(root.join("b.bin"), vec![2u8; 2048]).unwrap();
        fs::write(root.join("c.bin"), b"hello").unwrap();

        let manifest = build_manifest(&root, &mut |_| {}).unwrap();
        let outcome = verify_tree(&root, &manifest, &mut |_| {}).unwrap();
        assert!(!outcome.has_damage());
        assert_eq!(outcome.files_checked, 3);
        assert!(outcome.extra.is_empty());

        // Truncate one file, flip a byte in another, delete the third.
        fs::write(root.join("a.bin"), vec![1u8; 512]).unwrap();
        let mut data = fs::read(root.join("b.bin")).unwrap();
        data[100] ^= 0xff;
        fs::write(root.join("b.bin"), data).unwrap();
        fs::remove_file(root.join("c.bin")).unwrap();
        fs::write(root.join("save.dat"), b"user save").unwrap();

        let outcome = verify_tree(&root, &manifest, &mut |_| {}).unwrap();
        assert!(outcome.has_damage());
        assert_eq!(outcome.missing.len(), 1);
        assert_eq!(outcome.missing[0].path, "c.bin");
        assert_eq!(outcome.corrupt.len(), 2);
        assert!(outcome.corrupt.iter().any(|issue| issue.path == "a.bin"));
        assert!(outcome.corrupt.iter().any(|issue| issue.path == "b.bin"));
        assert_eq!(outcome.extra.len(), 1);
        assert_eq!(outcome.extra[0].path, "save.dat");

        let damaged = damaged_entries(&outcome);
        assert_eq!(damaged.len(), 3);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_paths_are_guarded() {
        let base = Path::new("/games/doom");
        assert_eq!(
            join_relative(base, "bin/game.sh"),
            Some(PathBuf::from("/games/doom/bin/game.sh"))
        );
        assert!(join_relative(base, "../escape").is_none());
        assert!(join_relative(base, "a/../../escape").is_none());
        assert!(join_relative(base, "/absolute").is_none());
        assert!(join_relative(base, "C:/windows").is_none());
        assert!(join_relative(base, "a\\b").is_none());
        assert!(join_relative(base, "").is_none());
    }

    #[test]
    fn manifest_reports_progress() {
        let root = temp_root("progress");
        fs::write(root.join("one.bin"), vec![0u8; 100]).unwrap();
        fs::write(root.join("two.bin"), vec![0u8; 200]).unwrap();

        let mut seen = Vec::new();
        let manifest = build_manifest(&root, &mut |progress| seen.push(progress)).unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(seen.len(), 2);
        assert_eq!(seen.last().unwrap().files_total, 2);
        assert_eq!(seen.last().unwrap().bytes_done, 300);

        let _ = fs::remove_dir_all(&root);
    }
}
