import type { ReactNode } from "react";
import type { GameDetail, InstallJob, InstalledGame } from "@/lib/types";
import type { BigPictureLayout } from "./layout";
import type { BigPictureNavigation } from "./navigation";

/* --------------------------------------------------------------------------
   The contract between the Big Picture screen and its four views (#141).
   Every layout receives the same object: one selection model, one set of
   actions, one install state. A view decides composition, never data.
   -------------------------------------------------------------------------- */

export interface BigPictureLayoutProps {
  nav: BigPictureNavigation;
  layout: BigPictureLayout;
  /** `gridColumns(viewport width)`: also the grid's up/down step. */
  columns: number;
  reduceMotion: boolean | null;
  /** The #142 cascade already played: a layout switch mounts settled. */
  settled: boolean;
  /** Detail for the selected game only; other cards use the summary fields. */
  detail: GameDetail | undefined;
  installed: Map<number, InstalledGame>;
  /** Active install job for the selected game. */
  job: InstallJob | undefined;
  /** Progress of `job`, or null when there is none. */
  percent: number | null;
  /** Play / Install / Download / Installing, built by the screen. */
  primary: ReactNode;
  onOpenGame: (gameId: number) => void;
  /** Per-row install state for the compact list. */
  jobFor: (gameId: number) => InstallJob | undefined;
}

/** Percent done for a job; `0` while the total size is still unknown. */
export function jobPercent(job: InstallJob | undefined): number | null {
  if (!job) return null;
  if (!job.bytesTotal) return 0;
  return Math.min(100, (job.bytesDone / job.bytesTotal) * 100);
}
