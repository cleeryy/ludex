import type { CueId } from "@/lib/ui-sound-specs";
import type { BigPictureCommand } from "./input";

/* --------------------------------------------------------------------------
   Command → cue table (#198).

   Big Picture routes every pad and keyboard press through a command, so the
   sound can be decided from the command alone. Two rules matter:

   - an edge press is silent: a direction that does not change the highlight
     has nothing to confirm, and a held stick must not click;
   - closing a level speaks with the level's own voice (overlay, app, or a
     plain "back"), never with the generic confirm.

   The screen also plays the cue of the action a pointer press triggers, so a
   mouse and a pad sound the same. Both paths share the manager's repeat gap,
   which collapses a double emission into one audible cue.
   -------------------------------------------------------------------------- */

export interface CommandView {
  /** The press actually changed the highlight (or moved to another surface). */
  moved: boolean;
  /** An overlay was open before the command ran. */
  overlayOpen: boolean;
  /** A system app (Settings, Power) was open before the command ran. */
  appOpen: boolean;
}

export function cueForCommand(command: BigPictureCommand, view: CommandView): CueId | null {
  switch (command.type) {
    case "move":
      // A press at the edge of a row is silent: there was no movement.
      return view.moved ? "move" : null;
    case "highlight":
      return view.moved ? "move" : null;
    case "shelf":
    case "page":
      return "tabChange";
    case "activate":
    case "select":
      return "confirm";
    case "open":
      return "overlayOpen";
    case "layout":
      // The layout picker is a surface of its own.
      return "overlayOpen";
    case "close":
      if (view.overlayOpen) return "overlayClose";
      if (view.appOpen) return "closeGroup";
      return "back";
    case "power":
      // Power is a system app: entering it is a group open, the confirmed
      // action itself plays `power`.
      return "openGroup";
    case "exit":
      // The confirm surface already played `overlayOpen`; quitting is `power`.
      return null;
    case "game":
      // The game page leaves Big Picture.
      return null;
    case "search":
      // Typing is not a console event; selecting a cell is `select`.
      return null;
    case "none":
      return null;
  }
}
