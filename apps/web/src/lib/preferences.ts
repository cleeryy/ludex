import {
  BIG_PICTURE_LAYOUT_KEY,
  DEFAULT_BIG_PICTURE_LAYOUT,
  sanitizeLayout,
  type BigPictureLayout,
} from "@/components/big-picture/layout";
import {
  DEFAULT_CONTROLLER_PREFS,
  readControllerPrefs,
  type BigPictureControllerPrefs,
} from "@/components/big-picture/controller";
import { notifyPreferencesChanged, PREFERENCES_CHANGED_EVENT } from "@/lib/preferences-events";
import { scopedStorageKey } from "@/lib/server-storage";
import {
  browserStorage,
  customThemeId,
  defaultConfig,
  readConfig,
  sanitizeSelection,
  type StorageLike,
} from "@/theme/storage";
import type { ThemeFile } from "@/theme/types";

/**
 * Account preference document (#201).
 *
 * One typed object carries the settings that should follow the user across
 * browsers and machines. Everything here is validated twice: the server
 * enforces the allowlist and the 64 KiB cap, and this module sanitises what it
 * reads back so an old document, a hand-edited cache or a newer server cannot
 * inject a value into the UI.
 *
 * Device-scoped settings deliberately stay out of the document:
 * install/download/temp roots, the bandwidth cap, Sunshine and Moonlight paths
 * or credentials, the controller hardware mapping, overlay settings, window
 * chrome, server profiles, tokens and the update channel. They describe this
 * machine, not the account, and a phone must not inherit a PC's install path.
 */

export const PREFERENCES_SCHEMA_VERSION = 1;

/** Local cache of the last server document, per account and per server. */
export const PREFERENCES_CACHE_PREFIX = "ludex.preferences.v1";
/** Account-level update reminder choice; device-level remind-later stays local. */
export const UPDATE_PROMPT_STORAGE_KEY = "ludex.update.pref";
/** Account-level update notification channels (#213). */
export const UPDATE_NOTIFICATIONS_STORAGE_KEY = "ludex.update.notifications";
/** Big Picture sound cues preference (reserved; stored so devices agree). */
export const SOUND_STORAGE_KEY = "ludex.bigPicture.sound";
/** Library view preference, scoped per server like the existing setting. */
export const LIBRARY_VIEW_STORAGE_KEY = "ludex.libraryView";
/** Fired whenever a local account-scoped preference changes. */
export { PREFERENCES_CHANGED_EVENT, notifyPreferencesChanged };

export type LibraryViewPreference = "grid" | "list";
export type UpdatePromptPreference = "always" | "notify" | "off";
/**
 * Where a new build is announced (#213): in-app and OS banner, in-app only, or
 * nowhere. The badge and the update center ignore this preference.
 */
export type UpdateNotificationPreference = "app-and-os" | "app" | "off";

export interface ThemePreference {
  /** Built-in id, `system`, or a `custom:<hash>` id from the theme registry. */
  selection: string;
  accent: string | null;
  tint: string | null;
}

export interface BigPicturePreference {
  layout: BigPictureLayout;
  /** Play UI sound cues on the console surface. */
  sound: boolean;
  /** Rumble on focus and confirm. */
  haptics: boolean;
}

export interface LibraryPreference {
  view: LibraryViewPreference;
}

export interface UpdatesPreference {
  /** `always` opens the launch prompt, `notify` only shows the badge, `off` hides both. */
  prompt: UpdatePromptPreference;
  /** Where a new build is announced: in-app and OS, in-app only, or off (#213). */
  notifications: UpdateNotificationPreference;
}

export interface AccountPreferences {
  theme: ThemePreference;
  bigPicture: BigPicturePreference;
  library: LibraryPreference;
  updates: UpdatesPreference;
}

export type PreferencesPatch = {
  [K in keyof AccountPreferences]?: Partial<AccountPreferences[K]>;
};

/** Wire shape of `GET/PUT/PATCH/DELETE /api/account/preferences`. */
export interface PreferencesResponse {
  document: unknown | null;
  revision: number;
  schemaVersion: number;
  updatedAt: string | null;
}

/** Cached document plus what is still owed to the server. */
export interface CachedPreferences {
  document: AccountPreferences;
  revision: number;
  /** True when a local change has not reached the server yet. */
  pending: boolean;
}

export function defaultPreferences(): AccountPreferences {
  return {
    theme: defaultConfig(),
    bigPicture: {
      layout: DEFAULT_BIG_PICTURE_LAYOUT,
      sound: true,
      haptics: DEFAULT_CONTROLLER_PREFS.haptics,
    },
    library: { view: "grid" },
    updates: { prompt: "always", notifications: "app-and-os" },
  };
}

/* -------------------------------- validation ------------------------------- */

const HEX_COLOR = /^#[0-9a-f]{3,8}$/i;

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function hexOrNull(value: unknown): string | null {
  return typeof value === "string" && HEX_COLOR.test(value) ? value.toLowerCase() : null;
}

function isLayout(value: unknown): value is BigPictureLayout {
  return sanitizeLayout(value) === value;
}

function isLibraryView(value: unknown): value is LibraryViewPreference {
  return value === "grid" || value === "list";
}

function isUpdatePrompt(value: unknown): value is UpdatePromptPreference {
  return value === "always" || value === "notify" || value === "off";
}

function isUpdateNotification(value: unknown): value is UpdateNotificationPreference {
  return value === "app-and-os" || value === "app" || value === "off";
}

/**
 * Coerces any JSON value into a complete document. Unknown keys are dropped,
 * wrong types fall back per field, and a missing document yields the defaults;
 * the caller never has to branch on partial data.
 */
export function sanitizePreferences(value: unknown): AccountPreferences {
  const defaults = defaultPreferences();
  if (!isRecord(value)) return defaults;

  const theme = isRecord(value.theme) ? value.theme : {};
  const bigPicture = isRecord(value.bigPicture) ? value.bigPicture : {};
  const library = isRecord(value.library) ? value.library : {};
  const updates = isRecord(value.updates) ? value.updates : {};

  return {
    theme: {
      selection:
        typeof theme.selection === "string"
          ? sanitizeSelection(theme.selection)
          : defaults.theme.selection,
      accent: hexOrNull(theme.accent),
      tint: hexOrNull(theme.tint),
    },
    bigPicture: {
      layout: isLayout(bigPicture.layout) ? bigPicture.layout : defaults.bigPicture.layout,
      sound: typeof bigPicture.sound === "boolean" ? bigPicture.sound : defaults.bigPicture.sound,
      haptics:
        typeof bigPicture.haptics === "boolean" ? bigPicture.haptics : defaults.bigPicture.haptics,
    },
    library: {
      view: isLibraryView(library.view) ? library.view : defaults.library.view,
    },
    updates: {
      prompt: isUpdatePrompt(updates.prompt) ? updates.prompt : defaults.updates.prompt,
      notifications: isUpdateNotification(updates.notifications)
        ? updates.notifications
        : defaults.updates.notifications,
    },
  };
}

/** Deep-merges a partial document over a base and re-sanitises the result. */
export function mergePreferences(
  base: AccountPreferences,
  patch: PreferencesPatch | null | undefined,
): AccountPreferences {
  if (!patch) return base;
  return sanitizePreferences({
    theme: { ...base.theme, ...(patch.theme ?? {}) },
    bigPicture: { ...base.bigPicture, ...(patch.bigPicture ?? {}) },
    library: { ...base.library, ...(patch.library ?? {}) },
    updates: { ...base.updates, ...(patch.updates ?? {}) },
  });
}

/** Only the keys that actually changed, so writes stay small and readable. */
export function preferencePatch(
  from: AccountPreferences,
  to: AccountPreferences,
): PreferencesPatch {
  const patch: PreferencesPatch = {};
  if (
    from.theme.selection !== to.theme.selection ||
    from.theme.accent !== to.theme.accent ||
    from.theme.tint !== to.theme.tint
  ) {
    patch.theme = { ...to.theme };
  }
  if (
    from.bigPicture.layout !== to.bigPicture.layout ||
    from.bigPicture.sound !== to.bigPicture.sound ||
    from.bigPicture.haptics !== to.bigPicture.haptics
  ) {
    patch.bigPicture = { ...to.bigPicture };
  }
  if (from.library.view !== to.library.view) patch.library = { ...to.library };
  if (
    from.updates.prompt !== to.updates.prompt ||
    from.updates.notifications !== to.updates.notifications
  ) {
    patch.updates = { ...to.updates };
  }
  return patch;
}

export function preferencesEqual(a: AccountPreferences, b: AccountPreferences): boolean {
  return (
    a.theme.selection === b.theme.selection &&
    a.theme.accent === b.theme.accent &&
    a.theme.tint === b.theme.tint &&
    a.bigPicture.layout === b.bigPicture.layout &&
    a.bigPicture.sound === b.bigPicture.sound &&
    a.bigPicture.haptics === b.bigPicture.haptics &&
    a.library.view === b.library.view &&
    a.updates.prompt === b.updates.prompt &&
    a.updates.notifications === b.updates.notifications
  );
}

/** A custom theme only renders where its file is installed. */
export function themeAvailable(selection: string, customThemes: readonly ThemeFile[]): boolean {
  if (!selection.startsWith("custom:")) return true;
  return customThemes.some((theme) => customThemeId(theme.name, theme.tokens) === selection);
}

/* ---------------------------------- cache --------------------------------- */

export function preferencesCacheKey(userId: number): string {
  return scopedStorageKey(`${PREFERENCES_CACHE_PREFIX}.${userId}`);
}

export function readCachedPreferences(
  userId: number,
  storage: StorageLike | null = browserStorage(),
): CachedPreferences | null {
  if (!storage) return null;
  try {
    const raw = storage.getItem(preferencesCacheKey(userId));
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed)) return null;
    return {
      document: sanitizePreferences(parsed.document),
      revision:
        typeof parsed.revision === "number" && Number.isFinite(parsed.revision)
          ? Math.max(0, parsed.revision)
          : 0,
      pending: parsed.pending === true,
    };
  } catch {
    return null;
  }
}

export function writeCachedPreferences(
  userId: number,
  cache: CachedPreferences,
  storage: StorageLike | null = browserStorage(),
): void {
  if (!storage) return;
  try {
    storage.setItem(preferencesCacheKey(userId), JSON.stringify(cache));
  } catch {
    /* private mode or a full quota: the session keeps working in memory */
  }
}

/* ------------------------------ local snapshot ----------------------------- */

function scopedKey(name: string): string {
  try {
    return scopedStorageKey(name);
  } catch {
    return name;
  }
}

export function readUpdatePrompt(storage: StorageLike | null = browserStorage()): UpdatePromptPreference {
  try {
    const value = storage?.getItem(UPDATE_PROMPT_STORAGE_KEY);
    return isUpdatePrompt(value) ? value : "always";
  } catch {
    return "always";
  }
}

export function writeUpdatePrompt(
  prompt: UpdatePromptPreference,
  storage: StorageLike | null = browserStorage(),
): void {
  try {
    storage?.setItem(UPDATE_PROMPT_STORAGE_KEY, prompt);
  } catch {
    /* ignore */
  }
}

export function readUpdateNotifications(
  storage: StorageLike | null = browserStorage(),
): UpdateNotificationPreference {
  try {
    const value = storage?.getItem(UPDATE_NOTIFICATIONS_STORAGE_KEY);
    return isUpdateNotification(value) ? value : "app-and-os";
  } catch {
    return "app-and-os";
  }
}

export function writeUpdateNotifications(
  notifications: UpdateNotificationPreference,
  storage: StorageLike | null = browserStorage(),
): void {
  try {
    storage?.setItem(UPDATE_NOTIFICATIONS_STORAGE_KEY, notifications);
  } catch {
    /* ignore */
  }
}

export function readSoundPreference(storage: StorageLike | null = browserStorage()): boolean {
  try {
    return storage?.getItem(SOUND_STORAGE_KEY) !== "off";
  } catch {
    return true;
  }
}

export function writeSoundPreference(
  enabled: boolean,
  storage: StorageLike | null = browserStorage(),
): void {
  try {
    storage?.setItem(SOUND_STORAGE_KEY, enabled ? "on" : "off");
  } catch {
    /* ignore */
  }
}

/** Current account-scoped values on this device, in document shape. */
export function readLocalPreferences(storage: StorageLike | null = browserStorage()): AccountPreferences {
  const defaults = defaultPreferences();
  const config = readConfig(storage);
  let layout: BigPictureLayout = defaults.bigPicture.layout;
  try {
    const raw = storage?.getItem(scopedKey(BIG_PICTURE_LAYOUT_KEY));
    if (raw) layout = sanitizeLayout(raw);
  } catch {
    /* keep the default */
  }
  let view: LibraryViewPreference = defaults.library.view;
  try {
    const raw = storage?.getItem(scopedKey(LIBRARY_VIEW_STORAGE_KEY));
    if (raw === "list") view = "list";
  } catch {
    /* keep the default */
  }
  let haptics = defaults.bigPicture.haptics;
  try {
    const prefs: BigPictureControllerPrefs = readControllerPrefs(
      storage,
      scopedKey("ludex.bigPicture.controller"),
    );
    haptics = prefs.haptics;
  } catch {
    /* keep the default */
  }
  return {
    theme: {
      selection: config.selection,
      accent: config.accent,
      tint: config.tint,
    },
    bigPicture: { layout, sound: readSoundPreference(storage), haptics },
    library: { view },
    updates: {
      prompt: readUpdatePrompt(storage),
      notifications: readUpdateNotifications(storage),
    },
  };
}
