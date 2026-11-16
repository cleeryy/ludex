//! Download crash containment (#234).
//!
//! A download is the only part of the desktop that can fail on data the user
//! did not author: a truncated `.part`, a hostile archive, a transport that
//! returns something unexpected. None of that may take the whole app down and
//! turn into a startup crash loop:
//!
//! - [`guard_worker`] polls an install/resume/verify future inside
//!   `catch_unwind`, so a panic fails **that job** instead of the process.
//!   `panic = "unwind"` in the release profile is what makes this possible; see
//!   `docs/RELEASING.md`.
//! - [`startup_resume_decision`] is the persisted two-strike rule applied by
//!   `install::auto_resume_on_startup`: after two consecutive unclean launches
//!   a job is parked instead of resumed again.
//! - [`safe_mode_requested`] implements `LUDEX_SAFE_MODE=1` / `--safe-mode`,
//!   the support escape hatch that skips every automatic resume.
//!
//! A native fault (segfault, OOM kill, `abort`) cannot be caught here; the
//! startup marker and the two-strike rule cover that case on the next launch.

use std::any::Any;
use std::cell::RefCell;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;

/// Consecutive unclean launches allowed before a job is parked (#234).
pub const MAX_RESUME_FAILURES: u32 = 2;

/// Environment variable a support session can set to skip all automatic
/// resumes (`LUDEX_SAFE_MODE=1`).
pub const SAFE_MODE_ENV: &str = "LUDEX_SAFE_MODE";

/// Equivalent command-line switch (`ludex --safe-mode`).
pub const SAFE_MODE_FLAG: &str = "--safe-mode";

/// Environment values accepted as "safe mode on".
fn truthy_env(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// True when safe mode was requested through the environment or an argument.
///
/// Pure so the startup path and the tests agree; the values are only read by
/// [`safe_mode_requested_now`].
pub fn safe_mode_requested<I>(args: I, env: Option<&str>) -> bool
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    env.is_some_and(truthy_env) || args.into_iter().any(|arg| arg.as_ref() == SAFE_MODE_FLAG)
}

/// Safe mode as requested by the running process (#234).
pub fn safe_mode_requested_now() -> bool {
    let env = std::env::var(SAFE_MODE_ENV).ok();
    safe_mode_requested(std::env::args(), env.as_deref())
}

/// What the startup path does with one interrupted job (#234).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupResume {
    /// Fewer than [`MAX_RESUME_FAILURES`] unclean launches: resume it.
    Resume,
    /// Repeated crashes: leave the row `error` and wait for a manual Resume.
    Park,
}

/// The two-strike rule, kept separate from the database so it is testable.
pub fn startup_resume_decision(failures: u32) -> StartupResume {
    if failures >= MAX_RESUME_FAILURES {
        StartupResume::Park
    } else {
        StartupResume::Resume
    }
}

/// Message and source location of a contained panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanicDetails {
    pub message: String,
    /// `file:line` reported by the panic hook, when it ran on this thread.
    pub location: Option<String>,
}

impl PanicDetails {
    /// `message (file:line)`, or just the message without a location.
    pub fn describe(&self) -> String {
        match &self.location {
            Some(location) => format!("{} ({location})", self.message),
            None => self.message.clone(),
        }
    }
}

thread_local! {
    /// Set by the panic hook on the panicking thread, consumed by
    /// [`guard_worker`] right after the unwind lands back in it.
    static LAST_PANIC: RefCell<Option<PanicDetails>> = const { RefCell::new(None) };
}

/// Called from the diagnostics panic hook (#171/#234) on the panicking thread.
pub fn note_panic(message: &str, location: Option<&str>) {
    LAST_PANIC.with(|slot| {
        *slot.borrow_mut() = Some(PanicDetails {
            message: message.to_string(),
            location: location.map(str::to_string),
        });
    });
}

/// Drop a stale record so a later panic is never attributed to this worker.
pub fn clear_last_panic() {
    LAST_PANIC.with(|slot| {
        slot.borrow_mut().take();
    });
}

/// Take the record written by the last panic on this thread, if any.
pub fn take_last_panic() -> Option<PanicDetails> {
    LAST_PANIC.with(|slot| slot.borrow_mut().take())
}

/// Human-readable message from a `catch_unwind` payload.
fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn panic_details(payload: &(dyn Any + Send)) -> PanicDetails {
    match take_last_panic() {
        Some(details) => details,
        None => PanicDetails {
            message: payload_message(payload),
            location: None,
        },
    }
}

/// Run a worker future so a panic becomes `Err`, not a dead process (#234).
///
/// The caller must turn `Err` into a normal job failure: the partial file and
/// staging folder are never touched here. `AssertUnwindSafe` is deliberate: the
/// future runs on its own task, nothing borrows it after a panic, and every
/// shared value it touched (`Mutex`-guarded job state) recovers from poisoning.
pub async fn guard_worker<F, T>(future: F) -> Result<T, PanicDetails>
where
    F: Future<Output = T>,
{
    clear_last_panic();
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(value) => Ok(value),
        Err(payload) => Err(panic_details(payload.as_ref())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_mode_accepts_the_documented_switches() {
        assert!(safe_mode_requested(["--safe-mode"], None));
        assert!(safe_mode_requested(
            ["ludex", "--safe-mode", "--console"],
            None
        ));
        assert!(safe_mode_requested(Vec::<String>::new(), Some("1")));
        assert!(safe_mode_requested(Vec::<String>::new(), Some(" TRUE ")));
        assert!(safe_mode_requested(Vec::<String>::new(), Some("on")));
        assert!(!safe_mode_requested(Vec::<String>::new(), Some("0")));
        assert!(!safe_mode_requested(Vec::<String>::new(), Some("false")));
        assert!(!safe_mode_requested(["ludex", "--console"], None));
    }

    #[test]
    fn two_unclean_starts_park_a_job_and_a_manual_resume_clears_the_streak() {
        assert_eq!(startup_resume_decision(0), StartupResume::Resume);
        assert_eq!(startup_resume_decision(1), StartupResume::Resume);
        assert_eq!(startup_resume_decision(2), StartupResume::Park);
        assert_eq!(startup_resume_decision(7), StartupResume::Park);
        // A manual Resume deletes the counter row, so the next decision is
        // `Resume` again: the user is never locked out of their download.
        assert_eq!(startup_resume_decision(0), StartupResume::Resume);
    }

    #[test]
    fn a_panic_record_is_written_and_consumed_once() {
        clear_last_panic();
        note_panic("boom", Some("install.rs:4242"));
        let details = take_last_panic().expect("the hook record must be available");
        assert_eq!(details.message, "boom");
        assert_eq!(details.location.as_deref(), Some("install.rs:4242"));
        assert_eq!(details.describe(), "boom (install.rs:4242)");
        assert!(take_last_panic().is_none(), "a record is consumed once");
    }

    #[tokio::test]
    async fn a_panicking_worker_becomes_an_error_instead_of_killing_the_app() {
        let outcome = guard_worker(async {
            panic!("the archive parser exploded");
        })
        .await;
        let panic = outcome.expect_err("the panic must be contained");
        assert!(
            panic.message.contains("the archive parser exploded"),
            "{panic:?}"
        );
    }

    #[tokio::test]
    async fn a_healthy_worker_returns_its_value() {
        let outcome = guard_worker(async { 7_u32 }).await;
        assert_eq!(outcome, Ok(7));
    }
}
