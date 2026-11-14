import type { MotionProps, Transition } from "motion/react";
import { CUE_SPECS, type CueId } from "@/lib/ui-sound-specs";

/* --------------------------------------------------------------------------
   Big Picture motion system (#220).

   One vocabulary for every console transition: the layout switch, the focus
   move, the shelf and page change, the screen push/pop, overlays, the power
   confirmation and the settings sections. Components never invent a duration
   or an easing; they ask this table.

   The numbers mirror the house tokens in styles/index.css (`--dur-1/2/3`,
   `--ease`, `--ease-out`) and the entrance in `entrance.ts`. Durations stay
   inside 120–320 ms, the band a console reads as instant while still being
   legible at ten feet.

   Rules encoded here, not repeated at every call site:

   - only transform and opacity animate; a transition never touches width,
     height, top, left, filter or backdrop-filter;
   - a new input interrupts the previous tween: Motion replaces the target on
     the next frame, and the state hub records which transition was cut;
   - a held or auto-repeated key lands on the final frame (`instant`), so a
     held shoulder cannot restart the shelf slide thirty times a second;
   - `prefers-reduced-motion` maps every kind to either a short opacity fade or
     no motion at all (see `REDUCED_MOTION`);
   - #229: surfaces spring, focus does not. A card that has a physical body
     (overlay, panel, screen, queue row, launch plate) settles with
     `{ type: "spring", visualDuration, bounce }`; the 120 ms focus move stays a
     tween so the ring never lags the d-pad. Progress bars and the blur never
     spring: a bar that overshoots lies about progress;
   - the cue of a transition fires once, at the start of the motion it
     describes, through `cueForMotion`. Surfaces own their cue; the command
     router only speaks for movement, so a single press can never play twice.
   -------------------------------------------------------------------------- */

/** One cubic-bezier, in the order CSS takes it. */
export type Ease = readonly [number, number, number, number];

/** cubic-bezier(0.16, 1, 0.3, 1) — the CSS `--ease-out` token. */
export const EASE_OUT: Ease = [0.16, 1, 0.3, 1];

/** cubic-bezier(0.2, 0.6, 0.2, 1) — the CSS `--ease` token. */
export const EASE_STANDARD: Ease = [0.2, 0.6, 0.2, 1];

/**
 * Seconds, because Motion speaks seconds. `styles/index.css` carries the same
 * values as `--dur-1` (120 ms), `--dur-2` (180 ms) and `--dur-3` (240 ms); the
 * token test reads both so the two tables cannot drift.
 */
export const MOTION_DURATION = {
  fast: 0.12,
  base: 0.18,
  slow: 0.24,
  exit: 0.14,
} as const;

/** The one fade kept under `prefers-reduced-motion`, in seconds (--dur-1). */
export const REDUCED_FADE = 0.12;

/** Travel before settling, in px. `shelf` is the rail slide, `push` a screen. */
export const MOTION_DISTANCE = {
  boot: 8,
  shelf: 18,
  push: 28,
  pop: 20,
  panel: 14,
  confirm: 18,
  settings: 6,
  queue: 4,
} as const;

/**
 * Spring surfaces (#229). `visualDuration` is the time the eye reads as "done",
 * `bounce` the overshoot: 0 for a surface that must not wobble (overlay, screen
 * push, queue row), a breath of it for a sheet or the launch plate.
 */
export const MOTION_SPRING = {
  /** A translucent overlay arrives, it does not bounce. */
  overlay: { visualDuration: 0.3, bounce: 0 },
  /** Panels and sheets: the only surfaces allowed to feel soft. */
  panel: { visualDuration: 0.34, bounce: 0.12 },
  /** A game's own page/detail surface, heavier because it replaces the screen. */
  gameDetail: { visualDuration: 0.4, bounce: 0.1 },
  /** A screen push: firm, because B must be able to cut it cleanly. */
  screenPush: { visualDuration: 0.36, bounce: 0 },
  /** One queue row appearing under the cursor. */
  queueRow: { visualDuration: 0.26, bounce: 0 },
  /** The launch plate: the longest move in the product, and the calmest. */
  launch: { visualDuration: 0.55, bounce: 0.08 },
} as const;

/** The window a launch FLIP may take, in ms (#229: 90–600 ms). */
export const LAUNCH_FLIP_MS = { min: 90, max: 600 } as const;

/** The console card cascade uses the same 22 ms rhythm as `.stagger-in`. */
export const MOTION_STAGGER = 0.022;

/** Shell reveal order: bar, home row, hint strip. */
export const BOOT_DELAY = { bar: 0, home: 0.03, footer: 0.06 } as const;

/** One shell reveal length, shared by the bar, the row and the footer. */
export const BOOT_DURATION = 0.2;

/**
 * How long the held-input suppression stays on the root, in ms. A commit
 * clears the Motion flag in an effect; the CSS attribute must outlive the
 * commit's style resolution, which a 0 ms timer does not reliably do. It stays
 * below the pad's 130 ms repeat so a fresh press always gets its own frame.
 */
export const INSTANT_HOLD_MS = 120;

export type MotionKind =
  | "boot"
  | "focus"
  | "shelf"
  | "page"
  | "layout"
  | "screenPush"
  | "screenPop"
  | "overlayOpen"
  | "overlayClose"
  | "panel"
  | "gameDetail"
  | "queueRow"
  | "launch"
  | "powerConfirm"
  | "settings";

export const MOTION_KINDS = [
  "boot",
  "focus",
  "shelf",
  "page",
  "layout",
  "screenPush",
  "screenPop",
  "overlayOpen",
  "overlayClose",
  "panel",
  "gameDetail",
  "queueRow",
  "launch",
  "powerConfirm",
  "settings",
] as const satisfies readonly MotionKind[];

/**
 * Reduced motion, one line per kind. `fade` keeps a 120 ms opacity crossfade —
 * the information still reads; `none` lands on the final frame immediately.
 * Focus feedback is `none` on purpose: the ring and the lift are the feedback,
 * and a fade would only delay the highlight. A spring surface always degrades
 * to the fade, never to a shorter spring.
 */
export const REDUCED_MOTION: Record<MotionKind, "fade" | "none"> = {
  boot: "fade",
  focus: "none",
  shelf: "fade",
  page: "fade",
  layout: "fade",
  screenPush: "fade",
  screenPop: "fade",
  overlayOpen: "fade",
  overlayClose: "fade",
  panel: "fade",
  gameDetail: "fade",
  queueRow: "fade",
  launch: "fade",
  powerConfirm: "fade",
  settings: "none",
};

/** A spring surface: Motion's `visualDuration` + `bounce`, both seconds/ratio. */
export interface MotionSpring {
  visualDuration: number;
  bounce: number;
}

export interface MotionSpec {
  /**
   * Enter budget, seconds. For a tween it is the duration it runs; for a
   * spring it is `spring.visualDuration`, kept here so the frame-budget probe
   * and the tests have one number to read per kind.
   */
  duration: number;
  /** Exit tween, seconds. */
  exit: number;
  ease: Ease;
  /** Set on a surface: `transitionOf` returns a Motion spring instead. */
  spring?: MotionSpring;
  /** Horizontal travel before settling, px. The sign comes from direction. */
  x?: number;
  /** Vertical travel before settling, px. */
  y?: number;
  /** Scale before settling. */
  scale?: number;
  /** Starting opacity. */
  opacity?: number;
  reduced: "fade" | "none";
  /** The cue that describes this transition, or null when it is silent. */
  cue: CueId | null;
  /** Where the cue sits relative to the motion. Every surface cue starts it. */
  cueAt: "start" | "end";
}

/** The table every Big Picture transition reads. */
export const MOTION: Record<MotionKind, MotionSpec> = {
  boot: {
    duration: 0.2,
    exit: MOTION_DURATION.exit,
    ease: EASE_OUT,
    y: MOTION_DISTANCE.boot,
    opacity: 0,
    reduced: REDUCED_MOTION.boot,
    cue: null,
    cueAt: "start",
  },
  focus: {
    // The rail card and the home tile do their own CSS move (120 ms, ease-out);
    // this entry is the shared decision, not a second tween.
    duration: MOTION_DURATION.fast,
    exit: MOTION_DURATION.fast,
    ease: EASE_OUT,
    reduced: REDUCED_MOTION.focus,
    cue: "move",
    cueAt: "start",
  },
  shelf: {
    duration: MOTION_DURATION.base,
    exit: MOTION_DURATION.fast,
    ease: EASE_STANDARD,
    x: MOTION_DISTANCE.shelf,
    opacity: 0.55,
    reduced: REDUCED_MOTION.shelf,
    cue: "click",
    cueAt: "start",
  },
  page: {
    duration: MOTION_DURATION.fast,
    exit: MOTION_DURATION.fast,
    ease: EASE_STANDARD,
    opacity: 0.7,
    reduced: REDUCED_MOTION.page,
    cue: "click",
    cueAt: "start",
  },
  layout: {
    duration: MOTION_DURATION.base,
    exit: MOTION_DURATION.fast,
    ease: EASE_OUT,
    scale: 0.995,
    opacity: 0.55,
    reduced: REDUCED_MOTION.layout,
    cue: "click",
    cueAt: "start",
  },
  screenPush: {
    duration: MOTION_SPRING.screenPush.visualDuration,
    exit: MOTION_DURATION.exit,
    ease: EASE_OUT,
    spring: MOTION_SPRING.screenPush,
    x: MOTION_DISTANCE.push,
    opacity: 0.4,
    reduced: REDUCED_MOTION.screenPush,
    cue: "openGroup",
    cueAt: "start",
  },
  screenPop: {
    duration: 0.2,
    exit: MOTION_DURATION.exit,
    ease: EASE_OUT,
    x: -MOTION_DISTANCE.pop,
    opacity: 0.55,
    reduced: REDUCED_MOTION.screenPop,
    cue: "closeGroup",
    cueAt: "start",
  },
  overlayOpen: {
    duration: MOTION_SPRING.overlay.visualDuration,
    exit: MOTION_DURATION.exit,
    ease: EASE_OUT,
    spring: MOTION_SPRING.overlay,
    y: MOTION_DISTANCE.panel,
    opacity: 0.5,
    reduced: REDUCED_MOTION.overlayOpen,
    cue: "click",
    cueAt: "start",
  },
  overlayClose: {
    duration: 0.16,
    exit: MOTION_DURATION.fast,
    ease: EASE_OUT,
    y: 8,
    opacity: 0.6,
    reduced: REDUCED_MOTION.overlayClose,
    cue: "click",
    cueAt: "start",
  },
  panel: {
    // #229: the sheet inside an overlay (command menu, queue, picker). It
    // arrives with the only soft spring in the language, and it is silent:
    // the open/close cue belongs to `overlayOpen`/`overlayClose`.
    duration: MOTION_SPRING.panel.visualDuration,
    exit: MOTION_DURATION.exit,
    ease: EASE_OUT,
    spring: MOTION_SPRING.panel,
    y: MOTION_DISTANCE.panel,
    opacity: 0.55,
    reduced: REDUCED_MOTION.panel,
    cue: null,
    cueAt: "start",
  },
  gameDetail: {
    // A game's own surface (install options, detail). Reserved vocabulary: the
    // console opens it through `panel` today, the game sheet lands in phase 2.
    duration: MOTION_SPRING.gameDetail.visualDuration,
    exit: MOTION_DURATION.exit,
    ease: EASE_OUT,
    spring: MOTION_SPRING.gameDetail,
    y: MOTION_DISTANCE.confirm,
    scale: 0.99,
    opacity: 0.45,
    reduced: REDUCED_MOTION.gameDetail,
    cue: null,
    cueAt: "start",
  },
  queueRow: {
    duration: MOTION_SPRING.queueRow.visualDuration,
    exit: MOTION_DURATION.fast,
    ease: EASE_OUT,
    spring: MOTION_SPRING.queueRow,
    y: MOTION_DISTANCE.queue,
    opacity: 0.5,
    reduced: REDUCED_MOTION.queueRow,
    cue: null,
    cueAt: "start",
  },
  launch: {
    // The handoff plate. Its travel is a FLIP computed from the selected cover
    // (see `launch-plate.ts`); the table contributes the spring, the budget and
    // the one cue of the press.
    duration: MOTION_SPRING.launch.visualDuration,
    exit: MOTION_DURATION.exit,
    ease: EASE_OUT,
    spring: MOTION_SPRING.launch,
    opacity: 0.55,
    reduced: REDUCED_MOTION.launch,
    cue: "launch",
    cueAt: "start",
  },
  powerConfirm: {
    duration: 0.22,
    exit: MOTION_DURATION.fast,
    ease: EASE_OUT,
    y: MOTION_DISTANCE.confirm,
    scale: 0.992,
    opacity: 0.4,
    reduced: REDUCED_MOTION.powerConfirm,
    cue: "confirm",
    cueAt: "start",
  },
  settings: {
    duration: 0.16,
    exit: REDUCED_FADE,
    ease: EASE_OUT,
    y: MOTION_DISTANCE.settings,
    opacity: 0.6,
    reduced: REDUCED_MOTION.settings,
    cue: "click",
    cueAt: "start",
  },
};

/* ------------------------------- frames ---------------------------------- */

export interface MotionFrame {
  initial?: MotionProps["initial"];
  animate?: MotionProps["animate"];
  exit?: MotionProps["exit"];
  transition?: Transition;
}

export interface MotionOptions {
  /** The user asked for less motion. */
  reduce?: boolean | null;
  /** The press was a key repeat or a held pad: land on the final frame. */
  instant?: boolean;
  /** The element mounts already settled (entrance played, screen remounted). */
  settled?: boolean;
  /** Travel sign for a shelf, page or screen transition. */
  direction?: 1 | -1;
}

function targetOf(spec: MotionSpec): Record<string, number> {
  const target: Record<string, number> = { opacity: 1 };
  if (spec.x !== undefined) target.x = 0;
  if (spec.y !== undefined) target.y = 0;
  if (spec.scale !== undefined) target.scale = 1;
  return target;
}

function fromOf(spec: MotionSpec, direction: 1 | -1): Record<string, number> {
  const from: Record<string, number> = {};
  if (spec.opacity !== undefined && spec.opacity !== 1) from.opacity = spec.opacity;
  if (spec.x !== undefined) from.x = spec.x * direction;
  if (spec.y !== undefined) from.y = spec.y;
  if (spec.scale !== undefined) from.scale = spec.scale;
  return from;
}

/**
 * Enter props for a kind. `instant` and `settled` both skip the tween; the
 * difference is why (a held key vs. a mount that must not replay). Reduced
 * motion keeps the kind's documented mapping: a 120 ms fade or nothing.
 *
 * #229: a surface with a `spring` returns Motion's
 * `{ type: "spring", visualDuration, bounce }` instead of a tween. The
 * `instant`/`reduce` branches above always win, so a held key still lands on
 * the final frame and reduced motion never springs.
 */
export function transitionOf(kind: MotionKind, options: MotionOptions = {}): MotionFrame {
  const spec = MOTION[kind];
  const target = targetOf(spec);
  const direction = options.direction ?? 1;
  const still: MotionFrame = { initial: false, animate: target, transition: { duration: 0 } };

  if (options.instant || options.settled) return still;

  if (options.reduce) {
    if (spec.reduced === "none") return still;
    return {
      initial: { opacity: 0 },
      animate: { opacity: 1 },
      transition: { duration: REDUCED_FADE, ease: spec.ease },
    };
  }

  const from = fromOf(spec, direction);
  if (Object.keys(from).length === 0) return still;
  return {
    initial: from,
    animate: target,
    transition: spec.spring ? springOf(spec.spring) : { duration: spec.duration, ease: spec.ease },
  };
}

/**
 * The Motion transition for one surface spring (#229). Exported so a surface
 * that supplies its own start frame — the launch FLIP — still speaks the same
 * spring as the table.
 */
export function springOf(spring: MotionSpring): Transition {
  return { type: "spring", visualDuration: spring.visualDuration, bounce: spring.bounce };
}

/**
 * Exit props. A transition that is interrupted never runs these — Motion drops
 * the exiting element the same frame — so they describe the calm close only.
 * Exits are opacity-only on purpose: the outgoing surface never travels, so a
 * push and a pop cannot disagree about which way is "back".
 */
export function exitOf(kind: MotionKind, options: MotionOptions = {}): MotionFrame {
  const spec = MOTION[kind];
  const duration = options.instant ? 0 : options.reduce ? REDUCED_FADE : spec.exit;
  const exit = {
    opacity: 0,
    // An exiting surface must never swallow the press that dismissed it.
    pointerEvents: "none",
    transition: { duration, ease: spec.ease },
  } as MotionProps["exit"];
  return { exit };
}

/* -------------------------------- cue sync -------------------------------- */

/**
 * The cue a transition speaks with. Surfaces call this once, before the state
 * change that starts the motion; the command router no longer decides these,
 * which is what removes the old double emission (open/close/layout/power).
 */
export function cueForMotion(kind: MotionKind): CueId | null {
  return MOTION[kind].cue;
}

/** Where that cue sits relative to the tween. Always `start` today. */
export function cuePhaseForMotion(kind: MotionKind): "start" | "end" {
  return MOTION[kind].cueAt;
}

/* --------------------------- interruption hub ----------------------------- */

export interface ActiveTransition {
  kind: MotionKind;
  /** Wall-clock ms at the press that started it. */
  startedAt: number;
  /** The press was a hold/repeat: the state landed instantly. */
  instant: boolean;
  /** The transition this one cut short, if any. */
  interrupted: MotionKind | null;
  /** Monotonic id; a late completion can check it is still the active one. */
  generation: number;
}

export interface TransitionHub {
  begin(kind: MotionKind, options?: { instant?: boolean; at?: number }): ActiveTransition;
  /** Completes the transition only when it is still the active one. */
  end(kind: MotionKind): boolean;
  active(): ActiveTransition | null;
  last(): ActiveTransition | null;
  interruptions(): number;
  /**
   * A transition never gates input: the next press always lands, and the new
   * animation replaces the old target on the next frame. Kept as a method so
   * the policy is visible and testable rather than implied.
   */
  accepts(): true;
}

export function createTransitionHub(clock: () => number = () => Date.now()): TransitionHub {
  let active: ActiveTransition | null = null;
  let last: ActiveTransition | null = null;
  let cuts = 0;
  let generation = 0;

  return {
    begin(kind, options = {}) {
      const interrupted = active ? active.kind : null;
      if (interrupted !== null) cuts += 1;
      const entry: ActiveTransition = {
        kind,
        startedAt: options.at ?? clock(),
        instant: options.instant === true,
        interrupted,
        generation: ++generation,
      };
      active = entry;
      last = entry;
      return entry;
    },
    end(kind) {
      if (!active || active.kind !== kind) return false;
      active = null;
      return true;
    },
    active: () => active,
    last: () => last,
    interruptions: () => cuts,
    accepts: () => true,
  };
}

/* ------------------------------- debug flag -------------------------------- */

/** `?motionDebug=1` (query or hash query) turns the frame-budget probe on. */
export function motionDebugEnabled(search?: string | null): boolean {
  if ((search === undefined || search === null) && typeof window === "undefined") return false;
  const source = search ?? `${window.location.search}${window.location.hash}`;
  const hashIndex = source.indexOf("#");
  const beforeHash = hashIndex === -1 ? source : source.slice(0, hashIndex);
  const afterHash = hashIndex === -1 ? "" : source.slice(hashIndex);
  const values = [new URLSearchParams(beforeHash.replace(/^\?/, "")).get("motionDebug")];
  const queryIndex = afterHash.indexOf("?");
  if (queryIndex !== -1) {
    values.push(new URLSearchParams(afterHash.slice(queryIndex + 1)).get("motionDebug"));
  }
  return values.some((value) => value === "1" || value?.toLowerCase() === "true");
}

/** Compile-time guard: every cue named by the table exists in the sound set. */
export function missingCues(): CueId[] {
  return MOTION_KINDS.map((kind) => MOTION[kind].cue).filter(
    (cue): cue is CueId => cue !== null && !CUE_SPECS[cue],
  );
}
