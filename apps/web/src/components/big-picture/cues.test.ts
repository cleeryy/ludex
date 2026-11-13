import { describe, expect, it } from "vitest";
import type { BigPictureCommand } from "./input";
import { cueForCommand, type CommandView } from "./cues";

/* --------------------------------------------------------------------------
   #198 + #220: the command table speaks for movement only. Every surface cue
   belongs to the motion table, fired once at the start of the transition; a
   command that opens or closes something is silent here on purpose.
   -------------------------------------------------------------------------- */

const still: CommandView = { moved: false };
const moved: CommandView = { moved: true };

function cue(command: BigPictureCommand, view: CommandView = moved) {
  return cueForCommand(command, view);
}

describe("cueForCommand", () => {
  it("plays move only when the highlight actually moved", () => {
    expect(cue({ type: "move", direction: "left" }, moved)).toBe("move");
    expect(cue({ type: "move", direction: "right" }, still)).toBeNull();
    expect(cue({ type: "highlight", direction: "down" }, moved)).toBe("move");
    expect(cue({ type: "highlight", direction: "down" }, still)).toBeNull();
  });

  it("uses the tab voice for a shelf or page change", () => {
    expect(cue({ type: "shelf", delta: -1 })).toBe("tabChange");
    expect(cue({ type: "shelf", delta: 1 })).toBe("tabChange");
    expect(cue({ type: "page", delta: -1 })).toBe("tabChange");
    expect(cue({ type: "page", delta: 1 })).toBe("tabChange");
  });

  it("stays silent when a surface opens or closes", () => {
    // The motion table owns these: overlayOpen, overlayClose, openGroup,
    // closeGroup, confirm. Emitting them here as well was the double cue.
    for (const overlay of ["menu", "search", "profiles", "downloads", "exit", "collections"] as const) {
      expect(cue({ type: "open", overlay })).toBeNull();
    }
    expect(cue({ type: "close" })).toBeNull();
    expect(cue({ type: "layout" })).toBeNull();
    expect(cue({ type: "power" })).toBeNull();
  });

  it("leaves activation to the action that runs", () => {
    // `confirm()` emits for a play/install, `openOverlay` for the queue, and
    // `openApp` for Settings/Power; the router would only duplicate the first.
    expect(cue({ type: "activate" })).toBeNull();
    expect(cue({ type: "select" })).toBeNull();
  });

  it("is silent for commands that leave Big Picture or type", () => {
    expect(cue({ type: "game" })).toBeNull();
    expect(cue({ type: "none" })).toBeNull();
    expect(cue({ type: "search", step: "submit" })).toBeNull();
    expect(cue({ type: "search", step: "backspace" })).toBeNull();
    expect(cue({ type: "exit" })).toBeNull();
  });
});
