import { describe, expect, it } from "vitest";
import { presetProfile } from "@/lib/controller-profiles";
import { readPadPresses, type PadLike } from "@/lib/gamepad-core";
import type { ControllerProfilesResponse } from "@/lib/types";
import {
  AUTO_PROFILE_ID,
  BIG_PICTURE_CONTROLLER_KEY,
  DEFAULT_CONTROLLER_PREFS,
  GUIDE_BUTTONS,
  cycleHapticsStrength,
  hapticsActive,
  hintsFor,
  isMenuSource,
  isPageSource,
  padFamily,
  padFamilyLabel,
  profileOptions,
  profileSummary,
  readControllerPrefs,
  resolveProfileChoice,
  sanitizeControllerPrefs,
  withBigPictureDefaults,
  writeControllerPrefs,
  type ControllerStorage,
} from "./controller";
import { hintContextFor } from "./input";
import { BIG_PICTURE_LAYOUTS } from "./layout";

/* --------------------------------------------------------------------------
   #138: pad families, contextual hints, the extra Big Picture mappings and the
   device-local profile choice. Pure functions, so the screen can trust them
   without a controller attached.
   -------------------------------------------------------------------------- */

function memoryStorage(entries: Record<string, string> = {}): ControllerStorage & {
  entries: Map<string, string>;
} {
  const map = new Map(Object.entries(entries));
  return {
    entries: map,
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => {
      map.set(key, value);
    },
  };
}

function profilesResponse(): ControllerProfilesResponse {
  const game = presetProfile("steamdeck");
  const global = presetProfile("dualsense");
  return {
    global: { gameId: null, gameTitle: null, profile: global, updatedAt: "2026-11-01T00:00:00Z" },
    games: [{ gameId: 7, gameTitle: "Hades", profile: game, updatedAt: "2026-11-02T00:00:00Z" }],
    presets: [
      { id: "xbox", label: "Xbox", profile: presetProfile("xbox") },
      { id: "dualsense", label: "DualSense", profile: presetProfile("dualsense") },
      { id: "steamdeck", label: "Steam Deck", profile: presetProfile("steamdeck") },
      { id: "generic", label: "Generic", profile: presetProfile("generic") },
    ],
  };
}

describe("pad family detection", () => {
  it("recognises the pads the target platforms report", () => {
    expect(padFamily("Xbox Wireless Controller")).toBe("xbox");
    expect(padFamily("Xbox 360 Controller (XInput STANDARD GAMEPAD)")).toBe("xbox");
    expect(padFamily("DualSense Wireless Controller")).toBe("dualsense");
    expect(padFamily("Sony PLAYSTATION(R)3 Controller")).toBe("dualsense");
    expect(padFamily("Steam Deck Controller")).toBe("steamdeck");
    expect(padFamily("steamdeck")).toBe("steamdeck");
  });

  it("falls back to the profile name, then to generic", () => {
    expect(padFamily("USB Gamepad", { name: "DualSense" })).toBe("dualsense");
    expect(padFamily("USB Gamepad", { name: "Steam Deck" })).toBe("steamdeck");
    expect(padFamily(null, { name: "Xbox" })).toBe("xbox");
    expect(padFamily("USB Gamepad", { name: "My profile" })).toBe("generic");
    expect(padFamily(undefined, null)).toBe("generic");
  });

  it("labels every family", () => {
    expect(padFamilyLabel("xbox")).toBe("Xbox");
    expect(padFamilyLabel("dualsense")).toBe("DualSense");
    expect(padFamilyLabel("steamdeck")).toBe("Steam Deck");
    expect(padFamilyLabel("generic")).toBe("Generic");
  });
});

describe("contextual button hints", () => {
  const base = { layout: "shelf" as const, family: "xbox" as const, connected: true };

  it("names Xbox buttons in the library and keeps every action advertised", () => {
    const hints = hintsFor("library", base);
    const ids = hints.map((hint) => hint.id);
    expect(ids).toEqual(["select", "shelf", "play", "search", "downloads", "menu", "exit"]);
    expect(hints.find((hint) => hint.id === "play")?.keys).toEqual(["A"]);
    expect(hints.find((hint) => hint.id === "search")?.keys).toEqual(["X"]);
    expect(hints.find((hint) => hint.id === "menu")?.keys).toEqual(["Menu", "Guide"]);
  });

  it("renames the same actions on a DualSense", () => {
    const hints = hintsFor("library", { ...base, family: "dualsense" });
    expect(hints.find((hint) => hint.id === "play")?.keys).toEqual(["Cross"]);
    expect(hints.find((hint) => hint.id === "search")?.keys).toEqual(["Square"]);
    expect(hints.find((hint) => hint.id === "shelf")?.keys).toEqual(["L1", "R1"]);
    expect(hints.find((hint) => hint.id === "menu")?.keys).toEqual(["Options", "PS"]);
  });

  it("falls back to keyboard labels without a pad", () => {
    const hints = hintsFor("library", { ...base, connected: false });
    expect(hints.find((hint) => hint.id === "play")?.keys).toEqual(["↵"]);
    expect(hints.find((hint) => hint.id === "search")?.keys).toEqual(["/"]);
    expect(hints.find((hint) => hint.id === "menu")?.keys).toEqual(["M"]);
    expect(hints.find((hint) => hint.id === "exit")?.keys).toEqual(["esc"]);
  });

  it("keeps the keyboard hint honest per layout", () => {
    const rail = hintsFor("library", { ...base, connected: false, layout: "shelf" });
    const grid = hintsFor("library", { ...base, connected: false, layout: "grid" });
    const dock = hintsFor("library", { ...base, connected: false, layout: "dock" });
    const spotlight = hintsFor("library", { ...base, connected: false, layout: "spotlight" });
    expect(rail.find((hint) => hint.id === "select")?.keys).toEqual(["←", "→"]);
    expect(grid.find((hint) => hint.id === "select")?.keys).toEqual(["←", "→", "↑", "↓"]);
    expect(dock.find((hint) => hint.id === "select")?.keys).toEqual(["←", "→"]);
    expect(spotlight.find((hint) => hint.id === "select")?.keys).toEqual(["←", "→"]);
    for (const layout of BIG_PICTURE_LAYOUTS) {
      expect(hintsFor("library", { ...base, layout }).length).toBe(7);
    }
  });

  it("advertises the overlay contexts, not the library", () => {
    const menu = hintsFor("menu", base).map((hint) => hint.id);
    expect(menu).toEqual(["choose", "open", "close"]);
    expect(hintsFor("exit", base).map((hint) => hint.id)).toEqual(["choose", "select", "close"]);
    expect(hintsFor("profiles", base).map((hint) => hint.id)).toEqual(["choose", "apply", "close"]);
    expect(hintsFor("downloads", base).map((hint) => hint.id)).toEqual(["choose", "open", "close"]);
    expect(hintsFor("picker", base).map((hint) => hint.id)).toEqual(["choose", "apply", "close"]);
    expect(hintsFor("actions", base).map((hint) => hint.id)).toEqual(["navigate", "select", "back"]);
  });

  it("teaches the on-screen keyboard with X, Y, LB and RB", () => {
    const hints = hintsFor("search", base);
    const ids = hints.map((hint) => hint.id);
    expect(ids).toEqual(["navigate", "type", "delete", "space", "clear", "open", "back"]);
    expect(hints.find((hint) => hint.id === "delete")?.keys).toEqual(["X"]);
    expect(hints.find((hint) => hint.id === "space")?.keys).toEqual(["Y"]);
    expect(hints.find((hint) => hint.id === "clear")?.keys).toEqual(["LB"]);
    expect(hints.find((hint) => hint.id === "open")?.keys).toEqual(["RB"]);
  });

  it("honours the physical keyboard inside search", () => {
    const hints = hintsFor("search", { ...base, connected: false });
    expect(hints.map((hint) => hint.id)).toEqual(["navigate", "open", "delete", "space", "back"]);
    expect(hints.find((hint) => hint.id === "open")?.keys).toEqual(["↵"]);
    expect(hints.find((hint) => hint.id === "delete")?.keys).toEqual(["⌫"]);
    expect(hints.find((hint) => hint.id === "space")?.keys).toEqual(["Space"]);
    // No dead hint for a binding that does not exist on a keyboard.
    expect(hints.find((hint) => hint.id === "clear")).toBeUndefined();
    expect(hints.find((hint) => hint.id === "type")).toBeUndefined();
  });

  it("maps an overlay to its hint context and back to the library", () => {
    expect(hintContextFor(null)).toBe("library");
    expect(hintContextFor("search")).toBe("search");
    expect(hintContextFor("exit")).toBe("exit");
  });
});

describe("Big Picture defaults on top of a profile", () => {
  it("maps Start and Guide to the menu when the profile leaves them free", () => {
    const profile = withBigPictureDefaults(presetProfile("xbox"));
    expect(profile.mappings.button9).toBe("menu");
    for (const index of GUIDE_BUTTONS) expect(profile.mappings[`button${index}`]).toBe("menu");
    expect(profile.mappings.button6).toBe("prevTab");
    expect(profile.mappings.button7).toBe("nextTab");
  });

  it("never overrides a remapped input", () => {
    const custom = {
      ...presetProfile("xbox"),
      mappings: { ...presetProfile("xbox").mappings, button9: "downloads" as const, button6: "confirm" as const },
    };
    const profile = withBigPictureDefaults(custom);
    expect(profile.mappings.button9).toBe("downloads");
    expect(profile.mappings.button6).toBe("confirm");
  });

  it("does not mutate the stored profile", () => {
    const source = presetProfile("xbox");
    const before = JSON.stringify(source.mappings);
    withBigPictureDefaults(source);
    expect(JSON.stringify(source.mappings)).toBe(before);
    expect(source.mappings.button9).toBeUndefined();
  });

  it("keeps the preset's own Steam Deck menu mapping", () => {
    const profile = withBigPictureDefaults(presetProfile("steamdeck"));
    expect(profile.mappings.button16).toBe("menu");
    expect(profile.mappings.button17).toBe("downloads");
  });

  it("applies a remapped profile to Big Picture navigation", () => {
    const remapped = {
      ...presetProfile("xbox"),
      mappings: {
        button0: "left" as const,
        button1: "confirm" as const,
        button2: "back" as const,
        button12: "up" as const,
        button13: "down" as const,
      },
    };
    const profile = withBigPictureDefaults(remapped);
    const padWith = (index: number): PadLike => ({
      index: 0,
      id: "Test pad",
      connected: true,
      mapping: "standard",
      timestamp: 0,
      axes: [0, 0, 0, 0],
      buttons: Array.from({ length: 18 }, (_, at) => ({ pressed: at === index, value: at === index ? 1 : 0 })),
    });
    expect(readPadPresses(profile, padWith(0)).map((press) => press.action)).toContain("left");
    expect(readPadPresses(profile, padWith(1)).map((press) => press.action)).toContain("confirm");
    expect(readPadPresses(profile, padWith(2)).map((press) => press.action)).toContain("back");
    // The added Start/Guide and trigger inputs survive the remap.
    expect(readPadPresses(profile, padWith(9)).map((press) => press.action)).toContain("menu");
    expect(readPadPresses(profile, padWith(7)).map((press) => press.action)).toContain("nextTab");
  });
});

describe("physical source discrimination", () => {
  it("separates triggers from shoulders", () => {
    expect(isPageSource({ kind: "button", index: 6 })).toBe(true);
    expect(isPageSource({ kind: "button", index: 7 })).toBe(true);
    expect(isPageSource({ kind: "button", index: 4 })).toBe(false);
    expect(isPageSource({ kind: "button", index: 5 })).toBe(false);
    expect(isPageSource({ kind: "axis", index: 4, direction: 1 })).toBe(true);
    expect(isPageSource({ kind: "axis", index: 0, direction: 1 })).toBe(false);
    expect(isPageSource(null)).toBe(false);
  });

  it("separates Start/Guide from X", () => {
    expect(isMenuSource({ kind: "button", index: 9 })).toBe(true);
    expect(isMenuSource({ kind: "button", index: 16 })).toBe(true);
    expect(isMenuSource({ kind: "button", index: 17 })).toBe(true);
    expect(isMenuSource({ kind: "button", index: 2 })).toBe(false);
    expect(isMenuSource({ kind: "axis", index: 1, direction: -1 })).toBe(false);
    expect(isMenuSource(null)).toBe(false);
  });
});

describe("Big Picture controller preferences", () => {
  it("defaults to the automatic profile, haptics on and sound at 45 %", () => {
    expect(DEFAULT_CONTROLLER_PREFS).toEqual({
      profileId: null,
      haptics: true,
      hapticsStrength: "standard",
      sound: { enabled: true, volume: 0.45 },
    });
    expect(readControllerPrefs(null)).toEqual(DEFAULT_CONTROLLER_PREFS);
    expect(readControllerPrefs(undefined)).toEqual(DEFAULT_CONTROLLER_PREFS);
  });

  it("round-trips through storage and honours a scoped key", () => {
    const storage = memoryStorage();
    const prefs = {
      profileId: "preset:dualsense",
      haptics: false,
      hapticsStrength: "strong" as const,
      sound: { enabled: true, volume: 0.7 },
    };
    const written = writeControllerPrefs(storage, prefs);
    expect(written).toEqual(prefs);
    expect(JSON.parse(storage.entries.get(BIG_PICTURE_CONTROLLER_KEY) ?? "null")).toEqual(prefs);
    expect(readControllerPrefs(storage)).toEqual(prefs);
    expect(readControllerPrefs(storage, "missing")).toEqual(DEFAULT_CONTROLLER_PREFS);
  });

  it("keeps a stored record from before the sound update working without a migration", () => {
    const storage = memoryStorage({
      [BIG_PICTURE_CONTROLLER_KEY]: JSON.stringify({ profileId: "account", haptics: false }),
    });
    expect(readControllerPrefs(storage)).toEqual({
      profileId: "account",
      haptics: false,
      hapticsStrength: "standard",
      sound: { enabled: true, volume: 0.45 },
    });
  });

  it("sanitises hand-edited JSON field by field", () => {
    expect(sanitizeControllerPrefs(null)).toEqual(DEFAULT_CONTROLLER_PREFS);
    expect(sanitizeControllerPrefs("nope")).toEqual(DEFAULT_CONTROLLER_PREFS);
    expect(sanitizeControllerPrefs({ profileId: "  ", haptics: "yes" })).toEqual(DEFAULT_CONTROLLER_PREFS);
    expect(sanitizeControllerPrefs({ profileId: "account", haptics: false })).toEqual({
      ...DEFAULT_CONTROLLER_PREFS,
      profileId: "account",
      haptics: false,
    });
    expect(
      sanitizeControllerPrefs({ hapticsStrength: "loud", sound: { enabled: "yes", volume: 4 } }),
    ).toEqual({
      ...DEFAULT_CONTROLLER_PREFS,
      hapticsStrength: "standard",
      sound: { enabled: true, volume: 1 },
    });
    expect(sanitizeControllerPrefs({ hapticsStrength: "off", sound: { volume: -2 } })).toEqual({
      ...DEFAULT_CONTROLLER_PREFS,
      hapticsStrength: "off",
      sound: { enabled: true, volume: 0 },
    });
  });

  it("never shares the default sound object between reads", () => {
    const first = sanitizeControllerPrefs(null);
    first.sound.volume = 0.1;
    expect(sanitizeControllerPrefs(null).sound.volume).toBe(0.45);
    expect(DEFAULT_CONTROLLER_PREFS.sound.volume).toBe(0.45);
  });

  it("gates haptics on the toggle, the strength and the profile", () => {
    expect(hapticsActive({ haptics: true, hapticsStrength: "standard" }, { vibration: true })).toBe(true);
    expect(hapticsActive({ haptics: true, hapticsStrength: "strong" }, { vibration: true })).toBe(true);
    expect(hapticsActive({ haptics: true, hapticsStrength: "off" }, { vibration: true })).toBe(false);
    expect(hapticsActive({ haptics: false, hapticsStrength: "standard" }, { vibration: true })).toBe(false);
    expect(hapticsActive({ haptics: true, hapticsStrength: "standard" }, { vibration: false })).toBe(false);
  });

  it("cycles Off → Standard → Strong in both directions", () => {
    expect(cycleHapticsStrength("off", 1)).toBe("standard");
    expect(cycleHapticsStrength("standard", 1)).toBe("strong");
    expect(cycleHapticsStrength("strong", 1)).toBe("off");
    expect(cycleHapticsStrength("off", -1)).toBe("strong");
    expect(cycleHapticsStrength("standard", -1)).toBe("off");
  });

  it("survives blocked storage and broken JSON", () => {
    const blocked: ControllerStorage = {
      getItem: () => {
        throw new Error("SecurityError");
      },
      setItem: () => {
        throw new Error("QuotaExceededError");
      },
    };
    expect(readControllerPrefs(blocked)).toEqual(DEFAULT_CONTROLLER_PREFS);
    expect(
      writeControllerPrefs(blocked, {
        profileId: "account",
        haptics: true,
        hapticsStrength: "standard",
        sound: { enabled: true, volume: 0.45 },
      }),
    ).toEqual({ ...DEFAULT_CONTROLLER_PREFS, profileId: "account" });
    expect(readControllerPrefs(memoryStorage({ [BIG_PICTURE_CONTROLLER_KEY]: "{" }))).toEqual(
      DEFAULT_CONTROLLER_PREFS,
    );
  });
});

describe("profile choices", () => {
  const fallback = presetProfile("xbox");

  it("lists the presets even before the query lands", () => {
    const options = profileOptions(undefined, fallback);
    expect(options.map((option) => option.id)).toEqual([
      AUTO_PROFILE_ID,
      "preset:xbox",
      "preset:dualsense",
      "preset:steamdeck",
      "preset:generic",
    ]);
    expect(options[0].profile).toBe(fallback);
  });

  it("lists the automatic choice, the account, per-game overrides and presets", () => {
    const options = profileOptions(profilesResponse(), fallback);
    expect(options.map((option) => option.id)).toEqual([
      AUTO_PROFILE_ID,
      "account",
      "game:7",
      "preset:xbox",
      "preset:dualsense",
      "preset:steamdeck",
      "preset:generic",
    ]);
    expect(options[2].label).toBe("Hades");
    expect(options[2].detail).toBe("Steam Deck");
  });

  it("resolves a stored choice and falls back to automatic", () => {
    const options = profileOptions(profilesResponse(), fallback);
    expect(resolveProfileChoice(options, "preset:dualsense", fallback).profile.name).toBe("DualSense");
    expect(resolveProfileChoice(options, "game:7", fallback).profile.name).toBe("Steam Deck");
    expect(resolveProfileChoice(options, "gone", fallback).id).toBe(AUTO_PROFILE_ID);
    expect(resolveProfileChoice(options, null, fallback).id).toBe(AUTO_PROFILE_ID);
  });

  it("summarises the values the profile applies", () => {
    const summary = profileSummary({ ...presetProfile("xbox"), deadzone: 0.25, curve: 1.4, triggerThreshold: 0.4 });
    expect(summary).toContain("0.25");
    expect(summary).toContain("1.40");
    expect(summary).toContain("0.40");
    expect(summary).not.toContain("Invert");
    const inverted = profileSummary({ ...presetProfile("xbox"), invertX: true, invertY: true });
    expect(inverted).toContain("Invert X");
    expect(inverted).toContain("Invert Y");
  });
});
