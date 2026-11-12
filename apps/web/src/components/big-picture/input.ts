import type { ActionSource, GamepadAction } from "@/lib/gamepad-core";
import { isMenuSource, isPageSource, type HintContext } from "./controller";
import type { MoveDirection } from "./layout";

/* --------------------------------------------------------------------------
   Input → command routing for Big Picture (#138).

   The screen used to translate keys and pad buttons straight into closures,
   which made "what does B do inside the profile overlay" a question answered in
   three places. This table answers it once, for both input devices, as a value:
   the screen only executes the command. Pure data in, pure data out — the test
   suite covers every context without a DOM.
   -------------------------------------------------------------------------- */

export type BigPictureOverlay = "menu" | "search" | "profiles" | "downloads" | "exit";

/** Steps the on-screen keyboard understands beyond the printable keys. */
export type SearchStep = "space" | "backspace" | "clear" | "submit";

export type BigPictureCommand =
  | { type: "move"; direction: MoveDirection }
  | { type: "shelf"; delta: -1 | 1 }
  | { type: "page"; delta: -1 | 1 }
  | { type: "activate" }
  | { type: "open"; overlay: BigPictureOverlay }
  | { type: "close" }
  | { type: "highlight"; direction: MoveDirection }
  | { type: "select" }
  | { type: "search"; step: SearchStep }
  | { type: "layout" }
  | { type: "game" }
  | { type: "none" };

/** The hint strip follows the same context the router used. */
export function hintContextFor(overlay: BigPictureOverlay | null): HintContext {
  return overlay ?? "library";
}

/**
 * One pad action, one command. With an overlay open the pad belongs to the
 * overlay: directions move its highlight, A selects, B/Start close it, and no
 * navigation reaches the library behind it. In the library, X/Square opens
 * search while Start/Options and Guide open the command overlay, LT/RT jump a
 * page and LB/RB change shelf — the distinction uses the physical source, so a
 * remapped button still fires the logical action.
 */
export function routeGamepadAction(
  overlay: BigPictureOverlay | null,
  action: GamepadAction,
  source: ActionSource | null = null,
): BigPictureCommand {
  if (overlay === "search") {
    switch (action) {
      case "left":
      case "right":
      case "up":
      case "down":
        return { type: "highlight", direction: action };
      case "confirm":
        return { type: "select" };
      case "back":
        return { type: "close" };
      case "menu":
        // Start/Guide closes the overlay; X keeps deleting, as the hint says.
        return isMenuSource(source) ? { type: "close" } : { type: "search", step: "backspace" };
      case "downloads":
        return { type: "search", step: "space" };
      case "prevTab":
        return { type: "search", step: "clear" };
      case "nextTab":
        return { type: "search", step: "submit" };
    }
  }

  if (overlay) {
    switch (action) {
      case "left":
      case "up":
        return { type: "highlight", direction: "up" };
      case "right":
      case "down":
        return { type: "highlight", direction: "down" };
      case "confirm":
        return { type: "select" };
      case "back":
      case "menu":
        return { type: "close" };
      case "downloads":
      case "prevTab":
      case "nextTab":
        return { type: "none" };
    }
  }

  switch (action) {
    case "left":
    case "right":
    case "up":
    case "down":
      return { type: "move", direction: action };
    case "confirm":
      return { type: "activate" };
    case "back":
      // B opens the visible exit confirmation instead of leaving immediately.
      return { type: "open", overlay: "exit" };
    case "menu":
      return isMenuSource(source) ? { type: "open", overlay: "menu" } : { type: "open", overlay: "search" };
    case "downloads":
      return { type: "open", overlay: "downloads" };
    case "prevTab":
      return isPageSource(source) ? { type: "page", delta: -1 } : { type: "shelf", delta: -1 };
    case "nextTab":
      return isPageSource(source) ? { type: "page", delta: 1 } : { type: "shelf", delta: 1 };
  }
}

/**
 * Keyboard parity (#138). Returns `null` when the key belongs to the DOM (the
 * search field's printable characters and its native backspace), so typing
 * stays native and only the navigation keys are claimed.
 */
export function routeKey(overlay: BigPictureOverlay | null, key: string): BigPictureCommand | null {
  if (overlay === "search") {
    switch (key) {
      case "Escape":
        return { type: "close" };
      case "Enter":
        // A keyboard player types letters directly, so Enter opens the
        // highlighted result, or the best match when the ring is still on the
        // keyboard. The pad keeps A for pressing the highlighted cell.
        return { type: "search", step: "submit" };
      case "ArrowLeft":
      case "ArrowRight":
      case "ArrowUp":
      case "ArrowDown":
        return { type: "highlight", direction: key.slice(5).toLowerCase() as MoveDirection };
      case "PageUp":
      case "PageDown":
        return null;
      default:
        return null;
    }
  }

  if (overlay) {
    switch (key) {
      case "ArrowUp":
      case "ArrowLeft":
        return { type: "highlight", direction: "up" };
      case "ArrowDown":
      case "ArrowRight":
        return { type: "highlight", direction: "down" };
      case "Enter":
        return { type: "select" };
      case "Escape":
        return { type: "close" };
      default:
        return null;
    }
  }

  switch (key) {
    case "ArrowLeft":
    case "ArrowRight":
    case "ArrowUp":
    case "ArrowDown":
      return { type: "move", direction: key.slice(5).toLowerCase() as MoveDirection };
    case "Enter":
      return { type: "activate" };
    case "Escape":
    case "Backspace":
      return { type: "open", overlay: "exit" };
    case "[":
      return { type: "shelf", delta: -1 };
    case "]":
      return { type: "shelf", delta: 1 };
    case "PageUp":
      return { type: "page", delta: -1 };
    case "PageDown":
      return { type: "page", delta: 1 };
    case "/":
    case "s":
      return { type: "open", overlay: "search" };
    case "m":
    case "M":
      return { type: "open", overlay: "menu" };
    case "p":
    case "P":
      return { type: "open", overlay: "profiles" };
    case "d":
    case "D":
      return { type: "open", overlay: "downloads" };
    case "l":
    case "L":
      return { type: "layout" };
    case "g":
    case "G":
      return { type: "game" };
    default:
      return null;
  }
}
