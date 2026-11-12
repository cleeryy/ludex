import fs from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import en from "../locales/en.json";
import {
  availableGameSections,
  DEFAULT_GAME_SECTION,
  GAME_SECTIONS,
  gameSectionById,
  gameSectionDescription,
  gameSectionLabel,
  gameSectionPath,
  isGameSectionAvailable,
  isGameSectionId,
  resolveGameSection,
  type GameSectionContext,
  type GameSectionId,
  type GameSectionMeta,
} from "./game-sections";

/**
 * #189: the game page is one sticky sub-nav and one group at a time. These
 * tests pin the taxonomy (order, availability, panel ownership), the `?tab=`
 * contract, and the two copy rules that made the old page confusing: the
 * launch forms no longer share a toast, and one `Build manifest` remains.
 */

const DESKTOP_INSTALLED: GameSectionContext = { kind: "game", native: true, installed: true };
const DESKTOP_MISSING: GameSectionContext = { kind: "game", native: true, installed: false };
const BROWSER_MISSING: GameSectionContext = { kind: "game", native: false, installed: false };
const ROM: GameSectionContext = { kind: "rom", native: false, installed: false };

const ids = (context: GameSectionContext) =>
  availableGameSections(context).map((section) => section.id);

const REQUIRED_PANELS = [
  "status",
  "details",
  "progress",
  "rom-launch",
  "emulator",
  "versions",
  "installed-files",
  "setup-program",
  "startup-command",
  "make-it-run",
  "linux-launch",
  "controller",
  "game-configs",
  "cloud-saves",
  "achievements",
];

describe("game page taxonomy", () => {
  it("keeps the table order the screen reads", () => {
    expect(GAME_SECTIONS.map((section) => section.id)).toEqual([
      "overview",
      "rom",
      "files",
      "launch",
      "settings",
      "saves",
      "achievements",
    ]);
  });

  it("gives every group a translated label and the multi-panel groups a description", () => {
    for (const section of GAME_SECTIONS as readonly GameSectionMeta[]) {
      expect(en, `${section.id} label`).toHaveProperty(section.labelKey);
      expect(gameSectionLabel(section.id)).toBe(section.labelKey);
      if (section.descriptionKey) {
        expect(en, `${section.id} description`).toHaveProperty(section.descriptionKey);
        expect(gameSectionDescription(section.id)).toBe(section.descriptionKey);
      } else {
        expect(gameSectionDescription(section.id)).toBeUndefined();
      }
    }
  });

  it("owns every panel exactly once", () => {
    const owned = GAME_SECTIONS.flatMap((section) => section.panels);
    expect(new Set(owned).size).toBe(owned.length);
    expect([...owned].sort()).toEqual([...REQUIRED_PANELS].sort());
  });

  it("keeps ids unique and addressable", () => {
    const sectionIds = GAME_SECTIONS.map((section) => section.id);
    expect(new Set(sectionIds).size).toBe(sectionIds.length);
    for (const id of sectionIds) {
      expect(isGameSectionId(id)).toBe(true);
      expect(gameSectionById(id)?.id).toBe(id);
    }
  });
});

describe("game page availability", () => {
  it("shows every non-ROM group for an installed game in the desktop app", () => {
    expect(ids(DESKTOP_INSTALLED)).toEqual([
      "overview",
      "files",
      "launch",
      "settings",
      "saves",
      "achievements",
    ]);
  });

  it("drops Settings until the game is installed on this device", () => {
    expect(ids(DESKTOP_MISSING)).toEqual(["overview", "files", "launch", "saves", "achievements"]);
    expect(isGameSectionAvailable(gameSectionById("settings")!, DESKTOP_MISSING)).toBe(false);
    expect(isGameSectionAvailable(gameSectionById("settings")!, DESKTOP_INSTALLED)).toBe(true);
  });

  it("drops Files, Settings and ROM for the browser build", () => {
    const browser = ids(BROWSER_MISSING);
    expect(browser).toEqual(["overview", "files", "launch", "saves", "achievements"]);
    expect(browser).not.toContain("rom");
    expect(browser).not.toContain("settings");
  });

  it("replaces Files and Settings with ROM for a ROM entry", () => {
    expect(ids(ROM)).toEqual(["overview", "rom", "launch", "saves", "achievements"]);
    expect(isGameSectionAvailable(gameSectionById("files")!, ROM)).toBe(false);
    expect(isGameSectionAvailable(gameSectionById("rom")!, ROM)).toBe(true);
    expect(isGameSectionAvailable(gameSectionById("rom")!, DESKTOP_INSTALLED)).toBe(false);
  });

  it("keeps Overview, Launch, Saves and Achievements everywhere", () => {
    for (const context of [DESKTOP_INSTALLED, DESKTOP_MISSING, BROWSER_MISSING, ROM]) {
      expect(ids(context)).toEqual(expect.arrayContaining(["overview", "launch", "saves", "achievements"]));
    }
  });
});

describe("game page deep links", () => {
  const available = ids(DESKTOP_INSTALLED);

  it("reads the group from ?tab=", () => {
    expect(resolveGameSection("files", available)).toBe("files");
    expect(resolveGameSection("achievements", available)).toBe("achievements");
  });

  it("falls back to Overview for a missing, unknown or unavailable group", () => {
    expect(resolveGameSection(null, available)).toBe(DEFAULT_GAME_SECTION);
    expect(resolveGameSection("", available)).toBe(DEFAULT_GAME_SECTION);
    expect(resolveGameSection("nope", available)).toBe(DEFAULT_GAME_SECTION);
    expect(resolveGameSection("settings", ids(BROWSER_MISSING))).toBe("overview");
    expect(resolveGameSection("files", ids(ROM))).toBe("overview");
  });

  it("keeps the bare path canonical for Overview", () => {
    expect(gameSectionPath(12, "overview")).toBe("/game/12");
    expect(gameSectionPath(12, "files")).toBe("/game/12?tab=files");
  });

  it("round-trips every available group through its path", () => {
    for (const id of available) {
      const path = gameSectionPath(12, id as GameSectionId);
      const param = path.includes("?") ? new URLSearchParams(path.split("?")[1]).get("tab") : null;
      expect(resolveGameSection(param, available), id).toBe(id);
    }
  });
});

describe("game page composition", () => {
  const gameRoot = fileURLToPath(new URL("../components/game/", import.meta.url));
  const screenPath = fileURLToPath(new URL("../screens/game-detail.tsx", import.meta.url));
  const gameSources = [
    screenPath,
    ...fs
      .readdirSync(gameRoot)
      .filter((file) => file.endsWith(".tsx"))
      .map((file) => `${gameRoot}${file}`),
  ];

  const count = (needle: string, files = gameSources) =>
    files.reduce((total, file) => total + fs.readFileSync(file, "utf8").split(needle).length - 1, 0);

  it("no longer shares one launch toast between the two forms", () => {
    expect(count("Launch options saved")).toBe(0);
    expect(count('t("Startup command saved")')).toBe(2);
    expect(count('t("Linux options saved")')).toBe(1);
  });

  it("keeps a single Build manifest entry point", () => {
    expect(count('t("Build manifest")')).toBe(1);
  });

  it("renders one group body and no aside", () => {
    const screen = fs.readFileSync(screenPath, "utf8");
    expect(screen).toContain("body(section.id)");
    expect(screen).toContain('heading={MULTI_PANEL.has(section.id) ? "section" : "panel"}');
    expect(screen).toContain('scroll="self"');
    expect(screen).not.toContain("<aside");
    expect(screen).not.toContain("Launch options");
  });

  it("keeps editor-only bodies out of a reader's groups", () => {
    const screen = fs.readFileSync(screenPath, "utf8");
    expect(screen).toContain('{isEditor && game.kind !== "rom" && <StartupCommandPanel');
    expect(fs.readFileSync(`${gameRoot}achievements.tsx`, "utf8")).toContain("isEditor ?");
    expect(fs.readFileSync(`${gameRoot}rom-panel.tsx`, "utf8")).toContain("isEditor ?");
    expect(fs.readFileSync(`${gameRoot}version-manager.tsx`, "utf8")).toContain("{isEditor &&");
  });

  it("names the controller region with its heading", () => {
    const controller = fs.readFileSync(
      fileURLToPath(new URL("../components/settings/controller-panel.tsx", import.meta.url)),
      "utf8",
    );
    expect(controller).toContain("aria-labelledby={headingId}");
    expect(controller).toContain("id={headingId}");
  });

  it("keeps the hero to the five allowed controls", () => {
    const screen = fs.readFileSync(screenPath, "utf8");
    expect(screen).not.toContain("Link existing installation</Button>");
    expect(screen).not.toContain("Add to collection</Button>");
    expect(screen).not.toContain("Artwork</Button>");
  });
});
