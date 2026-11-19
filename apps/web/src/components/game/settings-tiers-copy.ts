import { t } from "@/lib/i18n";
import type {
  SettingsTierId,
  Tier2SwitchId,
  TierOneCause,
  TierOneStatus,
} from "@/lib/settings-tiers";

/**
 * Translatable copy for the three tiers (#299), for the same reason as the
 * settings taxonomy: literal `t()` calls the extractor and the compiler can
 * check, resolved at render time so a language change applies immediately.
 *
 * Copy discipline is part of the feature: tiers 1 and 2 never show an exit
 * code, a `PROTON_*` variable name or a `winetricks` verb. A cause is always a
 * human sentence ("the compatibility layer is missing"), and the raw words stay
 * in tier 3 where the user asked for them. `settings-tiers.test.ts` enforces
 * that rule against every string below.
 */

export function tierLabel(id: SettingsTierId): string {
  switch (id) {
    case "works":
      return t("Just works");
    case "game":
      return t("Per game");
    case "advanced":
      return t("Advanced");
  }
}

export function tierDescription(id: SettingsTierId): string {
  switch (id) {
    case "works":
      return t("Ludex checks this game and fixes what it can. Nothing to configure.");
    case "game":
      return t("A few useful switches. Each one changes exactly one value in this game's profile.");
    case "advanced":
      return t("The raw launch settings. Unsupported: a hand-edited launch can stop the game from starting.");
  }
}

/* --------------------------------- tier 1 -------------------------------- */

export function tierOneTitle(status: TierOneStatus): string {
  switch (status) {
    case "checking":
      return t("Checking this game");
    case "ready":
      return t("Ready to play");
    case "needsFix":
      return t("Needs a fix");
  }
}

export function tierOneCause(cause: TierOneCause): string {
  switch (cause) {
    case "runtime":
      return t("The compatibility layer this game needs is not installed yet.");
    case "prefix":
      return t("This game's Windows folder is not ready yet.");
    case "vulkan":
      return t("This machine has no graphics driver that Windows games can use.");
    case "disk":
      return t("There is not enough free space for the compatibility layer.");
    case "arch":
      return t("This machine cannot run Windows games.");
    case "mode":
      return t("This game is set to start without the compatibility layer it needs.");
    case "manual":
      return t("Something on this machine needs your attention first.");
    case "generic":
      return t("Ludex found something that needs attention before this game can start.");
  }
}

export function tierOneDescription(status: TierOneStatus, cause: TierOneCause | null): string {
  switch (status) {
    case "checking":
      return t("Ludex is checking what this game needs.");
    case "ready":
      return t("Everything this game needs is installed. Nothing else to decide.");
    case "needsFix":
      return cause ? tierOneCause(cause) : t("Ludex found something that needs attention before this game can start.");
  }
}

export function tierOneAction(status: TierOneStatus, canApply: boolean): string {
  if (status === "checking") return t("Checking…");
  if (status === "ready") return t("Start game");
  return canApply ? t("Fix this game") : t("Show me what to do");
}

export function tier2Intro(): string {
  return t("Each switch changes one value in this game's profile, so every change is visible and reversible.");
}

/* --------------------------------- tier 2 -------------------------------- */

export function tier2Label(id: Tier2SwitchId): string {
  switch (id) {
    case "wrapper":
      return t("Game window");
    case "frameLimit":
      return t("Frame limit");
    case "hdr":
      return t("HDR");
    case "adaptiveSync":
      return t("Adaptive sync");
    case "overlay":
      return t("In-game overlay");
    case "audioDevice":
      return t("Audio device");
  }
}

export interface Tier2ExplanationState {
  enabled: boolean;
  /**
   * Why the switch cannot move, already translated by the caller (capability
   * probe, missing wrapper, other platform). It replaces the sentence.
   */
  blockedReason: string | null;
  /** The current cap, for the frame-limit sentence. */
  frameLimit: number;
  /** The sink name, for the audio sentence. */
  device: string;
}

export function tier2Explanation(id: Tier2SwitchId, state: Tier2ExplanationState): string {
  if (state.blockedReason) return state.blockedReason;
  switch (id) {
    case "wrapper":
      return state.enabled
        ? t("Runs the game in its own window at your resolution, so your desktop stays exactly as it is.")
        : t("Runs on your desktop, untouched.");
    case "frameLimit":
      return state.enabled
        ? t("Caps the game at {fps} fps inside the game window.", { fps: state.frameLimit })
        : t("No cap: the game follows the display's refresh rate.");
    case "hdr":
      return state.enabled
        ? t("Sends HDR to this display, which reports that it can show it.")
        : t("Standard colour; no HDR is sent.");
    case "adaptiveSync":
      return state.enabled
        ? t("The display follows the game's frame rate, which removes tearing.")
        : t("The display keeps its own refresh rate.");
    case "overlay":
      return state.enabled
        ? t("Shift+Tab and the Home button open Ludex over the game.")
        : t("The overlay stays closed for this game.");
    case "audioDevice":
      return state.enabled
        ? t("Sound goes to {device} instead of the system default.", { device: state.device })
        : t("Sound follows the system default device.");
  }
}

/* -------------------------- tier-2 blocked reasons ----------------------- */

export function advancedReason(code: "noWrapper" | "hdrBuild" | "vrrBuild" | "hdrOutput" | "vrrOutput"): string {
  switch (code) {
    case "noWrapper":
      return t("No game window tool is installed on this machine yet.");
    case "hdrBuild":
      return t("The installed game window tool cannot send HDR.");
    case "vrrBuild":
      return t("The installed game window tool cannot follow the game's frame rate.");
    case "hdrOutput":
      return t("This display reports no HDR support.");
    case "vrrOutput":
      return t("This display cannot follow the game's frame rate.");
  }
}

export function settingsTiersTitle(): string {
  return t("Launch setup");
}

export function settingsTiersDescription(): string {
  return t("Ludex keeps this game ready. The switches below are the only ones that matter; the raw settings stay behind the Advanced lock.");
}

export function advancedWarning(): string {
  return t("Raw environment variables, Wine overrides, the prefix path and the Proton build. Unsupported: a hand-edited launch can stop the game from starting, and support cannot reproduce it. Change one value at a time.");
}
