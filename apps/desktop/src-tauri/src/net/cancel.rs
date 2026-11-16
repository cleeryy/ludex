//! Cancel/pause cooperation for the install pipeline.
//!
//! Every blocking step of an install is a biased `tokio::select!` between a
//! user signal and real work. The rules are not obvious and were repeated at
//! each call site:
//!
//! 1. **Cancel wins over pause**: a cancel that races a pause must discard the
//!    partial archive, so the cancel branch is checked first (#232).
//! 2. **A pause is not a failure**: the caller keeps the `.part` file.
//! 3. **"Retry now" only skips a backoff**, never a real attempt of the budget.
//!
//! [`CancelSet`] states those rules once. It holds the two tokens of a job for
//! the duration of one step; cloning a token is cheap, so the install code
//! rebuilds the set at each stage boundary instead of threading it through.

use std::future::Future;

use tokio_util::sync::CancellationToken;

use crate::state::NetworkGate;

/// Why a step stopped before its work completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stopped {
    Cancelled,
    Paused,
}

/// The cancel/pause tokens of one job, with the biased waits that honour them.
#[derive(Debug, Clone)]
pub(crate) struct CancelSet {
    cancel: CancellationToken,
    pause: CancellationToken,
}

impl CancelSet {
    pub(crate) fn new(cancel: &CancellationToken, pause: &CancellationToken) -> Self {
        Self {
            cancel: cancel.clone(),
            pause: pause.clone(),
        }
    }

    /// Check both signals without waiting. Cancel wins over pause.
    pub(crate) fn check(&self) -> Result<(), Stopped> {
        if self.cancel.is_cancelled() {
            Err(Stopped::Cancelled)
        } else if self.pause.is_cancelled() {
            Err(Stopped::Paused)
        } else {
            Ok(())
        }
    }

    /// Check only the cancel signal.
    ///
    /// The commit stages use this: a pause requested while the archive is
    /// being extracted must not abort a commit that is already past the
    /// download, exactly as `check_cancel` behaved before the split.
    pub(crate) fn check_cancelled(&self) -> Result<(), Stopped> {
        if self.cancel.is_cancelled() {
            Err(Stopped::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Run `future`, but stop as soon as the job is cancelled or paused.
    pub(crate) async fn select<T>(&self, future: impl Future<Output = T>) -> Result<T, Stopped> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(Stopped::Cancelled),
            _ = self.pause.cancelled() => Err(Stopped::Paused),
            output = future => Ok(output),
        }
    }

    /// Sleep a backoff, honouring cancel, pause and "Retry now" (#212).
    ///
    /// `Ok(false)` means the user cancelled: the caller returns
    /// [`Stopped::Cancelled`] so the cleanup path is the same as a mid-transfer
    /// cancel. A "Retry now" click skips the remaining wait, not the attempt
    /// budget.
    pub(crate) async fn sleep(
        &self,
        delay: std::time::Duration,
        retry_now: Option<&tokio::sync::Notify>,
    ) -> Result<bool, Stopped> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Ok(false),
            _ = self.pause.cancelled() => Err(Stopped::Paused),
            _ = async {
                match retry_now {
                    Some(signal) => signal.notified().await,
                    None => std::future::pending().await,
                }
            } => Ok(true),
            _ = tokio::time::sleep(delay) => Ok(true),
        }
    }

    /// Park until the OS reports a network again (#212).
    pub(crate) async fn wait_online(&self, network: &NetworkGate) -> Result<(), Stopped> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(Stopped::Cancelled),
            _ = self.pause.cancelled() => Err(Stopped::Paused),
            _ = network.wait_online() => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn retry_now_skips_the_backoff_and_cancel_stops_it() {
        let signal = tokio::sync::Notify::new();
        let started = Instant::now();
        let stops = CancelSet::new(&CancellationToken::new(), &CancellationToken::new());
        let waiter = stops.sleep(Duration::from_secs(30), Some(&signal));
        tokio::pin!(waiter);
        tokio::select! {
            result = &mut waiter => panic!("the backoff ended too early: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        signal.notify_waiters();
        assert!(waiter.await.unwrap(), "retry now wakes the wait");
        assert!(started.elapsed() < Duration::from_secs(2));

        let cancel = CancellationToken::new();
        cancel.cancel();
        let started = Instant::now();
        let stops = CancelSet::new(&cancel, &CancellationToken::new());
        assert!(!stops.sleep(Duration::from_secs(30), None).await.unwrap());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancel_wins_over_pause_even_when_both_are_requested() {
        let cancel = CancellationToken::new();
        let pause = CancellationToken::new();
        cancel.cancel();
        pause.cancel();
        let stops = CancelSet::new(&cancel, &pause);
        assert_eq!(stops.check(), Err(Stopped::Cancelled));
    }

    #[test]
    fn a_pause_does_not_abort_a_commit_past_the_download() {
        // The commit stages check only the cancel signal: a pause requested
        // while the tree is being extracted takes effect later, it does not
        // throw away a finished extraction.
        let pause = CancellationToken::new();
        pause.cancel();
        let stops = CancelSet::new(&CancellationToken::new(), &pause);
        assert!(stops.check_cancelled().is_ok());
        assert_eq!(stops.check(), Err(Stopped::Paused));
    }

    #[test]
    fn pause_is_reported_alone() {
        let pause = CancellationToken::new();
        pause.cancel();
        let stops = CancelSet::new(&CancellationToken::new(), &pause);
        assert_eq!(stops.check(), Err(Stopped::Paused));
    }
}
