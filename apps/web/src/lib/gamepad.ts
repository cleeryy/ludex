import { useEffect, useRef, useState } from "react";
import type { ControllerProfile } from "@/lib/types";
import { actionForAxis, actionForButton, presetProfile } from "@/lib/controller-profiles";

/**
 * Gamepad API navigation for the library and Big Picture.
 *
 * One polling loop per mounted screen reads the first connected pad. The
 * logical actions come from the active [`ControllerProfile`] (#51): deadzone,
 * sensitivity, curve, inversion, remapping and trigger threshold are applied
 * here, so a remapped pad behaves the same everywhere. Without a stored
 * profile the Xbox preset is used, which matches the historical behaviour:
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
 * Directions repeat after a short delay; face and shoulder buttons fire once
 * per press. Actions are ignored while a modal dialog is open so the gamepad
 * cannot fight a form. The hook is inert when no pad is connected, which keeps
 * touch and keyboard users unaffected.
 */

export type GamepadAction =
  | "left"
  | "right"
  | "up"
  | "down"
  | "confirm"
  | "back"
  | "menu"
  | "downloads"
  | "prevTab"
  | "nextTab";

export interface GamepadState {
  connected: boolean;
  id: string | null;
}

const FIRST_REPEAT_MS = 380;
const REPEAT_MS = 130;
const REPEATABLE = new Set<GamepadAction>(["left", "right", "up", "down"]);
const MAX_BUTTONS = 32;

export function useGamepad(
  onAction: (action: GamepadAction) => void,
  options: { enabled?: boolean; profile?: ControllerProfile | null } = {},
): GamepadState {
  const enabled = options.enabled ?? true;
  const [state, setState] = useState<GamepadState>({ connected: false, id: null });
  const handlerRef = useRef(onAction);
  handlerRef.current = onAction;
  const profileRef = useRef<ControllerProfile>(options.profile ?? presetProfile("xbox"));
  profileRef.current = options.profile ?? presetProfile("xbox");

  useEffect(() => {
    if (!enabled) return;
    if (typeof navigator === "undefined" || typeof navigator.getGamepads !== "function") return;

    let frame = 0;
    let active = new Set<GamepadAction>();
    const nextRepeat = new Map<GamepadAction, number>();
    let connectedId: string | null = null;
    let lastRumble = 0;

    const refreshConnection = () => {
      const pads = Array.from(navigator.getGamepads?.() ?? []);
      const pad = pads.find((entry) => entry?.connected);
      const id = pad?.id ?? null;
      if (id !== connectedId) {
        connectedId = id;
        setState({ connected: Boolean(id), id });
      }
    };

    const isModalOpen = () => Boolean(document.querySelector('[role="dialog"][aria-modal="true"]'));

    const clearGamepadMarks = () => {
      document.querySelectorAll("[data-gamepad-focus]").forEach((element) => element.removeAttribute("data-gamepad-focus"));
    };
    window.addEventListener("pointerdown", clearGamepadMarks);

    const fire = (action: GamepadAction) => {
      if (isModalOpen()) return;
      handlerRef.current(action);
    };

    const vibrate = (pad: Gamepad) => {
      const profile = profileRef.current;
      if (!profile.vibration) return;
      const now = performance.now();
      if (now - lastRumble < 120) return;
      const actuator = pad.vibrationActuator;
      if (!actuator || typeof actuator.playEffect !== "function") return;
      lastRumble = now;
      void actuator.playEffect("dual-rumble", { duration: 70, strongMagnitude: 0.35, weakMagnitude: 0.25 }).catch(() => undefined);
    };

    const trigger = (action: GamepadAction, now: number, pad: Gamepad) => {
      if (active.has(action) && !REPEATABLE.has(action)) return;
      const next = nextRepeat.get(action);
      if (REPEATABLE.has(action)) {
        if (next !== undefined && now < next) return;
        nextRepeat.set(action, now + (next === undefined ? FIRST_REPEAT_MS : REPEAT_MS));
      }
      fire(action);
      vibrate(pad);
    };

    const loop = () => {
      refreshConnection();
      const pads = Array.from(navigator.getGamepads?.() ?? []);
      const pad = pads.find((entry) => entry?.connected);
      const now = performance.now();

      if (pad) {
        const profile = profileRef.current;
        const pressed = new Set<GamepadAction>();
        const buttons = Math.min(pad.buttons.length, MAX_BUTTONS);
        for (let index = 0; index < buttons; index += 1) {
          const button = pad.buttons[index];
          const action = actionForButton(profile, index, button?.value ?? (button?.pressed ? 1 : 0));
          if (action) pressed.add(action);
        }
        for (const axis of [0, 1, 2, 3]) {
          const x = pad.axes[axis * 2] ?? 0;
          const y = pad.axes[axis * 2 + 1] ?? 0;
          const action = actionForAxis(profile, axis, x, y);
          if (action) pressed.add(action);
        }

        for (const action of pressed) trigger(action, now, pad);
        for (const action of active) {
          if (!pressed.has(action)) nextRepeat.delete(action);
        }
        active = pressed;
      } else {
        active = new Set();
        nextRepeat.clear();
      }

      frame = requestAnimationFrame(loop);
    };

    const onConnected = () => refreshConnection();
    const onDisconnected = () => refreshConnection();
    window.addEventListener("gamepadconnected", onConnected);
    window.addEventListener("gamepaddisconnected", onDisconnected);
    refreshConnection();
    frame = requestAnimationFrame(loop);

    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("gamepadconnected", onConnected);
      window.removeEventListener("gamepaddisconnected", onDisconnected);
      window.removeEventListener("pointerdown", clearGamepadMarks);
    };
  }, [enabled]);

  return state;
}

/**
 * Moves DOM focus through a list of elements and marks the focused element so
 * the ring is visible even when the browser's focus-visible heuristic would
 * hide it (gamepad input is not a keyboard event).
 */
export function focusGamepadTarget(targets: HTMLElement[], index: number, smooth: boolean): void {
  const target = targets[index];
  if (!target) return;
  for (const element of targets) element.removeAttribute("data-gamepad-focus");
  target.setAttribute("data-gamepad-focus", "true");
  target.focus({ preventScroll: true });
  target.scrollIntoView({ block: "nearest", inline: "nearest", behavior: smooth ? "smooth" : "auto" });
}
