//! Shared domain layer for Ludex.
//!
//! `ludex-core` contains everything the server and the desktop app share, and
//! nothing that depends on a database, HTTP or an async runtime:
//!
//! - [`models`]: serde types for the JSON API (camelCase).
//! - [`naming`]: GameVault-compatible filename parser.
//! - [`slug`]: slug generation.
//! - [`archive`]: ZIP/RAR/7z detection, verification and streaming extraction.
//! - [`exe`]: game executable discovery after extraction.
//! - [`fingerprint`]: cheap archive fingerprints used for update detection.
//!
//! ```
//! use ludex_core::naming::parse_filename;
//!
//! let parsed = parse_filename("HITMAN 3 (v3.10.1) (2021).7z");
//! assert_eq!(parsed.title, "HITMAN 3");
//! assert_eq!(parsed.version.as_deref(), Some("3.10.1"));
//! assert_eq!(parsed.year, Some(2021));
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod archive;
pub mod exe;
pub mod fingerprint;
pub mod gamepad;
pub mod models;
pub mod naming;
pub mod rom;
pub mod slug;

pub use archive::{
    archive_tar_size, detect_format, extract_archive, is_archive_path, sanitize_entry_path,
    stream_archive_as_tar, stream_zip_as_tar, verify_archive, write_zip_entry_range,
    zip_entry_info, zip_stream_manifest, zip_tar_size, ArchiveError, ArchiveFormat, ArchiveReport,
    ExtractOptions, ExtractProgress, ExtractReport, StreamEntry, StreamManifest, TarStreamReport,
};
pub use exe::{
    clamp_scan_depth, discover_executables, is_excluded_folder, is_installer, pick_executable,
    pick_executable_with_depth, Discovery, ExecutableCandidate, DEFAULT_SCAN_DEPTH, MAX_SCAN_DEPTH,
    MIN_SCAN_DEPTH,
};
pub use fingerprint::{archive_fingerprint, is_fingerprint, FINGERPRINT_PREFIX};
pub use models::{
    GameDetail, GameSummary, GameVersion, LaunchInfo, NewsPost, Paginated, ProgressState,
    ProgressUpdate, ProviderRef, Role, Stats, Trailer, User,
};
pub use naming::{grouping_key, normalize_title, parse_filename, ParsedName};
pub use slug::slugify;
