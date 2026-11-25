import { describe, expect, it } from "vitest";
import type { InstalledGame } from "@/lib/types";
import {
  filterAndSortInstalled,
  storageSourceBucket,
  summarizeInstalledStorage,
} from "./storage-summary";

function entry(partial: Partial<InstalledGame> & { gameId: number; title: string; sizeBytes: number }): InstalledGame {
  return {
    gameId: partial.gameId,
    title: partial.title,
    versionId: 1,
    version: "1.0",
    installPath: partial.installPath ?? `/games/${partial.title}`,
    executable: null,
    installedAt: "2026-11-20T00:00:00Z",
    lastLaunchedAt: null,
    sizeBytes: partial.sizeBytes,
    source: partial.source ?? "ludex",
  };
}

describe("storage summary", () => {
  const sample: InstalledGame[] = [
    entry({ gameId: 1, title: "Cyberpunk", sizeBytes: 70_000, source: "ludex" }),
    entry({ gameId: 2, title: "Balatro", sizeBytes: 10_000, source: "adopted" }),
    entry({ gameId: 3, title: "Hades", sizeBytes: 20_000, source: "steam" }),
  ];

  it("buckets sources into ludex, adopted, and external", () => {
    expect(storageSourceBucket(undefined)).toBe("ludex");
    expect(storageSourceBucket("ludex")).toBe("ludex");
    expect(storageSourceBucket("adopted")).toBe("adopted");
    expect(storageSourceBucket("steam")).toBe("external");
    expect(storageSourceBucket("heroic")).toBe("external");
  });

  it("computes total bytes, source breakdown percentages, and largest install", () => {
    const summary = summarizeInstalledStorage(sample);
    expect(summary.totalBytes).toBe(100_000);
    expect(summary.count).toBe(3);
    expect(summary.bySource.ludex).toEqual({ count: 1, bytes: 70_000, percent: 70 });
    expect(summary.bySource.adopted).toEqual({ count: 1, bytes: 10_000, percent: 10 });
    expect(summary.bySource.external).toEqual({ count: 1, bytes: 20_000, percent: 20 });
    expect(summary.largest?.title).toBe("Cyberpunk");
  });

  it("filters by query or source and sorts by size or title", () => {
    expect(filterAndSortInstalled(sample, { sort: "sizeDesc" }).map((item) => item.title)).toEqual([
      "Cyberpunk",
      "Hades",
      "Balatro",
    ]);
    expect(filterAndSortInstalled(sample, { sort: "sizeAsc" }).map((item) => item.title)).toEqual([
      "Balatro",
      "Hades",
      "Cyberpunk",
    ]);
    expect(filterAndSortInstalled(sample, { sort: "title" }).map((item) => item.title)).toEqual([
      "Balatro",
      "Cyberpunk",
      "Hades",
    ]);
    expect(filterAndSortInstalled(sample, { source: "external" }).map((item) => item.title)).toEqual(["Hades"]);
    expect(filterAndSortInstalled(sample, { query: "bal" }).map((item) => item.title)).toEqual(["Balatro"]);
  });
});
