import { describe, expect, it } from "vitest";
import { qk } from "@/lib/query-keys";

/**
 * The factory must not rename a cache entry: these are the arrays the call
 * sites published before #237, and a changed shape would silently orphan the
 * cached data an update path still matches by prefix.
 */
describe("query keys", () => {
  it("keeps singleton keys", () => {
    expect(qk.installed).toEqual(["installed"]);
    expect(qk.gameUpdates).toEqual(["game-updates"]);
    expect(qk.stats).toEqual(["stats"]);
    expect(qk.platforms).toEqual(["platforms"]);
  });

  it("keeps the library variants apart", () => {
    expect(qk.games.root).toEqual(["games"]);
    expect(qk.games.all()).toEqual(["games", "all"]);
    expect(qk.games.pages()).toEqual(["games", "all", "pages"]);
    expect(qk.games.list("recent", { sort: "lastPlayed" })).toEqual(["games", "recent", { sort: "lastPlayed" }]);
    expect(qk.games.missingCount).toEqual(["games-missing-count"]);
  });

  it("keeps per-game keys, including an absent id", () => {
    expect(qk.game.detail(7)).toEqual(["game", 7]);
    expect(qk.game.detail(undefined)).toEqual(["game", undefined]);
    expect(qk.playtime.forGame(7)).toEqual(["playtime", 7]);
    expect(qk.gameSession.forGame(7)).toEqual(["game-session", 7]);
    expect(qk.saves.forGame(7)).toEqual(["saves", 7]);
  });

  it("keeps the collection editor split", () => {
    expect(qk.collections.detail(3)).toEqual(["collection", 3]);
    expect(qk.collections.games(3)).toEqual(["collection-games", 3]);
    expect(qk.collections.editorGames(3)).toEqual(["collection", 3, "games"]);
    expect(qk.collectionShares.forCollection(3)).toEqual(["collection-shares", 3]);
  });

  it("keeps the multi-argument keys", () => {
    expect(qk.metadata.candidates(2, "halo", "2001", null)).toEqual([
      "metadata-candidates",
      2,
      "halo",
      "2001",
      "*",
    ]);
    expect(qk.metadata.candidates(2, "halo", "", "igdb")).toEqual(["metadata-candidates", 2, "halo", "", "igdb"]);
    expect(qk.libraryHealth.list("error", "archive", 50)).toEqual(["library-health", "error", "archive", 50]);
    expect(qk.admin.backups.restorePreview("backup-1")).toEqual([
      "admin",
      "backups",
      "backup-1",
      "restore-preview",
    ]);
    expect(qk.desktopDownload(null)).toEqual(["desktop-download", "all"]);
    expect(qk.desktopDownload("windows")).toEqual(["desktop-download", "windows"]);
    expect(qk.recentGames.byLimit(6)).toEqual(["recent-games", 6]);
    expect(qk.sunshine.preview("/apps")).toEqual(["sunshine-preview", "/apps"]);
    expect(qk.sunshine.previewRoot).toEqual(["sunshine-preview"]);
    expect(qk.observability("7d")).toEqual(["observability", "7d"]);
  });

  it("keeps the invalidation prefixes exact", () => {
    // `invalidateQueries({ queryKey: qk.games.root })` must still match every
    // list variant, and the palette copy is invalidated by its own prefix.
    for (const key of [qk.games.all(), qk.games.pages(), qk.games.list("library", {})]) {
      expect(key.slice(0, 1)).toEqual(["games"]);
    }
    expect(qk.collections.root).toEqual(["collections"]);
    expect(qk.admin.games.root).toEqual(["admin-games"]);
    expect(qk.admin.games.list({ page: 1 })).toEqual(["admin-games", { page: 1 }]);
    expect(qk.admin.backups.root).toEqual(["admin", "backups"]);
    expect(qk.notifications.bell()).toEqual(["notifications", "bell"]);
  });
});
