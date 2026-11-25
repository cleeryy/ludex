/**
 * Pure aggregation, filtering, and sorting helpers for the Storage Space
 * Manager in Settings → Storage (#349).
 */

import type { InstalledGame } from "@/lib/types";

export type StorageSourceBucket = "ludex" | "adopted" | "external";
export type StorageSortOrder = "sizeDesc" | "sizeAsc" | "title";
export type StorageSourceFilter = "all" | StorageSourceBucket;

export interface StorageBucketStats {
  count: number;
  bytes: number;
  percent: number;
}

export interface StorageSummary {
  totalBytes: number;
  count: number;
  bySource: Record<StorageSourceBucket, StorageBucketStats>;
  largest: InstalledGame | null;
}

export function storageSourceBucket(source: string | undefined | null): StorageSourceBucket {
  if (!source || source === "ludex") return "ludex";
  if (source === "adopted") return "adopted";
  return "external";
}

export function summarizeInstalledStorage(games: readonly InstalledGame[]): StorageSummary {
  const bytes: Record<StorageSourceBucket, number> = { ludex: 0, adopted: 0, external: 0 };
  const counts: Record<StorageSourceBucket, number> = { ludex: 0, adopted: 0, external: 0 };
  let totalBytes = 0;
  let largest: InstalledGame | null = null;

  for (const game of games) {
    const size = Math.max(0, game.sizeBytes || 0);
    const bucket = storageSourceBucket(game.source);
    bytes[bucket] += size;
    counts[bucket] += 1;
    totalBytes += size;
    if (!largest || size > (largest.sizeBytes || 0)) {
      largest = game;
    }
  }

  const pct = (value: number) => (totalBytes > 0 ? Math.round((value / totalBytes) * 1000) / 10 : 0);

  return {
    totalBytes,
    count: games.length,
    bySource: {
      ludex: { count: counts.ludex, bytes: bytes.ludex, percent: pct(bytes.ludex) },
      adopted: { count: counts.adopted, bytes: bytes.adopted, percent: pct(bytes.adopted) },
      external: { count: counts.external, bytes: bytes.external, percent: pct(bytes.external) },
    },
    largest,
  };
}

export function filterAndSortInstalled(
  games: readonly InstalledGame[],
  options: {
    query?: string;
    source?: StorageSourceFilter;
    sort?: StorageSortOrder;
  } = {},
): InstalledGame[] {
  const needle = options.query?.trim().toLowerCase() ?? "";
  const source = options.source ?? "all";
  const sort = options.sort ?? "sizeDesc";

  const filtered = games.filter((game) => {
    if (source !== "all" && storageSourceBucket(game.source) !== source) return false;
    if (!needle) return true;
    return (
      game.title.toLowerCase().includes(needle) ||
      (game.installPath ?? "").toLowerCase().includes(needle)
    );
  });

  return [...filtered].sort((a, b) => {
    if (sort === "sizeDesc") {
      const diff = (b.sizeBytes || 0) - (a.sizeBytes || 0);
      return diff !== 0 ? diff : a.title.localeCompare(b.title);
    }
    if (sort === "sizeAsc") {
      const diff = (a.sizeBytes || 0) - (b.sizeBytes || 0);
      return diff !== 0 ? diff : a.title.localeCompare(b.title);
    }
    return a.title.localeCompare(b.title);
  });
}
