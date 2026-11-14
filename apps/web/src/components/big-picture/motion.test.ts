import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CUE_SPECS } from "@/lib/ui-sound-specs";
import {
  INSTANT_HOLD_MS,
  LAUNCH_FLIP_MS,
  MOTION,
  MOTION_DURATION,
  MOTION_KINDS,
  MOTION_SPRING,
  MOTION_STAGGER,
  REDUCED_FADE,
  REDUCED_MOTION,
  createTransitionHub,
  cueForMotion,
  cuePhaseForMotion,
  exitOf,
  missingCues,
  motionDebugEnabled,
  springOf,
  transitionOf,
  type MotionKind,
} from "./motion";

/* --------------------------------------------------------------------------
   #220: the motion vocabulary is a table, so it is tested like one. The token
   test reads the real stylesheet: a retune cannot land in CSS only. The cue
   test pins the reduced-motion split that #198 documented, so a surface cue
   cannot become "decorative" (and disappear) by accident.

   #229 adds the spring split and must not weaken the composite-only rule: a
   spring is still `transform`/`opacity` only, it just settles with physics
   instead of a bezier.
   -------------------------------------------------------------------------- */

const css = readFileSync(new URL("../../styles/index.css", import.meta.url), "utf8");

/** The three cues #198/#229 mark decorative; reduced motion drops them. */
const DECORATIVE_CUES = ["click", "openGroup", "closeGroup"] as const;

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

  it("keeps every tween inside the 120–320ms console band", () => {
    for (const kind of MOTION_KINDS) {
      const spec = MOTION[kind];
      if (spec.spring) continue;
      expect(spec.duration, kind).toBeGreaterThanOrEqual(0.12);
      expect(spec.duration, kind).toBeLessThanOrEqual(0.32);
      expect(spec.exit, kind).toBeGreaterThanOrEqual(0.1);
      expect(spec.exit, kind).toBeLessThanOrEqual(spec.duration);
    }
  });

  it("keeps every spring inside the 900ms surface budget", () => {
    for (const kind of MOTION_KINDS) {
      const spec = MOTION[kind];
      if (!spec.spring) continue;
      expect(spec.spring.visualDuration, `${kind}: visualDuration`).toBeGreaterThanOrEqual(0.24);
      expect(spec.spring.visualDuration, `${kind}: visualDuration`).toBeLessThanOrEqual(0.6);
      expect(spec.spring.bounce, `${kind}: bounce`).toBeGreaterThanOrEqual(0);
      expect(spec.spring.bounce, `${kind}: bounce`).toBeLessThanOrEqual(0.2);
      // The probe and the tests read `duration`; it must stay the spring's own
      // visual duration, not a leftover tween number.
      expect(spec.duration, `${kind}: probe budget`).toBe(spec.spring.visualDuration);
      expect(spec.exit, kind).toBeLessThanOrEqual(MOTION_DURATION.slow);
    }
  });

  it("springs only the six surfaces, with the values from the issue", () => {
    const surfaces: Partial<Record<MotionKind, { visualDuration: number; bounce: number }>> = {
      overlayOpen: { visualDuration: 0.3, bounce: 0 },
      panel: { visualDuration: 0.34, bounce: 0.12 },
      gameDetail: { visualDuration: 0.4, bounce: 0.1 },
      screenPush: { visualDuration: 0.36, bounce: 0 },
      queueRow: { visualDuration: 0.26, bounce: 0 },
      launch: { visualDuration: 0.55, bounce: 0.08 },
    };
    for (const kind of MOTION_KINDS) {
      const expected = surfaces[kind];
      if (!expected) {
        expect(MOTION[kind].spring, `${kind} stays a tween`).toBeUndefined();
        continue;
      }
      expect(MOTION[kind].spring, kind).toEqual(expected);
    }
    expect(MOTION_SPRING.overlay.bounce).toBe(0);
    expect(MOTION_SPRING.panel.bounce).toBeGreaterThan(0);
  });

  it("springs the surfaces and never the focus move", () => {
    for (const kind of ["overlayOpen", "panel", "gameDetail", "screenPush", "queueRow", "launch"] as const) {
      expect(transitionOf(kind, {}).transition, kind).toMatchObject({ type: "spring" });
    }
    // The ring follows the d-pad: 120 ms tween, no physics, no lag. The focus
    // row has no JS frame of its own (the rail card does the move in CSS), so
    // only the table entry is inspectable — it must not grow a spring.
    expect(MOTION.focus.spring).toBeUndefined();
    expect(MOTION.focus.duration).toBe(MOTION_DURATION.fast);
    expect(MOTION.focus.duration * 1_000).toBe(120);
    expect(springOf(MOTION_SPRING.launch)).toEqual({ type: "spring", visualDuration: 0.55, bounce: 0.08 });
  });

  it("keeps the launch FLIP in its 90–600ms window", () => {
    expect(LAUNCH_FLIP_MS.min).toBe(90);
    expect(LAUNCH_FLIP_MS.max).toBe(600);
    const spring = MOTION.launch.spring!;
    expect(spring.visualDuration * 1_000).toBeGreaterThanOrEqual(LAUNCH_FLIP_MS.min);
    expect(spring.visualDuration * 1_000).toBeLessThanOrEqual(LAUNCH_FLIP_MS.max);
  });

  it("uses the 22ms cascade rhythm, not a custom stagger", () => {
    expect(MOTION_STAGGER * 1000).toBe(22);
  });

  it("never animates a layout property", () => {
    const allowed = new Set(["x", "y", "scale", "opacity"]);
    for (const kind of MOTION_KINDS) {
      for (const key of Object.keys(MOTION[kind])) {
        if (["duration", "exit", "ease", "reduced", "cue", "cueAt", "spring"].includes(key)) continue;
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
    const seen = CONSOLE_SEQUENCE.map((kind) => cueForMotion(kind));
    expect(seen).toEqual([
      null,
      "move",
      "click",
      "click",
      "openGroup",
      "click",
      "click",
      "click",
      "confirm",
      "closeGroup",
    ]);
    // #229: the chrome deliberately speaks one voice, so adjacent clicks are
    // the point. What must never repeat across two surfaces is a movement or a
    // launch cue — that was the old double emission.
    for (const cue of ["move", "launch"] as const) {
      expect(seen.filter((entry) => entry === cue).length, cue).toBeLessThanOrEqual(1);
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
