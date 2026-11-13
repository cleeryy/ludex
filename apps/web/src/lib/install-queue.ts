import type { InstallJob } from "@/lib/types";

/**
 * Queue order for Downloads and Big Picture (#202).
 *
 * The desktop owns the real order: it persists a `queuePosition` per active
 * job and starts the lowest one when a slot frees. These helpers mirror that
 * rule in the browser so the list, the optimistic reorder and the disabled
 * states agree with what the desktop will confirm. Keeping them pure makes the
 * behaviour testable without React or Tauri.
 */
export type QueueMove = "top" | "up" | "down" | "position";

/**
 * Phases that own a scheduler slot: a download, the verification pass and the
 * extraction/commit are never reordered (#221). Only the extractor is
 * exclusive; a running download can coexist with an extraction, and neither is
 * moved by the queue controls.
 */
export const isRunningJob = (job: InstallJob): boolean =>
  job.phase === "downloading" ||
  job.phase === "verifying" ||
  job.phase === "extracting" ||
  job.phase === "finalizing";

/**
 * Phases that wait in the queue and can be moved. A paused job keeps its slot
 * (and its checkpoint), so it can be moved like a queued one.
 */
export const isReorderableJob = (job: InstallJob): boolean =>
  job.phase === "queued" || job.phase === "waitingForSpace" || job.phase === "paused";

/** Rows without a position sort last instead of jumping to the top. */
const positionOf = (job: InstallJob): number => job.queuePosition ?? Number.MAX_SAFE_INTEGER;

const comparePosition = (left: InstallJob, right: InstallJob): number =>
  positionOf(left) - positionOf(right) || left.jobId.localeCompare(right.jobId);

/**
 * Display order for the active list: the running install stays pinned at the
 * top, then the waiting queue in start order. Finished jobs are not part of
 * this list.
 */
export function orderActiveQueue(jobs: InstallJob[]): InstallJob[] {
  const running = jobs.filter(isRunningJob).sort(comparePosition);
  const waiting = jobs.filter(isReorderableJob).sort(comparePosition);
  return [...running, ...waiting];
}

/** The waiting part of the queue, in start order. */
export function waitingQueue(jobs: InstallJob[]): InstallJob[] {
  return jobs.filter(isReorderableJob).sort(comparePosition);
}

export const canMoveUp = (jobs: InstallJob[], jobId: string): boolean => {
  const waiting = waitingQueue(jobs);
  return waiting.findIndex((job) => job.jobId === jobId) > 0;
};

export const canMoveDown = (jobs: InstallJob[], jobId: string): boolean => {
  const waiting = waitingQueue(jobs);
  const index = waiting.findIndex((job) => job.jobId === jobId);
  return index >= 0 && index < waiting.length - 1;
};

/** Next free position, mirroring the desktop rule (#202). */
export function nextQueuePosition(jobs: InstallJob[]): number {
  const positions = jobs
    .map((job) => job.queuePosition)
    .filter((position): position is number => position !== null);
  return positions.length > 0 ? Math.max(...positions) + 1 : 0;
}

/**
 * Reorders the local list exactly like the desktop scheduler does and returns
 * the whole list with compacted `queuePosition` values.
 *
 * Throws when the job is running or finished, or when an explicit position is
 * outside the waiting queue: the caller shows the message instead of moving
 * something the desktop would refuse.
 */
export function moveInQueue(
  jobs: InstallJob[],
  jobId: string,
  move: QueueMove,
  position?: number,
): InstallJob[] {
  const target = jobs.find((job) => job.jobId === jobId);
  if (!target) throw new Error(`Unknown install job: ${jobId}`);
  if (isRunningJob(target)) {
    throw new Error("This install is running. Pause or cancel it before reordering the queue.");
  }
  if (!isReorderableJob(target)) {
    throw new Error("This install already finished and cannot be reordered.");
  }

  const waiting = waitingQueue(jobs);
  const current = waiting.findIndex((job) => job.jobId === jobId);
  let next: number;
  switch (move) {
    case "top":
      next = 0;
      break;
    case "up":
      next = Math.max(0, current - 1);
      break;
    case "down":
      next = Math.min(waiting.length - 1, current + 1);
      break;
    case "position":
      if (position === undefined) {
        throw new Error("A position is required to move an install to a specific place in the queue.");
      }
      if (!Number.isInteger(position) || position < 0 || position >= waiting.length) {
        throw new Error(
          `Position ${position} is out of range: the queue has ${waiting.length} waiting install(s).`,
        );
      }
      next = position;
      break;
  }
  if (next === current) return jobs;

  const reordered = [...waiting];
  const [moved] = reordered.splice(current, 1);
  reordered.splice(next, 0, moved);

  // Running jobs keep the front slots; the waiting queue follows in the order
  // chosen here. Every active job gets a compact position.
  const ranked = new Map<string, number>();
  [...jobs.filter(isRunningJob).sort(comparePosition), ...reordered].forEach((job, index) => {
    ranked.set(job.jobId, index);
  });
  return jobs.map((job) => {
    const rank = ranked.get(job.jobId);
    return rank === undefined ? job : { ...job, queuePosition: rank };
  });
}
