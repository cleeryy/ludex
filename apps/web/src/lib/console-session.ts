import type {
  ConsoleSessionEntryState,
  ConsoleSessionPlan,
  ConsoleSessionPlannedFile,
  ConsoleSessionStatus,
} from "@/lib/types";

/**
 * Console session (#286) presentation logic, kept out of the panel so the
 * counting and the refusal rules are tested without a desktop host.
 */

/** What a consent plan does, counted from the authoritative plan. */
export interface ConsoleSessionCounts {
  writes: number;
  restores: number;
  removes: number;
  backups: number;
}

export function planCounts(plan: ConsoleSessionPlan): ConsoleSessionCounts {
  const counts: ConsoleSessionCounts = { writes: 0, restores: 0, removes: 0, backups: 0 };
  for (const file of plan.files) {
    if (file.action === "remove") {
      counts.removes += 1;
      continue;
    }
    if (plan.enable) counts.writes += 1;
    else counts.restores += 1;
    if (file.backup) counts.backups += 1;
  }
  return counts;
}

/** What one planned file means to the user: write, restore or remove. */
export function plannedFileKind(
  file: ConsoleSessionPlannedFile,
  enable: boolean,
): "write" | "restore" | "remove" {
  if (file.action === "remove") return "remove";
  return enable ? "write" : "restore";
}

/** The manager name, or `null` when detection refused. */
export function managerLabel(status: ConsoleSessionStatus): string | null {
  return status.manager?.name ?? null;
}

/**
 * True when the seat is not the Wayland active seat Sunshine needs. The
 * toggle still works; the panel shows the warning because a stream would be
 * black otherwise.
 */
export function needsSeatAttention(status: ConsoleSessionStatus): boolean {
  return status.supported && !status.seat.compatible;
}

/** Whether the boot toggle can be pressed at all. */
export function canToggle(status: ConsoleSessionStatus): boolean {
  if (!status.supported || !status.pkexecAvailable) return false;
  // Disabling is always possible from the recorded state, even if the display
  // manager later disappeared; enabling needs a detected manager.
  return status.enabled || status.manager !== null;
}

/** Which sentence the toggle needs. The panel owns the translated copy. */
export type ToggleExplanation =
  | { kind: "unsupported"; reason: string }
  | { kind: "polkit" }
  | { kind: "refusal"; reason: string }
  | { kind: "enabled"; manager: string }
  | { kind: "ready"; manager: string };

export function toggleExplanation(status: ConsoleSessionStatus): ToggleExplanation {
  if (!status.supported) return { kind: "unsupported", reason: status.message };
  if (!status.pkexecAvailable) return { kind: "polkit" };
  if (status.enabled) {
    return { kind: "enabled", manager: status.manager?.name ?? status.session };
  }
  if (!status.manager) return { kind: "refusal", reason: status.managerReason ?? status.message };
  return { kind: "ready", manager: status.manager.name };
}

/** The entry row's action: install, remove, or resolve a conflict. */
export function entryAction(state: ConsoleSessionEntryState): "install" | "remove" | "conflict" {
  if (state === "installed") return "remove";
  if (state === "differs") return "conflict";
  return "install";
}
