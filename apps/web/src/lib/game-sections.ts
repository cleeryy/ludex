/**
 * The game-page taxonomy (#189).
 *
 * The installed-game area used to be ten peer blocks in one scroll, with four
 * heading styles and no addressable state. This table is the one definition of
 * the groups, their order, the panels each one owns and when they exist, so the
 * screen and the sub-nav cannot drift apart. `?tab=` resolves through it, and a
 * group this surface cannot show falls back to Overview instead of an empty
 * page.
 */

import { t } from "@/lib/i18n";

export type GameSectionId =
  | "overview"
  | "rom"
  | "files"
  | "launch"
  | "settings"
  | "saves"
  | "achievements";

/** What the page knows about the game and the surface it runs on. */
export interface GameSectionContext {
  /** `rom` entries have no archives and no config files. */
  kind: "game" | "rom";
  /** Tauri window: install folders, launch options and config editors exist. */
  native: boolean;
  /** Installed on this device; the Settings group needs a folder to edit. */
  installed: boolean;
}

export interface GameSectionMeta {
  id: GameSectionId;
  /** English message id; the UI translates it with `t()`. */
  labelKey: string;
  /** Only the multi-panel groups draw this under their `h2`. */
  descriptionKey?: string;
  /**
   * The blocks this group owns. Documentation with teeth: the test asserts
   * every panel appears exactly once, so a move cannot drop one.
   */
  panels: readonly string[];
}

export const GAME_SECTIONS = [
  {
    id: "overview",
    labelKey: "Overview",
    descriptionKey: "The facts, the install state and your progress on this account.",
    panels: ["status", "details", "progress"],
  },
  {
    id: "rom",
    labelKey: "ROM",
    panels: ["rom-launch", "emulator"],
  },
  {
    id: "files",
    labelKey: "Files",
    descriptionKey: "Archives on the server and the files on this device.",
    panels: ["versions", "installed-files", "setup-program"],
  },
  {
    id: "launch",
    labelKey: "Launch",
    descriptionKey: "How the desktop app starts this game.",
    panels: ["startup-command", "make-it-run", "linux-launch", "controller"],
  },
  {
    id: "settings",
    labelKey: "Settings",
    panels: ["game-configs"],
  },
  {
    id: "saves",
    labelKey: "Saves",
    panels: ["cloud-saves"],
  },
  {
    id: "achievements",
    labelKey: "Achievements",
    panels: ["achievements"],
  },
] as const satisfies readonly GameSectionMeta[];

export type GamePanelId = (typeof GAME_SECTIONS)[number]["panels"][number];

export const DEFAULT_GAME_SECTION: GameSectionId = "overview";

const SECTION_IDS = new Set<string>(GAME_SECTIONS.map((section) => section.id));

export function isGameSectionId(value: string | null | undefined): value is GameSectionId {
  return Boolean(value && SECTION_IDS.has(value));
}

export function gameSectionById(id: string): GameSectionMeta | undefined {
  return GAME_SECTIONS.find((section) => section.id === id);
}

/** Translated labels, as literal `t()` calls the extractor can check. */
export function gameSectionLabel(id: GameSectionId): string {
  switch (id) {
    case "overview":
      return t("Overview");
    case "rom":
      return t("ROM");
    case "files":
      return t("Files");
    case "launch":
      return t("Launch");
    case "settings":
      return t("Settings");
    case "saves":
      return t("Saves");
    case "achievements":
      return t("Achievements");
  }
}

export function gameSectionDescription(id: GameSectionId): string | undefined {
  switch (id) {
    case "overview":
      return t("The facts, the install state and your progress on this account.");
    case "files":
      return t("Archives on the server and the files on this device.");
    case "launch":
      return t("How the desktop app starts this game.");
    default:
      return undefined;
  }
}

/**
 * Table order minus what this game/surface cannot show. Overview is always
 * present, so the fallback target in `resolveGameSection` always exists.
 */
export function availableGameSections(context: GameSectionContext): GameSectionMeta[] {
  return GAME_SECTIONS.filter((section) => isGameSectionAvailable(section, context));
}

export function isGameSectionAvailable(
  section: GameSectionMeta,
  context: GameSectionContext,
): boolean {
  switch (section.id) {
    case "overview":
    case "launch":
    case "saves":
    case "achievements":
      return true;
    case "rom":
      return context.kind === "rom";
    case "files":
      return context.kind !== "rom";
    case "settings":
      return context.kind !== "rom" && context.native && context.installed;
  }
}

/**
 * The group a URL asks for. Unknown ids and groups this game does not have
 * land on Overview, so an uninstalled game keeps a `/game/12?tab=settings`
 * link working instead of painting an empty body.
 */
export function resolveGameSection(
  param: string | null | undefined,
  available: readonly GameSectionId[],
): GameSectionId {
  const requested = param?.trim().toLowerCase();
  if (requested && isGameSectionId(requested) && available.includes(requested)) return requested;
  return DEFAULT_GAME_SECTION;
}

/** Addressable link; Overview stays canonical so the bare path has no query. */
export function gameSectionPath(gameId: number, id: GameSectionId): string {
  return id === DEFAULT_GAME_SECTION ? `/game/${gameId}` : `/game/${gameId}?tab=${id}`;
}
