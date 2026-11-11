import { describe, expect, it } from "vitest";
import {
  buildLibraryModel,
  compareImportEntries,
  isIndexOnlyImport,
  launcherCounts,
  launcherLabel,
  launcherSource,
  launcherSourceOptions,
  unmatchedImports,
  type ImportLibraryEntry,
} from "./library-entries";
import type { GameSummary, InstalledGame } from "./types";

const summary = (id: number, title: string, extra: Partial<GameSummary> = {}): GameSummary => ({
  id,
  title,
  sortTitle: title,
  releaseDate: "2020-01-01",
  coverUrl: null,
  backgroundUrl: null,
  rating: 80,
  genres: ["Action"],
  earlyAccess: false,
  versionCount: 1,
  totalSize: 1024,
  state: "unplayed",
  minutesPlayed: 0,
  bookmarked: false,
  lastPlayedAt: null,
  maxPlayers: null,
  ...extra,
});

const installed = (
  gameId: number,
  title: string,
  extra: Partial<InstalledGame> = {},
): InstalledGame => ({
  gameId,
  versionId: 0,
  title,
  version: null,
  installPath: `/games/${title}`,
  executable: null,
  sizeBytes: 2048,
  installedAt: "2026-11-01T00:00:00Z",
  lastLaunchedAt: null,
  source: "steam",
  launchUri: `steam://rungameid/${Math.abs(gameId)}`,
  ...extra,
});

/** One matched Steam import + one unmatched Epic import. */
const games = [summary(1, "Alpha"), summary(2, "Beta")];
const installs = [
  installed(1, "Alpha"),
  installed(-7, "Gamma", {
    source: "epic",
    launchUri: "com.epicgames.launcher://apps/gamma",
    installPath: "C:/Epic/Gamma",
    sizeBytes: 4096,
    installedAt: "2026-11-05T00:00:00Z",
  }),
];

describe("launcher sources", () => {
  it("treats ludex and adopted rows as Ludex installs, not imports", () => {
    expect(launcherSource(undefined)).toBeNull();
    expect(launcherSource(null)).toBeNull();
    expect(launcherSource("ludex")).toBeNull();
    expect(launcherSource("adopted")).toBeNull();
  });

  it("labels known launchers and keeps unknown sources under Other", () => {
    expect(launcherSource("steam")).toBe("steam");
    expect(launcherSource("gog")).toBe("gog");
    expect(launcherSource("amazon")).toBe("other");
    expect(launcherLabel("steam")).toBe("Steam");
    expect(launcherLabel("itch")).toBe("itch.io");
    expect(launcherLabel("other")).toBe("Other");
    expect(launcherSourceOptions().map((option) => option.value)).toEqual([
      "steam",
      "epic",
      "gog",
      "ea",
      "ubisoft",
      "xbox",
      "itch",
      "heroic",
      "lutris",
      "other",
    ]);
  });

  it("counts imports per launcher for the source chips", () => {
    const counts = launcherCounts([
      ...installs,
      installed(3, "Delta", { source: "gog" }),
      installed(4, "Epsilon", { source: "ludex" }),
    ]);
    expect([...counts.entries()].sort()).toEqual([
      ["epic", 1],
      ["gog", 1],
      ["steam", 1],
    ]);
  });

  it("only lists imports missing from the server library", () => {
    expect(unmatchedImports(installs, new Set([1, 2])).map((entry) => entry.gameId)).toEqual([-7]);
    expect(isIndexOnlyImport(installs[1])).toBe(true);
    expect(isIndexOnlyImport(installed(2, "Beta", { source: "adopted" }))).toBe(false);
  });
});

describe("buildLibraryModel", () => {
  it("keeps matched imports on the server card and hides imports by default", () => {
    const model = buildLibraryModel(games, installs);
    expect(model.server.map((entry) => entry.gameId)).toEqual([1, 2]);
    expect(model.server[0].matchedImport?.source).toBe("steam");
    expect(model.server[0].launcher).toBe("steam");
    expect(model.server[1].matchedImport).toBeNull();
    expect(model.imports).toEqual([]);
    expect(model.total).toBe(2);
    expect(model.installedCount).toBe(1);
  });

  it("adds unmatched imports under Installed without duplicating a matched game", () => {
    const model = buildLibraryModel(games, installs, {
      installedOnly: true,
      includeUnmatched: true,
    });
    expect(model.server.map((entry) => entry.gameId)).toEqual([1]);
    expect(model.imports.map((entry) => entry.gameId)).toEqual([-7]);
    expect(model.total).toBe(2);
    expect(model.installedCount).toBe(2);
    expect(model.server.some((entry) => entry.gameId === -7)).toBe(false);
    expect(model.imports.some((entry) => entry.gameId === 1)).toBe(false);
  });

  it("filters both views by launcher source", () => {
    const epic = buildLibraryModel(games, installs, {
      source: "epic",
      includeUnmatched: true,
    });
    expect(epic.server).toEqual([]);
    expect(epic.imports.map((entry) => entry.gameId)).toEqual([-7]);

    const steam = buildLibraryModel(games, installs, {
      source: "steam",
      includeUnmatched: true,
    });
    expect(steam.server.map((entry) => entry.gameId)).toEqual([1]);
    expect(steam.imports).toEqual([]);

    expect(buildLibraryModel(games, installs, { source: "gog", includeUnmatched: true }).total).toBe(0);
  });

  it("includes imported titles in search", () => {
    const model = buildLibraryModel(games, installs, {
      query: "gam",
      includeUnmatched: true,
    });
    expect(model.server).toHaveLength(2);
    expect(model.imports.map((entry) => entry.title)).toEqual(["Gamma"]);
    expect(
      buildLibraryModel(games, installs, { query: "beta", includeUnmatched: true }).imports,
    ).toHaveLength(0);
  });

  it("sorts the imports group by title or added date", () => {
    const more = [
      installed(-9, "Zeta", { source: "gog", installedAt: "2026-10-01T00:00:00Z" }),
      installed(-7, "Gamma", { source: "epic", installedAt: "2026-11-05T00:00:00Z" }),
      installed(-8, "Delta", { source: "xbox", installedAt: "2026-11-03T00:00:00Z" }),
    ];
    const asc = buildLibraryModel(games, more, { includeUnmatched: true, sort: "title", order: "asc" });
    expect(asc.imports.map((entry) => entry.title)).toEqual(["Delta", "Gamma", "Zeta"]);
    const desc = buildLibraryModel(games, more, { includeUnmatched: true, sort: "title", order: "desc" });
    expect(desc.imports.map((entry) => entry.title)).toEqual(["Zeta", "Gamma", "Delta"]);
    const added = buildLibraryModel(games, more, { includeUnmatched: true, sort: "addedAt", order: "desc" });
    expect(added.imports.map((entry) => entry.title)).toEqual(["Gamma", "Delta", "Zeta"]);
  });

  it("drops an import as soon as its index row is gone, keeping the server card", () => {
    const afterRemove = installs.filter((entry) => entry.gameId !== -7);
    const model = buildLibraryModel(games, afterRemove, { installedOnly: true, includeUnmatched: true });
    expect(model.imports).toEqual([]);
    expect(model.installedCount).toBe(1);
    expect(model.server.map((entry) => entry.gameId)).toEqual([1]);
  });

  it("never treats a negative id as a server game", () => {
    const model = buildLibraryModel([], installs, { installedOnly: true });
    expect(model.server).toEqual([]);
    // Only negative ids are unmatched: a positive import is matched to a server
    // game and is never duplicated into the local imports group.
    expect(model.imports.map((entry) => entry.gameId)).toEqual([-7]);
    expect(model.imports.every((entry) => entry.kind === "import")).toBe(true);
  });

  it("keeps title order as the fallback for sorts imports cannot fill", () => {
    const entry = (title: string): ImportLibraryEntry => ({
      kind: "import",
      gameId: -1,
      title,
      sortTitle: title,
      sizeBytes: 1,
      releaseDate: null,
      launcher: "steam",
      install: installed(-1, title),
    });
    expect(compareImportEntries(entry("A"), entry("B"), "rating", "asc")).toBeLessThan(0);
    expect(compareImportEntries(entry("A"), entry("B"), "playtime", "desc")).toBeGreaterThan(0);
  });
});
