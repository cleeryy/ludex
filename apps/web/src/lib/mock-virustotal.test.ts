import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The demo adapter reads `localStorage` at import time, so the stub has to be
 * hoisted above the import. Everything else runs against the real mock API.
 */
const storage = vi.hoisted(() => {
  const store = new Map<string, string>();
  const api = {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => {
      store.set(key, String(value));
    },
    removeItem: (key: string) => {
      store.delete(key);
    },
    clear: () => store.clear(),
    key: (index: number) => [...store.keys()][index] ?? null,
    get length() {
      return store.size;
    },
  };
  (globalThis as unknown as { localStorage: typeof api }).localStorage = api;
  return { api, store };
});

import { mockApi } from "./mock";
import type { VirusTotalVerdict } from "./types";

/** `mara` is the demo administrator. */
const ADMIN = "mock.1.1700000000";

const SCENARIO_STORAGE = "ludex.demo.virustotal";

function setScenario(value: string | null) {
  if (value === null) storage.api.removeItem(SCENARIO_STORAGE);
  else storage.api.setItem(SCENARIO_STORAGE, value);
}

beforeEach(() => setScenario(null));

describe("mock VirusTotal scenarios", () => {
  it("reports a stable mix of verdicts by default", async () => {
    const { versions } = await mockApi.gameVirusTotal(ADMIN, 1);
    expect(versions.length).toBeGreaterThan(0);
    const allowed: VirusTotalVerdict[] = ["clean", "suspicious", "malicious", "not_found", "unknown"];
    for (const report of versions) {
      expect(allowed).toContain(report.verdict);
      expect(report.sha256).toMatch(/^[0-9a-f]{64}$/);
      expect(report.checkedAt).toBeTruthy();
    }
    // Same input, same output: the demo is deterministic between reloads.
    const again = await mockApi.gameVirusTotal(ADMIN, 1);
    expect(again.versions.map((entry) => entry.verdict)).toEqual(versions.map((entry) => entry.verdict));
  });

  it("gives every archive the chosen verdict", async () => {
    for (const scenario of ["clean", "suspicious", "malicious", "not_found", "quota_exceeded", "error"] as const) {
      setScenario(scenario);
      const { versions } = await mockApi.gameVirusTotal(ADMIN, 1);
      expect(versions.every((report) => report.verdict === scenario)).toBe(true);
      const first = versions[0];
      if (scenario === "clean" || scenario === "malicious" || scenario === "suspicious") {
        expect(first.permalink).toContain("virustotal.com");
        expect(first.detections?.length ?? 0).toBeGreaterThanOrEqual(scenario === "clean" ? 0 : 1);
      } else {
        expect(first.permalink).toBeNull();
        expect(first.reason).toBeTruthy();
      }
    }
  });

  it("reports the feature as disabled without a key", async () => {
    setScenario("disabled");
    const integration = await mockApi.adminVirusTotal(ADMIN);
    expect(integration).toMatchObject({ configured: false, source: "none", fingerprint: null });
    await expect(mockApi.scanVirusTotal(ADMIN, { gameId: 1 })).rejects.toMatchObject({ status: 409 });
  });

  it("shows an exhausted quota without a fake clean", async () => {
    setScenario("quota_exceeded");
    const integration = await mockApi.adminVirusTotal(ADMIN);
    expect(integration.quota).toMatchObject({ dailyLimit: 500, usedToday: 500, remaining: 0 });
    expect(integration.lastError).toContain("429");
    const { scans } = await mockApi.scanVirusTotal(ADMIN, { versionId: 11 });
    expect(scans[0].verdict).toBe("quota_exceeded");
    expect(scans[0].reason).toContain("QuotaExceeded");
  });

  it("derives the distinct admin errors", async () => {
    setScenario("quota_exceeded");
    expect((await mockApi.adminVirusTotal(ADMIN)).lastError).toContain("Quota");
    setScenario("error");
    expect((await mockApi.adminVirusTotal(ADMIN)).lastError).toContain("503");
    setScenario("unknown");
    expect((await mockApi.adminVirusTotal(ADMIN)).lastError).toContain("401");
  });

  it("marks a fresh scan as an API lookup", async () => {
    setScenario("clean");
    const { scans } = await mockApi.scanVirusTotal(ADMIN, { versionId: 11, force: true });
    expect(scans[0]).toMatchObject({ versionId: 11, verdict: "clean", source: "api" });
    const integration = await mockApi.adminVirusTotal(ADMIN);
    expect(integration.lastUsedAt).not.toBeNull();
  });

  it("requires consent, uploads a small archive and refuses a big one", async () => {
    setScenario("clean");
    // Two archives from the demo library: one under 32 MB, one repack above
    // the 650 MB upload limit.
    let small = 0;
    let big = 0;
    for (let id = 1; id <= 30 && (!small || !big); id += 1) {
      const game = await mockApi.getGame(id);
      for (const version of game.versions) {
        if (!small && version.size <= 32 * 1024 * 1024) small = version.id;
        if (!big && version.size > 650 * 1024 * 1024) big = version.id;
      }
    }
    expect(small).toBeGreaterThan(0);
    expect(big).toBeGreaterThan(0);

    await expect(
      mockApi.uploadVirusTotalVersion(ADMIN, { versionId: small, iUnderstand: "no" as "public-sharing" }),
    ).rejects.toMatchObject({ status: 422 });

    const uploaded = await mockApi.uploadVirusTotalVersion(ADMIN, {
      versionId: small,
      iUnderstand: "public-sharing",
    });
    expect(uploaded.scans[0]).toMatchObject({ versionId: small, source: "api" });

    await expect(
      mockApi.uploadVirusTotalVersion(ADMIN, { versionId: big, iUnderstand: "public-sharing" }),
    ).rejects.toMatchObject({ status: 413 });
  });

  it("tests, saves and deletes a key", async () => {
    const rejected = await mockApi.testVirusTotalKey(ADMIN, { apiKey: "invalid-key-value-here" });
    expect(rejected.ok).toBe(false);
    const accepted = await mockApi.testVirusTotalKey(ADMIN, { apiKey: "a".repeat(64) });
    expect(accepted.ok).toBe(true);

    const saved = await mockApi.saveVirusTotalKey(ADMIN, { apiKey: `${"b".repeat(60)}9f3c` });
    expect(saved).toMatchObject({ configured: true, source: "database", fingerprint: "...9f3c" });

    const cleared = await mockApi.deleteVirusTotalKey(ADMIN);
    expect(cleared.configured).toBe(false);
    // Restore the demo key so later tests see a configured adapter.
    await mockApi.saveVirusTotalKey(ADMIN, { apiKey: "demo-virustotal-key-4f2a" });
  });

  it("keeps a default-off re-check policy and validates the interval", async () => {
    const initial = await mockApi.adminVirusTotalRescan(ADMIN);
    expect(initial).toMatchObject({ enabled: false, intervalHours: 168, nextRunAt: null, due: false });

    await expect(
      mockApi.saveVirusTotalRescan(ADMIN, { enabled: true, intervalHours: 12, includeMalicious: false }),
    ).rejects.toMatchObject({ status: 400 });

    const enabled = await mockApi.saveVirusTotalRescan(ADMIN, {
      enabled: true,
      intervalHours: 24,
      includeMalicious: true,
    });
    expect(enabled.enabled).toBe(true);
    expect(enabled.includeMalicious).toBe(true);
    expect(enabled.nextRunAt).toBeTruthy();

    const ran = await mockApi.runVirusTotalRescan(ADMIN);
    expect(ran.lastRunAt).toBeTruthy();
    expect(ran.remaining).toBe(0);
    expect(ran.scannedLastRun).toBeGreaterThan(0);

    await mockApi.saveVirusTotalRescan(ADMIN, { enabled: false, intervalHours: 24, includeMalicious: false });
  });

  it("only lets an administrator in", async () => {
    // `ines` is a plain member.
    await expect(mockApi.adminVirusTotal("mock.3.1700000000")).rejects.toMatchObject({ status: 403 });
    await expect(mockApi.scanVirusTotal("mock.3.1700000000", { gameId: 1 })).rejects.toMatchObject({ status: 403 });
  });
});
