import { useSyncExternalStore } from "react";
import { DEFAULT_DISPLAY_PROFILE, withDisplayMode } from "@/lib/display-profile";
import type { LaunchMode, LaunchOptions } from "@/lib/types";

/**
 * The three settings tiers (#299).
 *
 * #283 asked for one surface that expresses intent instead of configuration:
 * a computed "this game is ready" state, six curated switches, and the raw
 * knobs only behind an explicit unlock. This module is the pure part: the tier
 * table, the curated switch → profile-field mapping, the readiness rule and the
 * device-local unlock flag. The component renders it; nothing here touches
 * React state or the Tauri bridge, so every rule is testable.
 *
 * ## One source of truth
 *
 * A tier-2 switch never owns storage. Each one reads and writes the field the
 * existing desktop features already use:
 *
 * | switch       | field                                     |
 * |--------------|-------------------------------------------|
 * | wrapper      | `display.mode` (Gamescope nested window)  |
 * | frameLimit   | `display.refresh` (nested refresh rate)   |
 * | hdr          | `display.hdr`                             |
 * | adaptiveSync | `display.adaptiveSync`                    |
 * | overlay      | `overlayDisabledGames` (overlay settings) |
 * | audioDevice  | `env.PULSE_SINK`                          |
 *
 * Five of the six live in `LaunchOptions`; the overlay opt-out is the one
 * existing feature whose store is the overlay settings document, so it maps to
 * `set_overlay_game_disabled` instead of duplicating a flag in the profile.
 * Tier 3 (the old launch-options panel) edits the very same fields, so no
 * setting exists in only one tier's storage.
 */

export type SettingsTierId = "works" | "game" | "advanced";

/** English message ids; the UI translates them with `t()`. */
export interface SettingsTierMeta {
  id: SettingsTierId;
  labelKey: string;
  descriptionKey: string;
}

export const SETTINGS_TIERS = [
  {
    id: "works",
    labelKey: "Just works",
    descriptionKey: "Ludex checks this game and fixes what it can. Nothing to configure.",
  },
  {
    id: "game",
    labelKey: "Per game",
    descriptionKey: "Six useful switches. Each one changes exactly one value in this game's profile.",
  },
  {
    id: "advanced",
    labelKey: "Advanced",
    descriptionKey: "The raw launch settings. Unsupported: a hand-edited launch can stop the game from starting.",
  },
] as const satisfies readonly SettingsTierMeta[];

/* --------------------------- advanced unlock ----------------------------- */

/**
 * Device-local, like the console shell flag: whether someone asked to see the
 * raw settings is a property of this machine, not of the account. Default is
 * locked, so a first run never renders tier 3.
 */
export const ADVANCED_TIERS_STORAGE_KEY = "ludex.settings.tiers.advanced";

type Listener = () => void;
const listeners = new Set<Listener>();

function readAdvanced(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(ADVANCED_TIERS_STORAGE_KEY) === "1";
  } catch {
    // Private mode or a full quota: stay locked rather than guess.
    return false;
  }
}

export function advancedTiersUnlocked(): boolean {
  return readAdvanced();
}

export function setAdvancedTiersUnlocked(enabled: boolean): void {
  if (typeof window !== "undefined") {
    try {
      if (enabled) window.localStorage.setItem(ADVANCED_TIERS_STORAGE_KEY, "1");
      else window.localStorage.removeItem(ADVANCED_TIERS_STORAGE_KEY);
    } catch {
      /* storage is optional: the live store below still re-renders this session */
    }
  }
  for (const listener of listeners) listener();
}

export function subscribeAdvancedTiers(listener: Listener): () => void {
  listeners.add(listener);
  const onStorage = (event: StorageEvent) => {
    if (event.key !== null && event.key !== ADVANCED_TIERS_STORAGE_KEY) return;
    for (const entry of listeners) entry();
  };
  if (typeof window !== "undefined") window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    if (typeof window !== "undefined") window.removeEventListener("storage", onStorage);
  };
}

/** Live flag for the surface; the lock button flips it without a reload. */
export function useAdvancedTiersUnlocked(): boolean {
  return useSyncExternalStore(subscribeAdvancedTiers, advancedTiersUnlocked, () => false);
}

/**
 * The tiers a user can see. Tier 3 is opt-in, so the first-run answer is the
 * two first tiers — this is the function the acceptance test pins.
 */
export function visibleSettingsTiers(unlocked: boolean = advancedTiersUnlocked()): SettingsTierId[] {
  return unlocked ? SETTINGS_TIERS.map((tier) => tier.id) : ["works", "game"];
}

/* ----------------------------- tier-2 switches --------------------------- */

export type Tier2SwitchId =
  | "wrapper"
  | "frameLimit"
  | "hdr"
  | "adaptiveSync"
  | "overlay"
  | "audioDevice";

/** Where a curated switch writes. Documented, and asserted by the tests. */
export type Tier2Storage = "launch-options" | "overlay-settings";

export interface Tier2SwitchMeta {
  id: Tier2SwitchId;
  /** English message id; the UI translates it with `t()`. */
  labelKey: string;
  storage: Tier2Storage;
  /** The one documented target, e.g. `display.mode`. Shown in docs/SETTINGS.md. */
  field: string;
  /** The display switches only exist where Gamescope can apply them. */
  platform: "linux" | "all";
}

export const TIER2_SWITCHES = [
  {
    id: "wrapper",
    labelKey: "Game window",
    storage: "launch-options",
    field: "display.mode",
    platform: "linux",
  },
  {
    id: "frameLimit",
    labelKey: "Frame limit",
    storage: "launch-options",
    field: "display.refresh",
    platform: "linux",
  },
  {
    id: "hdr",
    labelKey: "HDR",
    storage: "launch-options",
    field: "display.hdr",
    platform: "linux",
  },
  {
    id: "adaptiveSync",
    labelKey: "Adaptive sync",
    storage: "launch-options",
    field: "display.adaptiveSync",
    platform: "linux",
  },
  {
    id: "overlay",
    labelKey: "In-game overlay",
    storage: "overlay-settings",
    field: "overlayDisabledGames",
    platform: "all",
  },
  {
    id: "audioDevice",
    labelKey: "Audio device",
    storage: "launch-options",
    field: "env.PULSE_SINK",
    platform: "linux",
  },
] as const satisfies readonly Tier2SwitchMeta[];

/** Switches `withTier2Switch` can edit: everything except the overlay opt-out. */
export type LaunchOptionsTier2Switch = Exclude<Tier2SwitchId, "overlay">;

/** The six curated switches on Linux; the overlay alone everywhere else. */
export function tier2SwitchesFor(linux: boolean): readonly Tier2SwitchMeta[] {
  return TIER2_SWITCHES.filter((entry) => entry.platform === "all" || linux);
}

/** A first cap a user can understand; tier 3 can fine-tune it. */
export const DEFAULT_FRAME_LIMIT = 60;
/** PulseAudio/PipeWire sink name read by every game process. */
export const AUDIO_DEVICE_ENV_KEY = "PULSE_SINK";

/** The per-game profile, plus the one overlay fact tier 2 reads. */
export interface Tier2Sources {
  options: LaunchOptions;
  overlayDisabled: boolean;
}

/** An untouched profile, matching the desktop's own defaults. */
export function defaultLaunchOptions(): LaunchOptions {
  return {
    mode: "auto",
    toolId: "",
    prefixPath: "",
    env: {},
    winetricksVerbs: [],
    steamAppId: null,
    workingDir: "",
    display: { ...DEFAULT_DISPLAY_PROFILE },
  };
}

export function readTier2Switch(sources: Tier2Sources, id: Tier2SwitchId): boolean {
  switch (id) {
    case "wrapper":
      return sources.options.display.mode === "gamescope";
    case "frameLimit":
      return sources.options.display.refresh > 0;
    case "hdr":
      return sources.options.display.hdr;
    case "adaptiveSync":
      return sources.options.display.adaptiveSync;
    case "overlay":
      return !sources.overlayDisabled;
    case "audioDevice":
      return Boolean(sources.options.env[AUDIO_DEVICE_ENV_KEY]?.trim());
  }
}

/**
 * One switch, one profile edit. `wrapper` seeds the 1920×1080 the desktop
 * needs for a usable Gamescope profile the first time it is enabled; turning it
 * back off only flips the mode, so the resolution the user picked survives a
 * round trip. `audioDevice` writes (or removes) exactly one environment entry.
 * The overlay case is handled by `withOverlayDisabled` because its store is the
 * overlay settings document, not the profile.
 */
export function withTier2Switch(
  options: LaunchOptions,
  id: LaunchOptionsTier2Switch,
  enabled: boolean,
  audioDevice = "",
): LaunchOptions {
  switch (id) {
    case "wrapper":
      return { ...options, display: withDisplayMode(options.display, enabled ? "gamescope" : "desktop") };
    case "frameLimit":
      return {
        ...options,
        display: {
          ...options.display,
          refresh: enabled ? options.display.refresh || DEFAULT_FRAME_LIMIT : 0,
        },
      };
    case "hdr":
      return { ...options, display: { ...options.display, hdr: enabled } };
    case "adaptiveSync":
      return { ...options, display: { ...options.display, adaptiveSync: enabled } };
    case "audioDevice": {
      const env = { ...options.env };
      const device = audioDevice.trim();
      if (enabled && device) env[AUDIO_DEVICE_ENV_KEY] = device;
      else delete env[AUDIO_DEVICE_ENV_KEY];
      return { ...options, env };
    }
  }
}

/** The overlay opt-out list, with exactly one membership change. */
export function withOverlayDisabled(
  disabledGames: readonly number[],
  gameId: number,
  disabled: boolean,
): number[] {
  const next = new Set(disabledGames);
  if (disabled) next.add(gameId);
  else next.delete(gameId);
  return [...next].sort((left, right) => left - right);
}

/* --------------------------------- diffing ------------------------------- */

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Leaf paths that differ between two profiles, e.g. `["display.mode"]`. The
 * tier-2 contract is "exactly one documented change", and this is how the test
 * proves it instead of eyeballing a JSON blob.
 */
export function changedFields(before: unknown, after: unknown, prefix = ""): string[] {
  if (Object.is(before, after)) return [];
  if (Array.isArray(before) && Array.isArray(after)) {
    return JSON.stringify(before) === JSON.stringify(after) ? [] : [prefix || "value"];
  }
  if (isRecord(before) && isRecord(after)) {
    const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
    return [...keys].flatMap((key) =>
      changedFields(before[key], after[key], prefix ? `${prefix}.${key}` : key),
    );
  }
  return [prefix || "value"];
}

/* -------------------------------- tier 1 --------------------------------- */

export type TierOneCause =
  | "runtime"
  | "prefix"
  | "vulkan"
  | "disk"
  | "arch"
  | "mode"
  | "manual"
  | "generic";

export type TierOneStatus = "checking" | "ready" | "needsFix";

export interface TierOneInput {
  /** `RuntimeDiagnosis.ready`; null while the check is still running. */
  ready: boolean | null;
  platform: string;
  windowsGame: boolean;
  mode: LaunchMode;
  issues: ReadonlyArray<{ code: string; severity: "info" | "warning" | "blocking" }>;
}

export interface TierOneState {
  status: TierOneStatus;
  cause: TierOneCause | null;
}

/** A missing capability is a cause; a version string is not copy. */
const CAUSE_BY_ISSUE: Readonly<Record<string, TierOneCause>> = {
  "missing-umu": "runtime",
  "missing-proton": "runtime",
  "no-wrapper": "runtime",
  "launch-mode": "runtime",
  "prefix-missing": "prefix",
  "prefix-unsafe": "prefix",
  "no-vulkan": "vulkan",
  "disk-space": "disk",
  "unsupported-arch": "arch",
};

/**
 * The computed tier-1 verdict. No switch informs it: the runtime diagnosis is
 * the source, with one fallback for a Windows game explicitly told to start
 * without a compatibility layer.
 */
export function tierOneState(input: TierOneInput): TierOneState {
  if (input.ready === null) return { status: "checking", cause: null };
  if (input.ready) return { status: "ready", cause: null };

  const blocking = input.issues.find((issue) => issue.severity === "blocking");
  const candidate = blocking ?? input.issues[0];
  const cause: TierOneCause = candidate
    ? (CAUSE_BY_ISSUE[candidate.code] ?? "manual")
    : input.windowsGame && input.platform === "linux" && input.mode === "native"
      ? "mode"
      : "generic";
  return { status: "needsFix", cause };
}
