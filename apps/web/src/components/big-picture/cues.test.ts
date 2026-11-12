import { describe, expect, it } from "vitest";
import type { BigPictureCommand } from "./input";
import { cueForCommand, type CommandView } from "./cues";

/* --------------------------------------------------------------------------
   #198: the command table. Every routed command has exactly one decision, the
   null cases included, and an edge press (no movement) stays silent.
   -------------------------------------------------------------------------- */

const still: CommandView = { moved: false, overlayOpen: false, appOpen: false };
const moved: CommandView = { ...still, moved: true };

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

  it("confirms an activation or a selection", () => {
    expect(cue({ type: "activate" })).toBe("confirm");
    expect(cue({ type: "select" })).toBe("confirm");
  });

  it("announces an opening surface", () => {
    for (const overlay of ["menu", "search", "profiles", "downloads", "exit", "collections"] as const) {
      expect(cue({ type: "open", overlay })).toBe("overlayOpen");
    }
    expect(cue({ type: "layout" })).toBe("overlayOpen");
  });

  it("closes with the level's own voice", () => {
    expect(cue({ type: "close" }, { ...still, overlayOpen: true })).toBe("overlayClose");
    expect(cue({ type: "close" }, { ...still, appOpen: true })).toBe("closeGroup");
    expect(cue({ type: "close" }, still)).toBe("back");
  });

  it("enters Power as a system group; the confirmed action plays power", () => {
    expect(cue({ type: "power" })).toBe("openGroup");
    expect(cue({ type: "exit" })).toBeNull();
  });

  it("is silent for commands that leave Big Picture or type", () => {
    expect(cue({ type: "game" })).toBeNull();
    expect(cue({ type: "none" })).toBeNull();
    expect(cue({ type: "search", step: "submit" })).toBeNull();
    expect(cue({ type: "search", step: "backspace" })).toBeNull();
  });
});
