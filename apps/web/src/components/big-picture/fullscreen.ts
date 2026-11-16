/* --------------------------------------------------------------------------
   #233: Big Picture takes over the OS window while it is open, and every exit
   has to hand the window back in the state it was found.

   The screen used to remember only whether *it* called `setFullscreen(true)`.
   A window that was already fullscreen when Big Picture mounted (restored from
   a previous session, or a manual visit from a fullscreen desktop window) then
   matched neither branch: the enter effect saw fullscreen and did nothing, and
   the cleanup skipped its restore because it had not entered. The window stayed
   fullscreen with no way back from inside the screen.
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

/**
 * Whether the enter effect should switch fullscreen on. It must not touch a
 * window that is already fullscreen, and a console session owns its own window.
 */
export function shouldEnterFullscreen(input: {
  isNative: boolean;
  consoleReady: boolean;
  consoleActive: boolean;
  consoleFullscreen: boolean;
  alreadyFullscreen: boolean;
}): boolean {
  if (!input.isNative) return false;
  if (!input.consoleReady) return false;
  if (input.consoleActive && !input.consoleFullscreen) return false;
  return !input.alreadyFullscreen;
}
