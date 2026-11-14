import { t } from "@/lib/i18n";

/* --------------------------------------------------------------------------
   Launch handoff, model half (#229).

   Press Play and the selected cover becomes the screen: a FLIP from the cover
   rect to full-bleed, the library dimming to 0.35 under it, then a hold frame
   with the artwork, the title and a signal filament until the launch promise
   settles. Everything transform-only, so the plate never lays out the page
   behind it.

   The rules that are decisions rather than pixels live here, next to the
   numbers, and are tested without a DOM:

   - the FLIP is an exact rect → viewport transform (`scaleX`/`scaleY` differ:
     a portrait cover growing into a landscape screen is the point of the
     move);
   - the plate ends on the launch promise, and gives up at 8 s: past that the
     line says what is actually true (the desktop has not picked it up yet)
     instead of a spinner that lies;
   - B may dismiss the plate only once the promise side has given up — the
     native launch call has no abort, so cancelling mid-flight would pretend
     the game is not starting when it is.
   -------------------------------------------------------------------------- */

/** The selected cover, in viewport px. */
export interface PlateRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface PlateViewport {
  width: number;
  height: number;
}

/** A transform-only FLIP start: the plate is full-bleed, scaled onto the card. */
export interface PlateTransform {
  x: number;
  y: number;
  scaleX: number;
  scaleY: number;
}

/** The promise gets this long to speak for itself before the calm line. */
export const LAUNCH_CEILING_MS = 8_000;

/**
 * The shortest time the plate stays up. A launch that resolves in 80 ms would
 * otherwise flash the FLIP for a few frames; this floor lets the spring land
 * (the visual duration is 550 ms, so the plate can still be cut mid-flight —
 * it just never blinks).
 */
export const LAUNCH_MIN_HOLD_MS = 420;

/** How long a failed launch stays readable before the plate closes itself. */
export const LAUNCH_FAIL_HOLD_MS = 1_600;

/** The filament's one cycle, linear, in ms (#229). */
export const LAUNCH_FILAMENT_MS = 1_100;

/** The scrim value the library dims to, as an alpha (0.35 visible). */
export const LAUNCH_DIM_ALPHA = 0.65;

export type LaunchPlateStatus = "launching" | "slow" | "failed";

/**
 * The FLIP start frame. Returns null when there is nothing to fly from (a
 * recent-game launch with no card on screen, a zero-sized rect): the plate then
 * fades in, which is the honest fallback rather than a fabricated origin.
 */
export function flipFrom(rect: PlateRect | null, viewport: PlateViewport): PlateTransform | null {
  if (!rect || !viewport || viewport.width <= 0 || viewport.height <= 0) return null;
  if (rect.width <= 0 || rect.height <= 0) return null;
  return {
    x: rect.left,
    y: rect.top,
    scaleX: viewport.width / rect.width,
    scaleY: viewport.height / rect.height,
  };
}

/** Only a launch that has stopped believing is dismissable (see the header). */
export function plateCanDismiss(status: LaunchPlateStatus): boolean {
  return status !== "launching";
}

/** The plate's copy per status, in the console's existing vocabulary. */
export interface PlateCopy {
  eyebrow: string;
  line: string;
}

/**
 * The message ids the plate speaks. No new locale key is added in this pass:
 * every id already ships, and the test asserts they are in `en.json` so a typo
 * cannot ship as untranslated copy.
 */
export const PLATE_COPY_KEYS = {
  launching: "Launching {title}",
  starting: "Starting…",
  slow: "Waiting for the desktop to pick it up.",
  failed: "Could not launch the game",
} as const;

export function launchPlateCopy(status: LaunchPlateStatus, title: string): PlateCopy {
  const eyebrow = t(PLATE_COPY_KEYS.launching, { title });
  if (status === "failed") return { eyebrow, line: t(PLATE_COPY_KEYS.failed) };
  // No new locale key for the 8 s line: the honest sentence already ships.
  if (status === "slow") return { eyebrow, line: t(PLATE_COPY_KEYS.slow) };
  return { eyebrow, line: t(PLATE_COPY_KEYS.starting) };
}
