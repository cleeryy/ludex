import type { CueId } from "@/lib/ui-sound-specs";
import type { BigPictureCommand } from "./input";

/* --------------------------------------------------------------------------
   Command → cue table (#198, narrowed by #220).

   Big Picture routes every pad and keyboard press through a command, so the
   movement cue can be decided from the command alone. Two rules matter:

   - an edge press is silent: a direction that does not change the highlight
     has nothing to confirm, and a held stick must not click;
   - a command that opens or closes a surface does not speak here. The surface
     owns its cue through the motion table (`motion.ts`), fired once at the
     start of the transition. Deciding it in both places is how the old double
     cue happened: `open` played `overlayOpen` in the router and again in
     `openOverlay`.

   The screen also plays the cue of the action a pointer press triggers, so a
   mouse and a pad sound the same. Both paths share the manager's repeat gap,
   which collapses a double emission into one audible cue.
   -------------------------------------------------------------------------- */

export interface CommandView {
  /** The press actually changed the highlight (or moved to another surface). */
  moved: boolean;
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
    case "open":
    case "close":
    case "layout":
    case "power":
    case "exit":
    case "game":
    case "search":
    case "none":
      // The surface (overlay, app, layout picker, power panel) or the action it
      // runs owns the cue; firing here too would say it twice.
      return null;
  }
}
