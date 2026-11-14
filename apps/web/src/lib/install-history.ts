import type { InstallJob } from "./types";

/**
 * Downloads history presentation rules (#225).
 *
 * A finished install must show one row per game even when the desktop kept
 * several terminal attempts for it: a changed install location or stream flag
 * used to leave two rows, and a superseded attempt stayed visible. Kept pure so
 * the Downloads screen, the game page and the install dialog agree.
 */

/**
 * Message the desktop writes on the terminal row a new attempt replaces
 * (`db.rs::supersede_install_jobs`). Such a row is an audit trail, not a state
 * the player can act on, so the default history hides it.
 */
export const SUPERSEDED_MESSAGE = "Superseded by a new install";

/** True for the row a newer attempt left behind (#225). */
export function isSupersededJob(job: InstallJob): boolean {
  return job.message?.trim() === SUPERSEDED_MESSAGE;
}

/**
 * Rows with no timestamp in the IPC model fall back to the position in the
 * list. `install::list` emits history newest first, so the lower index is the
 * newer row; the optional field keeps the selector correct once the desktop
 * exposes it without changing the contract today.
 */
type DatedJob = InstallJob & { createdAt?: string | null };

function createdAtMs(job: InstallJob): number | null {
  const value = (job as DatedJob).createdAt;
  if (typeof value !== "string" || value.trim() === "") return null;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

/** Newest wins. Timestamps when both rows carry one, list position otherwise. */
function isNewer(candidate: InstallJob, candidateIndex: number, incumbent: InstallJob, incumbentIndex: number): boolean {
  const candidateAt = createdAtMs(candidate);
  const incumbentAt = createdAtMs(incumbent);
  if (candidateAt !== null && incumbentAt !== null) return candidateAt > incumbentAt;
  return candidateIndex < incumbentIndex;
}

/**
 * A `done` row beats a failed attempt for the same game: the install did
 * finish, the failure is an older attempt the player retried. Between rows of
 * the same kind the newest one is the state worth showing.
 */
function isPreferred(candidate: InstallJob, candidateIndex: number, incumbent: InstallJob, incumbentIndex: number): boolean {
  const candidateDone = candidate.phase === "done";
  const incumbentDone = incumbent.phase === "done";
  if (candidateDone !== incumbentDone) return candidateDone;
  return isNewer(candidate, candidateIndex, incumbent, incumbentIndex);
}

/**
 * History rows the Downloads screen should render.
 *
 * - a game with an active job has no history row: the active row is the truth;
 * - superseded attempts are hidden;
 * - one row per game, preferring `done` and otherwise the newest attempt,
 *   including when `history` arrived in another order;
 * - the kept rows stay in the order they arrived, so the list keeps the
 *   desktop's chronology instead of regrouping by game.
 */
export function visibleHistory(active: InstallJob[], history: InstallJob[]): InstallJob[] {
  const activeGames = new Set(active.map((job) => job.gameId));
  const chosen = new Map<number, { job: InstallJob; index: number }>();
  for (const [index, job] of history.entries()) {
    if (activeGames.has(job.gameId) || isSupersededJob(job)) continue;
    const incumbent = chosen.get(job.gameId);
    if (!incumbent) {
      chosen.set(job.gameId, { job, index });
      continue;
    }
    if (isPreferred(job, index, incumbent.job, incumbent.index)) {
      chosen.set(job.gameId, { job, index });
    }
  }
  const keep = new Set([...chosen.values()].map((entry) => entry.index));
  return history.filter((_job, index) => keep.has(index));
}
