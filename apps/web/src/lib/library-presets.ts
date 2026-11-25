/**
 * Saved library filter presets (#348).
 *
 * Built-in presets surface common Playnite/Steam-style slices ("Ready to play",
 * "Unplayed backlog", "Top rated", "Short sessions"), while user-saved presets
 * persist any active filter + sort combination in server-scoped localStorage.
 */

import { FILTER_KEYS, type FilterKey } from "@/lib/library-filters";
import { t } from "@/lib/i18n";
import { scopedStorageKey } from "@/lib/server-storage";

export const PRESETS_STORAGE_KEY = "ludex.library.presets";
const MAX_CUSTOM_PRESETS = 20;
const MAX_PRESET_NAME_LEN = 40;

export interface LibraryPreset {
  id: string;
  name: string;
  params: Readonly<Record<string, string>>;
  builtin?: boolean;
}

const VALID_KEY_SET = new Set<string>(FILTER_KEYS);

/** Built-in filter presets available on every Library screen. */
export function builtinPresets(includeInstalled: boolean): LibraryPreset[] {
  const list: LibraryPreset[] = [];
  if (includeInstalled) {
    list.push({
      id: "builtin:ready",
      name: t("Ready to play"),
      params: { installed: "true", sort: "lastPlayed", order: "desc" },
      builtin: true,
    });
  }
  list.push(
    {
      id: "builtin:backlog",
      name: t("Unplayed backlog"),
      params: { state: "unplayed", sort: "rating", order: "desc" },
      builtin: true,
    },
    {
      id: "builtin:top-rated",
      name: t("Top rated"),
      params: { minRating: "80", sort: "rating", order: "desc" },
      builtin: true,
    },
    {
      id: "builtin:short",
      name: t("Short sessions"),
      params: { playtimeMax: "120", sort: "title", order: "asc" },
      builtin: true,
    },
  );
  return list;
}

/** Extracts only recognized non-empty library filter and sort parameters. */
export function extractPresetParams(params: URLSearchParams): Record<string, string> {
  const out: Record<string, string> = {};
  for (const key of FILTER_KEYS) {
    const value = params.get(key)?.trim();
    if (value) out[key] = value;
  }
  return out;
}

/**
 * Replaces filter and sort params in `current` with those from `preset`,
 * preserving non-filter query flags such as `gamepadDebug`.
 */
export function applyPresetParams(
  current: URLSearchParams,
  preset: Readonly<Record<string, string>>,
): URLSearchParams {
  const next = new URLSearchParams(current);
  for (const key of FILTER_KEYS) next.delete(key);
  for (const [key, value] of Object.entries(preset)) {
    if (VALID_KEY_SET.has(key) && value.trim()) {
      next.set(key, value.trim());
    }
  }
  return next;
}

/** True when `current` carries the exact filter/sort set of `preset`. */
export function matchesPreset(
  current: URLSearchParams,
  preset: Readonly<Record<string, string>>,
): boolean {
  const active = extractPresetParams(current);
  const keys = new Set<FilterKey>([
    ...(Object.keys(active) as FilterKey[]),
    ...(Object.keys(preset).filter((key) => VALID_KEY_SET.has(key)) as FilterKey[]),
  ]);
  for (const key of keys) {
    const expected = preset[key] ?? "";
    const actual = active[key] ?? "";
    if (expected !== actual) return false;
  }
  return Object.keys(preset).length > 0;
}

function storageKey(): string {
  return scopedStorageKey(PRESETS_STORAGE_KEY);
}

function sanitiseRecord(raw: unknown): Record<string, string> {
  if (!raw || typeof raw !== "object") return {};
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(raw as Record<string, unknown>)) {
    if (!VALID_KEY_SET.has(key) || typeof value !== "string") continue;
    const trimmed = value.trim();
    if (trimmed) out[key] = trimmed;
  }
  return out;
}

export function loadCustomPresets(): LibraryPreset[] {
  try {
    const raw = localStorage.getItem(storageKey());
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    const out: LibraryPreset[] = [];
    const seen = new Set<string>();
    for (const item of parsed) {
      if (!item || typeof item !== "object") continue;
      const id = typeof item.id === "string" ? item.id.trim() : "";
      const name = typeof item.name === "string" ? item.name.trim().slice(0, MAX_PRESET_NAME_LEN) : "";
      const params = sanitiseRecord(item.params);
      if (!id || !name || seen.has(id) || Object.keys(params).length === 0) continue;
      seen.add(id);
      out.push({ id, name, params });
      if (out.length >= MAX_CUSTOM_PRESETS) break;
    }
    return out;
  } catch {
    return [];
  }
}

export function saveCustomPresets(presets: readonly LibraryPreset[]): void {
  try {
    const cleaned = presets
      .filter((preset) => !preset.builtin && preset.name.trim() && Object.keys(preset.params).length > 0)
      .slice(0, MAX_CUSTOM_PRESETS)
      .map((preset) => ({
        id: preset.id,
        name: preset.name.trim().slice(0, MAX_PRESET_NAME_LEN),
        params: sanitiseRecord(preset.params),
      }));
    if (cleaned.length === 0) {
      localStorage.removeItem(storageKey());
    } else {
      localStorage.setItem(storageKey(), JSON.stringify(cleaned));
    }
  } catch {
    // storage may be unavailable in private browsing
  }
}

export function addCustomPreset(
  name: string,
  params: Readonly<Record<string, string>>,
): LibraryPreset[] {
  const trimmed = name.trim().slice(0, MAX_PRESET_NAME_LEN);
  const cleaned = sanitiseRecord(params);
  const existing = loadCustomPresets();
  if (!trimmed || Object.keys(cleaned).length === 0) return existing;
  const id = `custom:${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
  const next = [...existing.filter((preset) => preset.name.toLowerCase() !== trimmed.toLowerCase()), {
    id,
    name: trimmed,
    params: cleaned,
  }].slice(-MAX_CUSTOM_PRESETS);
  saveCustomPresets(next);
  return next;
}

export function deleteCustomPreset(id: string): LibraryPreset[] {
  const next = loadCustomPresets().filter((preset) => preset.id !== id);
  saveCustomPresets(next);
  return next;
}
