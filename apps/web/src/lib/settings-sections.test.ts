import { describe, expect, it } from "vitest";
import en from "../locales/en.json";
import {
  availableSettingsSections,
  DEFAULT_SETTINGS_SECTION,
  filterSettingsSections,
  isSettingsSectionAvailable,
  isSettingsSectionId,
  LEGACY_SETTINGS_ANCHORS,
  resolveSettingsSection,
  SETTINGS_SECTIONS,
  settingsSectionById,
  settingsSectionPath,
  scoreSettingsSection,
  type SettingsSectionId,
} from "./settings-sections";
import { settingsSectionDescription, settingsSectionLabel } from "@/components/settings/settings-copy";

/** Every control group the old one-page settings exposed, once. */
const REQUIRED_PANELS = [
  "identity",
  "password",
  "public-profile",
  "theme",
  "accent",
  "tint",
  "theme-file",
  "bigpicture-layout",
  "window-chrome",
  "locale",
  "install-locations",
  "installed-games",
  "gamevault",
  "stream-install",
  "keep-archives",
  "bandwidth",
  "archive-passwords",
  "installer-args",
  "installer-timeout",
  "cloud-auto",
  "cloud-device",
  "save-database",
  "save-locations",
  "sunshine",
  "moonlight",
  "steam-proton",
  "launcher-import",
  "steam-folder",
  "discord-presence",
  "servers",
  "controllers",
  "emulators",
  "update-channel",
  "update-startup",
  "update-panel",
  "onboarding",
  "getting-started",
  "shortcuts",
  "about",
];

describe("settings taxonomy", () => {
  it("orders the categories the way the screen presents them", () => {
    expect(SETTINGS_SECTIONS.map((section) => section.id)).toEqual([
      "account",
      "appearance",
      "language",
      "storage",
      "downloads",
      "saves",
      "streaming",
      "compatibility",
      "integrations",
      "profiles",
      "updates",
      "help",
    ]);
  });

  it("gives every category a translated label and description", () => {
    for (const section of SETTINGS_SECTIONS) {
      expect(en, `${section.id} label`).toHaveProperty(section.labelKey);
      expect(en, `${section.id} description`).toHaveProperty(section.descriptionKey);
      expect(section.descriptionKey.trim().length).toBeGreaterThan(20);
    }
  });

  it("resolves labels and descriptions through the translatable copy table", () => {
    for (const section of SETTINGS_SECTIONS) {
      expect(settingsSectionLabel(section.id)).toBe(section.labelKey);
      expect(settingsSectionDescription(section.id)).toBe(section.descriptionKey);
    }
  });

  it("owns every panel exactly once", () => {
    const owned = SETTINGS_SECTIONS.flatMap((section) => section.panels);
    expect(new Set(owned).size).toBe(owned.length);
    expect([...owned].sort()).toEqual([...REQUIRED_PANELS].sort());
  });

  it("keeps section ids unique and addressable", () => {
    const ids = SETTINGS_SECTIONS.map((section) => section.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) {
      expect(isSettingsSectionId(id)).toBe(true);
      expect(settingsSectionById(id)?.id).toBe(id);
      expect(settingsSectionPath(id)).toBe(`/settings?section=${id}`);
    }
  });
});

describe("settings deep links", () => {
  it("reads the section from the query string", () => {
    expect(resolveSettingsSection("downloads")).toBe("downloads");
    expect(resolveSettingsSection("saves")).toBe("saves");
    expect(resolveSettingsSection(" remote ")).toBe(DEFAULT_SETTINGS_SECTION);
  });

  it("falls back to Account for an unknown or missing section", () => {
    expect(resolveSettingsSection(null)).toBe("account");
    expect(resolveSettingsSection("nope")).toBe("account");
    expect(resolveSettingsSection("")).toBe("account");
  });

  it("keeps old in-page anchors working", () => {
    expect(resolveSettingsSection(null, "#help")).toBe("help");
    expect(resolveSettingsSection(null, "#installed-heading")).toBe("storage");
    expect(resolveSettingsSection(null, "#community-profile-heading")).toBe("account");
    expect(resolveSettingsSection(null, "#compat-heading")).toBe("compatibility");
    expect(resolveSettingsSection(null, "#section-not-known")).toBe("account");
    for (const [anchor, section] of Object.entries(LEGACY_SETTINGS_ANCHORS)) {
      expect(resolveSettingsSection(null, `#${anchor}`), anchor).toBe(section);
    }
  });

  it("prefers ?section= over the hash", () => {
    expect(resolveSettingsSection("appearance", "#help")).toBe("appearance");
  });

  it("round-trips every section through its path", () => {
    for (const section of SETTINGS_SECTIONS) {
      const path = settingsSectionPath(section.id);
      const param = new URLSearchParams(path.split("?")[1]).get("section");
      expect(resolveSettingsSection(param)).toBe(section.id);
    }
  });
});

describe("settings availability", () => {
  it("hides Streaming and Compatibility outside the desktop app", () => {
    expect(availableSettingsSections({ desktop: false, native: false }).map((section) => section.id)).toEqual([
      "account",
      "appearance",
      "language",
      "storage",
      "downloads",
      "saves",
      "integrations",
      "profiles",
      "updates",
      "help",
    ]);
  });

  it("shows every category in the desktop app", () => {
    expect(availableSettingsSections({ desktop: true, native: true })).toHaveLength(SETTINGS_SECTIONS.length);
  });

  it("declares availability per section", () => {
    expect(isSettingsSectionAvailable(settingsSectionById("streaming")!, { desktop: true, native: false })).toBe(false);
    expect(isSettingsSectionAvailable(settingsSectionById("streaming")!, { desktop: true, native: true })).toBe(true);
    expect(isSettingsSectionAvailable(settingsSectionById("storage")!, { desktop: false, native: false })).toBe(true);
  });
});

describe("settings search", () => {
  const ids = (query: string) => filterSettingsSections(query).map((section) => section.id);
  const labelFor = (section: { labelKey: string }) => section.labelKey;

  it("returns everything for an empty query", () => {
    expect(filterSettingsSections("   ")).toHaveLength(SETTINGS_SECTIONS.length);
  });

  it("finds a section by its label", () => {
    expect(ids("appearance")).toContain("appearance");
    expect(ids("downloads")).toContain("downloads");
    expect(ids("language")).toContain("language");
  });

  it("finds a section by a keyword a user would type", () => {
    expect(ids("theme")).toContain("appearance");
    expect(ids("password")).toContain("account");
    expect(ids("cloud")).toContain("saves");
    expect(ids("sunshine")).toContain("streaming");
    expect(ids("proton")).toContain("compatibility");
    expect(ids("controller")).toContain("profiles");
    expect(ids("onboarding")).toContain("help");
    expect(ids("folder")).toContain("storage");
  });

  it("returns nothing for a query that matches no section", () => {
    expect(ids("zzzzz")).toEqual([]);
  });

  it("searches the visible label, so a translated interface still matches", () => {
    const french = (section: { id: SettingsSectionId }) =>
      ({ account: "Compte", appearance: "Apparence", downloads: "Téléchargements" } as Record<string, string>)[section.id] ??
      section.id;
    expect(filterSettingsSections("apparence", SETTINGS_SECTIONS, french).map((section) => section.id)).toContain(
      "appearance",
    );
    expect(labelFor(settingsSectionById("appearance")!)).toBe("Appearance");
  });

  it("scores a section the palette can rank", () => {
    const saves = settingsSectionById("saves")!;
    expect(scoreSettingsSection("saves", saves)).toBeGreaterThan(0);
    expect(scoreSettingsSection("zzzzz", saves)).toBe(0);
  });
});
