//! Process-tree tracking for launched games (#26).
//!
//! Games are rarely a single process: launchers, anti-cheat helpers and
//! renderer subprocesses come and go. Waiting on the root PID alone ends the
//! session as soon as the launcher exits, which under-reports playtime. This
//! module keeps a session alive while **any descendant** of the launched
//! process is running and attributes the whole session to the root game.
//!
//! The tracking core ([`TreeTracker`]) is a pure function over process
//! snapshots, so it is unit-tested with synthetic trees. The OS sampling lives
//! in [`ProcessSampler`] (sysinfo).
//!
//! ## Platform limits
//!
//! - A child that is spawned and orphaned (reparented to PID 1 / the Windows
//!   idle process) between two samples is lost. Sampling runs every second;
//!   launchers that exit in under a second can therefore end the session early.
//! - PID reuse can extend a session if an unrelated process inherits a tracked
//!   PID without an observed gap. Tracked PIDs are dropped as soon as they
//!   disappear from a snapshot, which makes this unlikely.
//! - On macOS the launcher uses `open`, so the tracked root is `open`; the
//!   actual game is found through the process tree (it becomes a descendant of
//!   LaunchServices, not of `open`), so macOS sessions rely on the same
//!   limitation and may end early. `open -W` would fix it at the cost of a
//!   blocking launcher.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

/// One process from an OS snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessInfo {
    /// Process identifier.
    pub pid: u32,
    /// Parent process identifier, when known.
    pub parent: Option<u32>,
}

/// Result of one tracking step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TreeStatus {
    /// True while at least one tracked process (root or descendant) exists.
    pub alive: bool,
    /// Number of live tracked processes.
    pub processes: usize,
    /// True once the tree is empty: the session should be closed.
    pub finished: bool,
}

/// Tracks the descendants of a root process across snapshots.
#[derive(Debug, Clone)]
pub struct TreeTracker {
    root: u32,
    tracked: BTreeSet<u32>,
    finished: bool,
}

impl TreeTracker {
    /// Create a tracker for a root PID.
    #[must_use]
    pub fn new(root: u32) -> Self {
        let mut tracked = BTreeSet::new();
        tracked.insert(root);
        Self {
            root,
            tracked,
            finished: false,
        }
    }

    /// PIDs currently tracked (root + known descendants), sorted.
    #[cfg(test)]
    fn tracked(&self) -> Vec<u32> {
        self.tracked.iter().copied().collect()
    }

    /// Feed one snapshot and return the new state.
    ///
    /// The tracker keeps every descendant it has ever observed while that PID
    /// still exists, so a grandchild stays tracked after its parent exits
    /// (unless it is reparented before the next sample, see module docs).
    pub fn step(&mut self, snapshot: &[ProcessInfo]) -> TreeStatus {
        if self.finished {
            return TreeStatus {
                alive: false,
                processes: 0,
                finished: true,
            };
        }

        let present: HashSet<u32> = snapshot.iter().map(|process| process.pid).collect();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for process in snapshot {
            if let Some(parent) = process.parent {
                if parent != process.pid {
                    children.entry(parent).or_default().push(process.pid);
                }
            }
        }

        // Drop processes that exited; keep the rest.
        self.tracked.retain(|pid| present.contains(pid));
        // The root is only re-added while it exists: a dead root must not
        // resurrect the session through PID reuse.
        if present.contains(&self.root) {
            self.tracked.insert(self.root);
        }

        let mut queue: VecDeque<u32> = self.tracked.iter().copied().collect();
        while let Some(pid) = queue.pop_front() {
            if let Some(known) = children.get(&pid) {
                for child in known {
                    if self.tracked.insert(*child) {
                        queue.push_back(*child);
                    }
                }
            }
        }

        let processes = self.tracked.len();
        let alive = processes > 0;
        if !alive {
            self.finished = true;
        }
        TreeStatus {
            alive,
            processes,
            finished: !alive,
        }
    }
}

/// Samples the OS process list with sysinfo.
///
/// A sampler is cheap to keep around: it reuses its internal [`sysinfo::System`]
/// and only refreshes process metadata.
pub struct ProcessSampler {
    /// `None` when the platform process layer could not initialise; snapshots
    /// are then empty and the session closes immediately.
    system: Option<sysinfo::System>,
}

impl ProcessSampler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            system: sysinfo::System::new().ok(),
        }
    }

    /// Current snapshot of every process (PID + parent).
    pub fn snapshot(&mut self) -> Vec<ProcessInfo> {
        let Some(system) = self.system.as_mut() else {
            return Vec::new();
        };
        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        system
            .processes()
            .iter()
            .map(|(pid, process)| ProcessInfo {
                pid: pid.as_u32(),
                parent: process.parent().map(|parent| parent.as_u32()),
            })
            .collect()
    }

    /// Kill every live descendant of `root`, children first, then the root.
    /// Returns the number of processes signalled.
    pub fn kill_tree(&mut self, root: u32) -> usize {
        let snapshot = self.snapshot();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for process in &snapshot {
            if let Some(parent) = process.parent {
                children.entry(parent).or_default().push(process.pid);
            }
        }

        // Collect the subtree in breadth-first order, then kill in reverse so
        // children die before their parents (which avoids respawns).
        let mut order = Vec::new();
        let mut queue = VecDeque::from([root]);
        while let Some(pid) = queue.pop_front() {
            order.push(pid);
            if let Some(next) = children.get(&pid) {
                queue.extend(next.iter().copied());
            }
        }

        let Some(system) = self.system.as_ref() else {
            return 0;
        };
        let mut killed = 0;
        for pid in order.iter().rev() {
            let Some(process) = system.process(sysinfo::Pid::from_u32(*pid)) else {
                continue;
            };
            if process.kill() {
                killed += 1;
            }
        }
        killed
    }
}

impl Default for ProcessSampler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent: Option<u32>) -> ProcessInfo {
        ProcessInfo { pid, parent }
    }

    #[test]
    fn root_and_children_keep_the_session_alive() {
        let mut tracker = TreeTracker::new(100);
        let status = tracker.step(&[
            process(100, Some(1)),
            process(101, Some(100)),
            process(102, Some(100)),
        ]);
        assert!(status.alive);
        assert_eq!(status.processes, 3);
        assert!(!status.finished);

        // The launcher exits, the game keeps running.
        let status = tracker.step(&[process(101, Some(1)), process(102, Some(1))]);
        assert!(status.alive, "children must keep the session alive");
        assert_eq!(status.processes, 2);
        assert_eq!(tracker.tracked(), vec![101, 102]);
    }

    #[test]
    fn late_grandchildren_are_adopted() {
        let mut tracker = TreeTracker::new(10);
        tracker.step(&[process(10, Some(1)), process(11, Some(10))]);
        // 11 spawns a renderer and the launcher exits.
        let status = tracker.step(&[
            process(11, Some(1)),
            process(12, Some(11)),
            process(13, Some(11)),
        ]);
        assert!(status.alive);
        assert_eq!(status.processes, 3);
        assert_eq!(tracker.tracked(), vec![11, 12, 13]);
    }

    #[test]
    fn session_finishes_when_the_tree_is_empty() {
        let mut tracker = TreeTracker::new(20);
        tracker.step(&[process(20, Some(1)), process(21, Some(20))]);
        let status = tracker.step(&[]);
        assert!(!status.alive);
        assert!(status.finished);
        // Once finished it stays finished even if the PID is reused.
        let status = tracker.step(&[process(20, Some(1))]);
        assert!(!status.alive);
        assert!(status.finished);
    }

    #[test]
    fn unrelated_processes_are_not_tracked() {
        let mut tracker = TreeTracker::new(30);
        let status = tracker.step(&[
            process(30, Some(1)),
            process(31, Some(1)),
            process(32, Some(99)),
        ]);
        assert_eq!(status.processes, 1);
        assert_eq!(tracker.tracked(), vec![30]);
    }

    #[test]
    fn cycles_and_self_parents_do_not_loop_forever() {
        let mut tracker = TreeTracker::new(40);
        let status = tracker.step(&[
            process(40, Some(41)),
            process(41, Some(40)),
            process(42, Some(42)),
        ]);
        assert!(status.alive);
        assert_eq!(tracker.tracked(), vec![40, 41]);
    }

    #[test]
    fn descendants_are_dropped_when_they_exit() {
        let mut tracker = TreeTracker::new(50);
        tracker.step(&[
            process(50, Some(1)),
            process(51, Some(50)),
            process(52, Some(50)),
        ]);
        let status = tracker.step(&[process(50, Some(1)), process(52, Some(1))]);
        assert_eq!(status.processes, 2);
        assert_eq!(tracker.tracked(), vec![50, 52]);
    }

    #[test]
    fn sampler_sees_the_current_process() {
        let mut sampler = ProcessSampler::new();
        let snapshot = sampler.snapshot();
        let current = std::process::id();
        assert!(
            snapshot.iter().any(|process| process.pid == current),
            "the test process must appear in its own snapshot"
        );
    }
}
