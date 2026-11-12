/**
 * The settings taxonomy (#174).
 *
 * Settings used to be thirteen `SectionHeader` blocks in one scroll, mixing
 * identity, presentation, network and machine settings in the order they were
 * written. This table is the one definition of the sections, their order, the
 * panels each one owns, when they are available, and the words that find them
 * in the in-page search and the command palette.
 *
 * The screen, the section nav and the palette all read this module; `?section=`
 * (and the legacy `#help`-style anchors) resolve through it, so a cold load
 * lands on the same section a link promised.
 */

import { scoreSearch } from "@/lib/search";

export type SettingsSectionId =
  | "account"
  | "appearance"
  | "language"
  | "storage"
  | "downloads"
  | "saves"
  | "streaming"
  | "compatibility"
  | "integrations"
  | "profiles"
  | "updates"
  | "help";

/** `desktop` needs the install bridge, `native` needs the Tauri window. */
export type SettingsSectionAvailability = "always" | "desktop" | "native";

export type SettingsIconName =
  | "account"
  | "appearance"
  | "language"
  | "storage"
  | "downloads"
  | "saves"
  | "streaming"
  | "compatibility"
  | "integrations"
  | "profiles"
  | "updates"
  | "help";

export interface SettingsSectionMeta {
  id: SettingsSectionId;
  /** English message id; the UI translates it with `t()`. */
  labelKey: string;
  descriptionKey: string;
  icon: SettingsIconName;
  availability: SettingsSectionAvailability;
  /** Words that find this section in search, beyond its translated label. */
  keywords: readonly string[];
  /**
   * The controls this section owns. Documentation with teeth: the test asserts
   * every panel appears exactly once, so moving the settings cannot drop one.
   */
  panels: readonly string[];
}

export const SETTINGS_SECTIONS = [
  {
    id: "account",
    labelKey: "Account",
    descriptionKey: "Your sign-in, your password and the profile other members can see.",
    icon: "account",
    availability: "always",
    keywords: ["identity", "email", "password", "login", "profile", "public", "bio", "display name"],
    panels: ["identity", "password", "public-profile"],
  },
  {
    id: "appearance",
    labelKey: "Appearance",
    descriptionKey: "Themes, accent colour and the shape of the window on this device.",
    icon: "appearance",
    availability: "always",
    keywords: ["theme", "dark", "light", "colour", "color", "accent", "contrast", "title bar", "big picture"],
    panels: ["theme", "accent", "tint", "theme-file", "bigpicture-layout", "window-chrome"],
  },
  {
    id: "language",
    labelKey: "Language",
    descriptionKey: "The language of the interface. Community translations fall back to English.",
    icon: "language",
    availability: "always",
    keywords: ["locale", "translation", "french", "english", "language"],
    panels: ["locale"],
  },
  {
    id: "storage",
    labelKey: "Storage",
    descriptionKey: "Where games are installed, what this machine already has, and the folders Ludex can link.",
    icon: "storage",
    availability: "always",
    keywords: ["disk", "folder", "install", "gamevault", "space", "drive", "library"],
    panels: ["install-locations", "installed-games", "gamevault"],
  },
  {
    id: "downloads",
    labelKey: "Downloads",
    descriptionKey: "How archives arrive, where they are kept, and how quiet installers run.",
    icon: "downloads",
    availability: "always",
    keywords: ["bandwidth", "archive", "password", "installer", "silent", "speed", "zip", "7z"],
    panels: ["stream-install", "keep-archives", "bandwidth", "archive-passwords", "installer-args", "installer-timeout"],
  },
  {
    id: "saves",
    labelKey: "Saves and cloud",
    descriptionKey: "Back up save folders to your server and teach Ludex where each game keeps its saves.",
    icon: "saves",
    availability: "always",
    keywords: ["cloud", "save", "backup", "snapshot", "device", "ludusavi", "sync"],
    panels: ["cloud-auto", "cloud-device", "save-database", "save-locations"],
  },
  {
    id: "streaming",
    labelKey: "Streaming",
    descriptionKey: "Sunshine and Moonlight: publish the library and start a stream.",
    icon: "streaming",
    availability: "native",
    keywords: ["sunshine", "moonlight", "remote play", "stream", "host"],
    panels: ["sunshine", "moonlight"],
  },
  {
    id: "compatibility",
    labelKey: "Compatibility",
    descriptionKey: "Steam, Proton, UMU and Wine detection for Linux launches.",
    icon: "compatibility",
    availability: "native",
    keywords: ["proton", "wine", "umu", "linux", "steam"],
    panels: ["steam-proton"],
  },
  {
    id: "integrations",
    labelKey: "Integrations",
    descriptionKey: "Import games from other launchers, talk to Discord, and keep several servers at hand.",
    icon: "integrations",
    availability: "always",
    keywords: ["steam", "epic", "gog", "launcher", "discord", "presence", "server", "import"],
    panels: ["launcher-import", "steam-folder", "discord-presence", "servers"],
  },
  {
    id: "profiles",
    labelKey: "Profiles",
    descriptionKey: "Controller mapping and the emulator commands used to launch ROMs.",
    icon: "profiles",
    availability: "always",
    keywords: ["controller", "gamepad", "deadzone", "vibration", "emulator", "rom"],
    panels: ["controllers", "emulators"],
  },
  {
    id: "updates",
    labelKey: "Updates",
    descriptionKey: "Which build you run, how it updates itself, and when it checks.",
    icon: "updates",
    availability: "always",
    keywords: ["version", "channel", "early access", "release", "update"],
    panels: ["update-channel", "update-startup", "update-panel"],
  },
  {
    id: "help",
    labelKey: "Help and about",
    descriptionKey: "Version, the first-run checklist, the keyboard shortcuts and your privacy controls.",
    icon: "help",
    availability: "always",
    keywords: ["about", "help", "onboarding", "shortcuts", "docs", "version", "keyboard", "privacy", "diagnostics", "crash", "report", "telemetry"],
    panels: ["onboarding", "getting-started", "shortcuts", "about", "privacy-diagnostics", "admin-diagnostics"],
  },
] as const satisfies readonly SettingsSectionMeta[];

export type SettingsPanelId = (typeof SETTINGS_SECTIONS)[number]["panels"][number];

export const DEFAULT_SETTINGS_SECTION: SettingsSectionId = "account";

const SECTION_IDS = new Set<string>(SETTINGS_SECTIONS.map((section) => section.id));

export function isSettingsSectionId(value: string | null | undefined): value is SettingsSectionId {
  return Boolean(value && SECTION_IDS.has(value));
}

export function settingsSectionById(id: string): SettingsSectionMeta | undefined {
  return SETTINGS_SECTIONS.find((section) => section.id === id);
}

/**
 * Anchors the old one-page settings exposed, plus the section ids themselves.
 * `HashRouter` swallows a plain `#help`; links now use `?section=`, and old
 * bookmarks still land in the right place.
 */
export const LEGACY_SETTINGS_ANCHORS: Readonly<Record<string, SettingsSectionId>> = {
  help: "help",
  "community-profile-heading": "account",
  "installed-heading": "storage",
  "integrations-heading": "integrations",
  "save-database-heading": "saves",
  "save-locations-heading": "saves",
  "compat-heading": "compatibility",
  "streaming-heading": "streaming",
  "controllers-heading": "profiles",
};

/**
 * The section a URL asks for. `?section=` wins; a legacy hash is read next;
 * anything unknown falls back to Account so the screen always renders.
 */
export function resolveSettingsSection(
  param: string | null | undefined,
  hash?: string | null,
): SettingsSectionId {
  const requested = param?.trim();
  if (isSettingsSectionId(requested)) return requested;

  const anchor = hash?.replace(/^#/, "").trim().toLowerCase();
  if (anchor) {
    const legacy = LEGACY_SETTINGS_ANCHORS[anchor];
    if (legacy) return legacy;
    if (isSettingsSectionId(anchor)) return anchor;
  }
  return DEFAULT_SETTINGS_SECTION;
}

/** Addressable link to a section; the command palette and dialogs use it too. */
export function settingsSectionPath(id: SettingsSectionId): string {
  return `/settings?section=${id}`;
}

export interface SettingsAvailabilityContext {
  desktop: boolean;
  native: boolean;
}

export function isSettingsSectionAvailable(
  section: SettingsSectionMeta,
  context: SettingsAvailabilityContext,
): boolean {
  switch (section.availability) {
    case "always":
      return true;
    case "desktop":
      return context.desktop;
    case "native":
      return context.native;
  }
}

export function availableSettingsSections(context: SettingsAvailabilityContext): SettingsSectionMeta[] {
  return SETTINGS_SECTIONS.filter((section) => isSettingsSectionAvailable(section, context));
}

/**
 * Section search. Matches the translated label first, then the English
 * keywords, with the same fuzzy scorer the palette uses. `labelFor` lets the
 * caller pass `t`; without it the (English) message ids are searched.
 */
export function filterSettingsSections(
  query: string,
  sections: readonly SettingsSectionMeta[] = SETTINGS_SECTIONS,
  labelFor: (section: SettingsSectionMeta) => string = (section) => section.labelKey,
): SettingsSectionMeta[] {
  const trimmed = query.trim();
  if (!trimmed) return [...sections];
  return sections.filter((section) => scoreSettingsSection(trimmed, section, labelFor) > 0);
}

/** Exposed for the palette, which ranks sections and games in one list. */
export function scoreSettingsSection(
  query: string,
  section: SettingsSectionMeta,
  labelFor: (section: SettingsSectionMeta) => string = (section) => section.labelKey,
): number {
  return scoreSearch(query, labelFor(section), [...section.keywords, section.id]);
}
