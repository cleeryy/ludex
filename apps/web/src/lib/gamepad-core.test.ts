import { describe, expect, it, vi } from "vitest";
import { presetProfile } from "@/lib/controller-profiles";
import {
  applyGamepadFocus,
  collectRepeatActions,
  createRepeatTracker,
  cueHapticPattern,
  cueHapticPulses,
  FIRST_REPEAT_MS,
  hapticPattern,
  hasBlockingModal,
  moveTargetIndex,
  padHasInput,
  parseGamepadDebugFlag,
  readPadActions,
  readPadPresses,
  REPEAT_MS,
  restoreGamepadFocus,
  rowsFromRects,
  sanitizeHapticsStrength,
  scaleHapticPattern,
  selectActivePad,
  type GamepadAction,
  type PadLike,
  type TargetRect,
} from "@/lib/gamepad-core";
import { CUE_IDS } from "@/lib/ui-sound-specs";

function pad(overrides: Partial<PadLike> = {}): PadLike {
  return {
    index: 0,
    id: "Xbox Wireless Controller",
    connected: true,
    mapping: "standard",
    timestamp: 0,
    axes: [0, 0, 0, 0],
    buttons: Array.from({ length: 18 }, () => ({ pressed: false, value: 0 })),
    ...overrides,
  };
}

function buttons(pressed: Record<number, number>): Array<{ pressed: boolean; value: number }> {
  return Array.from({ length: 18 }, (_, index) => ({
    pressed: index in pressed,
    value: pressed[index] ?? 0,
  }));
}

function press(...actions: GamepadAction[]): Set<GamepadAction> {
  return new Set<GamepadAction>(actions);
}

function target() {
  return {
    focus: vi.fn(),
    setAttribute: vi.fn(),
    removeAttribute: vi.fn(),
    scrollIntoView: vi.fn(),
    offsetParent: {},
  };
}

describe("gamepad detection and hot-plug", () => {
  const empty = { activeIndex: null, connected: [] };

  it("returns null when nothing is connected", () => {
    expect(selectActivePad([], empty).pad).toBeNull();
    expect(selectActivePad([null, undefined], empty).selection.activeIndex).toBeNull();
  });

  it("selects the only pad and reports its index", () => {
    const only = pad({ index: 2, id: "DualSense", mapping: "standard" });
    const result = selectActivePad([null, null, only], empty);
    expect(result.pad?.id).toBe("DualSense");
    expect(result.selection).toEqual({ activeIndex: 2, connected: [2] });
  });

  it("keeps the active pad while it is connected", () => {
    const first = pad({ index: 0 });
    const second = pad({ index: 1, id: "DualSense" });
    const result = selectActivePad([first, second], { activeIndex: 0, connected: [0, 1] });
    expect(result.pad?.index).toBe(0);
  });

  it("switches to the pad that produced input most recently", () => {
    const first = pad({ index: 0 });
    const second = pad({ index: 1, id: "DualSense" });
    const result = selectActivePad([first, second], { activeIndex: 0, connected: [0, 1] }, null, {
      index: 1,
      at: 260,
    });
    expect(result.pad?.index).toBe(1);
    expect(result.selection.activeIndex).toBe(1);
  });

  it("ignores activity from a pad that is no longer connected", () => {
    const first = pad({ index: 0 });
    const second = pad({ index: 1, id: "DualSense" });
    const result = selectActivePad([first, second], { activeIndex: 0, connected: [0, 1] }, null, {
      index: 4,
      at: 260,
    });
    expect(result.pad?.index).toBe(0);
  });

  it("recovers when the active pad is unplugged", () => {
    const remaining = pad({ index: 1, id: "DualSense" });
    const result = selectActivePad([null, remaining], { activeIndex: 0, connected: [0, 1] });
    expect(result.pad?.index).toBe(1);
    expect(result.selection.connected).toEqual([1]);
  });

  it("uses the freshly connected pad when nothing was active", () => {
    const first = pad({ index: 0 });
    const second = pad({ index: 1, id: "DualSense" });
    expect(selectActivePad([first, second], empty, 1).pad?.index).toBe(1);
  });

  it("reports which pads carry input", () => {
    expect(padHasInput(pad())).toBe(false);
    expect(padHasInput(pad({ axes: [0, -0.9, 0, 0] }))).toBe(true);
    expect(padHasInput(pad({ buttons: buttons({ 0: 1 }) }))).toBe(true);
  });
});

describe("gamepad input decoding", () => {
  const profile = presetProfile("xbox");

  it("maps d-pad buttons and face buttons", () => {
    const actions = readPadActions(
      profile,
      pad({ buttons: buttons({ 0: 1, 12: 1, 15: 1, 4: 1 }) }),
    );
    expect(actions.has("confirm")).toBe(true);
    expect(actions.has("up")).toBe(true);
    expect(actions.has("right")).toBe(true);
    expect(actions.has("prevTab")).toBe(true);
  });

  it("reads the left stick vertical component as up/down, not left/right", () => {
    expect(readPadActions(profile, pad({ axes: [0, -0.9, 0, 0] })).has("up")).toBe(true);
    expect(readPadActions(profile, pad({ axes: [0, -0.9, 0, 0] })).has("left")).toBe(false);
    expect(readPadActions(profile, pad({ axes: [0, 0.9, 0, 0] })).has("down")).toBe(true);
    expect(readPadActions(profile, pad({ axes: [0.9, 0, 0, 0] })).has("right")).toBe(true);
    expect(readPadActions(profile, pad({ axes: [-0.9, 0, 0, 0] })).has("left")).toBe(true);
  });

  it("moves only the dominant direction on a diagonal stick", () => {
    expect(readPadActions(profile, pad({ axes: [0.95, -0.95, 0, 0] }))).toEqual(new Set(["right"]));
    expect(readPadActions(profile, pad({ axes: [0.3, -0.95, 0, 0] }))).toEqual(new Set(["up"]));
  });

  it("applies the deadzone before firing a direction", () => {
    expect(readPadActions(profile, pad({ axes: [0.1, 0.1, 0, 0] })).size).toBe(0);
    expect(readPadActions(profile, pad({ axes: [0.95, 0, 0, 0] })).has("right")).toBe(true);
  });

  it("honours inversion and the response curve from the profile", () => {
    const inverted = { ...profile, invertY: true };
    expect(readPadActions(inverted, pad({ axes: [0, -0.9, 0, 0] })).has("down")).toBe(true);
    expect(readPadActions(inverted, pad({ axes: [0, -0.9, 0, 0] })).has("up")).toBe(false);

    const linear = { ...profile, curve: 1 };
    const curved = { ...profile, curve: 0.5 };
    // 0.4 is inside the deadzone-normalised shoulder only after the curve.
    expect(readPadActions(linear, pad({ axes: [0.4, 0, 0, 0] })).size).toBe(0);
    expect(readPadActions(curved, pad({ axes: [0.4, 0, 0, 0] })).has("right")).toBe(true);
  });

  it("treats pressed buttons reported with value 0 as pressed", () => {
    const stuck = pad({ buttons: buttons({}) });
    const repaired = Array.from(stuck.buttons, (button, index) =>
      index === 0 ? { pressed: true, value: 0 } : button,
    );
    expect(readPadActions(profile, pad({ buttons: repaired })).has("confirm")).toBe(true);
  });
});

describe("gamepad action sources", () => {
  const profile = presetProfile("xbox");

  it("reports the physical button behind each action", () => {
    const presses = readPadPresses(profile, pad({ buttons: buttons({ 0: 1, 2: 1, 4: 1 }) }));
    const source = (action: GamepadAction) => presses.find((press) => press.action === action)?.source;
    expect(source("confirm")).toEqual({ kind: "button", index: 0 });
    expect(source("prevTab")).toEqual({ kind: "button", index: 4 });
    expect(source("menu")).toEqual({ kind: "button", index: 2 });
  });

  it("reports the stick axis and the push direction", () => {
    const presses = readPadPresses(profile, pad({ axes: [0, -0.9, 0, 0] }));
    expect(presses).toEqual([{ action: "up", source: { kind: "axis", index: 1, direction: -1 } }]);
    expect(readPadPresses(profile, pad({ axes: [0.9, 0, 0, 0] }))).toEqual([
      { action: "right", source: { kind: "axis", index: 0, direction: 1 } },
    ]);
  });

  it("keeps one entry per action, preferring the first physical source", () => {
    const presses = readPadPresses(profile, pad({ axes: [0.95, 0, 0, 0], buttons: buttons({ 15: 1 }) }));
    expect(presses.filter((press) => press.action === "right")).toHaveLength(1);
    expect(presses.find((press) => press.action === "right")?.source).toEqual({ kind: "button", index: 15 });
  });

  it("never reports a source for an action the profile does not map", () => {
    const presses = readPadPresses(profile, pad({ buttons: buttons({ 8: 1, 10: 1, 11: 1 }) }));
    expect(presses).toEqual([]);
  });
});

describe("haptic patterns", () => {
  it("gives confirms a stronger pattern than focus moves", () => {
    const confirm = hapticPattern("confirm");
    const move = hapticPattern("left");
    expect(confirm).not.toBeNull();
    expect(move).not.toBeNull();
    expect(confirm!.duration).toBeGreaterThan(move!.duration);
    expect(confirm!.strongMagnitude).toBeGreaterThan(move!.strongMagnitude);
  });

  it("covers every logical action the interface accepts", () => {
    const actions: GamepadAction[] = [
      "left",
      "right",
      "up",
      "down",
      "confirm",
      "back",
      "menu",
      "downloads",
      "prevTab",
      "nextTab",
    ];
    for (const action of actions) {
      const pattern = hapticPattern(action);
      expect(pattern).not.toBeNull();
      expect(pattern!.duration).toBeGreaterThan(0);
      expect(pattern!.strongMagnitude).toBeGreaterThan(0);
      expect(pattern!.strongMagnitude).toBeLessThanOrEqual(1);
    }
  });
});

describe("cue haptics (#198)", () => {
  it("has a pattern for every cue", () => {
    for (const cue of CUE_IDS) {
      const pattern = cueHapticPattern(cue);
      expect(pattern, cue).not.toBeNull();
      expect(pattern!.duration).toBeGreaterThan(0);
      expect(pattern!.strongMagnitude).toBeGreaterThan(0);
      expect(pattern!.strongMagnitude).toBeLessThanOrEqual(1);
      expect(pattern!.weakMagnitude).toBeLessThanOrEqual(1);
    }
  });

  it("feels different per cue: launch and power are heavier than a move", () => {
    const move = cueHapticPattern("move")!;
    const confirm = cueHapticPattern("confirm")!;
    const launch = cueHapticPattern("launch")!;
    const power = cueHapticPattern("power")!;
    expect(confirm.strongMagnitude).toBeGreaterThan(move.strongMagnitude);
    expect(launch.duration).toBeGreaterThan(confirm.duration);
    expect(power.duration).toBeGreaterThan(launch.duration);
    expect(power.strongMagnitude).toBeGreaterThanOrEqual(launch.strongMagnitude);
  });

  it("scales by strength, clamps at 1 and silences at off", () => {
    const base = cueHapticPattern("confirm")!;
    expect(scaleHapticPattern(base, "standard")).toEqual(base);
    expect(scaleHapticPattern(base, "off")).toBeNull();
    const strong = scaleHapticPattern(base, "strong")!;
    expect(strong.strongMagnitude).toBeGreaterThan(base.strongMagnitude);
    expect(strong.strongMagnitude).toBeLessThanOrEqual(1);
    expect(strong.duration).toBe(base.duration);

    const power = cueHapticPattern("power")!;
    expect(scaleHapticPattern(power, "strong")!.strongMagnitude).toBeCloseTo(0.81, 2);
    expect(
      scaleHapticPattern({ duration: 50, strongMagnitude: 0.9, weakMagnitude: 0.9 }, "strong")!.strongMagnitude,
    ).toBe(1);
  });

  it("sends error as two 40 ms pulses 60 ms apart", () => {
    const pulses = cueHapticPulses("error");
    expect(pulses).toHaveLength(2);
    expect(pulses[0].duration).toBe(40);
    expect(pulses[1].delayMs - pulses[0].duration).toBe(60);
  });

  it("keeps one pulse for every other cue", () => {
    for (const cue of CUE_IDS) {
      if (cue === "error") continue;
      const pulses = cueHapticPulses(cue);
      expect(pulses, cue).toHaveLength(1);
      expect(pulses[0].delayMs).toBe(0);
    }
  });

  it("falls back to standard for unknown stored strengths", () => {
    expect(sanitizeHapticsStrength("off")).toBe("off");
    expect(sanitizeHapticsStrength("strong")).toBe("strong");
    expect(sanitizeHapticsStrength("standard")).toBe("standard");
    expect(sanitizeHapticsStrength("loud")).toBe("standard");
    expect(sanitizeHapticsStrength(undefined)).toBe("standard");
  });
});

describe("gamepad repeat timing", () => {  it("fires once per press, then repeats after the initial delay", () => {
    const tracker = createRepeatTracker();
    expect(collectRepeatActions(tracker, press("right"), 1_000)).toEqual(["right"]);
    expect(collectRepeatActions(tracker, press("right"), 1_050)).toEqual([]);
    expect(collectRepeatActions(tracker, press("right"), 1_000 + FIRST_REPEAT_MS)).toEqual([
      "right",
    ]);
    expect(
      collectRepeatActions(tracker, press("right"), 1_000 + FIRST_REPEAT_MS + REPEAT_MS - 1),
    ).toEqual([]);
    expect(
      collectRepeatActions(tracker, press("right"), 1_000 + FIRST_REPEAT_MS + REPEAT_MS),
    ).toEqual(["right"]);
  });

  it("re-arms immediately after release", () => {
    const tracker = createRepeatTracker();
    collectRepeatActions(tracker, press("left"), 0);
    expect(collectRepeatActions(tracker, press(), 100)).toEqual([]);
    expect(collectRepeatActions(tracker, press("left"), 200)).toEqual(["left"]);
  });

  it("never repeats confirm, back, menu, downloads or shoulders", () => {
    const tracker = createRepeatTracker();
    expect(collectRepeatActions(tracker, press("confirm"), 0)).toEqual(["confirm"]);
    expect(collectRepeatActions(tracker, press("confirm"), 5_000)).toEqual([]);
    expect(collectRepeatActions(tracker, press("nextTab"), 6_000)).toEqual(["nextTab"]);
    expect(collectRepeatActions(tracker, press("nextTab"), 7_000)).toEqual([]);
  });

  it("swallows actions while a modal is open without replaying a burst", () => {
    const tracker = createRepeatTracker();
    expect(collectRepeatActions(tracker, press("down"), 0, true)).toEqual([]);
    expect(collectRepeatActions(tracker, press("down"), 100, true)).toEqual([]);
    expect(collectRepeatActions(tracker, press("down"), 200, false)).toEqual([]);
    expect(collectRepeatActions(tracker, press("down"), FIRST_REPEAT_MS, false)).toEqual([
      "down",
    ]);
  });
});

describe("gamepad target geometry", () => {
  const grid: TargetRect[] = [];
  for (let row = 0; row < 3; row += 1) {
    for (let column = 0; column < 4; column += 1) {
      grid.push({ top: row * 120, left: column * 90, width: 80, height: 100 });
    }
  }

  it("groups a grid into rows ordered left to right", () => {
    expect(rowsFromRects(grid).map((row) => row.length)).toEqual([4, 4, 4]);
    expect(rowsFromRects(grid)[1]).toEqual([4, 5, 6, 7]);
  });

  it("moves inside a row and clamps at the edges", () => {
    expect(moveTargetIndex(grid, 1, "right")).toBe(2);
    expect(moveTargetIndex(grid, 2, "left")).toBe(1);
    expect(moveTargetIndex(grid, 0, "left")).toBe(0);
    expect(moveTargetIndex(grid, 3, "right")).toBe(3);
  });

  it("moves between rows by column", () => {
    expect(moveTargetIndex(grid, 1, "down")).toBe(5);
    expect(moveTargetIndex(grid, 6, "up")).toBe(2);
    expect(moveTargetIndex(grid, 0, "up")).toBe(0);
    expect(moveTargetIndex(grid, 9, "down")).toBe(9);
  });

  it("handles a shorter last row and a single-column list", () => {
    const ragged: TargetRect[] = [
      { top: 0, left: 0, width: 80, height: 100 },
      { top: 0, left: 90, width: 80, height: 100 },
      { top: 0, left: 180, width: 80, height: 100 },
      { top: 120, left: 0, width: 80, height: 100 },
    ];
    expect(moveTargetIndex(ragged, 2, "down")).toBe(3);
    expect(moveTargetIndex(ragged, 1, "down")).toBe(3);

    const list: TargetRect[] = Array.from({ length: 5 }, (_, index) => ({
      top: index * 44,
      left: 0,
      width: 320,
      height: 40,
    }));
    expect(moveTargetIndex(list, 1, "down")).toBe(2);
    expect(moveTargetIndex(list, 2, "up")).toBe(1);
    expect(moveTargetIndex(list, 3, "right")).toBe(3);
  });

  it("returns 0 for an empty target list", () => {
    expect(moveTargetIndex([], 3, "down")).toBe(0);
  });
});

describe("gamepad modal guard and focus restore", () => {
  it("only treats aria-modal dialogs as blocking", () => {
    expect(hasBlockingModal(null)).toBe(false);
    expect(hasBlockingModal({ querySelector: () => null })).toBe(false);
    expect(
      hasBlockingModal({
        querySelector: (selector) =>
          selector === '[role="dialog"][aria-modal="true"]' ? {} : null,
      }),
    ).toBe(true);
  });

  it("marks and focuses the resolved target", () => {
    const first = target();
    const second = target();
    expect(applyGamepadFocus([first, second], 1, { smooth: true })).toBe(second);
    expect(first.removeAttribute).toHaveBeenCalledWith("data-gamepad-focus");
    expect(second.setAttribute).toHaveBeenCalledWith("data-gamepad-focus", "true");
    expect(second.focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(second.scrollIntoView).toHaveBeenCalledWith({
      block: "nearest",
      inline: "nearest",
      behavior: "smooth",
    });
  });

  it("does not scroll when the caller owns scrolling", () => {
    const only = target();
    applyGamepadFocus([only], 0, { scroll: false });
    expect(only.scrollIntoView).not.toHaveBeenCalled();
  });

  it("restores the marked target after a dialog closes", () => {
    const marked = target();
    const restored = restoreGamepadFocus({
      querySelector: (selector) =>
        selector === '[data-gamepad-focus="true"]' ? marked : null,
    });
    expect(restored).toBe(marked);
    expect(marked.focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(marked.setAttribute).toHaveBeenCalledWith("data-gamepad-focus", "true");
  });

  it("falls back to the first target only when asked and refuses hidden ones", () => {
    const first = target();
    const host = {
      querySelector: (selector: string) =>
        selector === '[data-gamepad-focus="true"]' ? null : first,
    };
    expect(restoreGamepadFocus(host)).toBeNull();
    expect(restoreGamepadFocus(host, { fallback: true })).toBe(first);

    const hidden = { ...target(), offsetParent: null };
    const hiddenHost = {
      querySelector: (selector: string) =>
        selector === '[data-gamepad-focus="true"]' ? null : hidden,
    };
    expect(restoreGamepadFocus(hiddenHost, { fallback: true })).toBeNull();
  });
});

describe("gamepad debug flag", () => {
  it("accepts the flag in the search, the hash query and as true", () => {
    expect(parseGamepadDebugFlag("?gamepadDebug=1")).toBe(true);
    expect(parseGamepadDebugFlag("#/library?gamepadDebug=1")).toBe(true);
    expect(parseGamepadDebugFlag("#/library?gamepadDebug=true")).toBe(true);
    expect(parseGamepadDebugFlag("?gamepadDebug=true#/library")).toBe(true);
  });

  it("stays off for absent or false values", () => {
    expect(parseGamepadDebugFlag("")).toBe(false);
    expect(parseGamepadDebugFlag("#/library")).toBe(false);
    expect(parseGamepadDebugFlag("?gamepadDebug=0")).toBe(false);
    expect(parseGamepadDebugFlag("?gamepadDebug=yes")).toBe(false);
  });
});
