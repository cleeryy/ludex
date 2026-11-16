/* --------------------------------------------------------------------------
   #233: Big Picture takes over the OS window while it is open, and every exit
   has to hand the window back in the state it was found.

   The screen used to remember only whether *it* called `setFullscreen(true)`.
   A window that was already fullscreen when Big Picture mounted (restored from
   a previous session, or a manual visit from a fullscreen desktop window) then
   matched neither branch: the enter effect saw fullscreen and did nothing, and
   the cleanup skipped its restore because it had not entered. The window stayed
   fullscreen with no way back from inside the screen.

   #269: the window belongs to one Rust policy now. This module only decides
   whether Big Picture may take the window at all; when the policy says the
   native side owns it (a tray-less console session), this screen never calls
   `setFullscreen`.
   -------------------------------------------------------------------------- */

/** What the cleanup has to do with the window it was given. */
export type FullscreenRestore =
  /** Big Picture owns this fullscreen; hand the window back windowed. */
  | "exit"
  /** The window was already fullscreen and stays that way. */
  | "keep"
  /** Big Picture never took the window (console session, browser build). */
  | "none";

export interface FullscreenEntry {
  /** Fullscreen state at mount, before Big Picture touched anything. */
  wasFullscreen: boolean;
  /** Whether Big Picture itself switched fullscreen on. */
  entered: boolean;
}

/**
 * Decide how to restore the window on unmount.
 *
 * A window that was already fullscreen on mount is returned windowed, because
 * Big Picture is a screen the user leaves rather than a fullscreen mode they
 * chose to stay in — that is what makes the manual path escape a window that a
 * crash or a console session left fullscreen. A window Big Picture switched on
 * itself is likewise handed back.
 */
export function fullscreenRestoreOnExit(entry: FullscreenEntry): FullscreenRestore {
  if (entry.entered) return "exit";
  if (entry.wasFullscreen) return "exit";
  return "none";
}

/** The subset of the Rust window policy this decision needs (#269). */
export interface WindowPolicyView {
  fullscreen: boolean;
  tray: boolean;
}

/**
 * What Big Picture should do with the OS window:
 * - `none`: not the desktop app;
 * - `wait`: no policy yet (it may arrive on the next render);
 * - `native`: Rust owns the window (console session, or a policy that already
 *   put it fullscreen) — never fight it;
 * - `take-over`: the desktop window is this screen's to take, and to hand back
 *   on the way out (#233).
 */
export type BigPictureFullscreenPlan = "none" | "wait" | "native" | "take-over";

export interface FullscreenPlanInput {
  native: boolean;
  /**
   * `undefined` while loading, `null` on a desktop without `window_state`
   * (older build) — then the console-mode fields below are the fallback.
   */
  policy: WindowPolicyView | null | undefined;
  legacyConsoleReady: boolean;
  legacyConsoleActive: boolean;
  legacyConsoleFullscreen: boolean;
}

export function bigPictureFullscreenPlan(input: FullscreenPlanInput): BigPictureFullscreenPlan {
  if (!input.native) return "none";
  if (input.policy === undefined) return "wait";
  if (input.policy !== null) {
    // A tray-less window is a console session: Rust applied decorations and
    // fullscreen (`--windowed` included), so this screen has nothing to do.
    if (!input.policy.tray || input.policy.fullscreen) return "native";
    return "take-over";
  }
  // Pre-#269 behaviour when the desktop does not expose a policy.
  if (!input.legacyConsoleReady) return "wait";
  if (input.legacyConsoleActive && !input.legacyConsoleFullscreen) return "native";
  return "take-over";
}
