import { useEffect, useRef, useState } from "react";

/**
 * Gamepad API navigation for the library and Big Picture.
 *
 * One polling loop per mounted screen reads the first connected pad:
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

const AXIS_THRESHOLD = 0.55;
const FIRST_REPEAT_MS = 380;
const REPEAT_MS = 130;

const DIRECTION_BUTTONS: Record<number, GamepadAction> = {
  12: "up",
  13: "down",
  14: "left",
  15: "right",
};

const EDGE_BUTTONS: Record<number, GamepadAction> = {
  0: "confirm",
  1: "back",
  2: "menu",
  3: "downloads",
  4: "prevTab",
  5: "nextTab",
};

export function useGamepad(
  onAction: (action: GamepadAction) => void,
  options: { enabled?: boolean } = {},
): GamepadState {
  const enabled = options.enabled ?? true;
  const [state, setState] = useState<GamepadState>({ connected: false, id: null });
  const handlerRef = useRef(onAction);
  handlerRef.current = onAction;

  useEffect(() => {
    if (!enabled) return;
    if (typeof navigator === "undefined" || typeof navigator.getGamepads !== "function") return;

    let frame = 0;
    const pressed = new Map<string, boolean>();
    const nextRepeat = new Map<string, number>();
    const lastAxis = new Map<"x" | "y", string>();
    let connectedId: string | null = null;

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

    const fireRepeatable = (action: GamepadAction, now: number) => {
      const next = nextRepeat.get(action) ?? 0;
      if (now >= next) {
        fire(action);
        nextRepeat.set(action, now + (next === 0 ? FIRST_REPEAT_MS : REPEAT_MS));
      }
    };

    const loop = () => {
      refreshConnection();
      const pads = Array.from(navigator.getGamepads?.() ?? []);
      const pad = pads.find((entry) => entry?.connected);
      const now = performance.now();

      if (pad) {
        for (const [index, action] of Object.entries(DIRECTION_BUTTONS)) {
          const isDown = Boolean(pad.buttons[Number(index)]?.pressed);
          const wasDown = pressed.get(`b${index}`) ?? false;
          if (isDown && !wasDown) fireRepeatable(action, now);
          if (isDown) nextRepeat.set(action, nextRepeat.get(action) ?? now + FIRST_REPEAT_MS);
          else nextRepeat.delete(action);
          pressed.set(`b${index}`, isDown);
        }

        for (const [index, action] of Object.entries(EDGE_BUTTONS)) {
          const isDown = Boolean(pad.buttons[Number(index)]?.pressed);
          const wasDown = pressed.get(`b${index}`) ?? false;
          if (isDown && !wasDown) fire(action);
          pressed.set(`b${index}`, isDown);
        }

        const x = pad.axes[0] ?? 0;
        const y = pad.axes[1] ?? 0;
        const axis = (name: "x" | "y", value: number, negative: GamepadAction, positive: GamepadAction) => {
          const key = `a${name}${value < 0 ? "-" : "+"}`;
          const active = value < -AXIS_THRESHOLD ? negative : value > AXIS_THRESHOLD ? positive : null;
          if (active) {
            if (key !== lastAxis.get(name)) {
              lastAxis.set(name, key);
              fireRepeatable(active, now);
              nextRepeat.set(active, now + FIRST_REPEAT_MS);
            } else {
              fireRepeatable(active, now);
            }
          } else {
            lastAxis.delete(name);
            nextRepeat.delete(negative);
            nextRepeat.delete(positive);
          }
        };
        axis("x", x, "left", "right");
        axis("y", y, "up", "down");
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
