/**
 * `useInstallAction`: the primary action of one game, wired to the downloads
 * store (#237). See `lib/install-action.ts` for the matrix itself.
 *
 * The hook reads two per-game selectors, not the whole queue, so a library card
 * still only repaints when its own job changes. Surfaces that cannot resume a
 * failed checkpoint (the Console starts a fresh job) pass `offerResume: false`.
 */

import { installAction, installActionLabel, jobPercent } from "@/lib/install-action";
import type { InstallAction } from "@/lib/install-action";
import { isNative } from "@/lib/native";
import type { InstallJob } from "@/lib/types";
import { useJob, useResumableJob } from "@/state/downloads";

export interface UseInstallActionOptions {
  /** This device has the game installed. */
  installed?: boolean;
  /** The server has a newer version of this install. */
  update?: boolean;
  /** A live session for this game; only the game page can bring one back. */
  running?: boolean;
  /** Native desktop build; defaults to the current runtime. */
  desktop?: boolean;
  /** At least one archive exists; defaults to true. */
  archiveAvailable?: boolean;
  /** Offer the checkpoint of a failed job (default true). */
  offerResume?: boolean;
  /** Resume paused/failed checkpoints in place; false opens the Downloads queue. */
  resumeInPlace?: boolean;
}

export interface UseInstallActionResult extends InstallAction {
  /** Translated button copy, including the live percent while installing. */
  label: string;
  /** Active job progress (0-100), null when nothing is downloading. */
  percent: number | null;
  /** The job the primary action continues: paused first, then resumable. */
  job: InstallJob | undefined;
  /** The job that owns this game right now, if any. */
  activeJob: InstallJob | undefined;
  /** The failed history row that can be picked up, if any. */
  resumableJob: InstallJob | undefined;
}

export function useInstallAction(gameId: number, options: UseInstallActionOptions = {}): UseInstallActionResult {
  const activeJob = useJob(gameId);
  const resumableJob = useResumableJob(gameId);
  const offerResume = options.offerResume ?? true;

  const action = installAction({
    desktop: options.desktop ?? isNative(),
    running: options.running,
    installed: options.installed,
    update: options.update,
    jobPhase: activeJob?.phase ?? null,
    resumable: !activeJob && offerResume && Boolean(resumableJob),
    resumeInPlace: options.resumeInPlace,
    archiveAvailable: options.archiveAvailable,
  });

  const percent = activeJob ? jobPercent(activeJob) : null;
  const job = action.intent === "resume-install" ? activeJob ?? resumableJob : activeJob;

  return {
    ...action,
    label: installActionLabel(action, percent),
    percent,
    job,
    activeJob,
    resumableJob,
  };
}
