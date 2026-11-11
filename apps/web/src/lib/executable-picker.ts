import type { GameExecutable, GameExecutableKind } from "./types";

/**
 * Pure helpers behind the Launch options executable picker (#143).
 *
 * The component is thin on purpose: filtering, the loading/empty/error state
 * and the keyboard cursor are plain functions so they stay testable without a
 * DOM or an extra test dependency.
 */

export type PickerState = "loading" | "error" | "empty" | "ready";

/** State to render for one filtered picker. */
export function pickerState(input: {
  isLoading: boolean;
  error: unknown;
  entries: GameExecutable[] | undefined;
  kind: GameExecutableKind;
}): PickerState {
  if (input.isLoading) return "loading";
  if (input.error) return "error";
  if (!input.entries || !input.entries.some((entry) => entry.kind === input.kind)) {
    return "empty";
  }
  return "ready";
}

/** Entries of one kind, filtered by a case-insensitive search over path/name. */
export function filterExecutables(
  entries: GameExecutable[],
  kind: GameExecutableKind,
  query: string,
): GameExecutable[] {
  const needle = query.trim().toLowerCase();
  return entries.filter((entry) => {
    if (entry.kind !== kind) return false;
    if (!needle) return true;
    return (
      entry.path.toLowerCase().includes(needle) ||
      entry.name.toLowerCase().includes(needle)
    );
  });
}

/** The program Ludex would start now, when it is part of this picker. */
export function recommendedExecutable(
  entries: GameExecutable[],
  kind: GameExecutableKind,
): GameExecutable | null {
  return entries.find((entry) => entry.kind === kind && entry.recommended) ?? null;
}

/**
 * Next cursor position for a keyboard event, or `null` when the key is not a
 * navigation key. The list wraps around like a native combo box.
 */
export function activeIndexForKey(
  current: number,
  key: string,
  length: number,
): number | null {
  if (length === 0) return null;
  switch (key) {
    case "ArrowDown":
      return current < 0 ? 0 : (current + 1) % length;
    case "ArrowUp":
      return current <= 0 ? length - 1 : current - 1;
    case "Home":
      return 0;
    case "End":
      return length - 1;
    default:
      return null;
  }
}

/** Keep the cursor inside a list that shrank after filtering. */
export function clampActiveIndex(current: number, length: number): number {
  if (length === 0) return -1;
  if (current < 0) return -1;
  return current >= length ? length - 1 : current;
}
