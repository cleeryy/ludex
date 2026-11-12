/**
 * Running-game decisions for the game page (#191).
 *
 * The hero's primary action and the force-close confirmation are pure state
 * rules so the "running vs installed vs not installed" ordering and the
 * "confirmation before a hard kill" promise are unit-tested without a live
 * Tauri process.
 */

import type { GameSession } from "@/lib/types";

/** What the game hero's primary button does. */
export type HeroAction =
  | "resume"
  | "play"
  | "install"
  | "download"
  | "installing"
  | "resumeInstall";

export interface HeroActionInput {
  /** A live session for this game (desktop only). */
  running: boolean;
  /** Native desktop build: installs, launches and resume are available. */
  desktop: boolean;
  installed: boolean;
  /** Phase of the active install job, when there is one. */
  activeJobPhase?: string | null;
  hasActiveJob?: boolean;
  /** A paused/partial download that can be picked up again. */
  hasResumableJob?: boolean;
}

/**
 * Resume wins over every other action: a running game must never show Play
 * (that path would refuse with "already running") or Install. The remaining
 * order matches the pre-#191 hero: Play, paused install, running install,
 * resumable install, Install (desktop) or Download (browser).
 */
export function heroAction(input: HeroActionInput): HeroAction {
  if (input.running && input.desktop) return "resume";
  if (input.desktop && input.installed) return "play";
  if (input.activeJobPhase === "paused") return "resumeInstall";
  if (input.hasActiveJob) return "installing";
  if (input.hasResumableJob) return "resumeInstall";
  return input.desktop ? "install" : "download";
}

/** The live session of this game, or null. */
export function sessionForGame(
  session: GameSession | null | undefined,
  gameId: number,
): GameSession | null {
  if (!session || session.gameId !== gameId) return null;
  return session;
}

/** Steps of the force-close confirmation. */
export type ForceClosePhase = "idle" | "confirming" | "closing";

export type ForceCloseEvent = "ask" | "cancel" | "confirm" | "done";

/**
 * Confirmation state machine: a force close always asks first, cannot be
 * confirmed twice, and returns to `idle` once the command settled (success or
 * failure) so the page can offer the action again.
 */
export function nextForceClosePhase(
  phase: ForceClosePhase,
  event: ForceCloseEvent,
): ForceClosePhase {
  switch (event) {
    case "ask":
      return phase === "closing" ? phase : "confirming";
    case "cancel":
      return phase === "confirming" ? "idle" : phase;
    case "confirm":
      return phase === "confirming" ? "closing" : phase;
    case "done":
      return "idle";
    default:
      return phase;
  }
}
