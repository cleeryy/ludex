//! WebView2 profile reset (#177): a Windows update must show the new web UI.
//!
//! Tauri on Windows runs inside WebView2, whose profile lives in
//! `%LOCALAPPDATA%\<identifier>\EBWebView` and survives an NSIS update. Older
//! builds registered the PWA service worker and cached the shell there, so a
//! freshly updated binary could keep loading the previous `index.html` and
//! assets. macOS stores nothing comparable in WKWebView, which is why only
//! Windows showed the stale UI.
//!
//! [`maybe_reset`] runs from `run()` before `tauri::Builder` is built, so the
//! webview has not opened the profile yet. It compares the running version
//! with `<root>/webui-version` and, on a change, deletes the WebView2 caches
//! that can hold the old frontend before recording the new version. Local
//! Storage, cookies and Network are left untouched: the desktop token is
//! restored from the settings/keychain and the theme and locale must survive.
//!
//! The reset is best-effort. A directory that cannot be removed logs a warning,
//! keeps the old marker and is retried on the next launch; it never fails
//! startup.

// Only Windows runs the reset; the tests exercise it on every platform.
#![cfg_attr(not(windows), allow(dead_code))]

use std::io::ErrorKind;
use std::path::Path;

/// Marker file under the WebView2 root, holding the version of the last reset.
pub const MARKER_FILE: &str = "webui-version";

/// `EBWebView/Default` directories that may serve a previous frontend. Local
/// Storage and Network stay, so the signed-in account and the theme survive.
const STALE_DIRS: [&str; 3] = ["Service Worker", "Cache", "Code Cache"];

/// What [`reset_if_stale`] did; mainly for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetOutcome {
    /// The marker already holds the running version; nothing was touched.
    Current,
    /// The caches were deleted and the marker rewritten.
    Reset,
    /// A directory could not be removed (or the marker could not be written);
    /// the next launch retries.
    Failed,
}

/// Delete the WebView2 caches when `version` differs from the marker content.
///
/// `root` is the directory holding `EBWebView` (the app passes
/// `%LOCALAPPDATA%\<identifier>`) and `marker` is normally
/// `root/webui-version`. A missing cache directory counts as success, so a
/// first launch without a profile still records the version. On any other
/// error the marker is not written, keeping the old version for the retry.
pub fn reset_if_stale(root: &Path, version: &str, marker: &Path) -> ResetOutcome {
    let previous = std::fs::read_to_string(marker).ok();
    if previous.as_deref().map(str::trim) == Some(version) {
        return ResetOutcome::Current;
    }

    let default = root.join("EBWebView").join("Default");
    let mut failed = false;
    for name in STALE_DIRS {
        let path = default.join(name);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                // Locked by a running WebView2, or a file where a directory is
                // expected: the marker stays on the old version.
                tracing::warn!(
                    error = %error,
                    path = %path.display(),
                    "could not delete a WebView2 cache"
                );
                failed = true;
            }
        }
    }
    if failed {
        return ResetOutcome::Failed;
    }

    // The profile root may not exist yet on a first launch.
    if let Some(parent) = marker.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                error = %error,
                path = %parent.display(),
                "could not create the WebView2 profile root"
            );
            return ResetOutcome::Failed;
        }
    }
    if let Err(error) = std::fs::write(marker, version) {
        tracing::warn!(
            error = %error,
            path = %marker.display(),
            "could not record the WebView2 web UI version"
        );
        return ResetOutcome::Failed;
    }

    tracing::info!(version, "reset the WebView2 profile after a version change");
    ResetOutcome::Reset
}

/// Reset the profile described by the build-time `context`.
///
/// Called before `tauri::Builder` so no window holds the profile yet. Only
/// Windows has a persistent WebView2 profile; the other platforms are a no-op.
#[cfg(windows)]
pub fn maybe_reset<R: tauri::Runtime>(context: &tauri::Context<R>) {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        tracing::warn!("LOCALAPPDATA is not set; skipping the WebView2 profile reset");
        return;
    };
    let root = std::path::PathBuf::from(local_app_data).join(context.config().identifier.as_str());
    let version = context.package_info().version.to_string();
    let marker = root.join(MARKER_FILE);
    reset_if_stale(&root, &version, &marker);
}

/// WebView2 only exists on Windows.
#[cfg(not(windows))]
pub fn maybe_reset<R: tauri::Runtime>(_context: &tauri::Context<R>) {}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ludex-webview-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Create `EBWebView/Default/<name>/stale` so a deletion is observable.
    fn cache_dir(root: &Path, name: &str) {
        let dir = root.join("EBWebView").join("Default").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stale"), b"old").unwrap();
    }

    #[test]
    fn unchanged_version_keeps_the_profile() {
        let root = temp_root("current");
        cache_dir(&root, "Cache");
        let marker = root.join(MARKER_FILE);
        std::fs::write(&marker, "0.4.0").unwrap();

        assert_eq!(
            reset_if_stale(&root, "0.4.0", &marker),
            ResetOutcome::Current
        );
        assert!(root.join("EBWebView/Default/Cache/stale").exists());
    }

    #[test]
    fn version_change_drops_the_caches_and_keeps_local_storage() {
        let root = temp_root("changed");
        for name in STALE_DIRS {
            cache_dir(&root, name);
        }
        // Survives the reset: account, theme and locale live here.
        cache_dir(&root, "Local Storage");
        let marker = root.join(MARKER_FILE);
        std::fs::write(&marker, "0.3.0").unwrap();

        assert_eq!(reset_if_stale(&root, "0.4.0", &marker), ResetOutcome::Reset);
        for name in STALE_DIRS {
            assert!(
                !root.join("EBWebView").join("Default").join(name).exists(),
                "{name} should be gone"
            );
        }
        assert!(root.join("EBWebView/Default/Local Storage/stale").exists());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "0.4.0");
    }

    #[test]
    fn missing_dirs_still_record_the_version() {
        let root = temp_root("missing");
        let marker = root.join(MARKER_FILE);

        assert_eq!(reset_if_stale(&root, "0.4.0", &marker), ResetOutcome::Reset);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "0.4.0");
    }

    #[test]
    fn failed_delete_leaves_the_old_marker() {
        let root = temp_root("failed");
        let default = root.join("EBWebView").join("Default");
        std::fs::create_dir_all(&default).unwrap();
        // A file where a cache directory is expected: `remove_dir_all` cannot
        // delete it, on every platform.
        std::fs::write(default.join("Cache"), b"not a directory").unwrap();
        let marker = root.join(MARKER_FILE);
        std::fs::write(&marker, "0.3.0").unwrap();

        assert_eq!(
            reset_if_stale(&root, "0.4.0", &marker),
            ResetOutcome::Failed
        );
        // Keeping the old version makes the next launch retry the reset.
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "0.3.0");
        assert!(default.join("Cache").exists());
    }
}
