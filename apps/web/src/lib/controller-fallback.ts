import type { ControllerAction, ControllerProfile } from "@/lib/types";
import { presetProfile } from "@/lib/controller-profiles";

/* --------------------------------------------------------------------------
   The universal fallback (#230).

   A pad that does not report the standard mapping, or that has fewer than the
   12 buttons the layout needs, would otherwise be an "unsupported pad" dead
   end. The Gamepad API still exposes raw buttons and axes, so we synthesise a
   best-effort profile: buttons 0-3 are guessed as confirm/back/menu/downloads,
   the left stick always drives directions, and a d-pad reported as a discrete
   hat axis is read as a d-pad.

   Two traps are handled on purpose:

   - A DirectInput trigger axis rests at -1 and rises to +1. Reading it as a
     direction would hold "left" forever, so an axis that only ever sits in the
     negative half is classified as a trigger and never mapped.
   - The right stick (axes 2/3) is left dead rather than guessed: a wrong
     direction is worse than no direction, and the d-pad/stick still navigate.

   The result is always navigable in the library and Console mode; the manager
   marks the pad "Generic — remap if a button is wrong".
   -------------------------------------------------------------------------- */

/** The bits of a `Gamepad` the fallback classifier reads. */
export interface FallbackPadLike {
  mapping?: string;
  /** The button list, or just its length when the caller only kept counts. */
  buttons: readonly { pressed: boolean; value: number }[] | number;
  /** The axis values, or just their count. */
  axes: readonly number[] | number;
}

function buttonCount(pad: FallbackPadLike): number {
  return typeof pad.buttons === "number" ? pad.buttons : pad.buttons.length;
}

function axisCount(pad: FallbackPadLike): number {
  return typeof pad.axes === "number" ? pad.axes : pad.axes.length;
}

function axisValues(pad: FallbackPadLike): number[] | null {
  return typeof pad.axes === "number" ? null : Array.from(pad.axes);
}

/** The layout expects a standard mapping and at least 12 buttons. */
export function needsFallbackProfile(pad: FallbackPadLike | null | undefined): boolean {
  if (!pad) return false;
  return pad.mapping !== "standard" || buttonCount(pad) < 12;
}

export interface AxisClassification {
  /** Axes 0-3: the two sticks, left alone. */
  sticks: number[];
  /** Axes that report a discrete hat (eight directions), usable as a d-pad. */
  hats: number[];
  /** Axes that rest at -1: analog triggers, never mapped to a direction. */
  triggers: number[];
}

/** Values a hat axis can take: eighths on Linux, quarters on some adapters. */
const DISCRETE_STEPS = [0, 1 / 7, 2 / 7, 3 / 7, 4 / 7, 5 / 7, 6 / 7, 1, 0.25, 0.5, 0.75];

function nearDiscrete(value: number): boolean {
  const magnitude = Math.abs(value);
  if (!Number.isFinite(magnitude) || magnitude > 1) return false;
  return DISCRETE_STEPS.some((step) => Math.abs(magnitude - step) <= 0.06);
}

/**
 * Sorts the axes of a non-standard pad into sticks, hats and triggers. Pass a
 * few samples (the manager and the console keep a short ring buffer) so a hat
 * that happens to sit at 0 while every sample was taken is still recognised.
 */
export function classifyAxes(samples: readonly (readonly number[])[] | null | undefined): AxisClassification {
  const rows = (samples ?? []).filter((row) => Array.isArray(row) && row.length > 0);
  const width = rows.reduce((widest, row) => Math.max(widest, row.length), 0);
  const sticks: number[] = [];
  const hats: number[] = [];
  const triggers: number[] = [];
  for (let axis = 0; axis < width; axis += 1) {
    const values = rows.map((row) => row[axis]).filter((value): value is number => Number.isFinite(value));
    if (values.length === 0) continue;
    const min = Math.min(...values);
    const max = Math.max(...values);
    // Rest at -1 and never crossed the middle: a trigger, not a direction.
    if (axis >= 4 && min <= -0.9 && max <= -0.5) {
      triggers.push(axis);
      continue;
    }
    if (axis < 4) {
      sticks.push(axis);
      continue;
    }
    if (values.every((value) => nearDiscrete(value))) hats.push(axis);
    // Anything else past the sticks is analog and unmapped: no guessing.
  }
  return { sticks, hats, triggers };
}

export interface FallbackProfileOptions {
  /** The profile the pad would otherwise use; its tuning is kept. */
  base?: ControllerProfile;
  /** Axis samples collected over a few frames (one snapshot when omitted). */
  samples?: readonly (readonly number[])[] | null;
  classification?: AxisClassification | null;
}

/** Guesses a usable profile for a pad the standard layout does not cover. */
export function fallbackProfile(
  pad: FallbackPadLike,
  options: FallbackProfileOptions = {},
): ControllerProfile {
  const base = options.base ?? presetProfile("generic");
  if (!needsFallbackProfile(pad)) return base;

  const mappings: Record<string, ControllerAction> = {};
  const guesses: ControllerAction[] = ["confirm", "back", "menu", "downloads"];
  const buttons = buttonCount(pad);
  for (let index = 0; index < Math.min(buttons, 4); index += 1) {
    mappings[`button${index}`] = guesses[index];
  }
  if (buttons >= 6) {
    mappings.button4 = "prevTab";
    mappings.button5 = "nextTab";
  }
  // A standard d-pad cluster, when the pad actually has one.
  if (buttons >= 16) {
    mappings.button12 = "up";
    mappings.button13 = "down";
    mappings.button14 = "left";
    mappings.button15 = "right";
  }
  // The left stick always works, on every pad that has axes at all.
  const axes = axisCount(pad);
  if (axes >= 2) {
    mappings["axis0-"] = "left";
    mappings["axis0+"] = "right";
    mappings["axis1-"] = "up";
    mappings["axis1+"] = "down";
  }
  const frame = axisValues(pad);
  const classification = options.classification ?? classifyAxes(options.samples ?? (frame ? [frame] : []));
  const [hatX, hatY] = classification.hats;
  if (hatX !== undefined) {
    mappings[`axis${hatX}-`] = "left";
    mappings[`axis${hatX}+`] = "right";
  }
  if (hatY !== undefined) {
    mappings[`axis${hatY}-`] = "up";
    mappings[`axis${hatY}+`] = "down";
  }
  return { ...base, mappings };
}
