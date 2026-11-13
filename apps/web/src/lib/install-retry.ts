import type { InstallJob, InstallResumeMode } from "@/lib/types";

/**
 * Retry/resume decisions for Downloads (#205/#208).
 *
 * Every retry reuses the existing job id: the desktop rebuilds the worker from
 * the persisted job parameters and continues at the `.part`/entry checkpoint.
 * A new job is only created by an explicit install from the game page, never by
 * a retry button. Keeping this pure makes the rule testable without React.
 */
export type RetryAction = "focus" | "resume";

/** Phases whose row can be retried. `done` has nothing left to do. */
export const RESUMABLE_PHASES: ReadonlySet<InstallJob["phase"]> = new Set([
  "paused",
  "error",
  "interrupted",
  "cancelled",
]);

export type RetryLabel = "resume" | "restart" | "restart-stream" | "focus";

export interface RetryPlan {
  /** `focus` returns the already-running job of the same game. */
  action: RetryAction;
  jobId: string;
  /** `null` keeps the job's own transport. */
  mode: InstallResumeMode | null;
  /** True when the action starts from zero and needs a confirmation. */
  confirmRestart: boolean;
  label: RetryLabel;
}

/** True when the job's row offers a retry action at all. */
export const canRetryJob = (job: InstallJob): boolean => RESUMABLE_PHASES.has(job.phase);

/** Seconds until the next automatic retry (#212), `null` when none is set. */
export function secondsUntilRetry(nextRetryAt: string | null, now = Date.now()): number | null {
  if (!nextRetryAt) return null;
  const at = Date.parse(nextRetryAt);
  if (!Number.isFinite(at)) return null;
  return Math.max(0, Math.ceil((at - now) / 1000));
}

/**
 * True when "Retry now" can skip a real wait (#212). A job parked because the
 * OS is offline has no timer to skip: it resumes on its own at `online`.
 */
export const canRetryNow = (job: InstallJob): boolean =>
  job.reconnecting === true && job.nextRetryAt !== null && job.waitingForNetwork !== true;

/**
 * True when retrying continues at the checkpoint. A failed job without one
 * (free-space error before the first byte, a legacy tar stream) is a restart,
 * and the UI must say so instead of promising a resume.
 */
export const isTrulyResumable = (job: InstallJob): boolean =>
  job.phase === "paused" || job.resumable === true;

/**
 * Plans the retry of `job`.
 *
 * - An active job for the same game wins: focus it, never start a second.
 * - Otherwise resume the same job id. `mode` switches the transport
 *   (`download`/`stream`) without touching the row.
 * - A job without a checkpoint restarts from zero and is flagged for the
 *   confirmation dialog.
 */
export function planRetry(
  job: InstallJob,
  active: InstallJob[],
  mode: InstallResumeMode | null = null,
): RetryPlan {
  const running = active.find(
    (entry) => entry.gameId === job.gameId && entry.jobId !== job.jobId,
  );
  if (running) {
    return {
      action: "focus",
      jobId: running.jobId,
      mode: null,
      confirmRestart: false,
      label: "focus",
    };
  }

  const resumable = isTrulyResumable(job);
  const label: RetryLabel = resumable
    ? "resume"
    : mode === "stream" || (mode === null && job.stream)
      ? "restart-stream"
      : "restart";
  return {
    action: "resume",
    jobId: job.jobId,
    mode,
    confirmRestart: !resumable,
    label,
  };
}
