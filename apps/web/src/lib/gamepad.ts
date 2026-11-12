import { useEffect, useRef, useState } from "react";
import type { ControllerProfile } from "@/lib/types";
import { presetProfile } from "@/lib/controller-profiles";
import {
  applyGamepadFocus,
  collectRepeatActions,
  createRepeatTracker,
  hapticPattern,
  hasBlockingModal,
  parseGamepadDebugFlag,
  readPadPresses,
  resetRepeatTracker,
  restoreGamepadFocus,
  selectActivePad,
  padHasInput,
  type ActionSource,
  type GamepadAction,
  type PadActivity,
  type PadLike,
  type PadSelection,
} from "@/lib/gamepad-core";

/**
 * Gamepad navigation for the library and Big Picture (#137).
 *
 * One polling loop per mounted screen reads the most recently active pad
 * (sticky while it stays connected, newest `timestamp` wins otherwise). The
 * logical actions come from the active [`ControllerProfile`] (#51): deadzone,
 * sensitivity, curve, inversion, remapping and trigger threshold are applied
 * in `controller-profiles.ts`, so a remapped pad behaves the same everywhere.
 * Without a stored profile the Xbox preset is used:
 *
 * | Input | Action |
 * |---|---|
 * | D-pad / left stick | `left` `right` `up` `down` |
 * | A (0) | `confirm` |
 * | B (1) | `back` |
 * | X (2) | `menu` |
 * | Y (3) | `downloads` |
 * | LB / RB (4/5) | `prevTab` / `nextTab` |
 *
 * The handler also receives the physical {@link ActionSource} behind the action
 * (#138): Big Picture tells a shoulder from a trigger and X from Start/Guide
 * without changing the logical action set. Directions repeat after a short
 * delay; face and shoulder buttons fire once per press. Optional haptics play a
 * `dual-rumble` pattern for each accepted input, gated on the profile's
 * `vibration` flag, the `haptics` option and a supporting `vibrationActuator`;
 * otherwise they are a silent no-op. Input is ignored while a real modal dialog
 * is open and focus is returned to the gamepad target when it closes. Polling is
 * rAF-based, starts and stops with the route and the page visibility, and
 * re-scans on focus and on hot-plug events, which is what Windows/WebView2 needs
 * when a pad only reports itself after its first button press.
 *
 * Add `?gamepadDebug=1` to the route to get an overlay with the pad id,
 * mapping, axes/buttons, resolved profile and the last action; the same flag
 * turns on `console.debug` lines.
 */

export type { ActionSource, GamepadAction } from "@/lib/gamepad-core";

export interface GamepadDebugInfo {
  connected: boolean;
  id: string | null;
  index: number | null;
  mapping: string;
  profile: string;
  axes: number[];
  buttons: Array<{ index: number; value: number }>;
  action: GamepadAction | null;
  actionAt: number | null;
  updatedAt: number;
}

export interface GamepadState {
  connected: boolean;
  /** Active controller id, as reported by the Gamepad API. */
  id: string | null;
  index: number | null;
  mapping: string | null;
  /** Only populated when `?gamepadDebug=1` is present. */
  debug: GamepadDebugInfo | null;
}

/** Reads the flag from the real location, hash routing included. */
export function gamepadDebugEnabled(search?: string | null): boolean {
  if (search !== undefined && search !== null) return parseGamepadDebugFlag(search);
  if (typeof window === "undefined") return false;
  return parseGamepadDebugFlag(`${window.location.search}${window.location.hash}`);
}

export function useGamepad(
  onAction: (action: GamepadAction, source: ActionSource | null) => void,
  options: { enabled?: boolean; profile?: ControllerProfile | null; debug?: boolean; haptics?: boolean } = {},
): GamepadState {
  const enabled = options.enabled ?? true;
  const debug = options.debug ?? gamepadDebugEnabled();
  const [state, setState] = useState<GamepadState>({
    connected: false,
    id: null,
    index: null,
    mapping: null,
    debug: null,
  });
  const handlerRef = useRef(onAction);
  handlerRef.current = onAction;
  const profileRef = useRef<ControllerProfile>(options.profile ?? presetProfile("xbox"));
  profileRef.current = options.profile ?? presetProfile("xbox");
  const debugRef = useRef(debug);
  debugRef.current = debug;
  const hapticsRef = useRef(options.haptics ?? true);
  hapticsRef.current = options.haptics ?? true;

  useEffect(() => {
    if (!enabled) return;
    if (typeof navigator === "undefined" || typeof navigator.getGamepads !== "function") return;
    if (typeof window === "undefined" || typeof document === "undefined") return;

    let frame = 0;
    let running = false;
    let disposed = false;
    let lastDebugAt = 0;
    let lastTraceAt = 0;
    let lastRumbleAt = 0;
    let modalOpen = false;
    let restoreWasMarked = false;
    let lastAction: GamepadAction | null = null;
    let lastActionAt: number | null = null;
    let currentKey: string | null | undefined = undefined;

    const repeat = createRepeatTracker();
    let selection: PadSelection = { activeIndex: null, connected: [] };
    let activity: PadActivity | null = null;
    let preferredIndex: number | null = null;

    const padsNow = (): PadLike[] => {
      try {
        return Array.from(navigator.getGamepads?.() ?? []).filter((pad): pad is Gamepad =>
          Boolean(pad && pad.connected),
        );
      } catch {
        // Some WebViews throw in insecure contexts; treat it as "no pad".
        return [];
      }
    };

    const publish = (pad: PadLike | null) => {
      if (disposed) return;
      setState((previous) => {
        const next = {
          connected: Boolean(pad),
          id: pad?.id ?? null,
          index: pad?.index ?? null,
          mapping: pad?.mapping ?? null,
        };
        if (
          previous.connected === next.connected &&
          previous.id === next.id &&
          previous.index === next.index &&
          previous.mapping === next.mapping
        ) {
          return previous;
        }
        return { ...previous, ...next };
      });
    };

    const syncPad = (): PadLike | null => {
      const pads = padsNow();
      const now = performance.now();
      const triggered = pads.find((pad) => padHasInput(pad));
      if (triggered && triggered.index !== activity?.index) {
        activity = { index: triggered.index, at: now };
      }
      if (activity && !pads.some((pad) => pad.index === activity?.index)) activity = null;
      const result = selectActivePad(pads, selection, preferredIndex, activity);
      selection = result.selection;
      const pad = result.pad;
      const key = pad ? `${pad.index}:${pad.id}` : null;
      if (key !== currentKey) {
        currentKey = key;
        resetRepeatTracker(repeat);
        publish(pad);
        if (debugRef.current) {
          console.debug(
            "[gamepad] active pad",
            pad ? { id: pad.id, index: pad.index, mapping: pad.mapping } : null,
          );
        }
      }
      return pad;
    };

    const pressedButtons = (pad: PadLike) => {
      const list: Array<{ index: number; value: number }> = [];
      const count = Math.min(pad.buttons.length, 32);
      for (let index = 0; index < count; index += 1) {
        const button = pad.buttons[index];
        if (!button) continue;
        if (button.pressed || button.value > 0.5) {
          list.push({ index, value: Math.round((button.value ?? 0) * 100) / 100 });
        }
      }
      return list;
    };

    const vibrate = (pad: PadLike, now: number, action: GamepadAction) => {
      const profile = profileRef.current;
      if (!profile.vibration || !hapticsRef.current) return;
      if (now - lastRumbleAt < 120) return;
      const pattern = hapticPattern(action);
      if (!pattern) return;
      const actuator = (pad as unknown as Gamepad).vibrationActuator;
      if (!actuator || typeof actuator.playEffect !== "function") return;
      lastRumbleAt = now;
      try {
        const result = actuator.playEffect("dual-rumble", pattern) as Promise<unknown> | undefined;
        if (result && typeof result.catch === "function") void result.catch(() => undefined);
      } catch {
        /* a pad that refuses the effect is a silent no-op */
      }
    };

    const debugFrame = (pad: PadLike | null, now: number) => {
      if (!debugRef.current || now - lastDebugAt < 100) return;
      lastDebugAt = now;
      setState((previous) => ({
        ...previous,
        debug: {
          connected: Boolean(pad),
          id: pad?.id ?? null,
          index: pad?.index ?? null,
          mapping: pad?.mapping ?? "",
          profile: profileRef.current.name,
          axes: pad ? Array.from(pad.axes).map((value) => Math.round(value * 100) / 100) : [],
          buttons: pad ? pressedButtons(pad) : [],
          action: lastAction,
          actionAt: lastActionAt,
          updatedAt: now,
        },
      }));
      if (pad && now - lastTraceAt >= 1_000) {
        lastTraceAt = now;
        console.debug("[gamepad] frame", {
          id: pad.id,
          index: pad.index,
          mapping: pad.mapping,
          profile: profileRef.current.name,
          axes: Array.from(pad.axes).map((value) => Math.round(value * 100) / 100),
          buttons: pressedButtons(pad),
        });
      }
    };

    const step = (now: number) => {
      if (disposed) return;
      const pad = syncPad();
      debugFrame(pad, now);
      if (!pad) {
        modalOpen = false;
        restoreWasMarked = false;
        return;
      }

      const modal = hasBlockingModal(document);
      if (modal !== modalOpen) {
        modalOpen = modal;
        if (modal) {
          restoreWasMarked = Boolean(
            document.querySelector('[data-gamepad-focus="true"]'),
          );
        } else {
          const restored = restoreGamepadFocus(document, { fallback: restoreWasMarked });
          if (debugRef.current && restored) console.debug("[gamepad] focus restored");
          restoreWasMarked = false;
        }
      }

      const profile = profileRef.current;
      const presses = readPadPresses(profile, pad);
      const pressed = new Set<GamepadAction>(presses.map((press) => press.action));
      const sourceFor = new Map(presses.map((press) => [press.action, press.source]));
      const backEdge = pressed.has("back") && !repeat.pressed.has("back");
      const actions = collectRepeatActions(repeat, pressed, now, modal);
      if (modal && backEdge) {
        // A controller-only player opened the dialog with X; B dismisses it the
        // same way Escape would. Nothing else reaches the screen while open.
        document.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Escape",
            code: "Escape",
            bubbles: true,
            cancelable: true,
          }),
        );
      }
      for (const action of actions) {
        handlerRef.current(action, sourceFor.get(action) ?? null);
        vibrate(pad, now, action);
        lastAction = action;
        lastActionAt = now;
        if (debugRef.current) {
          console.debug("[gamepad] action", {
            action,
            id: pad.id,
            index: pad.index,
            mapping: pad.mapping,
            profile: profile.name,
            axes: Array.from(pad.axes).map((value) => Math.round(value * 100) / 100),
            buttons: pressedButtons(pad),
          });
        }
      }
    };

    const loop = (now: number) => {
      if (!running) return;
      step(now);
      frame = requestAnimationFrame(loop);
    };

    const start = () => {
      if (running || disposed) return;
      running = true;
      frame = requestAnimationFrame(loop);
    };

    const stop = () => {
      running = false;
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
      resetRepeatTracker(repeat);
    };

    const rescan = () => {
      const pad = syncPad();
      debugFrame(pad, performance.now());
    };

    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        stop();
        return;
      }
      rescan();
      start();
    };

    const onConnected = (event: GamepadEvent) => {
      preferredIndex = event.gamepad?.index ?? preferredIndex;
      rescan();
      if (document.visibilityState !== "hidden") start();
    };

    const onDisconnected = () => {
      resetRepeatTracker(repeat);
      rescan();
      if (document.visibilityState !== "hidden") start();
    };

    const clearMarks = () => {
      document
        .querySelectorAll("[data-gamepad-focus]")
        .forEach((element) => element.removeAttribute("data-gamepad-focus"));
    };

    const onGesture = () => {
      // Chromium/WebView2 may only expose pads after a user gesture on the
      // document, so a click or key press is also a cheap opportunity to
      // re-scan.
      rescan();
    };

    // Keep the ring on whatever target actually owns focus (keyboard, mouse or
    // gamepad), so `currentIndex` and the visible ring cannot drift apart.
    const onFocusIn = (event: FocusEvent) => {
      if (selection.activeIndex === null) return;
      const element = event.target as HTMLElement | null;
      const target = element?.closest?.("[data-gamepad-target]") as HTMLElement | null;
      if (!target) return;
      document
        .querySelectorAll("[data-gamepad-focus]")
        .forEach((marked) => marked.removeAttribute("data-gamepad-focus"));
      target.setAttribute("data-gamepad-focus", "true");
    };

    window.addEventListener("gamepadconnected", onConnected);
    window.addEventListener("gamepaddisconnected", onDisconnected);
    window.addEventListener("focus", rescan);
    window.addEventListener("pointerdown", clearMarks);
    window.addEventListener("pointerdown", onGesture);
    window.addEventListener("keydown", onGesture);
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("visibilitychange", onVisibility);

    rescan();
    if (document.visibilityState !== "hidden") start();

    return () => {
      disposed = true;
      stop();
      window.removeEventListener("gamepadconnected", onConnected);
      window.removeEventListener("gamepaddisconnected", onDisconnected);
      window.removeEventListener("focus", rescan);
      window.removeEventListener("pointerdown", clearMarks);
      window.removeEventListener("pointerdown", onGesture);
      window.removeEventListener("keydown", onGesture);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [enabled]);

  return state;
}

/**
 * Moves DOM focus through a list of elements and marks the focused element so
 * the ring is visible even when the browser's focus-visible heuristic would
 * hide it (gamepad input is not a keyboard event). `scroll: false` is for
 * rails that already scroll themselves (Big Picture).
 */
export function focusGamepadTarget(
  targets: HTMLElement[],
  index: number,
  smooth: boolean,
  options: { scroll?: boolean } = {},
): boolean {
  if (typeof document !== "undefined") {
    document
      .querySelectorAll("[data-gamepad-focus]")
      .forEach((element) => element.removeAttribute("data-gamepad-focus"));
  }
  return Boolean(applyGamepadFocus(targets, index, { smooth, scroll: options.scroll }));
}
