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
pub mod models;
pub mod naming;
pub mod slug;

pub use archive::{
    detect_format, extract_archive, is_archive_path, sanitize_entry_path, verify_archive,
    ArchiveError, ArchiveFormat, ArchiveReport, ExtractOptions, ExtractProgress, ExtractReport,
};
pub use exe::{is_installer, pick_executable};
pub use models::{
    GameDetail, GameSummary, GameVersion, LaunchInfo, NewsPost, Paginated, ProgressState,
    ProgressUpdate, ProviderRef, Role, Stats, Trailer, User,
};
pub use naming::{grouping_key, normalize_title, parse_filename, ParsedName};
pub use slug::slugify;
