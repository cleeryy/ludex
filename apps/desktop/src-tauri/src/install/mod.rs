//! Install pipeline: resolve the archive or file manifest, download with
//! resume, extract to a staging folder, verify the tree, move it into place and
//! index the result.
//!
//! Jobs are persisted in SQLite so an interrupted download stays visible after
//! a restart and the next attempt resumes the `.part` file with HTTP Range.
//! Only one extract/commit runs at a time; downloads may run ahead of the
//! extractor (`prefetching`).
//!
//! # Module map
//!
//! | Module | Owns |
//! |---|---|
//! | [`job`] | Value types: `InstallJob`, `InstallParams`, phases, queue moves, `Failure`. No I/O. |
//! | [`plan`] | Pure queue/path planning: candidates, positions, `.part`/staging paths, staging ownership. |
//! | [`scheduler`] | Job registry: begin/dispatch, pause/resume/cancel, worker lifecycle, finalizers, SQLite persistence, startup resume. |
//! | [`source`] | Transport choice: local library shortcut, ZIP stream and its long-horizon retry. |
//! | [`download`] | Resumable archive transfer, retry loop, disk preflight, HTTP adapter. |
//! | [`files`] | File-manifest transport (#231): parallel per-file download, verification and checkpoints. |
//! | [`verify`] | Server-published archive digest, one-pass on-disk verification, space preflight. |
//! | [`commit`] | Commit stages: layout, transport stages, extract/copy, move, manifest, index. |
//!
//! Transport-neutral primitives live in [`crate::net`] (`RetryPolicy`,
//! `ProgressSink`, `CancelSet`). The public command surface is re-exported here
//! unchanged, so `commands.rs` keeps calling `install::begin`, `install::list`,
//! etc.

mod commit;
mod download;
mod files;
mod job;
mod plan;
mod scheduler;
mod source;
mod verify;

pub(crate) use job::{Failure, JobPaths, INTERRUPTED_MESSAGE};
pub use job::{InstallJob, InstallParams, InstallPhase, QueueMove, ResumeMode};
#[cfg(test)]
pub(crate) use job::{LEGACY_INTERRUPTED_MESSAGE, RESUME_DISABLED_MESSAGE};

pub(crate) use download::{download_file, Throttle};
/// Compatibility alias: the retry policy used to be called `DownloadTuning`
/// throughout the install pipeline.
pub(crate) type DownloadTuning<'a> = crate::net::retry::RetryPolicy<'a>;

pub(crate) use crate::net::progress::DownloadProgress;

pub(crate) use scheduler::{
    auto_resume_on_startup, clear_resume_markers_on_clean_exit, dispatch, persist,
    rebuild_space_ledger, sweep_stale_staging,
};
pub use scheduler::{
    begin, cancel, check_space, job, list, pause, reorder, resume_with_mode, retry_now,
    space_ledger,
};

pub(crate) use plan::absolute_path;

pub(crate) use source::{local_archive_path, safe_tar_path, StreamFileHash, StreamOutcome};
