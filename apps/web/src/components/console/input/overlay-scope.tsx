import type { ActionSource, GamepadAction } from "@/lib/gamepad";
import { useInputScope, type InputScopeOptions } from "@/components/console/input/input-provider";

/**
 * The top-surface input layer (#267).
 *
 * While a menu, the quick switcher, the power confirm or the launch plate is
 * up, this scope sits above the library scope and dispatches the same console
 * commands. The library underneath stops receiving presses — which is what
 * removes the double dispatch — and popping the scope restores the exact card
 * the overlay covered.
 */
export function ConsoleOverlayInputScope({
  onAction,
  onKey,
  options,
}: {
  onAction: (action: GamepadAction, source: ActionSource | null, repeated?: boolean) => void;
  onKey: (event: KeyboardEvent) => void;
  options: InputScopeOptions;
}) {
  useInputScope({ id: "console-overlay", onAction, onKey, options });
  return null;
}
