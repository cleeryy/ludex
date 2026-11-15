/**
 * VirusTotal presentation rules (#224).
 *
 * The server owns the verdict; this module owns the words and the tones that
 * go with it, because the one thing the UI must never do is turn `unknown`
 * into a green badge. Every mapping, the upload gate and the scan queue budget
 * live here so they can be tested without rendering.
 */

import { t } from "@/lib/i18n";
import { formatDuration } from "@/lib/format";
import type {
  ScanReport,
  VirusTotalDetection,
  VirusTotalQuota,
  VirusTotalStats,
  VirusTotalVerdict,
} from "@/lib/types";

/** Public API facts that shape the UI (issue #224). */
export const VIRUSTOTAL_DAILY_LIMIT = 500;
export const VIRUSTOTAL_RATE_PER_MINUTE = 4;
/** Direct `POST /files` limit. */
export const VT_UPLOAD_DIRECT_LIMIT = 32 * 1024 * 1024;
/** `GET /files/upload_url` limit; bigger archives can never be uploaded. */
export const VT_UPLOAD_URL_LIMIT = 650 * 1024 * 1024;

export type VerdictTone = "ok" | "signal" | "danger" | "outline";

/** Severity used to pick the worst verdict of a game's versions. */
export const VERDICT_SEVERITY: Record<VirusTotalVerdict, number> = {
  malicious: 7,
  suspicious: 6,
  quota_exceeded: 5,
  error: 4,
  unknown: 3,
  not_found: 2,
  clean: 1,
  disabled: 0,
};

/** Engines that actually answered: the denominator of the "0/72" badge. */
export function engineTotal(stats: VirusTotalStats | null | undefined): number {
  if (!stats) return 0;
  return (
    stats.malicious + stats.suspicious + stats.undetected + stats.harmless + stats.timeout + stats.failure
  );
}

/** `0/72` style ratio: detections over engines that answered. */
export function detectionRatio(stats: VirusTotalStats | null | undefined): string {
  const total = engineTotal(stats);
  const hits = (stats?.malicious ?? 0) + (stats?.suspicious ?? 0);
  if (total === 0) return "—";
  return `${hits}/${total}`;
}

export function verdictTone(verdict: VirusTotalVerdict): VerdictTone {
  switch (verdict) {
    case "clean":
      return "ok";
    case "suspicious":
      return "signal";
    case "malicious":
      return "danger";
    default:
      return "outline";
  }
}

/** Long label: the answer to "what did VirusTotal say?". */
export function verdictLabel(verdict: VirusTotalVerdict): string {
  switch (verdict) {
    case "clean":
      return t("Clean");
    case "suspicious":
      return t("Suspicious");
    case "malicious":
      return t("Malicious");
    case "not_found":
      return t("Never seen");
    case "unknown":
      return t("Not checked");
    case "disabled":
      return t("Disabled");
    case "quota_exceeded":
      return t("Quota reached");
    case "error":
      return t("Check failed");
  }
}

/**
 * Short badge text. A number when engines answered, otherwise the state — and
 * never a word that reads as "safe" for `not_found`/`unknown`.
 */
export function verdictBadgeLabel(report: Pick<ScanReport, "verdict" | "stats">): string {
  if (report.verdict === "clean" || report.verdict === "suspicious" || report.verdict === "malicious") {
    return detectionRatio(report.stats);
  }
  return verdictLabel(report.verdict);
}

/** One honest sentence per state; the copy is the contract with the user. */
export function verdictExplanation(report: Pick<ScanReport, "verdict"> | null | undefined): string {
  if (!report) return t("No archive has been checked yet.");
  switch (report.verdict) {
    case "clean":
      return t("No engine flagged this file. That is a signal, not a guarantee.");
    case "suspicious":
      return t("Some engines flagged this file as suspicious.");
    case "malicious":
      return t("Engines detected malware in this file.");
    case "not_found":
      return t("VirusTotal does not know this file. That is not the same as safe.");
    case "unknown":
      return t("The hash was not checked, so there is no verdict.");
    case "disabled":
      return t("VirusTotal is not configured on this server.");
    case "quota_exceeded":
      return t("The daily VirusTotal quota is used up. The next scan resumes after the reset.");
    case "error":
      return t("The lookup failed. This says nothing about the file either way.");
  }
}

export interface VerdictSummary {
  verdict: VirusTotalVerdict;
  tone: VerdictTone;
  /** Short badge text, e.g. `0/72`, `Never seen`. */
  badge: string;
  label: string;
  explanation: string;
}

export function summaryFor(report: Pick<ScanReport, "verdict" | "stats"> | null | undefined): VerdictSummary | null {
  if (!report) return null;
  return {
    verdict: report.verdict,
    tone: verdictTone(report.verdict),
    badge: verdictBadgeLabel(report),
    label: verdictLabel(report.verdict),
    explanation: verdictExplanation(report),
  };
}

/** Worst verdict across a game's versions; `null` when nothing is reported. */
export function worstVerdict(reports: readonly ScanReport[] | null | undefined): VirusTotalVerdict | null {
  if (!reports || reports.length === 0) return null;
  return reports.reduce<VirusTotalVerdict>((worst, report) => {
    return VERDICT_SEVERITY[report.verdict] > VERDICT_SEVERITY[worst] ? report.verdict : worst;
  }, "disabled");
}

/**
 * True only for `clean`. Everything else — including `not_found` — must not
 * borrow the safe styling.
 */
export function isClean(verdict: VirusTotalVerdict): boolean {
  return verdict === "clean";
}

/** Engine hits worth naming, most severe first, capped for the popover. */
export function topDetections(
  report: Pick<ScanReport, "detections"> | null | undefined,
  limit = 4,
): VirusTotalDetection[] {
  const detections = report?.detections ?? [];
  const ranked = [...detections].sort((left, right) => {
    const severity = (category: string) => (category === "malicious" ? 2 : category === "suspicious" ? 1 : 0);
    return severity(right.category) - severity(left.category);
  });
  return ranked.slice(0, limit);
}

/* ------------------------------ admin errors ----------------------------- */

export type IntegrationErrorKind = "invalid_key" | "quota" | "outage" | "other";

/**
 * The contract hands the UI a single `lastError` string, so the distinct
 * reasons the issue asks for (invalid key, quota, outage) are recognised by
 * their marker text. Order matters: 401 first, then 429/quota, then 5xx.
 */
export function classifyIntegrationError(lastError: string | null | undefined): IntegrationErrorKind | null {
  if (!lastError?.trim()) return null;
  const text = lastError.toLowerCase();
  if (text.includes("401") || text.includes("invalid") || text.includes("unauthori") || text.includes("wrong api key")) {
    return "invalid_key";
  }
  if (text.includes("429") || text.includes("quota") || text.includes("rate limit") || text.includes("too many")) {
    return "quota";
  }
  if (text.includes("5") && (text.includes("503") || text.includes("502") || text.includes("500") || text.includes("unavailable") || text.includes("timeout"))) {
    return "outage";
  }
  return "other";
}

export function integrationErrorLabel(kind: IntegrationErrorKind | null): string | null {
  switch (kind) {
    case "invalid_key":
      return t("Invalid key");
    case "quota":
      return t("Quota exhausted");
    case "outage":
      return t("VirusTotal outage");
    case "other":
      return t("Other error");
    default:
      return null;
  }
}

/** Share of the daily budget already spent, 0–100. */
export function quotaUsedPercent(quota: VirusTotalQuota | null | undefined): number {
  if (!quota || quota.dailyLimit <= 0) return 0;
  return Math.min(100, Math.max(0, (quota.usedToday / quota.dailyLimit) * 100));
}

/* ------------------------------ upload gate ------------------------------ */

export type UploadTransport = "direct" | "upload_url";

export interface UploadEligibility {
  /** The archive can be uploaded at all (size limit). */
  possible: boolean;
  /** Transport the server would use, `null` when the file is too big. */
  transport: UploadTransport | null;
  /** `true` only when consent, admin and size all agree. */
  allowed: boolean;
  /** Why the button is disabled; `null` when allowed. */
  reason: string | null;
}

/**
 * Per-file upload gate (#224). Nothing here ever uploads on its own: the
 * caller renders a checkbox, then a confirmation, then calls the API.
 */
export function uploadEligibility(
  sizeBytes: number,
  options: { admin: boolean; consent: boolean },
): UploadEligibility {
  if (!options.admin) {
    return { possible: false, transport: null, allowed: false, reason: t("Only an administrator can upload a file.") };
  }
  if (!Number.isFinite(sizeBytes) || sizeBytes <= 0) {
    return { possible: false, transport: null, allowed: false, reason: t("This archive has no known size yet.") };
  }
  if (sizeBytes > VT_UPLOAD_URL_LIMIT) {
    return {
      possible: false,
      transport: null,
      allowed: false,
      reason: t("VirusTotal refuses files over 650 MB. A repack this size can only be checked by hash."),
    };
  }
  const transport: UploadTransport = sizeBytes <= VT_UPLOAD_DIRECT_LIMIT ? "direct" : "upload_url";
  if (!options.consent) {
    return {
      possible: true,
      transport,
      allowed: false,
      reason: t("Tick the public-sharing warning to enable the upload."),
    };
  }
  return { possible: true, transport, allowed: true, reason: null };
}

/** Copy for the two transports, so the dialog states what will happen. */
export function uploadTransportLabel(transport: UploadTransport | null): string | null {
  if (transport === "direct") return t("Direct upload, up to 32 MB");
  if (transport === "upload_url") return t("Upload URL, up to 650 MB");
  return null;
}

/* ------------------------------ scan queue ------------------------------- */

export interface ScanQueueSlot {
  /** 0-based place in the queue. */
  index: number;
  /** Milliseconds after the queue started when this scan may fire. */
  startsAtMs: number;
}

export interface ScanQueueSchedule {
  slots: ScanQueueSlot[];
  /** Time until the last scan may start. */
  etaSeconds: number;
  /** Time until the last response is in, including one request interval. */
  drainSeconds: number;
}

/**
 * Token-bucket schedule for the public 4 requests/minute budget. The queue
 * never fires faster than one scan per interval, so a bulk action cannot burn
 * the daily allowance in one burst.
 */
export function scanQueueSchedule(
  count: number,
  options: { perMinute?: number } = {},
): ScanQueueSchedule {
  const perMinute = Math.max(1, options.perMinute ?? VIRUSTOTAL_RATE_PER_MINUTE);
  const intervalMs = 60_000 / perMinute;
  const total = Math.max(0, Math.floor(count));
  const slots = Array.from({ length: total }, (_, index) => ({
    index,
    startsAtMs: Math.round(index * intervalMs),
  }));
  const etaSeconds = total === 0 ? 0 : Math.round(((total - 1) * intervalMs) / 1000);
  return { slots, etaSeconds, drainSeconds: total === 0 ? 0 : etaSeconds + Math.round(intervalMs / 1000) };
}

/** "8 scans · about 1 min 45 s at 4 requests per minute", for the queue strip. */
export function scanQueueEtaLabel(count: number, options: { perMinute?: number } = {}): string | null {
  if (count <= 0) return null;
  const perMinute = Math.max(1, options.perMinute ?? VIRUSTOTAL_RATE_PER_MINUTE);
  const { etaSeconds } = scanQueueSchedule(count, { perMinute });
  if (etaSeconds === 0) return t("1 scan queued · 4 requests per minute");
  return t("{count} scans queued · about {duration} at {rate} requests per minute", {
    count,
    duration: formatDuration(etaSeconds),
    rate: perMinute,
  });
}

/**
 * True when nothing authoritative is known yet: no report, or a verdict that
 * only says "we could not check". `not_found` is an answer, so it is not
 * "unscanned" — re-querying it would burn quota for the same 404.
 */
export function isUnscanned(
  report: Pick<ScanReport, "verdict"> | null | undefined,
): boolean {
  if (!report) return true;
  return report.verdict === "unknown" || report.verdict === "error" || report.verdict === "quota_exceeded";
}

/** Whether the popover may offer "Scan now" for this report. */
export function canScanNow(
  report: Pick<ScanReport, "verdict"> | null | undefined,
  options: { admin: boolean; lastScanAt?: number | null; now?: number; cooldownMs?: number },
): boolean {
  if (!options.admin) return false;
  if (!report || report.verdict === "disabled") return false;
  const cooldown = options.cooldownMs ?? 15_000;
  const now = options.now ?? Date.now();
  if (options.lastScanAt && now - options.lastScanAt < cooldown) return false;
  return true;
}
