/**
 * Library model shared by the server shelf and the local launcher index (#154).
 *
 * `api.listGames()` only knows the server library, while Settings → Installed
 * merges the desktop `installed_games` table. This module merges both into one
 * display model: matched launcher imports stay on the server card (source
 * badge), unmatched imports get their own `Launcher imports` group and use
 * their negative local id. Everything here is pure so the merge, the source
 * filter, the search and the installed count are testable without React.
 */

import type { GameListQuery, GameSummary, InstalledGame, LauncherId } from "@/lib/types";
import { t } from "@/lib/i18n";

/** Launchers the desktop importer writes into `installed_games.source` (#116). */
export const LAUNCHER_SOURCES = [
  "steam",
  "epic",
  "gog",
  "ea",
  "ubisoft",
  "xbox",
  "itch",
  "heroic",
  "lutris",
] as const satisfies readonly LauncherId[];

/** A launcher id, or `other` for a source the UI does not know yet. */
export type LauncherSourceId = LauncherId | "other";

export type LibrarySort = NonNullable<GameListQuery["sort"]>;
export type LibraryOrder = NonNullable<GameListQuery["order"]>;

/**
 * The launcher that owns an install, or null for entries Ludex installed
 * (`ludex`) or linked in place (`adopted`). Those are real Ludex rows and
 * already appear on their server game card.
 */
export function launcherSource(source?: string | null): LauncherSourceId | null {
  if (!source || source === "ludex" || source === "adopted") return null;
  return (LAUNCHER_SOURCES as readonly string[]).includes(source)
    ? (source as LauncherId)
    : "other";
}

export function launcherLabel(source: LauncherSourceId): string {
  switch (source) {
    case "steam":
      return "Steam";
    case "epic":
      return "Epic";
    case "gog":
      return "GOG";
    case "ea":
      return "EA";
    case "ubisoft":
      return "Ubisoft";
    case "xbox":
      return "Xbox";
    case "itch":
      return "itch.io";
    case "heroic":
      return "Heroic";
    case "lutris":
      return "Lutris";
    default:
      return t("Other");
  }
}

export function launcherSourceOptions(): { value: LauncherSourceId; label: string }[] {
  return [
    ...LAUNCHER_SOURCES.map((source) => ({ value: source, label: launcherLabel(source) })),
    { value: "other" as const, label: launcherLabel("other") },
  ];
}

/** How many imports each launcher contributes, for the source chips. */
export function launcherCounts(installs: readonly InstalledGame[]): Map<LauncherSourceId, number> {
  const counts = new Map<LauncherSourceId, number>();
  for (const install of installs) {
    const source = launcherSource(install.source);
    if (!source) continue;
    counts.set(source, (counts.get(source) ?? 0) + 1);
  }
  return counts;
}

/** Launcher imports are removed from the local index only; their files stay (#154). */
export function isIndexOnlyImport(install: InstalledGame): boolean {
  return launcherSource(install.source) !== null;
}

/** Imports whose game id is not in the server library (unmatched, negative id). */
export function unmatchedImports(
  installs: readonly InstalledGame[],
  serverIds: ReadonlySet<number>,
): InstalledGame[] {
  return installs.filter(
    (install) =>
      isIndexOnlyImport(install) && install.gameId < 0 && !serverIds.has(install.gameId),
  );
}

export interface LibraryEntryBase {
  /** Server id for server rows, negative local id for imports. */
  gameId: number;
  title: string;
  sortTitle: string;
  sizeBytes: number;
  releaseDate: string | null;
  /** Launcher that owns the install, when the row is a launcher import. */
  launcher: LauncherSourceId | null;
  /** The local index row behind this entry, when the game is installed. */
  install: InstalledGame | null;
}

export interface ServerLibraryEntry extends LibraryEntryBase {
  kind: "server";
  game: GameSummary;
  /** The launcher import matched onto this server game (#116), if any. */
  matchedImport: InstalledGame | null;
}

export interface ImportLibraryEntry extends LibraryEntryBase {
  kind: "import";
  install: InstalledGame;
}

export type LibraryEntry = ServerLibraryEntry | ImportLibraryEntry;

export interface BuildLibraryOptions {
  /** Only entries installed on this machine, imports included. */
  installedOnly?: boolean;
  /** Launcher chip filter; empty or `any` keeps every source. */
  source?: LauncherSourceId | "any" | "" | null;
  /** Case-insensitive title search for entries the server query cannot see. */
  query?: string;
  sort?: LibrarySort;
  order?: LibraryOrder;
  /**
   * Show unmatched imports even when no filter asks for them. The Library
   * screen uses this for its always-visible `Launcher imports` group.
   */
  includeUnmatched?: boolean;
}

export interface LibraryModel {
  /** Server-backed entries in the order the server returned them. */
  server: ServerLibraryEntry[];
  /** Unmatched launcher imports, sorted locally (the Launcher imports group). */
  imports: ImportLibraryEntry[];
  /** Server entries plus unmatched imports. */
  total: number;
  /** Games installed on this machine: server rows with an index entry + imports. */
  installedCount: number;
}

const normalize = (value: string): string => value.trim().toLocaleLowerCase();

function matchesQuery(title: string, sortTitle: string, query: string): boolean {
  if (!query) return true;
  return normalize(title).includes(query) || normalize(sortTitle).includes(query);
}

/** Imports carry no rating, release date or playtime; those sorts fall back to title. */
export function compareImportEntries(
  a: ImportLibraryEntry,
  b: ImportLibraryEntry,
  sort: LibrarySort = "title",
  order: LibraryOrder = "asc",
): number {
  const direction = order === "desc" ? -1 : 1;
  switch (sort) {
    case "addedAt":
      return direction * a.install.installedAt.localeCompare(b.install.installedAt);
    case "lastPlayed":
      return direction * (a.install.lastLaunchedAt ?? "").localeCompare(b.install.lastLaunchedAt ?? "");
    default:
      return direction * a.sortTitle.localeCompare(b.sortTitle);
  }
}

/**
 * Merge the server shelf with the local install index.
 *
 * - A launcher import whose game id exists on the server is represented once,
 *   by its server card, with the import attached as `matchedImport`.
 * - Unmatched imports (negative ids) are only surfaced when the Installed
 *   filter, a launcher source filter or `includeUnmatched` asks for them.
 * - Removing an import from the array is enough for it to disappear: the model
 *   never invents entries the index does not have.
 */
export function buildLibraryModel(
  games: readonly GameSummary[],
  installs: readonly InstalledGame[],
  options: BuildLibraryOptions = {},
): LibraryModel {
  const installedOnly = Boolean(options.installedOnly);
  const source =
    options.source && options.source !== "any" ? (options.source as LauncherSourceId) : null;
  const query = normalize(options.query ?? "");
  const sort = options.sort ?? "title";
  const order = options.order ?? "asc";
  const includeUnmatched = options.includeUnmatched ?? (installedOnly || source !== null);

  const installsById = new Map<number, InstalledGame>();
  for (const install of installs) installsById.set(install.gameId, install);
  const serverIds = new Set(games.map((game) => game.id));

  const server: ServerLibraryEntry[] = [];
  for (const game of games) {
    const install = installsById.get(game.id) ?? null;
    const matchedImport = install && isIndexOnlyImport(install) ? install : null;
    if (installedOnly && !install) continue;
    if (source && (!matchedImport || launcherSource(matchedImport.source) !== source)) continue;
    server.push({
      kind: "server",
      gameId: game.id,
      title: game.title,
      sortTitle: game.sortTitle,
      sizeBytes: game.totalSize,
      releaseDate: game.releaseDate,
      launcher: matchedImport ? launcherSource(matchedImport.source) : null,
      install,
      game,
      matchedImport,
    });
  }

  const imports: ImportLibraryEntry[] =
    includeUnmatched || installedOnly || source !== null
      ? unmatchedImports(installs, serverIds)
          .filter((install) => (source ? launcherSource(install.source) === source : true))
          .filter((install) => matchesQuery(install.title, install.title, query))
          .map((install) => ({
            kind: "import" as const,
            gameId: install.gameId,
            title: install.title,
            sortTitle: install.title,
            sizeBytes: install.sizeBytes,
            releaseDate: null,
            launcher: launcherSource(install.source),
            install,
          }))
          .sort((a, b) => compareImportEntries(a, b, sort, order))
      : [];

  const installedCount =
    server.filter((entry) => entry.install !== null).length + imports.length;

  return { server, imports, total: server.length + imports.length, installedCount };
}
