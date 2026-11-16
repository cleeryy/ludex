/**
 * The primary action for a game (#237).
 *
 * One matrix decides what the install button, the library card and the Console
 * offer: play, resume a session, update, install, download, or the state of the
 * job already running for that game. Before this module the same branch was
 * written three times (`game-session.ts` for the hero, `console-shell.tsx` for
 * the couch, `game-card.tsx` for the grid) and the job phases were compared to
 * literal arrays in four more places.
 *
 * `update` is an availability flag, not a primary kind: an installed game stays
 * playable while a newer version downloads, so the surfaces show Play plus an
 * Update affordance instead of replacing the main action. Everything here is
 * pure; `use-install-action.ts` wires it to the downloads store.
 */

import type { InstallJob, InstallPhase } from "@/lib/types";
import { formatPercent } from "@/lib/format";
import { t } from "@/lib/i18n";

/** What the main button of a game does right now. */
export type InstallActionKind =
  | "resume"
  | "play"
  | "install"
  | "download"
  | "installing"
  | "queued"
  | "paused"
  | "resumeInstall";

/** What pressing the main button should call. */
export type InstallActionIntent =
  | "resume-session"
  | "play"
  | "install"
  | "download"
  | "open-downloads"
  | "resume-install";

export interface InstallActionInput {
  /** Native desktop build: installs and launches work locally. */
  desktop: boolean;
  /** A live session for this game (desktop only). */
  running?: boolean;
  installed?: boolean;
  /** The server has a newer version of this install. */
  update?: boolean;
  /** Phase of the active job, when one owns this game. */
  jobPhase?: InstallPhase | null;
  /** A failed job with a usable checkpoint waiting in history. */
  resumable?: boolean;
  /**
   * Resume a paused/failed checkpoint from the button (default). When false the
   * surface opens the Downloads queue instead: the Console leaves pausing and
   * retrying to the full screen (#196).
   */
  resumeInPlace?: boolean;
  /** At least one archive is downloadable. Defaults to true. */
  archiveAvailable?: boolean;
}

export interface InstallAction {
  kind: InstallActionKind;
  intent: InstallActionIntent;
  /** Render the secondary Update action (installed, newer version, idle). */
  update: boolean;
  /** No archive and no local install: the button cannot do anything. */
  disabled: boolean;
}

const ACTIVE_PHASES: ReadonlySet<InstallPhase> = new Set<InstallPhase>([
  "downloading",
  "verifying",
  "extracting",
  "finalizing",
]);

/**
 * Ordering, unchanged from the pre-#191 hero (#191): a running session wins,
 * then an installed game (its files stay playable while an update downloads),
 * then a paused checkpoint, then a running job, then a failed one. `install`
 * and `download` are the same action on two builds.
 */
export function installAction(input: InstallActionInput): InstallAction {
  const archiveAvailable = input.archiveAvailable ?? true;
  const resumeInPlace = input.resumeInPlace ?? true;
  const update = Boolean(input.installed && input.update && !input.jobPhase);

  if (input.desktop && input.running) {
    return { kind: "resume", intent: "resume-session", update, disabled: false };
  }
  if (input.desktop && input.installed) {
    return { kind: "play", intent: "play", update, disabled: false };
  }
  if (input.jobPhase === "paused") {
    return {
      kind: "paused",
      intent: resumeInPlace ? "resume-install" : "open-downloads",
      update,
      disabled: false,
    };
  }
  if (input.jobPhase === "queued" || input.jobPhase === "waitingForSpace") {
    return { kind: "queued", intent: "open-downloads", update, disabled: false };
  }
  if (input.jobPhase && ACTIVE_PHASES.has(input.jobPhase)) {
    return { kind: "installing", intent: "open-downloads", update, disabled: false };
  }
  if (input.resumable && resumeInPlace) {
    return { kind: "resumeInstall", intent: "resume-install", update, disabled: false };
  }
  if (input.desktop) {
    return { kind: "install", intent: "install", update, disabled: !archiveAvailable };
  }
  return { kind: "download", intent: "download", update, disabled: !archiveAvailable };
}

/** Button copy for one action; existing message ids only. */
export function installActionLabel(action: InstallAction, percent: number | null): string {
  switch (action.intent) {
    case "resume-session":
      return t("Resume");
    case "play":
      return t("Play");
    case "install":
      return t("Install");
    case "download":
      return t("Download");
    case "resume-install":
      return t("Resume install");
    case "open-downloads":
      if (action.kind === "paused") return t("Paused");
      if (action.kind === "queued") return t("Queued");
      return t("Installing {percent}", { percent: percent !== null ? formatPercent(percent) : "…" });
  }
}

/** Which footer the install dialog must draw for the job it shows. */
export type InstallJobAction =
  | "none"
  | "done"
  | "paused"
  | "queued"
  | "installing"
  | "resumable"
  | "restart"
  | "stream-failed";

/**
 * Classify a job once (#237). The install dialog used to test three literal
 * phase arrays plus `stream`/`resumable` flags; every other surface that shows
 * a job should read this instead of re-listing phases.
 */
export function installJobAction(job: InstallJob | undefined): InstallJobAction {
  if (!job) return "none";
  if (job.phase === "done") return "done";
  if (job.phase === "paused") return "paused";
  if (job.phase === "queued" || job.phase === "waitingForSpace") return "queued";
  if (ACTIVE_PHASES.has(job.phase)) return "installing";
  // A stream without a checkpoint cannot resume from bytes (#205/#208).
  if (job.stream === true && job.resumable !== true) return "stream-failed";
  if (job.resumable === true) return "resumable";
  return "restart";
}

/** Percent done for a job; `0` while the total size is still unknown. */
export function jobPercent(job: InstallJob | undefined): number | null {
  if (!job) return null;
  if (!job.bytesTotal) return 0;
  return Math.min(100, (job.bytesDone / job.bytesTotal) * 100);
}
