import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CUE_SPECS } from "@/lib/ui-sound-specs";
import {
  INSTANT_HOLD_MS,
  MOTION,
  MOTION_DURATION,
  MOTION_KINDS,
  MOTION_STAGGER,
  REDUCED_FADE,
  REDUCED_MOTION,
  createTransitionHub,
  cueForMotion,
  cuePhaseForMotion,
  exitOf,
  missingCues,
  motionDebugEnabled,
  transitionOf,
  type MotionKind,
} from "./motion";

/* --------------------------------------------------------------------------
   #220: the motion vocabulary is a table, so it is tested like one. The token
   test reads the real stylesheet: a retune cannot land in CSS only. The cue
   test pins the reduced-motion split that #198 documented, so a surface cue
   cannot become "decorative" (and disappear) by accident.
   -------------------------------------------------------------------------- */

const css = readFileSync(new URL("../../styles/index.css", import.meta.url), "utf8");

/** The five cues #198 marks decorative; reduced motion drops them. */
const DECORATIVE_CUES = ["openGroup", "closeGroup", "overlayOpen", "overlayClose", "tabChange"] as const;

/** The console sequence the acceptance record walks. */
const CONSOLE_SEQUENCE: MotionKind[] = [
  "boot",
  "focus",
  "shelf",
  "layout",
  "screenPush",
  "settings",
  "overlayOpen",
  "overlayClose",
  "powerConfirm",
  "screenPop",
];

describe("motion tokens", () => {
  it("mirrors the CSS duration and easing tokens", () => {
    expect(css).toContain(`--dur-1: ${MOTION_DURATION.fast * 1000}ms;`);
    expect(css).toContain(`--dur-2: ${MOTION_DURATION.base * 1000}ms;`);
    expect(css).toContain(`--dur-3: ${MOTION_DURATION.slow * 1000}ms;`);
    expect(css).toContain("--ease: cubic-bezier(0.2, 0.6, 0.2, 1);");
    expect(css).toContain("--ease-out: cubic-bezier(0.16, 1, 0.3, 1);");
  });

  it("keeps every transition inside the 120–320ms console band", () => {
    for (const kind of MOTION_KINDS) {
      const spec = MOTION[kind];
      expect(spec.duration, kind).toBeGreaterThanOrEqual(0.12);
      expect(spec.duration, kind).toBeLessThanOrEqual(0.32);
      expect(spec.exit, kind).toBeGreaterThanOrEqual(0.1);
      expect(spec.exit, kind).toBeLessThanOrEqual(spec.duration);
    }
  });

  it("uses the 22ms cascade rhythm, not a custom stagger", () => {
    expect(MOTION_STAGGER * 1000).toBe(22);
  });

  it("never animates a layout property", () => {
    const allowed = new Set(["x", "y", "scale", "opacity"]);
    for (const kind of MOTION_KINDS) {
      for (const key of Object.keys(MOTION[kind])) {
        if (["duration", "exit", "ease", "reduced", "cue", "cueAt"].includes(key)) continue;
        expect(allowed.has(key), `${kind}.${key}`).toBe(true);
      }
    }
    for (const kind of MOTION_KINDS) {
      const frame = transitionOf(kind, {});
      for (const target of [frame.initial, frame.animate]) {
        if (!target) continue;
        for (const key of Object.keys(target)) expect(allowed.has(key), `${kind} ${key}`).toBe(true);
      }
    }
    expect(css).not.toMatch(/transition:\s*all/);
  });
});

describe("reduced motion mapping", () => {
  it("documents one mapping per kind and matches the spec", () => {
    expect(Object.keys(REDUCED_MOTION).sort()).toEqual([...MOTION_KINDS].sort());
    for (const kind of MOTION_KINDS) {
      expect(MOTION[kind].reduced, kind).toBe(REDUCED_MOTION[kind]);
    }
  });

  it("maps fade to a 120ms opacity tween and none to no motion", () => {
    expect(REDUCED_FADE).toBe(0.12);
    for (const kind of MOTION_KINDS) {
      const frame = transitionOf(kind, { reduce: true });
      if (REDUCED_MOTION[kind] === "none") {
        expect(frame.initial, kind).toBe(false);
        expect(frame.transition, kind).toMatchObject({ duration: 0 });
        continue;
      }
      expect(frame.initial, kind).toEqual({ opacity: 0 });
      expect(frame.transition, kind).toMatchObject({ duration: REDUCED_FADE });
    }
  });

  it("drops focus travel but keeps the ring (no delay in the cue path)", () => {
    expect(REDUCED_MOTION.focus).toBe("none");
    expect(transitionOf("focus", { reduce: true })).toMatchObject({ initial: false });
  });
});

describe("transition frames", () => {
  it("lands on the final frame for a held or repeated input", () => {
    for (const kind of MOTION_KINDS) {
      const frame = transitionOf(kind, { instant: true });
      expect(frame.initial, kind).toBe(false);
      expect(frame.transition, kind).toMatchObject({ duration: 0 });
    }
  });

  it("keeps the held suppression past a frame but under the pad repeat", () => {
    // Long enough for the commit's style resolution to see it, short enough
    // that a fresh press is never swallowed (the pad repeats every 130 ms).
    expect(INSTANT_HOLD_MS).toBeGreaterThan(16.7);
    expect(INSTANT_HOLD_MS).toBeLessThan(130);
  });

  it("mounts settled without replaying the entrance", () => {
    expect(transitionOf("boot", { settled: true })).toMatchObject({ initial: false });
    expect(transitionOf("layout", { settled: true })).toMatchObject({ initial: false });
  });

  it("moves only on enter; exits are an opacity fade with pointer-events off", () => {
    // `focus` has no tween of its own: the rail card scales in CSS, so there is
    // no JS frame to inspect here.
    for (const kind of MOTION_KINDS.filter((entry) => entry !== "focus")) {
      const frame = transitionOf(kind, {});
      expect(frame.initial, kind).not.toBe(false);
      const exit = exitOf(kind, {}).exit as Record<string, unknown>;
      expect(exit).toMatchObject({ opacity: 0, pointerEvents: "none" });
      expect(exit).not.toHaveProperty("x");
      expect(exit).not.toHaveProperty("y");
      expect(exit).not.toHaveProperty("scale");
    }
  });

  it("flips the shelf travel with the direction", () => {
    const forward = transitionOf("shelf", { direction: 1 }).initial as Record<string, number>;
    const backward = transitionOf("shelf", { direction: -1 }).initial as Record<string, number>;
    expect(forward.x).toBeGreaterThan(0);
    expect(backward.x).toBe(-forward.x);
  });
});

describe("interruption hub", () => {
  it("records the cut and keeps the newest transition active", () => {
    let now = 0;
    const hub = createTransitionHub(() => now);
    const first = hub.begin("shelf", { at: 0 });
    expect(first.interrupted).toBeNull();
    now = 40;
    const second = hub.begin("layout", { at: 40 });
    expect(second.interrupted).toBe("shelf");
    expect(hub.active()?.kind).toBe("layout");
    expect(hub.interruptions()).toBe(1);
  });

  it("never blocks input", () => {
    const hub = createTransitionHub();
    hub.begin("screenPush");
    expect(hub.accepts()).toBe(true);
    hub.begin("overlayOpen");
    expect(hub.accepts()).toBe(true);
  });

  it("ignores a late completion from the transition that was cut", () => {
    const hub = createTransitionHub();
    hub.begin("shelf");
    hub.begin("layout");
    expect(hub.end("shelf")).toBe(false);
    expect(hub.active()?.kind).toBe("layout");
    expect(hub.end("layout")).toBe(true);
    expect(hub.active()).toBeNull();
    expect(hub.last()?.kind).toBe("layout");
  });

  it("counts a restart of the same kind as a new generation", () => {
    const hub = createTransitionHub();
    const first = hub.begin("focus");
    const second = hub.begin("focus");
    expect(second.generation).toBe(first.generation + 1);
    expect(second.interrupted).toBe("focus");
    expect(hub.interruptions()).toBe(1);
  });

  it("survives a whole console walk", () => {
    const hub = createTransitionHub();
    for (const kind of CONSOLE_SEQUENCE) hub.begin(kind);
    expect(hub.last()?.kind).toBe("screenPop");
    expect(hub.interruptions()).toBe(CONSOLE_SEQUENCE.length - 1);
  });
});

describe("cue sync", () => {
  it("only names cues that ship with the sound set", () => {
    expect(missingCues()).toEqual([]);
    for (const kind of MOTION_KINDS) {
      const cue = cueForMotion(kind);
      if (cue) expect(CUE_SPECS[cue]).toBeDefined();
    }
  });

  it("fires every surface cue at the motion start", () => {
    for (const kind of MOTION_KINDS) {
      if (cueForMotion(kind)) expect(cuePhaseForMotion(kind), kind).toBe("start");
    }
  });

  it("keeps decorative cues non-essential and functional cues essential", () => {
    for (const kind of MOTION_KINDS) {
      const cue = cueForMotion(kind);
      if (!cue) continue;
      const decorative = (DECORATIVE_CUES as readonly string[]).includes(cue);
      expect(CUE_SPECS[cue].essential, `${kind}:${cue}`).toBe(!decorative);
    }
  });

  it("walks the console sequence with one cue per surface, in order", () => {
    const seen = CONSOLE_SEQUENCE.map((kind) => cueForMotion(kind)).filter(
      (cue): cue is NonNullable<typeof cue> => cue !== null,
    );
    expect(seen).toEqual(["move", "tabChange", "confirm", "openGroup", "tabChange", "overlayOpen", "overlayClose", "confirm", "closeGroup"]);
    // No surface repeats the cue of the surface before it (the old double cue).
    for (let index = 1; index < seen.length; index += 1) {
      expect(seen[index]).not.toBe(seen[index - 1]);
    }
  });

  it("leaves boot silent: the shell reveal is not a console event", () => {
    expect(cueForMotion("boot")).toBeNull();
  });
});

describe("motionDebugEnabled", () => {
  it("reads the query in both the search and the hash query", () => {
    expect(motionDebugEnabled("?motionDebug=1")).toBe(true);
    expect(motionDebugEnabled("?foo=bar&motionDebug=true")).toBe(true);
    expect(motionDebugEnabled("?#/bigpicture?motionDebug=1")).toBe(true);
    expect(motionDebugEnabled("?motionDebug=0")).toBe(false);
    expect(motionDebugEnabled("")).toBe(false);
  });
});
