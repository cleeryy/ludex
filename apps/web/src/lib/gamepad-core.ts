import type { ControllerAction, ControllerProfile } from "@/lib/types";
import { actionForAxis, actionForButton, applyStick } from "@/lib/controller-profiles";

/**
 * Pure gamepad logic (#137), kept out of the React hook so it can be tested
 * without a browser: pad selection, input decoding, repeat timing, spatial
 * target movement, the modal guard and focus restore.
 *
 * The physical-to-logical mapping itself lives in `controller-profiles.ts`
 * (#51) and is shared with the Rust validator. This module only decides *which*
 * pad to read, *when* an action may fire, and *where* the focus ring goes.
 */

export type GamepadAction = ControllerAction;

/** The shape of a `Gamepad` the code actually reads (easy to fake in tests). */
export interface PadButtonLike {
  pressed: boolean;
  value: number;
}

export interface PadLike {
  index: number;
  id: string;
  connected: boolean;
  mapping: string;
  timestamp: number;
  axes: readonly number[];
  buttons: readonly PadButtonLike[];
}

export const FIRST_REPEAT_MS = 380;
export const REPEAT_MS = 130;
export const MAX_BUTTONS = 32;

export const REPEATABLE_ACTIONS: ReadonlySet<GamepadAction> = new Set<GamepadAction>([
  "left",
  "right",
  "up",
  "down",
]);

/* -------------------------------- detection -------------------------------- */

export interface PadSelection {
  /** Index of the pad currently driving the UI, `null` when none. */
  activeIndex: number | null;
  connected: number[];
}

/** Most recent pad that actually produced input (button or stick movement). */
export interface PadActivity {
  index: number;
  at: number;
}

/**
 * Picks the pad to read. The active pad is sticky while it stays connected;
 * the pad that last produced input takes over (that is what "the most recent
 * active controller" means). `Gamepad.timestamp` is deliberately not used:
 * some Chromium/WebView2 builds refresh it every polling frame even when a pad
 * is idle, which would make two connected pads flap every frame.
 * `preferredIndex` is the pad named by the last `gamepadconnected` event and
 * is used when nothing is active yet.
 */
export function selectActivePad(
  pads: readonly (PadLike | null | undefined)[],
  previous: PadSelection,
  preferredIndex: number | null = null,
  activity: PadActivity | null = null,
): { pad: PadLike | null; selection: PadSelection } {
  const connected = pads.filter((pad): pad is PadLike => Boolean(pad && pad.connected));
  if (connected.length === 0) {
    return { pad: null, selection: { activeIndex: null, connected: [] } };
  }

  const connectedIndices = connected.map((pad) => pad.index).sort((a, b) => a - b);
  const byIndex = new Map(connected.map((pad) => [pad.index, pad]));
  const active =
    (activity !== null ? byIndex.get(activity.index) : undefined) ??
    (previous.activeIndex !== null ? byIndex.get(previous.activeIndex) : undefined) ??
    (preferredIndex !== null ? byIndex.get(preferredIndex) : undefined) ??
    connected[0];

  return { pad: active, selection: { activeIndex: active.index, connected: connectedIndices } };
}

/** True when the pad's current state carries input worth following. */
export function padHasInput(pad: PadLike): boolean {
  const buttons = Math.min(pad.buttons.length, MAX_BUTTONS);
  for (let index = 0; index < buttons; index += 1) {
    const button = pad.buttons[index];
    if (button && (button.pressed || button.value > 0.6)) return true;
  }
  const axes = Math.min(pad.axes.length, 8);
  for (let index = 0; index < axes; index += 1) {
    if (Math.abs(pad.axes[index] ?? 0) > 0.6) return true;
  }
  return false;
}

/* --------------------------------- decoding -------------------------------- */

function buttonValue(button: PadButtonLike | undefined): number {
  if (!button) return 0;
  const value = Number.isFinite(button.value) ? button.value : 0;
  // Some WebView2/driver combinations report `pressed: true, value: 0`.
  return button.pressed ? Math.max(1, value) : value;
}

/**
 * Reads every pressed logical action. Sticks are decomposed per component:
 * `axis0`/`axis1` are the left stick X/Y and `axis2`/`axis3` the right stick
 * X/Y, so the default profile maps `axis1-` to `up` (previously the vertical
 * component was read from the wrong slots and up/down were swapped with the
 * right stick). A diagonal only moves the dominant direction, like the Rust
 * mirror. Axes 4-7 keep working for remapped profiles.
 */
export function readPadActions(profile: ControllerProfile, pad: PadLike): Set<GamepadAction> {
  const pressed = new Set<GamepadAction>();

  const buttons = Math.min(pad.buttons.length, MAX_BUTTONS);
  for (let index = 0; index < buttons; index += 1) {
    const action = actionForButton(profile, index, buttonValue(pad.buttons[index]));
    if (action) pressed.add(action);
  }

  const axes = pad.axes;
  const sticks: Array<[xAxis: number, yAxis: number, x: number, y: number]> = [
    [0, 1, axes[0] ?? 0, axes[1] ?? 0],
    [2, 3, axes[2] ?? 0, axes[3] ?? 0],
  ];
  for (const [xAxis, yAxis, x, y] of sticks) {
    // Compare the shaped components, then hand only the dominant raw
    // component to the mapper (which shapes it again) so a diagonal is one
    // move, not two.
    const horizontal = applyStick(profile, x, y, true);
    const vertical = applyStick(profile, x, y, false);
    const action =
      Math.abs(horizontal) >= Math.abs(vertical)
        ? actionForAxis(profile, xAxis, x, 0)
        : actionForAxis(profile, yAxis, 0, y);
    if (action) pressed.add(action);
  }

  // Remapped axes 4-7 are read as single components; the defaults map none of
  // them, and analog triggers are normally exposed as buttons 6/7 as well.
  const components: Array<[axis: number, value: number]> = [
    [4, axes[4] ?? 0],
    [5, axes[5] ?? 0],
    [6, axes[6] ?? 0],
    [7, axes[7] ?? 0],
  ];
  for (const [axis, value] of components) {
    const action = actionForAxis(profile, axis, value, 0);
    if (action) pressed.add(action);
  }

  return pressed;
}

/* ---------------------------------- repeat --------------------------------- */

export interface RepeatTracker {
  pressed: Set<GamepadAction>;
  nextAt: Map<GamepadAction, number>;
}

export function createRepeatTracker(): RepeatTracker {
  return { pressed: new Set(), nextAt: new Map() };
}

export function resetRepeatTracker(tracker: RepeatTracker): void {
  tracker.pressed.clear();
  tracker.nextAt.clear();
}

/**
 * Edge-detects the pressed set and returns the actions that may fire this
 * frame. Face/shoulder buttons fire once per press; directions fire on press
 * then repeat after {@link FIRST_REPEAT_MS}, every {@link REPEAT_MS} after.
 * `blocked` (a real modal dialog) still tracks edges so closing the dialog
 * never replays a burst of queued moves.
 */
export function collectRepeatActions(
  tracker: RepeatTracker,
  pressed: ReadonlySet<GamepadAction>,
  now: number,
  blocked = false,
): GamepadAction[] {
  const fire: GamepadAction[] = [];

  for (const action of pressed) {
    const repeatable = REPEATABLE_ACTIONS.has(action);
    if (!tracker.pressed.has(action)) {
      if (repeatable) tracker.nextAt.set(action, now + FIRST_REPEAT_MS);
      fire.push(action);
      continue;
    }
    if (!repeatable) continue;
    const due = tracker.nextAt.get(action) ?? now;
    if (now >= due) {
      tracker.nextAt.set(action, now + REPEAT_MS);
      fire.push(action);
    }
  }

  for (const action of tracker.pressed) {
    if (!pressed.has(action)) tracker.nextAt.delete(action);
  }
  tracker.pressed = new Set(pressed);

  return blocked ? [] : fire;
}

/* -------------------------------- geometry --------------------------------- */

export interface TargetRect {
  top: number;
  left: number;
  width: number;
  height: number;
}

export type MoveAction = "left" | "right" | "up" | "down";

/**
 * Groups targets into visual rows using their rectangles, not CSS classes:
 * two targets share a row when their vertical centres are closer than half the
 * shorter one. Rows and columns are ordered by geometry so a grid keeps its
 * reading order even when the DOM order is unusual.
 */
export function rowsFromRects(rects: readonly TargetRect[]): number[][] {
  const rows: number[][] = [];
  const centres: number[] = [];

  rects.forEach((rect, index) => {
    const centre = rect.top + rect.height / 2;
    const tolerance = Math.max(4, Math.min(rect.height, 96) / 2);
    const row = centres.findIndex((value) => Math.abs(centre - value) <= tolerance);
    if (row === -1) {
      rows.push([index]);
      centres.push(centre);
    } else {
      rows[row].push(index);
    }
  });

  for (const row of rows) row.sort((a, b) => rects[a].left - rects[b].left);
  return rows;
}

/**
 * One d-pad/stick move. Left/right stop at the row edges and up/down move to
 * the same column on the previous/next row (clamped when that row is shorter),
 * which is what a grid feels like. A single-column list behaves like a list.
 */
export function moveTargetIndex(
  rects: readonly TargetRect[],
  index: number,
  action: MoveAction,
): number {
  if (rects.length === 0) return 0;
  const current = Math.max(0, Math.min(index, rects.length - 1));
  const rows = rowsFromRects(rects);
  const rowIndex = rows.findIndex((row) => row.includes(current));
  const row = rows[rowIndex] ?? [current];
  const column = Math.max(0, row.indexOf(current));

  switch (action) {
    case "left":
      return row[Math.max(0, column - 1)] ?? current;
    case "right":
      return row[Math.min(row.length - 1, column + 1)] ?? current;
    case "up": {
      const previous = rows[rowIndex - 1];
      if (!previous) return current;
      return previous[Math.min(column, previous.length - 1)] ?? current;
    }
    case "down": {
      const next = rows[rowIndex + 1];
      if (!next) return current;
      return next[Math.min(column, next.length - 1)] ?? current;
    }
    default:
      return current;
  }
}

/* ------------------------------ modal / focus ------------------------------ */

export interface QueryHost {
  querySelector(selector: string): unknown;
}

/**
 * Only a real modal dialog (`role="dialog"` + `aria-modal="true"`, as rendered
 * by `components/ui/dialog`) swallows gamepad input. A non-modal filter sheet
 * or an inline panel must not block navigation.
 */
export function hasBlockingModal(host?: QueryHost | null): boolean {
  if (!host) return false;
  try {
    return Boolean(host.querySelector('[role="dialog"][aria-modal="true"]'));
  } catch {
    return false;
  }
}

export interface FocusTarget {
  focus(options?: FocusOptions): void;
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
  hasAttribute?(name: string): boolean;
  scrollIntoView?(options?: ScrollIntoViewOptions): void;
  offsetParent?: unknown;
}

export interface FocusHost extends QueryHost {
  querySelector(selector: string): FocusTarget | null;
}

/**
 * Moves the gamepad ring to `targets[index]`. The attribute is what keeps the
 * ring visible: programmatic focus is not a keyboard event, so the browser's
 * `:focus-visible` heuristic would otherwise hide it.
 */
export function applyGamepadFocus(
  targets: readonly FocusTarget[],
  index: number,
  options: { smooth?: boolean; scroll?: boolean } = {},
): FocusTarget | null {
  const target = targets[index];
  if (!target) return null;
  for (const element of targets) element.removeAttribute("data-gamepad-focus");
  target.setAttribute("data-gamepad-focus", "true");
  target.focus({ preventScroll: true });
  if (options.scroll !== false) {
    target.scrollIntoView?.({
      block: "nearest",
      inline: "nearest",
      behavior: options.smooth ? "smooth" : "auto",
    });
  }
  return target;
}

/**
 * Returns focus to the target the gamepad was on before a dialog opened. The
 * caller decides whether falling back to the first target is acceptable (only
 * when the gamepad owned focus before the dialog). Hidden targets are refused.
 */
export function restoreGamepadFocus(
  host?: FocusHost | null,
  options: { fallback?: boolean } = {},
): FocusTarget | null {
  if (!host) return null;
  const marked = host.querySelector('[data-gamepad-focus="true"]');
  const target = marked ?? (options.fallback ? host.querySelector("[data-gamepad-target]") : null);
  if (!target) return null;
  if ("offsetParent" in target && target.offsetParent === null) return null;
  target.setAttribute("data-gamepad-focus", "true");
  target.focus({ preventScroll: true });
  return target;
}

/* ---------------------------------- debug ---------------------------------- */

/**
 * `?gamepadDebug=1` (also accepted before or after the hash, and as `true`).
 * The diagnostic overlay and the `console.debug` trace share this flag.
 */
export function parseGamepadDebugFlag(source: string): boolean {
  const hashIndex = source.indexOf("#");
  const beforeHash = hashIndex === -1 ? source : source.slice(0, hashIndex);
  const afterHash = hashIndex === -1 ? "" : source.slice(hashIndex);
  const candidates: Array<string | null> = [
    new URLSearchParams(beforeHash.replace(/^\?/, "")).get("gamepadDebug"),
  ];
  const queryIndex = afterHash.indexOf("?");
  if (queryIndex !== -1) {
    candidates.push(new URLSearchParams(afterHash.slice(queryIndex + 1)).get("gamepadDebug"));
  }
  return candidates.some((value) => value === "1" || value?.toLowerCase() === "true");
}
