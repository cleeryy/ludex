//! Thin adapter over `ludex-core` and `ludex-server`.
//!
//! Everything the desktop app needs from the shared crates goes through this
//! module so that API adjustments stay in one place.

use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

/// True for `Redist`, `Support`, `Tools` and similar folders (#134, #143).
pub use ludex_core::exe::is_excluded_folder;
/// Default folder depth for GameVault-style scans (#134).
pub use ludex_core::exe::DEFAULT_SCAN_DEPTH;
/// Read-only executable discovery result (#134).
pub use ludex_core::exe::{clamp_scan_depth, discover_executables, Discovery};

/// Folder-name slug for a title (`ludex_core::slug::slugify`).
pub fn slugify(value: &str) -> String {
    ludex_core::slug::slugify(value)
}

/// Best executable inside an extracted game tree.
pub fn pick_executable(root: &Path, title: &str) -> Option<PathBuf> {
    ludex_core::exe::pick_executable(root, title)
}

/// One extraction progress snapshot handed to the desktop UI (#232).
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtractUpdate {
    /// Files fully written.
    pub entries_done: u64,
    /// Files in the archive.
    pub entries_total: u64,
    /// Bytes written.
    pub bytes_done: u64,
    /// Declared uncompressed bytes.
    pub bytes_total: u64,
    /// Average bytes per second since the extraction started.
    pub bytes_per_second: u64,
    /// Seconds left for the declared bytes, when known.
    pub eta_seconds: Option<u64>,
}

/// Extract an archive into `destination`.
///
/// `progress` receives one [`ExtractUpdate`] per callback, throttled by the
/// core extractor (about every 1 MiB or 100 ms). `cancel` stops the extraction
/// between entries and every 1 MiB chunk; the staging directory is left in
/// place so the caller can decide what to keep. Configured passwords are
/// handed to the core extractor, which also tries the empty password as a
/// fallback.
pub fn extract_archive(
    archive: &Path,
    destination: &Path,
    passwords: &[String],
    cancel: Option<CancellationToken>,
    progress: &mut dyn FnMut(ExtractUpdate),
) -> Result<(), String> {
    use ludex_core::archive::{extract_archive, CancelCheck, ExtractOptions, ExtractProgress};

    let mut options = ExtractOptions::with_passwords(passwords.to_vec());
    if let Some(token) = cancel {
        options.cancel = Some(CancelCheck::new(move || token.is_cancelled()));
    }

    let mut callback = |update: ExtractProgress| {
        progress(ExtractUpdate {
            entries_done: update.entries_done as u64,
            entries_total: update.entries_total as u64,
            bytes_done: update.bytes_done,
            bytes_total: update.bytes_total,
            bytes_per_second: update.bytes_per_second,
            eta_seconds: update.eta_seconds,
        });
    };

    extract_archive(archive, destination, &options, &mut callback)
        .map(|_report| ())
        .map_err(|error| format!("Could not extract {}: {error}", archive.display()))
}
