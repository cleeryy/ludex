import { describe, expect, it } from "vitest";
import type { GameVersionInfo } from "@/lib/types";
import {
  canDeleteVersion,
  freedVersionBytes,
  isRollbackInstall,
  isRollbackVersion,
  preferredDiffersFromInstalled,
  preferredVersion,
  sortVersionsNewestFirst,
  versionHashShort,
  versionLabel,
  versionState,
} from "@/lib/game-versions";

function version(overrides: Partial<GameVersionInfo> & { id: number }): GameVersionInfo {
  return {
    fileName: `Game v${overrides.id}.zip`,
    size: 1000,
    version: String(overrides.id),
    releaseDate: `2026-0${Math.min(overrides.id, 9)}-01T00:00:00Z`,
    earlyAccess: false,
    archiveHash: null,
    updatedAt: null,
    downloadUrl: `/api/games/1/download?versionId=${overrides.id}`,
    indexedAt: null,
    missing: false,
    rootUnavailable: false,
    ...overrides,
  };
}

const newest = version({ id: 3 });
const middle = version({ id: 2 });
const oldest = version({ id: 1 });

describe("version labels and ordering", () => {
  it("labels parsed versions and falls back to the file name", () => {
    expect(versionLabel(newest)).toBe("v3");
    expect(versionLabel({ version: null, fileName: "Mystery.rar" })).toBe("Mystery.rar");
    expect(versionLabel({ version: "  ", fileName: "Mystery.rar" })).toBe("Mystery.rar");
  });

  it("sorts newest first by release date then id", () => {
    expect(sortVersionsNewestFirst([oldest, newest, middle]).map((entry) => entry.id)).toEqual([3, 2, 1]);

    const undated = [
      version({ id: 1, releaseDate: null }),
      version({ id: 2, releaseDate: null }),
    ];
    expect(sortVersionsNewestFirst(undated).map((entry) => entry.id)).toEqual([2, 1]);

    const byIndex = [
      version({ id: 1, releaseDate: null, indexedAt: "2026-02-01T00:00:00Z" }),
      version({ id: 2, releaseDate: null, indexedAt: "2026-05-01T00:00:00Z" }),
    ];
    expect(sortVersionsNewestFirst(byIndex).map((entry) => entry.id)).toEqual([2, 1]);
  });

  it("uses the pin, else the newest available version", () => {
    expect(preferredVersion([oldest, newest, middle], 2)?.id).toBe(2);
    expect(preferredVersion([oldest, newest, middle], null)?.id).toBe(3);
    expect(preferredVersion([oldest, newest, middle], 99)?.id).toBe(3);
    expect(preferredVersion([], 1)).toBeNull();

    // The server falls back to the newest available version when the pinned
    // archive is missing or its root is offline.
    const missingPin = version({ id: 4, missing: true });
    expect(preferredVersion([missingPin, newest], 4)?.id).toBe(3);
    const offlinePin = version({ id: 5, rootUnavailable: true });
    expect(preferredVersion([offlinePin, newest], 5)?.id).toBe(3);
  });
});

describe("version state", () => {
  it("marks the default, available, missing and offline rows", () => {
    expect(versionState(newest, 3)).toBe("default");
    expect(versionState(newest, null)).toBe("available");
    expect(versionState(version({ id: 4, missing: true }), 4)).toBe("missing");
    expect(versionState(version({ id: 5, rootUnavailable: true }), 5)).toBe("unavailable");
  });

  it("shortens fingerprints without losing the digest prefix", () => {
    expect(versionHashShort("sha256-sample:abcdef0123456789")).toBe("abcdef012345");
    expect(versionHashShort("abcdef0123456789")).toBe("abcdef012345");
    expect(versionHashShort(null)).toBeNull();
    expect(versionHashShort("sha256-sample:")).toBeNull();
  });
});

describe("rollback decisions", () => {
  it("treats a lower row as a rollback and ignores unknown rows", () => {
    expect(isRollbackVersion(oldest, newest, [newest, middle, oldest])).toBe(true);
    expect(isRollbackVersion(newest, oldest, [newest, middle, oldest])).toBe(false);
    expect(isRollbackVersion(oldest, oldest, [newest, middle, oldest])).toBe(false);
    expect(isRollbackVersion({ id: 99 }, newest, [newest, middle, oldest])).toBe(false);
    expect(isRollbackVersion(null, newest, [newest, middle, oldest])).toBe(false);
  });

  it("warns when installing an older version over a newer install", () => {
    expect(isRollbackInstall(oldest.id, newest.id, [newest, middle, oldest])).toBe(true);
    expect(isRollbackInstall(newest.id, oldest.id, [newest, middle, oldest])).toBe(false);
    expect(isRollbackInstall(oldest.id, null, [newest, middle, oldest])).toBe(false);
    expect(isRollbackInstall(oldest.id, 99, [newest, middle, oldest])).toBe(false);
  });

  it("detects when the preferred version differs from the installed one", () => {
    expect(preferredDiffersFromInstalled(1, 3)).toBe(true);
    expect(preferredDiffersFromInstalled(3, 3)).toBe(false);
    expect(preferredDiffersFromInstalled(null, 3)).toBe(false);
    expect(preferredDiffersFromInstalled(1, null)).toBe(false);
  });
});

describe("delete eligibility", () => {
  it("refuses missing, offline and last available versions", () => {
    expect(canDeleteVersion([newest, middle, oldest], oldest)).toEqual({ allowed: true, reason: null });
    expect(canDeleteVersion([newest, middle, oldest], version({ id: 4, missing: true }))).toEqual({
      allowed: false,
      reason: "missing",
    });
    expect(
      canDeleteVersion([newest, middle, oldest], version({ id: 4, rootUnavailable: true })),
    ).toEqual({ allowed: false, reason: "unavailable" });
    expect(canDeleteVersion([newest], newest)).toEqual({ allowed: false, reason: "last" });
    expect(canDeleteVersion([newest, oldest], oldest)).toEqual({ allowed: true, reason: null });
  });

  it("counts a missing row as unavailable capacity", () => {
    const missing = version({ id: 2, missing: true });
    expect(canDeleteVersion([newest, missing], newest)).toEqual({ allowed: false, reason: "last" });
  });

  it("reports the freed bytes", () => {
    expect(freedVersionBytes({ size: 2048 })).toBe(2048);
    expect(freedVersionBytes({ size: -1 })).toBe(0);
  });
});
