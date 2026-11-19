import type { DisplayProfile, GamescopeInfo } from "@/lib/types";

/**
 * Pure helpers for the per-game display profile (#287).
 *
 * The Rust side owns the Gamescope command; this module only holds the draft
 * defaults and the human summary shown in the launch options.
 */

/** Desktop default: a strict no-op, matching the backend default. */
export const DEFAULT_DISPLAY_PROFILE: DisplayProfile = {
  mode: "desktop",
  width: 0,
  height: 0,
  refresh: 0,
  filter: "",
  hdr: false,
};

/** First value a newly enabled profile gets; the user can edit both. */
export const DEFAULT_GAMESCOPE_WIDTH = 1920;
export const DEFAULT_GAMESCOPE_HEIGHT = 1080;

/**
 * Switch the profile mode, seeding a usable resolution the first time Gamescope
 * is enabled. Turning it back to desktop keeps the dimensions so toggling does
 * not lose the user's settings.
 */
export function withDisplayMode(
  profile: DisplayProfile,
  mode: DisplayProfile["mode"],
): DisplayProfile {
  if (mode === "desktop") return { ...profile, mode };
  return {
    ...profile,
    mode,
    width: profile.width > 0 ? profile.width : DEFAULT_GAMESCOPE_WIDTH,
    height: profile.height > 0 ? profile.height : DEFAULT_GAMESCOPE_HEIGHT,
  };
}

/**
 * `1920 × 1080 · 60 Hz · FSR · HDR`, or null for the desktop default. The
 * separator is data, not copy: no translation needed.
 */
export function displayProfileSummary(profile: DisplayProfile): string | null {
  if (profile.mode !== "gamescope") return null;
  const parts = [`${profile.width} × ${profile.height}`];
  if (profile.refresh > 0) parts.push(`${profile.refresh} Hz`);
  if (profile.filter) parts.push(profile.filter.toUpperCase());
  if (profile.hdr) parts.push("HDR");
  return parts.join(" · ");
}

/** Filters the installed Gamescope lists for `--filter`. */
export function availableFilters(gamescope: GamescopeInfo | null | undefined): string[] {
  return gamescope?.filters ?? [];
}
