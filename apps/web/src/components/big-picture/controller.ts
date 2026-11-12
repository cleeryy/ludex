import { CONTROLLER_PRESETS, presetProfile } from "@/lib/controller-profiles";
import type { ActionSource } from "@/lib/gamepad-core";
import { t } from "@/lib/i18n";
import type { ControllerAction, ControllerProfile, ControllerProfilesResponse } from "@/lib/types";
import type { BigPictureLayout } from "./layout";

/* --------------------------------------------------------------------------
   Controller-facing Big Picture logic (#138).

   Everything that can be decided without the DOM lives here: which pad family
   the hints should name, what each context advertises, the extra inputs Big
   Picture maps on top of the stored profile (#51), and the device-local
   preference that picks a profile without leaving the screen. The screen and
   the overlay components only render these decisions, so the hint strip, the
   action router and the tests share one source of truth.
   -------------------------------------------------------------------------- */

/* -------------------------------- pad family ------------------------------- */

export type PadFamily = "xbox" | "dualsense" | "steamdeck" | "generic";

/**
 * Guesses the family from the pad id string the browser reports. A pad that
 * identifies itself generically falls back to the name of the active profile,
 * then to `generic` (whose button hints are the numbered Xbox layout most
 * third-party pads expose through the standard mapping).
 */
export function padFamily(
  id: string | null | undefined,
  profile?: Pick<ControllerProfile, "name"> | null,
): PadFamily {
  const fromId = familyFromText(id);
  if (fromId) return fromId;
  return familyFromText(profile?.name) ?? "generic";
}

function familyFromText(value: string | null | undefined): PadFamily | null {
  if (!value) return null;
  const text = value.toLowerCase();
  if (/(steam ?deck|steamdeck)/.test(text)) return "steamdeck";
  if (/(xbox|xinput)/.test(text)) return "xbox";
  if (/(dualsense|dualshock|playstation|ps5|ps4|wireless controller|sony)/.test(text)) return "dualsense";
  return null;
}

export function padFamilyLabel(family: PadFamily): string {
  switch (family) {
    case "xbox":
      return "Xbox"; // pad family names, not interface copy
    case "dualsense":
      return "DualSense";
    case "steamdeck":
      return "Steam Deck";
    case "generic":
      return "Generic";
  }
}

/* ---------------------------------- hints ---------------------------------- */

/** Which physical input a hint advertises, before the family renames it. */
export type HintInput =
  | "confirm"
  | "back"
  | "search"
  | "downloads"
  | "shelf"
  | "page"
  | "menu"
  | "navigate"
  | "prevTab"
  | "nextTab";

export type HintId =
  | "navigate"
  | "select"
  | "play"
  | "shelf"
  | "page"
  | "search"
  | "downloads"
  | "menu"
  | "exit"
  | "choose"
  | "apply"
  | "close"
  | "type"
  | "delete"
  | "space"
  | "clear"
  | "open"
  | "cancel"
  | "back";

export interface Hint {
  id: HintId;
  /** Button labels, e.g. `["A"]` on Xbox or `["Cross"]` on DualSense. */
  keys: readonly string[];
}

export type HintContext =
  | "library"
  | "picker"
  | "menu"
  | "exit"
  | "profiles"
  | "downloads"
  | "search";

export interface HintOptions {
  layout: BigPictureLayout;
  family: PadFamily;
  /** Keyboard hints when no pad is driving the screen. */
  connected: boolean;
}

const PAD_LABELS: Record<PadFamily, Record<HintInput, string[]>> = {
  xbox: {
    confirm: ["A"],
    back: ["B"],
    search: ["X"],
    downloads: ["Y"],
    shelf: ["LB", "RB"],
    page: ["LT", "RT"],
    menu: ["Menu", "Guide"],
    navigate: ["D-pad"],
    prevTab: ["LB"],
    nextTab: ["RB"],
  },
  dualsense: {
    confirm: ["Cross"],
    back: ["Circle"],
    search: ["Square"],
    downloads: ["Triangle"],
    shelf: ["L1", "R1"],
    page: ["L2", "R2"],
    menu: ["Options", "PS"],
    navigate: ["D-pad"],
    prevTab: ["L1"],
    nextTab: ["R1"],
  },
  steamdeck: {
    confirm: ["A"],
    back: ["B"],
    search: ["X"],
    downloads: ["Y"],
    shelf: ["L1", "R1"],
    page: ["L2", "R2"],
    menu: ["Menu", "Steam"],
    navigate: ["D-pad"],
    prevTab: ["L1"],
    nextTab: ["R1"],
  },
  generic: {
    confirm: ["A"],
    back: ["B"],
    search: ["X"],
    downloads: ["Y"],
    shelf: ["L1", "R1"],
    page: ["L2", "R2"],
    menu: ["Start", "Guide"],
    navigate: ["D-pad"],
    prevTab: ["L1"],
    nextTab: ["R1"],
  },
};

/** Keyboard parity (#138): every controller action keeps a key binding. */
const KEYBOARD_LABELS: Record<HintInput, string[]> = {
  confirm: ["↵"],
  back: ["esc"],
  search: ["/"],
  downloads: ["D"],
  shelf: ["[", "]"],
  page: ["PgUp", "PgDn"],
  menu: ["M"],
  navigate: ["←", "→", "↑", "↓"],
  prevTab: ["["],
  nextTab: ["]"],
};

/** The rails only walk sideways, so their keyboard hint stays honest. */
function keyboardNavigate(layout: BigPictureLayout): string[] {
  return layout === "shelf" || layout === "hero" ? ["←", "→"] : ["←", "→", "↑", "↓"];
}

/**
 * Button hints for the active context (#138). Contexts change with the screen
 * (library, quick switcher, overlays, search) and the pad family changes the
 * labels (Xbox A/B/X/Y vs DualSense Cross/Circle/Square/Triangle). Without a
 * controller the same list is expressed in keys, so mouse/keyboard players
 * keep parity and the strip never advertises an action the screen ignores.
 */
export function hintsFor(context: HintContext, { layout, family, connected }: HintOptions): Hint[] {
  const pad = PAD_LABELS[family];
  const keys = (input: HintInput): string[] =>
    connected ? pad[input] : input === "navigate" ? keyboardNavigate(layout) : KEYBOARD_LABELS[input];

  switch (context) {
    case "library":
      return [
        { id: "select", keys: keys("navigate") },
        { id: "shelf", keys: keys("shelf") },
        { id: "page", keys: keys("page") },
        { id: "play", keys: keys("confirm") },
        { id: "search", keys: keys("search") },
        { id: "downloads", keys: keys("downloads") },
        { id: "menu", keys: keys("menu") },
        { id: "exit", keys: keys("back") },
      ];
    case "picker":
      return [
        { id: "choose", keys: keys("navigate") },
        { id: "apply", keys: keys("confirm") },
        { id: "close", keys: keys("back") },
      ];
    case "menu":
      return [
        { id: "choose", keys: keys("navigate") },
        { id: "open", keys: keys("confirm") },
        { id: "close", keys: keys("back") },
      ];
    case "exit":
      return [
        { id: "choose", keys: keys("navigate") },
        { id: "select", keys: keys("confirm") },
        { id: "close", keys: keys("back") },
      ];
    case "profiles":
      return [
        { id: "choose", keys: keys("navigate") },
        { id: "apply", keys: keys("confirm") },
        { id: "close", keys: keys("back") },
      ];
    case "downloads":
      return [
        { id: "choose", keys: keys("navigate") },
        { id: "open", keys: keys("confirm") },
        { id: "close", keys: keys("back") },
      ];
    case "search":
      if (!connected) {
        // A physical keyboard deletes with Backspace, adds a real space and
        // opens the best match with Enter; there is no keyboard "type" or
        // "clear" hint because those are on-screen-keyboard actions.
        return [
          { id: "navigate", keys: ["←", "→", "↑", "↓"] },
          { id: "open", keys: ["↵"] },
          { id: "delete", keys: ["⌫"] },
          { id: "space", keys: ["Space"] },
          { id: "back", keys: ["esc"] },
        ];
      }
      return [
        { id: "navigate", keys: keys("navigate") },
        { id: "type", keys: keys("confirm") },
        { id: "delete", keys: keys("search") },
        { id: "space", keys: keys("downloads") },
        { id: "clear", keys: keys("prevTab") },
        { id: "open", keys: keys("nextTab") },
        { id: "back", keys: keys("back") },
      ];
  }
}

/* ----------------------------- extra mappings ------------------------------ */

/** Start/Options on every standard-mapped pad. */
export const START_BUTTON = 9;
/** Guide/Home; Chrome reports it as 16, some Steam Deck builds as 17. */
export const GUIDE_BUTTONS = [16, 17] as const;
/** Analog triggers double as page jumps when they are not remapped. */
export const TRIGGER_BUTTONS = [6, 7] as const;

/**
 * The stored profile is the account/game profile (#51): Big Picture only adds
 * the inputs that profile deliberately leaves unmapped — Start and Guide open
 * the command overlay, LT/RT jump a page. A remapped input always wins, which
 * is what keeps remapping from breaking navigation: the same logical actions
 * (left/right/up/down/confirm/back/menu/downloads/prevTab/nextTab) drive the
 * screen whatever physical button produced them.
 */
export function withBigPictureDefaults(profile: ControllerProfile): ControllerProfile {
  const mappings: Record<string, ControllerAction | string> = { ...profile.mappings };
  const ensure = (input: string, action: ControllerAction) => {
    if (!mappings[input]) mappings[input] = action;
  };
  ensure(`button${START_BUTTON}`, "menu");
  for (const index of GUIDE_BUTTONS) ensure(`button${index}`, "menu");
  ensure(`button${TRIGGER_BUTTONS[0]}`, "prevTab");
  ensure(`button${TRIGGER_BUTTONS[1]}`, "nextTab");
  return { ...profile, mappings };
}

/** True when the press came from a trigger rather than a shoulder. */
export function isPageSource(source: ActionSource | null | undefined): boolean {
  if (!source) return false;
  if (source.kind === "button") return source.index === TRIGGER_BUTTONS[0] || source.index === TRIGGER_BUTTONS[1];
  return source.index === 4 || source.index === 5;
}

/** Start/Guide open the command overlay; X/Square open search. */
export function isMenuSource(source: ActionSource | null | undefined): boolean {
  if (!source || source.kind !== "button") return false;
  return source.index === START_BUTTON || GUIDE_BUTTONS.some((index) => index === source.index);
}

/* ------------------------------ profile choice ----------------------------- */

export const BIG_PICTURE_CONTROLLER_KEY = "ludex.bigPicture.controller";

export interface BigPictureControllerPrefs {
  /** `null` follows the account/game profile, exactly like the library. */
  profileId: string | null;
  /** Optional rumble on focus and confirm (#138). */
  haptics: boolean;
}

export const DEFAULT_CONTROLLER_PREFS: BigPictureControllerPrefs = {
  profileId: null,
  haptics: true,
};

export type ControllerStorage = Pick<Storage, "getItem" | "setItem">;

export function sanitizeControllerPrefs(value: unknown): BigPictureControllerPrefs {
  if (!value || typeof value !== "object") return { ...DEFAULT_CONTROLLER_PREFS };
  const record = value as Record<string, unknown>;
  const profileId = typeof record.profileId === "string" && record.profileId.trim() ? record.profileId : null;
  const haptics = typeof record.haptics === "boolean" ? record.haptics : DEFAULT_CONTROLLER_PREFS.haptics;
  return { profileId, haptics };
}

export function readControllerPrefs(
  storage: ControllerStorage | null | undefined,
  key: string = BIG_PICTURE_CONTROLLER_KEY,
): BigPictureControllerPrefs {
  if (!storage) return { ...DEFAULT_CONTROLLER_PREFS };
  try {
    const raw = storage.getItem(key);
    if (!raw) return { ...DEFAULT_CONTROLLER_PREFS };
    return sanitizeControllerPrefs(JSON.parse(raw));
  } catch {
    // Private mode, a blocked origin or hand-edited JSON: session defaults.
    return { ...DEFAULT_CONTROLLER_PREFS };
  }
}

/** Stores the sanitized value and returns it, so callers keep one source. */
export function writeControllerPrefs(
  storage: ControllerStorage | null | undefined,
  prefs: BigPictureControllerPrefs,
  key: string = BIG_PICTURE_CONTROLLER_KEY,
): BigPictureControllerPrefs {
  const next = sanitizeControllerPrefs(prefs);
  try {
    storage?.setItem(key, JSON.stringify(next));
  } catch {
    /* the preference is session-only when storage is unavailable */
  }
  return next;
}

export const AUTO_PROFILE_ID = "auto";

export interface ProfileOption {
  id: string;
  label: string;
  /** Secondary line: the profile's own name. */
  detail: string;
  kind: "auto" | "account" | "game" | "preset";
  profile: ControllerProfile;
}

/**
 * The profiles a player can switch to without leaving Big Picture: the
 * automatic resolution (account, then per-game override), the saved account
 * profile, every saved per-game override, and the four built-in presets. The
 * presets are listed even before the query lands, so the overlay is usable
 * offline and on a cold start.
 */
export function profileOptions(
  data: ControllerProfilesResponse | undefined,
  fallback: ControllerProfile,
): ProfileOption[] {
  const options: ProfileOption[] = [
    { id: AUTO_PROFILE_ID, label: t("Automatic"), detail: fallback.name, kind: "auto", profile: fallback },
  ];
  if (data?.global) {
    options.push({
      id: "account",
      label: t("Account"),
      detail: data.global.profile.name,
      kind: "account",
      profile: data.global.profile,
    });
  }
  for (const entry of data?.games ?? []) {
    options.push({
      id: `game:${entry.gameId}`,
      label: entry.gameTitle ?? t("This game"),
      detail: entry.profile.name,
      kind: "game",
      profile: entry.profile,
    });
  }
  const presets = data?.presets?.length
    ? data.presets
    : CONTROLLER_PRESETS.map((id) => ({ id, label: presetProfile(id).name, profile: presetProfile(id) }));
  for (const preset of presets) {
    options.push({
      id: `preset:${preset.id}`,
      label: preset.label,
      detail: t("Preset"),
      kind: "preset",
      profile: preset.profile,
    });
  }
  return options;
}

/** The selected option, or the automatic one when the stored id is gone. */
export function resolveProfileChoice(
  options: readonly ProfileOption[],
  profileId: string | null,
  fallback: ControllerProfile,
): ProfileOption {
  return (
    options.find((option) => option.id === profileId) ?? {
      id: AUTO_PROFILE_ID,
      label: t("Automatic"),
      detail: fallback.name,
      kind: "auto",
      profile: fallback,
    }
  );
}

/** One technical line for the profile list: deadzone, curve, triggers, invert. */
export function profileSummary(profile: ControllerProfile): string {
  const parts = [
    `${t("Deadzone")} ${profile.deadzone.toFixed(2)}`,
    `${t("Curve")} ${profile.curve.toFixed(2)}`,
    `${t("Triggers")} ${profile.triggerThreshold.toFixed(2)}`,
  ];
  if (profile.invertX) parts.push(t("Invert X"));
  if (profile.invertY) parts.push(t("Invert Y"));
  return parts.join(" · ");
}
