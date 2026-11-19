import type {
  DisplayCapabilities,
  DisplayCapability,
  DisplayOutput,
  DisplayProfile,
  GamescopeInfo,
} from "@/lib/types";

/**
 * Pure helpers for the per-game display profile (#287) and its capability
 * switches (#292).
 *
 * The Rust side owns the Gamescope command and the fallback warnings; this
 * module holds the draft defaults, the human summary and the gating rules the
 * two switches use.
 */

/** Desktop default: a strict no-op, matching the backend default. */
export const DEFAULT_DISPLAY_PROFILE: DisplayProfile = {
  mode: "desktop",
  width: 0,
  height: 0,
  refresh: 0,
  filter: "",
  adaptiveSync: false,
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
 * `1920 × 1080 · 60 Hz · FSR · VRR · HDR`, or null for the desktop default. The
 * separator is data, not copy: no translation needed.
 */
export function displayProfileSummary(profile: DisplayProfile): string | null {
  if (profile.mode !== "gamescope") return null;
  const parts = [`${profile.width} × ${profile.height}`];
  if (profile.refresh > 0) parts.push(`${profile.refresh} Hz`);
  if (profile.filter) parts.push(profile.filter.toUpperCase());
  if (profile.adaptiveSync) parts.push("VRR");
  if (profile.hdr) parts.push("HDR");
  return parts.join(" · ");
}

/** Filters the installed Gamescope lists for `--filter`. */
export function availableFilters(gamescope: GamescopeInfo | null | undefined): string[] {
  return gamescope?.filters ?? [];
}

/** The active output, falling back to the first connected one (#292). */
export function activeDisplayOutput(
  capabilities: DisplayCapabilities | null | undefined,
): DisplayOutput | null {
  if (!capabilities) return null;
  return (
    capabilities.outputs.find((output) => output.connector === capabilities.active) ??
    capabilities.outputs[0] ??
    null
  );
}

/**
 * Why a switch is disabled, or null when it can be turned on (#292):
 *
 * - `noGamescope`: nothing would apply the flag;
 * - `unsupportedBuild`: the installed Gamescope does not list the flag;
 * - `output`: the active output explicitly reports the capability as
 *   incompatible (`unknown` never blocks: only an explicit no is a reason).
 */
export type DisplayCapabilityBlock = "noGamescope" | "unsupportedBuild" | "output";

export function displayCapabilityBlock(
  kind: "vrr" | "hdr",
  gamescope: GamescopeInfo | null | undefined,
  capabilities: DisplayCapabilities | null | undefined,
): DisplayCapabilityBlock | null {
  if (!gamescope) return "noGamescope";
  if (kind === "vrr" && !gamescope.adaptiveSync) return "unsupportedBuild";
  if (kind === "hdr" && !gamescope.hdr) return "unsupportedBuild";
  const verdict: DisplayCapability | undefined = activeDisplayOutput(capabilities)?.[kind];
  return verdict === "incapable" ? "output" : null;
}

/**
 * One data line describing the active output, e.g.
 * `card0-DP-1 · VRR: supported · HDR: incompatible · 240 Hz · 10-bit`.
 * Status words are machine vocabulary, kept untranslated like the Gamescope
 * version line next to it.
 */
export function displayCapabilityLine(
  capabilities: DisplayCapabilities | null | undefined,
): string | null {
  const output = activeDisplayOutput(capabilities);
  if (!output) return null;
  const parts = [
    output.connector,
    `VRR: ${output.vrr === "incapable" ? "incompatible" : output.vrr}`,
    `HDR: ${output.hdr === "incapable" ? "no metadata" : output.hdr}`,
  ];
  if (output.maxRefresh) parts.push(`${output.maxRefresh} Hz`);
  if (output.colorDepth) parts.push(`${output.colorDepth}-bit`);
  return parts.join(" · ");
}
