import type { MotionProps } from "motion/react";

/* --------------------------------------------------------------------------
   Big Picture entrance (#142).

   One orchestrated reveal per open, then the screen holds still: changing the
   selection or the shelf keeps its existing micro-interactions and never
   replays this cascade. Numbers follow the house tokens in styles/index.css:
   --dur-3 (240ms) for the readable cue, --ease-out for every curve. Only
   opacity and transform animate; the backdrop artwork gets a fade and nothing
   else. Every cue has finished before 450ms, and the card stagger is capped
   the same way .stagger-in caps its grid.
   -------------------------------------------------------------------------- */

/** cubic-bezier(0.16, 1, 0.3, 1) — the `--ease-out` token. */
const EASE_OUT: [number, number, number, number] = [0.16, 1, 0.3, 1];

/** Same 22ms rhythm as `.stagger-in`; eight cards get distinct delays. */
export const CARD_STAGGER = 0.022;
export const CARD_STAGGER_CAP = 7;

export const BACKDROP_CUE = { opacity: 0.25, duration: 0.42 };
export const HERO_CUE = { delay: 0.03, duration: 0.3, y: 14, scale: 0.985 };
export const COPY_CUE = { delay: 0.05, duration: 0.28, y: 10 };
export const HEADER_CUE = { delay: 0.06, duration: 0.22, y: 6 };
export const CARD_CUE = { delay: 0.08, duration: 0.2, y: 8 };

/** Last card settles at 0.08 + 7 × 0.022 + 0.2 = 434ms. */
export const ENTRANCE_MS = Math.ceil(
  (CARD_CUE.delay + CARD_STAGGER_CAP * CARD_STAGGER + CARD_CUE.duration) * 1000,
);

export type RevealOptions = {
  /** Vertical travel before settling, in px. 0 keeps the element still. */
  y?: number;
  /** Starting scale. 1 keeps the element still. */
  scale?: number;
  /** Final opacity; the backdrop artwork settles below 1. */
  opacity?: number;
  /** Seconds before the cue starts. */
  delay?: number;
  /** Seconds the cue takes; defaults to --dur-3. */
  duration?: number;
  /** Mount already settled (a card added after the entrance has played). */
  settled?: boolean;
};

export type Reveal = Pick<MotionProps, "initial" | "animate" | "transition">;

/** Staggered delay for the card at `position`, capped at eight items. */
export function cardDelay(position: number): number {
  return CARD_CUE.delay + Math.min(position, CARD_STAGGER_CAP) * CARD_STAGGER;
}

/**
 * Motion props for one cue. With `prefers-reduced-motion` the element only
 * fades in place: no travel, no scale, no stagger.
 */
export function reveal(reduce: boolean | null | undefined, options: RevealOptions = {}): Reveal {
  const { y = 0, scale = 1, opacity = 1, delay = 0, duration = 0.24, settled = false } = options;
  const target = { opacity, y: 0, scale: 1 };

  if (settled) return { initial: false, animate: target, transition: { duration: 0 } };
  if (reduce) return { initial: { opacity: 0 }, animate: { opacity }, transition: { duration: 0.12 } };
  return {
    initial: { opacity: 0, y, scale },
    animate: target,
    transition: { duration, delay, ease: EASE_OUT },
  };
}
