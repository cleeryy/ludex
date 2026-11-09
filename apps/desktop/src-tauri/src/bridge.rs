//! Thin adapter over `ludex-core` and `ludex-server`.
//!
//! Everything the desktop app needs from the shared crates goes through this
//! module so that API adjustments stay in one place.

use std::path::{Path, PathBuf};

/// Folder-name slug for a title (`ludex_core::slug::slugify`).
pub fn slugify(value: &str) -> String {
    ludex_core::slug::slugify(value)
}

/// Best executable inside an extracted game tree.
pub fn pick_executable(root: &Path, title: &str) -> Option<PathBuf> {
    ludex_core::exe::pick_executable(root, title)
}

/// Extract an archive into `destination`.
///
/// `progress` receives `(entries_done, entries_total, bytes_done, bytes_total)`.
/// Configured passwords are handed to the core extractor, which also tries the
/// empty password as a fallback.
pub fn extract_archive(
    archive: &Path,
    destination: &Path,
    passwords: &[String],
    progress: &mut dyn FnMut(u64, Option<u64>, u64, Option<u64>),
) -> Result<(), String> {
    use ludex_core::archive::{extract_archive, ExtractOptions, ExtractProgress};

    let options = ExtractOptions::with_passwords(passwords.to_vec());

    let mut callback = |update: ExtractProgress| {
        progress(
            update.entries_done as u64,
            Some(update.entries_total as u64),
            update.bytes_done,
            Some(update.bytes_total),
        );
    };

    extract_archive(archive, destination, &options, &mut callback)
        .map(|_report| ())
        .map_err(|error| format!("Could not extract {}: {error}", archive.display()))
}
