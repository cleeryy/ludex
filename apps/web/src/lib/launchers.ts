import type { LauncherId, LauncherSource } from "@/lib/types";

/**
 * Launcher sources (#155), in the order the desktop scan reports them. The ids
 * are the ones stored in `LocalSettings.launcherSources` and sent to
 * `launcher_scan` / `launcher_import`; the labels mirror `Launcher::label()` in
 * the Rust backend.
 */
export const LAUNCHER_SOURCES: ReadonlyArray<{ id: LauncherId; label: string }> = [
  { id: "steam", label: "Steam" },
  { id: "epic", label: "Epic Games" },
  { id: "gog", label: "GOG Galaxy" },
  { id: "ea", label: "EA app" },
  { id: "ubisoft", label: "Ubisoft Connect" },
  { id: "xbox", label: "Xbox / Microsoft Store" },
  { id: "itch", label: "itch.io" },
  { id: "heroic", label: "Heroic" },
  { id: "lutris", label: "Lutris" },
];

/** Every source id, in display order. */
export const ALL_LAUNCHER_IDS: LauncherId[] = LAUNCHER_SOURCES.map((source) => source.id);

/**
 * Canonical form of a stored source list: known ids only, deduplicated, in
 * display order. A missing value (a settings file written before #155) means
 * every source; an empty array means the user unchecked everything.
 */
export function normalizeLauncherSources(
  values: readonly string[] | null | undefined,
): LauncherId[] {
  if (!values) return [...ALL_LAUNCHER_IDS];
  const enabled = new Set(values.map((value) => String(value).trim().toLowerCase()));
  return ALL_LAUNCHER_IDS.filter((id) => enabled.has(id));
}

/** What the preview shows next to one source (#155). */
export type LauncherSourceState = "disabled" | "missing" | "empty" | "detected";

/**
 * Per-source status. "disabled" wins over everything else: a source the user
 * turned off was never read, so "not detected" would be a lie.
 */
export function launcherSourceState(
  source: LauncherSource | undefined,
  enabled: boolean,
): LauncherSourceState {
  if (!enabled) return "disabled";
  if (!source || !source.available) return "missing";
  return source.found > 0 ? "detected" : "empty";
}
