import { describe, expect, it } from "vitest";
import type { ScanReport, VirusTotalStats } from "./types";
import {
  VERDICT_SEVERITY,
  VT_UPLOAD_DIRECT_LIMIT,
  VT_UPLOAD_URL_LIMIT,
  canScanNow,
  classifyIntegrationError,
  detectionRatio,
  engineTotal,
  isClean,
  isUnscanned,
  quotaUsedPercent,
  rescanIntervalLabel,
  rescanIntervalOptions,
  rescanSpanDays,
  rescanSpansMultipleDays,
  scanQueueEtaLabel,
  scanQueueSchedule,
  summaryFor,
  topDetections,
  uploadEligibility,
  uploadTransportLabel,
  verdictBadgeLabel,
  verdictExplanation,
  verdictLabel,
  verdictTone,
  worstVerdict,
} from "./virustotal";

function stats(patch: Partial<VirusTotalStats> = {}): VirusTotalStats {
  return { malicious: 0, suspicious: 0, undetected: 5, harmless: 67, timeout: 0, failure: 0, ...patch };
}

function report(patch: Partial<ScanReport> = {}): ScanReport {
  return {
    versionId: 1,
    sha256: "a".repeat(64),
    verdict: "clean",
    stats: stats(),
    checkedAt: "2026-11-14T10:00:00Z",
    source: "cache",
    permalink: "https://www.virustotal.com/gui/file/aaa",
    ...patch,
  };
}

describe("verdict → badge mapping", () => {
  it("counts engines that answered", () => {
    expect(engineTotal(stats())).toBe(72);
    expect(engineTotal(stats({ malicious: 7, suspicious: 2, undetected: 12, harmless: 51 }))).toBe(72);
    expect(engineTotal(null)).toBe(0);
    expect(detectionRatio(stats())).toBe("0/72");
    expect(detectionRatio(stats({ malicious: 7, undetected: 12, harmless: 53 }))).toBe("7/72");
    expect(detectionRatio(stats({ suspicious: 3, undetected: 9, harmless: 60 }))).toBe("3/72");
    expect(detectionRatio({ malicious: 0, suspicious: 0, undetected: 0, harmless: 0, timeout: 0, failure: 0 })).toBe("—");
  });

  it("maps every verdict to its tone and label", () => {
    expect(verdictTone("clean")).toBe("ok");
    expect(verdictTone("suspicious")).toBe("signal");
    expect(verdictTone("malicious")).toBe("danger");
    for (const verdict of ["not_found", "unknown", "disabled", "quota_exceeded", "error"] as const) {
      expect(verdictTone(verdict)).toBe("outline");
    }
    expect(verdictLabel("not_found")).toBe("Never seen");
    expect(verdictLabel("unknown")).toBe("Not checked");
    expect(verdictLabel("disabled")).toBe("Disabled");
    expect(verdictLabel("quota_exceeded")).toBe("Quota reached");
    expect(verdictLabel("error")).toBe("Check failed");
  });

  it("shows the ratio only when engines answered", () => {
    expect(verdictBadgeLabel(report())).toBe("0/72");
    expect(
      verdictBadgeLabel(
        report({ verdict: "malicious", stats: stats({ malicious: 7, suspicious: 2, undetected: 12, harmless: 51 }) }),
      ),
    ).toBe("9/72");
    expect(verdictBadgeLabel(report({ verdict: "not_found", stats: stats({ undetected: 0, harmless: 0 }) }))).toBe("Never seen");
  });

  it("never renders unknown or not_found as clean", () => {
    expect(isClean("clean")).toBe(true);
    for (const verdict of ["unknown", "not_found", "disabled", "quota_exceeded", "error", "suspicious", "malicious"] as const) {
      expect(isClean(verdict)).toBe(false);
      expect(summaryFor(report({ verdict }))?.tone).not.toBe("ok");
      expect(summaryFor(report({ verdict }))?.label).not.toBe(verdictLabel("clean"));
    }
    expect(verdictExplanation(report({ verdict: "not_found" }))).toContain("not the same as safe");
  });

  it("explains each state in one honest line", () => {
    expect(verdictExplanation(null)).toBe("No archive has been checked yet.");
    expect(verdictExplanation(report({ verdict: "clean" }))).toContain("not a guarantee");
    expect(verdictExplanation(report({ verdict: "disabled" }))).toContain("not configured");
    expect(verdictExplanation(report({ verdict: "quota_exceeded" }))).toContain("quota");
    expect(verdictExplanation(report({ verdict: "error" }))).toContain("says nothing about the file");
  });

  it("summary carries badge, label and explanation together", () => {
    expect(summaryFor(undefined)).toBeNull();
    const summary = summaryFor(report({ verdict: "malicious", stats: stats({ malicious: 7, undetected: 12, harmless: 53 }) }));
    expect(summary).toMatchObject({ verdict: "malicious", tone: "danger", badge: "7/72", label: "Malicious" });
  });
});

describe("worst verdict for a game", () => {
  it("returns null with no report", () => {
    expect(worstVerdict([])).toBeNull();
    expect(worstVerdict(null)).toBeNull();
  });

  it("ranks malicious above suspicious above not_found above clean", () => {
    expect(worstVerdict([report({ verdict: "clean" }), report({ verdict: "malicious" })])).toBe("malicious");
    expect(worstVerdict([report({ verdict: "clean" }), report({ verdict: "suspicious" })])).toBe("suspicious");
    expect(worstVerdict([report({ verdict: "clean" }), report({ verdict: "not_found" })])).toBe("not_found");
    expect(worstVerdict([report({ verdict: "clean" })])).toBe("clean");
    expect(VERDICT_SEVERITY.malicious).toBeGreaterThan(VERDICT_SEVERITY.suspicious);
  });
});

describe("popover detail", () => {
  it("lists the most severe engines first and caps the list", () => {
    const hits = topDetections({
      detections: [
        { engine: "A", category: "harmless", result: null, version: null },
        { engine: "B", category: "suspicious", result: "heur", version: null },
        { engine: "C", category: "malicious", result: "trojan", version: null },
        { engine: "D", category: "malicious", result: "dropper", version: null },
        { engine: "E", category: "malicious", result: "miner", version: null },
      ],
    });
    expect(hits.map((hit) => hit.engine)).toEqual(["C", "D", "E", "B"]);
    expect(topDetections({ detections: [] })).toEqual([]);
    expect(topDetections(null)).toEqual([]);
  });

  it("only offers Scan now to admins, and not while disabled", () => {
    const now = 1_000_000;
    expect(canScanNow(report(), { admin: false, now })).toBe(false);
    expect(canScanNow(report(), { admin: true, now })).toBe(true);
    expect(canScanNow(report({ verdict: "disabled" }), { admin: true, now })).toBe(false);
    expect(canScanNow(null, { admin: true, now })).toBe(false);
    expect(canScanNow(report(), { admin: true, now, lastScanAt: now - 1_000 })).toBe(false);
    expect(canScanNow(report(), { admin: true, now, lastScanAt: now - 20_000 })).toBe(true);
  });
});

describe("unscanned rows", () => {
  it("treats only non-answers as unscanned", () => {
    expect(isUnscanned(null)).toBe(true);
    expect(isUnscanned({ verdict: "unknown" })).toBe(true);
    expect(isUnscanned({ verdict: "error" })).toBe(true);
    expect(isUnscanned({ verdict: "quota_exceeded" })).toBe(true);
    // A real answer stays out of the bulk queue: re-asking burns quota.
    expect(isUnscanned({ verdict: "not_found" })).toBe(false);
    expect(isUnscanned({ verdict: "clean" })).toBe(false);
    expect(isUnscanned({ verdict: "malicious" })).toBe(false);
  });
});

describe("upload gate", () => {
  const MB = 1024 * 1024;

  it("stays closed without consent", () => {
    const gate = uploadEligibility(20 * MB, { admin: true, consent: false });
    expect(gate.allowed).toBe(false);
    expect(gate.possible).toBe(true);
    expect(gate.transport).toBe("direct");
    expect(gate.reason).toContain("Tick");
  });

  it("stays closed for non-admins", () => {
    const gate = uploadEligibility(20 * MB, { admin: false, consent: true });
    expect(gate.allowed).toBe(false);
    expect(gate.reason).toContain("administrator");
  });

  it("picks direct below 32 MB and the upload URL up to 650 MB", () => {
    expect(uploadEligibility(VT_UPLOAD_DIRECT_LIMIT, { admin: true, consent: true })).toMatchObject({
      allowed: true,
      transport: "direct",
    });
    expect(uploadEligibility(VT_UPLOAD_DIRECT_LIMIT + 1, { admin: true, consent: true })).toMatchObject({
      allowed: true,
      transport: "upload_url",
    });
    expect(uploadEligibility(VT_UPLOAD_URL_LIMIT, { admin: true, consent: true })).toMatchObject({
      allowed: true,
      transport: "upload_url",
    });
  });

  it("refuses a repack bigger than the 650 MB limit", () => {
    const gate = uploadEligibility(40 * 1024 * MB, { admin: true, consent: true });
    expect(gate.allowed).toBe(false);
    expect(gate.possible).toBe(false);
    expect(gate.transport).toBeNull();
    expect(gate.reason).toContain("650 MB");
    expect(uploadTransportLabel(gate.transport)).toBeNull();
  });

  it("refuses an archive with no known size", () => {
    expect(uploadEligibility(0, { admin: true, consent: true }).allowed).toBe(false);
  });
});

describe("scan queue budget", () => {
  it("spaces scans one per 15 s at 4 per minute", () => {
    const schedule = scanQueueSchedule(8);
    expect(schedule.slots).toHaveLength(8);
    expect(schedule.slots[0].startsAtMs).toBe(0);
    expect(schedule.slots[1].startsAtMs).toBe(15_000);
    expect(schedule.slots[7].startsAtMs).toBe(105_000);
    expect(schedule.etaSeconds).toBe(105);
    expect(schedule.drainSeconds).toBe(120);
  });

  it("has no ETA for an empty queue", () => {
    expect(scanQueueSchedule(0)).toEqual({ slots: [], etaSeconds: 0, drainSeconds: 0 });
    expect(scanQueueEtaLabel(0)).toBeNull();
  });

  it("labels the queue with the rate and the wait", () => {
    expect(scanQueueEtaLabel(1)).toContain("4 requests per minute");
    const label = scanQueueEtaLabel(8) ?? "";
    expect(label).toContain("8");
    expect(label).toContain("1m 45s");
  });
});

describe("integration errors", () => {
  it("distinguishes an invalid key, the quota and an outage", () => {
    expect(classifyIntegrationError("401 Wrong API key")).toBe("invalid_key");
    expect(classifyIntegrationError("429 QuotaExceededError")).toBe("quota");
    expect(classifyIntegrationError("503 Service Unavailable")).toBe("outage");
    expect(classifyIntegrationError("socket hang up")).toBe("other");
    expect(classifyIntegrationError(null)).toBeNull();
    expect(classifyIntegrationError("  ")).toBeNull();
  });

  it("clamps quota usage", () => {
    expect(quotaUsedPercent({ dailyLimit: 500, usedToday: 137, remaining: 363, resetAt: "" })).toBeCloseTo(27.4, 1);
    expect(quotaUsedPercent({ dailyLimit: 500, usedToday: 900, remaining: 0, resetAt: "" })).toBe(100);
    expect(quotaUsedPercent(null)).toBe(0);
  });
});

describe("periodic re-check policy (#239)", () => {
  it("labels every offered interval and nothing else", () => {
    expect(rescanIntervalLabel(0)).toBe("Manual only");
    expect(rescanIntervalLabel(24)).toBe("Daily");
    expect(rescanIntervalLabel(168)).toBe("Weekly");
    expect(rescanIntervalLabel(720)).toBe("Monthly");
    expect(rescanIntervalLabel(48)).toBe("Custom");
    expect(rescanIntervalOptions().map((option) => option.value)).toEqual([0, 24, 168, 720]);
  });

  it("says how many days a library needs at 500 requests per day", () => {
    expect(rescanSpanDays(0)).toBe(0);
    expect(rescanSpanDays(500)).toBe(1);
    expect(rescanSpanDays(501)).toBe(2);
    expect(rescanSpanDays(1_200)).toBe(3);
    expect(rescanSpansMultipleDays(500)).toBe(false);
    expect(rescanSpansMultipleDays(501)).toBe(true);
    expect(rescanSpanDays(120, 0)).toBe(0);
  });
});
