/**
 * Library filter model shared by the toolbar, the filter sheet, the URL and
 * localStorage. Filters live in the query string so a view is shareable; the
 * last used set is restored when the library is opened without params.
 */

import { stateLabel, sortLabel } from "@/lib/format";
import { t } from "@/lib/i18n";
import type { GameListQuery, ProgressState } from "@/lib/types";

export const FILTER_STORAGE_KEY = "ludex.library.filters";

export const FILTER_KEYS = [
  "q",
  "genre",
  "state",
  "earlyAccess",
  "installed",
  "yearFrom",
  "yearTo",
  "players",
  "playtimeMin",
  "playtimeMax",
  "minRating",
  "provider",
  "sort",
  "order",
] as const;

export type FilterKey = (typeof FILTER_KEYS)[number];

export interface LibraryFilters {
  q: string;
  genre: string;
  state: ProgressState | "";
  earlyAccess: boolean;
  installed: boolean;
  yearFrom: string;
  yearTo: string;
  players: string;
  playtimeMin: string;
  playtimeMax: string;
  minRating: string;
  provider: string;
  sort: NonNullable<GameListQuery["sort"]>;
  order: NonNullable<GameListQuery["order"]>;
}

export const PROGRESS_STATES: readonly ProgressState[] = [
  "unplayed",
  "playing",
  "completed",
  "onHold",
  "abandoned",
  "planToPlay",
];

export const SORT_VALUES = ["title", "releaseDate", "addedAt", "lastPlayed", "playtime", "rating"] as const;
export type SortValue = (typeof SORT_VALUES)[number];

export function sortOptions(): { value: SortValue; label: string }[] {
  return SORT_VALUES.map((value) => ({ value, label: sortLabel(value) }));
}

export function stateOptions(): { value: string; label: string }[] {
  return [{ value: "any", label: t("Any state") }, ...PROGRESS_STATES.map((value) => ({ value, label: stateLabel(value) }))];
}

export function playerOptions(): { value: string; label: string }[] {
  return [
    { value: "any", label: t("Any") },
    { value: "2", label: t("2+ players") },
    { value: "3", label: t("3+ players") },
    { value: "4", label: t("4+ players") },
    { value: "8", label: t("8+ players") },
  ];
}

export function ratingOptions(): { value: string; label: string }[] {
  return [
    { value: "any", label: t("Any") },
    { value: "60", label: t("60 and up") },
    { value: "70", label: t("70 and up") },
    { value: "80", label: t("80 and up") },
    { value: "90", label: t("90 and up") },
  ];
}

export function providerOptions(): { value: string; label: string }[] {
  return [
    { value: "any", label: t("Any source") },
    { value: "igdb", label: "IGDB" },
    { value: "steam", label: "Steam" },
    { value: "demo", label: t("Demo data") },
  ];
}

export function providerLabel(value: string): string {
  switch (value) {
    case "igdb":
      return "IGDB";
    case "steam":
      return "Steam";
    case "demo":
      return t("Demo data");
    default:
      return value;
  }
}

export function defaultSort(mode: string): NonNullable<GameListQuery["sort"]> {
  return mode === "recent" ? "lastPlayed" : "title";
}

export function readFilters(
  params: URLSearchParams,
  mode: string,
): LibraryFilters {
  const sort = (params.get("sort") ?? defaultSort(mode)) as LibraryFilters["sort"];
  return {
    q: params.get("q") ?? "",
    genre: params.get("genre") ?? "",
    state: (params.get("state") as ProgressState | null) ?? "",
    earlyAccess: params.get("earlyAccess") === "true",
    installed: params.get("installed") === "true",
    yearFrom: params.get("yearFrom") ?? "",
    yearTo: params.get("yearTo") ?? "",
    players: params.get("players") ?? "",
    playtimeMin: params.get("playtimeMin") ?? "",
    playtimeMax: params.get("playtimeMax") ?? "",
    minRating: params.get("minRating") ?? "",
    provider: params.get("provider") ?? "",
    sort,
    order: (params.get("order") ?? (sort === "title" ? "asc" : "desc")) as LibraryFilters["order"],
  };
}

/** Writes one filter into a copy of the params, deleting it when empty. */
export function setFilterParam(params: URLSearchParams, key: FilterKey, value: string | boolean | null): URLSearchParams {
  const next = new URLSearchParams(params);
  if (value === null || value === "" || value === false || value === "any") {
    next.delete(key);
  } else {
    next.set(key, value === true ? "true" : String(value));
  }
  return next;
}

export function clearFilterParams(params: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(params);
  for (const key of FILTER_KEYS) {
    if (key === "sort" || key === "order") continue;
    next.delete(key);
  }
  return next;
}

/** True when the URL carries at least one filter (sort alone does not count). */
export function hasFilterParams(params: URLSearchParams): boolean {
  return FILTER_KEYS.some((key) => key !== "sort" && key !== "order" && params.has(key));
}

export function filterQuery(filters: LibraryFilters): GameListQuery {
  return {
    q: filters.q.trim() || undefined,
    genre: filters.genre || undefined,
    state: filters.state || undefined,
    earlyAccess: filters.earlyAccess || undefined,
    yearFrom: filters.yearFrom ? Number(filters.yearFrom) : undefined,
    yearTo: filters.yearTo ? Number(filters.yearTo) : undefined,
    players: filters.players ? Number(filters.players) : undefined,
    playtimeMin: filters.playtimeMin ? Number(filters.playtimeMin) : undefined,
    playtimeMax: filters.playtimeMax ? Number(filters.playtimeMax) : undefined,
    minRating: filters.minRating ? Number(filters.minRating) : undefined,
    provider: filters.provider || undefined,
    sort: filters.sort,
    order: filters.order,
  };
}

export interface FilterChip {
  key: FilterKey;
  label: string;
}

export function activeFilterChips(filters: LibraryFilters): FilterChip[] {
  const chips: FilterChip[] = [];
  if (filters.q.trim()) chips.push({ key: "q", label: t("“{query}”", { query: filters.q.trim() }) });
  if (filters.genre) chips.push({ key: "genre", label: filters.genre });
  if (filters.state) chips.push({ key: "state", label: stateLabel(filters.state) });
  if (filters.earlyAccess) chips.push({ key: "earlyAccess", label: t("Early access") });
  if (filters.installed) chips.push({ key: "installed", label: t("Installed") });
  if (filters.yearFrom || filters.yearTo) {
    const label =
      filters.yearFrom && filters.yearTo
        ? t("{from}–{to}", { from: filters.yearFrom, to: filters.yearTo })
        : filters.yearFrom
          ? t("From {year}", { year: filters.yearFrom })
          : t("Until {year}", { year: filters.yearTo });
    chips.push({ key: "yearFrom", label });
  }
  if (filters.players) chips.push({ key: "players", label: t("{count}+ players", { count: Number(filters.players) }) });
  if (filters.playtimeMin || filters.playtimeMax) {
    const label =
      filters.playtimeMin && filters.playtimeMax
        ? t("{from}–{to} min played", { from: filters.playtimeMin, to: filters.playtimeMax })
        : filters.playtimeMin
          ? t("{count}+ min played", { count: Number(filters.playtimeMin) })
          : t("Up to {count} min played", { count: Number(filters.playtimeMax) });
    chips.push({ key: "playtimeMin", label });
  }
  if (filters.minRating) chips.push({ key: "minRating", label: t("{count}+ rating", { count: Number(filters.minRating) }) });
  if (filters.provider) chips.push({ key: "provider", label: t("Source: {provider}", { provider: providerLabel(filters.provider) }) });
  return chips;
}

export function saveFilters(params: URLSearchParams): void {
  try {
    const stored = new URLSearchParams();
    for (const key of FILTER_KEYS) {
      const value = params.get(key);
      if (value) stored.set(key, value);
    }
    if ([...stored.keys()].length === 0) localStorage.removeItem(FILTER_STORAGE_KEY);
    else localStorage.setItem(FILTER_STORAGE_KEY, stored.toString());
  } catch {
    // Private mode or quota errors: persistence is best-effort.
  }
}

export function loadFilters(): URLSearchParams | null {
  try {
    const raw = localStorage.getItem(FILTER_STORAGE_KEY);
    if (!raw) return null;
    const params = new URLSearchParams(raw);
    return [...params.keys()].length ? params : null;
  } catch {
    return null;
  }
}

export function removeFilterChip(key: FilterKey): Partial<LibraryFilters> {
  switch (key) {
    case "q":
      return { q: "" };
    case "genre":
      return { genre: "" };
    case "state":
      return { state: "" };
    case "earlyAccess":
      return { earlyAccess: false };
    case "installed":
      return { installed: false };
    case "yearFrom":
      return { yearFrom: "", yearTo: "" };
    case "players":
      return { players: "" };
    case "playtimeMin":
      return { playtimeMin: "", playtimeMax: "" };
    case "minRating":
      return { minRating: "" };
    case "provider":
      return { provider: "" };
    default:
      return {};
  }
}
