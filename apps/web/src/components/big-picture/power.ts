import { t } from "@/lib/i18n";
import { gridStepIndex, stepIndex, type MoveDirection } from "./layout";

/* --------------------------------------------------------------------------
   The Power surface (#196).

   Power is the one place in Big Picture that can end the session, so it is
   modelled as data: which actions this build can actually perform, the honest
   consequence of each, and a confirmation that always starts on Cancel. The
   screen renders it; the test suite proves the default and the platform gates.
   -------------------------------------------------------------------------- */

export type PowerActionId = "end-session" | "sleep" | "restart" | "quit" | "quit-game" | "exit";

export interface PowerAction {
  id: PowerActionId;
  label: string;
  detail: string;
  tone: "default" | "danger";
  enabled: boolean;
  /** Why the action is unavailable, shown in the row instead of a toast. */
  reason?: string;
}

export interface PowerContext {
  /** A native console session started by Sunshine. */
  console: boolean;
  /** The Tauri shell, where the process-level commands exist. */
  native: boolean;
  /** Title of the game this Big Picture session launched, if any. */
  gameTitle: string | null;
}

const DESKTOP_ONLY = () => t("Only available in the desktop app.");

/**
 * The visible actions in couch order. Mode-aware copy is the point: quitting
 * the process stops downloads, so the row says so instead of repeating the old
 * "downloads continue in the background" line.
 */
export function powerActions(context: PowerContext): PowerAction[] {
  const actions: PowerAction[] = [];

  if (context.console) {
    actions.push({
      id: "end-session",
      label: t("End session"),
      detail: t("The session ends and the stream returns to the host desktop."),
      tone: "default",
      enabled: context.native,
      reason: context.native ? undefined : DESKTOP_ONLY(),
    });
  }

  actions.push({
    id: "sleep",
    label: t("Sleep this PC"),
    detail: t("Sleep keeps the session running. Press a button on the controller to wake the PC."),
    tone: "default",
    enabled: context.native,
    reason: context.native ? undefined : DESKTOP_ONLY(),
  });

  actions.push({
    id: "restart",
    label: t("Restart Ludex"),
    detail: t("Ludex restarts. The session ends; downloads resume when it opens again."),
    tone: "default",
    enabled: context.native,
    reason: context.native ? undefined : DESKTOP_ONLY(),
  });

  actions.push({
    id: "quit",
    label: `${t("Quit")} Ludex`,
    detail: context.console
      ? t("Quit to desktop. Downloads stop.")
      : t("Close Ludex. Downloads stop."),
    tone: "danger",
    enabled: context.native,
    reason: context.native ? undefined : DESKTOP_ONLY(),
  });

  if (context.gameTitle) {
    actions.push({
      id: "quit-game",
      label: `${t("Quit")} ${context.gameTitle}`,
      detail: t("Close {title} and return to Big Picture.", { title: context.gameTitle }),
      tone: "danger",
      // Closing the game is a native launch-session command; a browser tab has
      // no process to stop.
      enabled: context.native,
      reason: context.native ? undefined : DESKTOP_ONLY(),
    });
  }

  if (!context.console) {
    actions.push({
      id: "exit",
      label: t("Exit Big Picture"),
      detail: t("Leave the couch interface."),
      tone: "default",
      enabled: true,
    });
  }

  return actions;
}

/** Power is a two-column grid; the d-pad keeps its geometric meaning. */
export const POWER_GRID_COLUMNS = 2;

export function powerGridStep(index: number, direction: MoveDirection, total: number): number {
  if (direction === "left" || direction === "right") {
    return stepIndex(index, direction === "left" ? -1 : 1, total);
  }
  return gridStepIndex(index, direction === "up" ? -1 : 1, POWER_GRID_COLUMNS, total);
}

/* ----------------------------- confirmation ------------------------------ */

/** Row 0 confirms, row 1 cancels. The highlight always starts on row 1. */
export const POWER_CONFIRM_CANCEL_INDEX = 1;

export type PowerConfirmChoice = "confirm" | "cancel";

export function powerConfirmActive(): number {
  return POWER_CONFIRM_CANCEL_INDEX;
}

export function resolvePowerConfirm(index: number): PowerConfirmChoice {
  return index === 0 ? "confirm" : "cancel";
}

/** Backing out of Big Picture itself is safe and needs no second press. */
export function needsPowerConfirm(action: PowerActionId): boolean {
  return action !== "exit";
}
