//! Transport-neutral building blocks of the install pipeline.
//!
//! The install pipeline talks to two things that are notoriously hard to test
//! inside a Tauri app: time (retry backoff) and the user (cancel/pause). The
//! download loop used to inline both, so every policy change meant editing a
//! 528-line function. They now live here as small values that can be exercised
//! without a runtime:
//!
//! - [`RetryPolicy`] is the immutable budget of one transfer (idle timeout,
//!   attempt cap, retry window, backoff) plus [`RetryState`], the mutable
//!   counter that enforces it.
//! - [`CancelSet`] owns the cancel/pause tokens and every biased wait that must
//!   honour them, so "cancel wins over pause" is stated once.
//! - [`ProgressSink`] is the one-method callback a transfer reports to; the
//!   install layer implements it over a job row.

pub(crate) mod cancel;
pub(crate) mod progress;
pub(crate) mod retry;

pub(crate) use cancel::{CancelSet, Stopped};
pub(crate) use progress::{DownloadProgress, ProgressSink};
pub(crate) use retry::{RetryPolicy, RetryState};
