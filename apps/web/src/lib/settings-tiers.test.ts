// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import en from "../locales/en.json";
import { DEFAULT_DISPLAY_PROFILE } from "./display-profile";
import {
  ADVANCED_TIERS_STORAGE_KEY,
  advancedTiersUnlocked,
  AUDIO_DEVICE_ENV_KEY,
  changedFields,
  DEFAULT_FRAME_LIMIT,
  defaultLaunchOptions,
  readTier2Switch,
  setAdvancedTiersUnlocked,
  TIER2_SWITCHES,
  tier2SwitchesFor,
  tierOneState,
  visibleSettingsTiers,
  withOverlayDisabled,
  withTier2Switch,
  type Tier2SwitchId,
} from "./settings-tiers";
import {
  advancedReason,
  settingsTiersDescription,
  settingsTiersTitle,
  tier2Explanation,
  tier2Intro,
  tier2Label,
  tierDescription,
  tierLabel,
  tierOneAction,
  tierOneDescription,
  tierOneTitle,
} from "@/components/game/settings-tiers-copy";

/**
 * #299 acceptance, pinned as rules:
 *
 * - a first run never sees tier 3;
 * - each tier-2 switch changes exactly the documented profile fields;
 * - every switch has an explanation in the English catalog;
 * - tiers 1 and 2 never leak an exit code, a `PROTON_*` name or a
 *   `winetricks` verb.
 */

const SOURCES = { options: defaultLaunchOptions(), overlayDisabled: false };

afterEach(() => {
  window.localStorage.clear();
});

describe("tier table", () => {
  it("lists the six curated switches in the order the surface reads", () => {
    expect(TIER2_SWITCHES.map((entry) => entry.id)).toEqual([
      "wrapper",
      "frameLimit",
      "hdr",
      "adaptiveSync",
      "overlay",
      "audioDevice",
    ]);
    expect(new Set(TIER2_SWITCHES.map((entry) => entry.id)).size).toBe(6);
  });

  it("documents one storage target and one field per switch", () => {
    const expected: Record<Tier2SwitchId, { storage: string; field: string }> = {
      wrapper: { storage: "launch-options", field: "display.mode" },
      frameLimit: { storage: "launch-options", field: "display.refresh" },
      hdr: { storage: "launch-options", field: "display.hdr" },
      adaptiveSync: { storage: "launch-options", field: "display.adaptiveSync" },
      overlay: { storage: "overlay-settings", field: "overlayDisabledGames" },
      audioDevice: { storage: "launch-options", field: "env.PULSE_SINK" },
    };
    for (const entry of TIER2_SWITCHES) {
      expect(expected[entry.id], entry.id).toEqual({ storage: entry.storage, field: entry.field });
      expect(en, `${entry.id} label`).toHaveProperty(entry.labelKey);
    }
  });

  it("keeps all six on Linux and only the overlay elsewhere", () => {
    expect(tier2SwitchesFor(true)).toHaveLength(6);
    expect(tier2SwitchesFor(false).map((entry) => entry.id)).toEqual(["overlay"]);
  });
});

describe("tier-2 reads", () => {
  it("reads the untouched profile as off, except the overlay", () => {
    expect(readTier2Switch(SOURCES, "wrapper")).toBe(false);
    expect(readTier2Switch(SOURCES, "frameLimit")).toBe(false);
    expect(readTier2Switch(SOURCES, "hdr")).toBe(false);
    expect(readTier2Switch(SOURCES, "adaptiveSync")).toBe(false);
    expect(readTier2Switch(SOURCES, "overlay")).toBe(true);
    expect(readTier2Switch(SOURCES, "audioDevice")).toBe(false);
  });

  it("reads the fields the raw panel writes", () => {
    const options = {
      ...defaultLaunchOptions(),
      env: { [AUDIO_DEVICE_ENV_KEY]: "alsa_output.hdmi" },
      display: { ...DEFAULT_DISPLAY_PROFILE, mode: "gamescope" as const, refresh: 120, hdr: true, adaptiveSync: true },
    };
    for (const id of ["wrapper", "frameLimit", "hdr", "adaptiveSync", "audioDevice"] as const) {
      expect(readTier2Switch({ options, overlayDisabled: false }, id), id).toBe(true);
    }
    expect(readTier2Switch({ options, overlayDisabled: true }, "overlay")).toBe(false);
  });
});

describe("tier-2 writes: exactly one documented change", () => {
  const diff = (before: ReturnType<typeof defaultLaunchOptions>, after: ReturnType<typeof withTier2Switch>) =>
    changedFields(before, after);

  it("wrapper flips display.mode and seeds the resolution once", () => {
    const before = defaultLaunchOptions();
    const on = withTier2Switch(before, "wrapper", true);
    expect(diff(before, on).sort()).toEqual(["display.height", "display.mode", "display.width"]);
    expect(on.display.mode).toBe("gamescope");
    expect(on.display.width).toBeGreaterThan(0);

    const off = withTier2Switch(on, "wrapper", false);
    expect(diff(on, off)).toEqual(["display.mode"]);
    expect(off.display.mode).toBe("desktop");
    // The resolution the user picked survives the round trip (#287 behaviour).
    expect(off.display.width).toBe(on.display.width);
  });

  it("frame limit writes display.refresh only", () => {
    const before = defaultLaunchOptions();
    const on = withTier2Switch(before, "frameLimit", true);
    expect(diff(before, on)).toEqual(["display.refresh"]);
    expect(on.display.refresh).toBe(DEFAULT_FRAME_LIMIT);

    const off = withTier2Switch(on, "frameLimit", false);
    expect(diff(on, off)).toEqual(["display.refresh"]);
    expect(off.display.refresh).toBe(0);
  });

  it("HDR and adaptive sync write their own flag only", () => {
    const before = defaultLaunchOptions();
    const hdr = withTier2Switch(before, "hdr", true);
    expect(diff(before, hdr)).toEqual(["display.hdr"]);
    const vrr = withTier2Switch(before, "adaptiveSync", true);
    expect(diff(before, vrr)).toEqual(["display.adaptiveSync"]);
  });

  it("audio device adds or removes exactly one environment entry", () => {
    const before = { ...defaultLaunchOptions(), env: { DXVK_HUD: "fps" } };
    const on = withTier2Switch(before, "audioDevice", true, "  alsa_output.hdmi  ");
    expect(diff(before, on)).toEqual(["env.PULSE_SINK"]);
    expect(on.env[AUDIO_DEVICE_ENV_KEY]).toBe("alsa_output.hdmi");
    expect(on.env.DXVK_HUD).toBe("fps");

    const off = withTier2Switch(on, "audioDevice", false);
    expect(diff(on, off)).toEqual(["env.PULSE_SINK"]);
    expect(off.env).toEqual({ DXVK_HUD: "fps" });
  });

  it("an empty device name never writes a blank sink", () => {
    const before = defaultLaunchOptions();
    const next = withTier2Switch(before, "audioDevice", true, "   ");
    expect(diff(before, next)).toEqual([]);
  });

  it("overlay opt-out touches one membership, sorted", () => {
    const before = [2, 9];
    const disabled = withOverlayDisabled(before, 5, true);
    expect(disabled).toEqual([2, 5, 9]);
    expect(disabled).toHaveLength(before.length + 1);
    expect(withOverlayDisabled(disabled, 5, false)).toEqual([2, 9]);
    expect(withOverlayDisabled(before, 9, true)).toEqual([2, 9]);
  });

  it("diffs nested fields as leaf paths, not as blobs", () => {
    expect(changedFields({ a: { b: 1, c: 2 } }, { a: { b: 1, c: 3 } })).toEqual(["a.c"]);
    expect(changedFields({ list: [1, 2] }, { list: [2, 1] })).toEqual(["list"]);
    expect(changedFields(defaultLaunchOptions(), defaultLaunchOptions())).toEqual([]);
  });
});

describe("tier-2 explanations", () => {
  const states = [
    { enabled: true, blockedReason: null, frameLimit: 60, device: "Headset" },
    { enabled: false, blockedReason: null, frameLimit: 60, device: "Headset" },
  ];

  it("gives every switch a sentence in both states", () => {
    for (const entry of TIER2_SWITCHES) {
      for (const state of states) {
        const text = tier2Explanation(entry.id, state);
        expect(text.trim().length, `${entry.id} (${state.enabled})`).toBeGreaterThan(20);
      }
    }
  });

  it("uses the caller's reason verbatim when a switch is blocked", () => {
    expect(
      tier2Explanation("hdr", {
        enabled: false,
        blockedReason: "This display reports no HDR support.",
        frameLimit: 0,
        device: "",
      }),
    ).toBe("This display reports no HDR support.");
  });

  it("names the value the switch owns", () => {
    expect(tier2Explanation("audioDevice", states[0])).toContain("Headset");
    expect(tier2Explanation("frameLimit", states[0])).toContain("60");
  });

  it("keeps tiers 1 and 2 free of raw vocabulary", () => {
    const forbidden = [/exit\s+(code|status)/i, /\bPROTON_[A-Z0-9_]+/, /winetricks/i];
    const copy: string[] = [];
    for (const id of ["works", "game", "advanced"] as const) {
      copy.push(tierLabel(id), tierDescription(id));
    }
    copy.push(settingsTiersTitle(), settingsTiersDescription(), tier2Intro());
    copy.push(
      advancedReason("noWrapper"),
      advancedReason("hdrBuild"),
      advancedReason("vrrBuild"),
      advancedReason("hdrOutput"),
      advancedReason("vrrOutput"),
    );
    for (const entry of TIER2_SWITCHES) {
      copy.push(tier2Label(entry.id));
      for (const state of states) copy.push(tier2Explanation(entry.id, state));
    }
    for (const status of ["checking", "ready", "needsFix"] as const) {
      copy.push(tierOneTitle(status), tierOneAction(status, true), tierOneAction(status, false));
    }
    for (const cause of ["runtime", "prefix", "vulkan", "disk", "arch", "mode", "manual", "generic"] as const) {
      copy.push(tierOneDescription("needsFix", cause));
    }
    for (const text of copy) {
      for (const pattern of forbidden) {
        expect(pattern.test(text), `${pattern} in ${JSON.stringify(text)}`).toBe(false);
      }
    }
  });
});

describe("tier 1 verdict", () => {
  it("is computed from the runtime diagnosis, not from a switch", () => {
    expect(tierOneState({ ready: null, platform: "linux", windowsGame: true, mode: "auto", issues: [] }).status).toBe(
      "checking",
    );
    expect(tierOneState({ ready: true, platform: "linux", windowsGame: true, mode: "auto", issues: [] }).status).toBe(
      "ready",
    );
  });

  it("maps the blocking issue to a human cause", () => {
    const cause = (code: string) =>
      tierOneState({
        ready: false,
        platform: "linux",
        windowsGame: true,
        mode: "auto",
        issues: [{ code, severity: "blocking" }],
      }).cause;
    expect(cause("missing-umu")).toBe("runtime");
    expect(cause("missing-proton")).toBe("runtime");
    expect(cause("prefix-unsafe")).toBe("prefix");
    expect(cause("no-vulkan")).toBe("vulkan");
    expect(cause("disk-space")).toBe("disk");
    expect(cause("unsupported-arch")).toBe("arch");
    expect(cause("something-new")).toBe("manual");
  });

  it("falls back to the launch mode for a Windows game told to run natively", () => {
    expect(
      tierOneState({ ready: false, platform: "linux", windowsGame: true, mode: "native", issues: [] }).cause,
    ).toBe("mode");
    expect(tierOneState({ ready: false, platform: "linux", windowsGame: true, mode: "auto", issues: [] }).cause).toBe(
      "generic",
    );
  });
});

describe("advanced unlock", () => {
  it("is locked on a first run", () => {
    expect(advancedTiersUnlocked()).toBe(false);
    expect(visibleSettingsTiers()).toEqual(["works", "game"]);
  });

  it("persists across a reload and hides tier 3 again when locked", () => {
    setAdvancedTiersUnlocked(true);
    expect(window.localStorage.getItem(ADVANCED_TIERS_STORAGE_KEY)).toBe("1");
    expect(advancedTiersUnlocked()).toBe(true);
    expect(visibleSettingsTiers()).toEqual(["works", "game", "advanced"]);

    setAdvancedTiersUnlocked(false);
    expect(window.localStorage.getItem(ADVANCED_TIERS_STORAGE_KEY)).toBeNull();
    expect(visibleSettingsTiers()).toEqual(["works", "game"]);
  });
});
