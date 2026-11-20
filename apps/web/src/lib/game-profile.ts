import type { ProfileChange } from "./types";

/** Text for one side of a profile diff; an em dash means "absent". */
export function profileValue(value: string | null): string {
  if (value === null || value === "") return "—";
  return value;
}

/**
 * Short label for a dotted diff path (#300). Environment keys and unknown
 * future keys are the two families that need more than the raw path.
 */
export function profileFieldLabel(field: string): string {
  if (field.startsWith("launch.env.")) return `ENV ${field.slice("launch.env.".length)}`;
  if (field.startsWith("future.")) return `Future ${field.slice("future.".length)}`;
  return field;
}

/** True when the proposal changes something the user can see. */
export function proposalHasChanges(diff: ProfileChange[]): boolean {
  return diff.length > 0;
}
