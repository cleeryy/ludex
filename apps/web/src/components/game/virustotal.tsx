/**
 * VirusTotal surfaces (#224).
 *
 * One badge, one detail popover, one install warning and one game-page block,
 * all driven by the same report shape. The rules that matter live in
 * `lib/virustotal.ts`: `not_found` is never green, `unknown` is never green,
 * and nothing is ever uploaded without an explicit checkbox and confirmation.
 */

import { useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  ExternalLink,
  RefreshCw,
  ShieldCheck,
  ShieldQuestion,
  ShieldX,
  Upload,
} from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatDate, formatRelative } from "@/lib/format";
import type { GameVirusTotalResponse, ScanReport } from "@/lib/types";
import {
  canScanNow,
  summaryFor,
  topDetections,
  uploadEligibility,
  uploadTransportLabel,
  verdictExplanation,
  verdictLabel,
} from "@/lib/virustotal";
import { useIsAdmin } from "@/state/auth";
import { Badge, Button, Checkbox } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { Popover } from "@/components/ui/popover";
import { SettingsGroup } from "@/components/settings/settings-section";
import { t } from "@/lib/i18n";

/** `GET /api/games/{id}/virustotal`, cached per game. */
export function useGameVirusTotal(gameId: number, enabled = true) {
  return useQuery<GameVirusTotalResponse>({
    queryKey: ["game-virustotal", gameId],
    queryFn: () => api.gameVirusTotal(gameId),
    enabled: enabled && Number.isFinite(gameId) && gameId > 0,
    staleTime: 60_000,
  });
}

export function findReport(
  data: GameVirusTotalResponse | undefined,
  versionId: number | null | undefined,
): ScanReport | null {
  if (!data || versionId == null) return null;
  return data.versions.find((report) => report.versionId === versionId) ?? null;
}

function VerdictIcon({ verdict, className }: { verdict: ScanReport["verdict"]; className?: string }) {
  if (verdict === "malicious") return <ShieldX className={className} aria-hidden="true" />;
  if (verdict === "suspicious") return <ShieldQuestion className={className} aria-hidden="true" />;
  if (verdict === "clean") return <ShieldCheck className={className} aria-hidden="true" />;
  return <ShieldQuestion className={className} aria-hidden="true" />;
}

/* --------------------------------- badge --------------------------------- */

/**
 * The small badge from the issue: `0/72` green, `N/72` amber when some engines
 * are suspicious, `N/72` red when any engine detects malware, and the state
 * name otherwise. Clicking opens the detail popover.
 */
export function VirusTotalBadge({
  report,
  gameId,
  sizeBytes,
  fileName,
  showDate = false,
  className,
}: {
  report: ScanReport | null;
  gameId: number;
  /** Archive size, quoted by the upload dialog. */
  sizeBytes?: number;
  fileName?: string;
  showDate?: boolean;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  // The upload dialog lives outside the popover: a portal inside it would be
  // "outside" the popover panel, and the next click would close both.
  const [uploadOpen, setUploadOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const isAdmin = useIsAdmin();
  const summary = summaryFor(report);
  const verdict = report?.verdict ?? "unknown";
  const tone = summary?.tone ?? "outline";
  const label = summary?.label ?? verdictLabel("unknown");
  const badge = summary?.badge ?? verdictLabel("unknown");

  const tones: Record<string, string> = {
    ok: "border-ok/40 bg-ok/10 text-ok",
    signal: "border-signal/50 bg-signal/12 text-signal-text",
    danger: "border-danger/45 bg-danger/10 text-danger",
    outline: "border-line-2 bg-transparent text-ink-3",
  };

  return (
    <>
      <button
        ref={anchor}
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={t("VirusTotal: {label} ({badge})", { label, badge })}
        onClick={() => setOpen((value) => !value)}
        className={cn(
          "inline-flex h-5 shrink-0 items-center gap-1.5 rounded-[2px] border px-1.5 text-[11px] font-medium leading-none transition-colors",
          tones[tone],
          "hover:brightness-110 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-signal",
          className,
        )}
      >
        <VerdictIcon verdict={verdict} className="size-3" />
        <span className="num">{badge}</span>
        {showDate && report && (
          <span className="hidden font-normal opacity-70 sm:inline">{formatRelative(report.checkedAt)}</span>
        )}
      </button>
      <Popover
        open={open}
        onOpenChange={setOpen}
        anchorRef={anchor}
        align="start"
        maxHeight={460}
        label={t("VirusTotal report")}
        className="w-[320px] max-w-[calc(100vw-16px)]"
      >
        <VirusTotalDetail
          report={report}
          gameId={gameId}
          onUpload={() => {
            setOpen(false);
            setUploadOpen(true);
          }}
        />
      </Popover>
      {isAdmin && report && (
        <UploadDialog
          open={uploadOpen}
          onOpenChange={setUploadOpen}
          report={report}
          gameId={gameId}
          sizeBytes={sizeBytes}
          fileName={fileName}
        />
      )}
    </>
  );
}

/* -------------------------------- detail --------------------------------- */

function StatCell({ label, value, tone }: { label: string; value: number; tone?: "danger" | "signal" }) {
  return (
    <div className="bg-surface px-2 py-1.5">
      <p className={cn("num text-[14px] leading-none", tone === "danger" ? "text-danger" : tone === "signal" ? "text-signal-text" : "text-ink")}>
        {value}
      </p>
      <p className="mt-1 text-[10px] text-ink-3">{label}</p>
    </div>
  );
}

export function VirusTotalDetail({
  report,
  gameId,
  onUpload,
}: {
  report: ScanReport | null;
  gameId: number;
  /** Opens the upload dialog, which the badge renders outside the popover. */
  onUpload: () => void;
}) {
  const isAdmin = useIsAdmin();
  const queryClient = useQueryClient();
  const summary = summaryFor(report);
  const detections = useMemo(() => topDetections(report), [report]);

  const scan = useMutation({
    mutationFn: () => api.scanVirusTotal({ versionId: report!.versionId, force: true }),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["game-virustotal", gameId] });
    },
  });

  if (!report) {
    return (
      <div className="flex flex-col gap-2 p-3.5">
        <p className="text-[12.5px] text-ink-2">{t("No archive has been checked yet.")}</p>
        <p className="text-[11.5px] leading-relaxed text-ink-3">
          {t("VirusTotal looks files up by SHA-256. An unchecked archive is not a safe archive.")}
        </p>
      </div>
    );
  }

  const tone = summary?.tone ?? "outline";
  const stats = report.stats;
  const inactive = report.verdict === "not_found" || report.verdict === "unknown" ||
    report.verdict === "disabled" || report.verdict === "quota_exceeded" || report.verdict === "error";

  return (
    <div className="flex flex-col">
      <div className={cn(
        "flex items-start gap-2.5 border-b border-line px-3.5 py-3",
        tone === "danger" && "bg-danger/6",
        tone === "signal" && "bg-signal/6",
      )}>
        <VerdictIcon
          verdict={report.verdict}
          className={cn(
            "mt-0.5 size-4 shrink-0",
            tone === "ok" ? "text-ok" : tone === "signal" ? "text-signal-text" : tone === "danger" ? "text-danger" : "text-ink-3",
          )}
        />
        <div className="min-w-0 flex-1">
          <p className="text-[13px] font-medium text-ink">
            {summary?.label}
            {!inactive && (
              <span className="num ms-2 text-[11.5px] font-normal text-ink-3">
                {summary?.badge}
              </span>
            )}
          </p>
          <p className="mt-1 text-[11.5px] leading-relaxed text-ink-2">{verdictExplanation(report)}</p>
          <p className="mt-1 font-mono text-[10px] text-ink-3">
            {t("Checked {date}", { date: formatRelative(report.checkedAt) })}
            {report.source === "cache" ? ` · ${t("from cache")}` : ` · ${t("fresh lookup")}`}
          </p>
        </div>
      </div>

      {!inactive && (
        <>
          <div className="grid grid-cols-3 gap-px border-b border-line bg-line">
            <StatCell label={t("Malicious")} value={stats.malicious} tone={stats.malicious > 0 ? "danger" : undefined} />
            <StatCell label={t("Suspicious")} value={stats.suspicious} tone={stats.suspicious > 0 ? "signal" : undefined} />
            <StatCell label={t("Undetected")} value={stats.undetected} />
            <StatCell label={t("Harmless")} value={stats.harmless} />
            <StatCell label={t("Timeout")} value={stats.timeout} />
            <StatCell label={t("Failure")} value={stats.failure} />
          </div>

          {(report.meaningfulName || report.lastAnalysisDate) && (
            <dl className="border-b border-line px-3.5 py-2.5 text-[11.5px]">
              {report.meaningfulName && (
                <div className="flex items-baseline justify-between gap-3">
                  <dt className="text-ink-3">{t("Known as")}</dt>
                  <dd className="min-w-0 truncate text-right font-mono text-[10.5px] text-ink-2" title={report.meaningfulName}>
                    {report.meaningfulName}
                  </dd>
                </div>
              )}
              {report.lastAnalysisDate && (
                <div className="mt-1 flex items-baseline justify-between gap-3">
                  <dt className="text-ink-3">{t("Last analysis")}</dt>
                  <dd className="text-ink-2">{formatDate(report.lastAnalysisDate)}</dd>
                </div>
              )}
            </dl>
          )}

          {detections.length > 0 && (
            <div className="border-b border-line px-3.5 py-2.5">
              <p className="text-[10.5px] uppercase tracking-[0.08em] text-ink-3">{t("Engine detections")}</p>
              <ul className="mt-1.5 flex flex-col gap-1">
                {detections.map((hit) => (
                  <li key={`${hit.engine}-${hit.result ?? ""}`} className="flex items-baseline justify-between gap-3 text-[11.5px]">
                    <span className="truncate text-ink-2">{hit.engine}</span>
                    <span
                      className={cn(
                        "min-w-0 truncate text-right font-mono text-[10.5px]",
                        hit.category === "malicious" ? "text-danger" : "text-signal-text",
                      )}
                      title={hit.result ?? undefined}
                    >
                      {hit.result ?? t("detected")}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}

      {report.reason && inactive && (
        <p className="border-b border-line px-3.5 py-2.5 text-[11.5px] leading-relaxed text-ink-3">{report.reason}</p>
      )}

      <div className="flex flex-wrap items-center gap-1.5 px-3 py-2.5">
        {report.permalink && (
          <a
            href={report.permalink}
            target="_blank"
            rel="noreferrer"
            className="inline-flex h-7 items-center gap-1 rounded-[2px] px-2 text-[11.5px] text-ink-2 transition-colors hover:bg-surface-2 hover:text-ink"
          >
            {t("Open on VirusTotal")}
            <ExternalLink className="size-3" aria-hidden="true" />
          </a>
        )}
        {canScanNow(report, { admin: isAdmin }) && (
          <Button
            variant="ghost"
            size="sm"
            loading={scan.isPending}
            icon={<RefreshCw className="size-3.5" />}
            onClick={() => scan.mutate()}
          >
            {t("Scan now")}
          </Button>
        )}
        {isAdmin && report.verdict !== "disabled" && (
          <Button
            variant="ghost"
            size="sm"
            icon={<Upload className="size-3.5" />}
            title={t("Upload this file for analysis")}
            onClick={onUpload}
          >
            {t("Upload")}
          </Button>
        )}
      </div>

      {scan.isError && (
        <p role="alert" className="border-t border-line px-3.5 py-2 text-[11.5px] text-danger">
          {errorMessage(scan.error)}
        </p>
      )}

    </div>
  );
}

/* -------------------------------- upload --------------------------------- */

/**
 * Per-file upload (#224): public-sharing warning, size limit, checkbox, then a
 * confirmation. Nothing else in the app offers an upload, and "Scan now" never
 * falls back to it.
 */
export function UploadDialog({
  open,
  onOpenChange,
  report,
  gameId,
  sizeBytes,
  fileName,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  report: ScanReport;
  gameId: number;
  sizeBytes?: number;
  fileName?: string;
}) {
  const queryClient = useQueryClient();
  const [consent, setConsent] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const gate = uploadEligibility(sizeBytes ?? 0, { admin: true, consent });

  const upload = useMutation({
    mutationFn: () =>
      api.uploadVirusTotalVersion({ versionId: report.versionId, iUnderstand: "public-sharing" }),
    onSuccess: () => {
      setError(null);
      void queryClient.invalidateQueries({ queryKey: ["game-virustotal", gameId] });
      onOpenChange(false);
    },
    onError: (cause: Error) => setError(cause.message),
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) setError(null);
        onOpenChange(next);
      }}
      title={t("Upload this file to VirusTotal?")}
      description={fileName}
      size="sm"
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button
            variant="danger"
            loading={upload.isPending}
            disabled={!gate.allowed}
            icon={<Upload className="size-4" />}
            onClick={() => upload.mutate()}
          >
            {t("Upload and share")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-[12.5px] leading-relaxed text-ink-2">
        <p className="flex items-start gap-2 border-l-2 border-danger pl-3 text-danger">
          <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>
            {t(
              "Uploading shares the file with VirusTotal's community and security partners. The report becomes public, and only upload files you are allowed to redistribute.",
            )}
          </span>
        </p>
        <p className="text-ink-3">
          {t("A hash lookup is enough to read a verdict. Upload only when you want new engines to analyse this exact file.")}
        </p>
        <dl className="flex flex-col gap-1.5 border-t border-line pt-3 text-[12px]">
          <div className="flex items-baseline justify-between gap-3">
            <dt className="text-ink-3">{t("Archive size")}</dt>
            <dd className="num text-ink-2">{sizeBytes ? formatBytes(sizeBytes) : "—"}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-3">
            <dt className="text-ink-3">{t("Transport")}</dt>
            <dd className="text-ink-2">{uploadTransportLabel(gate.transport) ?? t("Not possible")}</dd>
          </div>
        </dl>
        <p className="text-[11.5px] text-ink-3">
          {t("Up to 32 MB goes through a direct upload; up to 650 MB uses an upload URL. Bigger archives cannot be uploaded at all.")}
        </p>
        <div className="flex items-start gap-2.5 border border-line-2 bg-surface/60 p-3 text-[12px] text-ink-2">
          <Checkbox
            checked={consent}
            onCheckedChange={setConsent}
            label={t("I understand this file becomes publicly shared")}
          />
          <button
            type="button"
            onClick={() => setConsent(!consent)}
            className="text-left"
          >
            {t("I understand this file becomes publicly shared")}
          </button>
        </div>
        {gate.reason && !gate.allowed && (
          <p className="text-[11.5px] leading-relaxed text-signal-text">{gate.reason}</p>
        )}
        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[12px] text-danger">
            {error}
          </p>
        )}
      </div>
    </Dialog>
  );
}

/* ------------------------------- warnings -------------------------------- */

/**
 * Install-time warning (`install-dialog`, Downloads rows). Non-blocking by
 * design: `malicious` is loud but never cancels anything, and `unknown` /
 * `not_found` stay grey so nobody reads them as safe.
 */
export function VirusTotalWarning({
  report,
  compact = false,
  className,
}: {
  report: ScanReport | null;
  compact?: boolean;
  className?: string;
}) {
  if (!report) return null;
  const verdict = report.verdict;
  const stats = report.stats;
  const total = stats.malicious + stats.suspicious + stats.undetected + stats.harmless + stats.timeout + stats.failure;

  const line = (() => {
    switch (verdict) {
      case "malicious":
        return t("VirusTotal detects malware in this archive: {hits} of {total} engines. Installing is still allowed.", {
          hits: stats.malicious,
          total,
        });
      case "suspicious":
        return t("VirusTotal flags this archive as suspicious: {hits} of {total} engines. Treat it with care.", {
          hits: stats.suspicious,
          total,
        });
      case "not_found":
        return t("VirusTotal does not know this file. Nobody has checked it, which is not the same as safe.");
      case "unknown":
        return t("This archive has no VirusTotal verdict: the hash was not checked.");
      case "quota_exceeded":
        return t("VirusTotal could not check this archive: the daily quota is used up. There is no verdict.");
      case "error":
        return t("VirusTotal could not check this archive: the lookup failed. There is no verdict.");
      case "disabled":
        return t("VirusTotal is not configured on this server, so this archive has not been checked.");
      default:
        return null;
    }
  })();

  if (!line) return null;

  const loud = verdict === "malicious" || verdict === "suspicious";

  if (compact) {
    return (
      <span
        className={cn(
          "inline-flex items-center gap-1.5 text-[11px]",
          verdict === "malicious" ? "text-danger" : verdict === "suspicious" ? "text-signal-text" : "text-ink-3",
          className,
        )}
      >
        {loud ? <AlertTriangle className="size-3 shrink-0" aria-hidden="true" /> : <ShieldQuestion className="size-3 shrink-0" aria-hidden="true" />}
        {line}
      </span>
    );
  }

  return (
    <div
      role={verdict === "malicious" ? "alert" : undefined}
      className={cn(
        "flex items-start gap-2 border-l-2 pl-3 text-[11.5px] leading-relaxed",
        verdict === "malicious"
          ? "border-danger text-danger"
          : verdict === "suspicious"
            ? "border-signal text-signal-text"
            : "border-line-2 text-ink-3",
        className,
      )}
    >
      {loud ? (
        <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
      ) : (
        <ShieldQuestion className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
      )}
      <span>
        {line}
        {report.permalink && loud && (
          <>
            {" "}
            <a href={report.permalink} target="_blank" rel="noreferrer" className="underline underline-offset-4">
              {t("Open on VirusTotal")}
            </a>
          </>
        )}
      </span>
    </div>
  );
}

/** Query-backed variant for screens that only know the ids. */
export function VirusTotalArchiveWarning({
  gameId,
  versionId,
  compact = false,
  className,
}: {
  gameId: number;
  versionId: number | null | undefined;
  compact?: boolean;
  className?: string;
}) {
  const { data } = useGameVirusTotal(gameId);
  const report = findReport(data, versionId);
  return <VirusTotalWarning report={report} compact={compact} className={className} />;
}

/* ------------------------------ game block ------------------------------- */

/**
 * Game page → Overview (#189): one compact block instead of a second scroll
 * section. Each version gets the badge; the footer says what was checked.
 */
export function VirusTotalSummary({
  gameId,
  versions,
  enabled = true,
}: {
  gameId: number;
  versions: { id: number; label: string; sizeBytes: number; fileName: string }[];
  enabled?: boolean;
}) {
  const { data, isPending } = useGameVirusTotal(gameId, enabled && versions.length > 0);
  if (versions.length === 0) return null;

  const reports = data?.versions ?? [];

  return (
    <SettingsGroup
      title={t("VirusTotal")}
      description={t("VirusTotal looks archives up by SHA-256. It never uploads anything on its own.")}
      actions={<Badge tone="outline">{t("Advisory")}</Badge>}
    >
      <ul className="border-t border-line">
        {versions.map((version) => {
          const report = reports.find((entry) => entry.versionId === version.id) ?? null;
          return (
            <li key={version.id} className="flex items-center justify-between gap-3 border-b border-line py-2.5">
              <div className="min-w-0">
                <p className="truncate text-[12.5px] text-ink-2">{version.label}</p>
                <p className="truncate font-mono text-[10px] text-ink-3">{formatBytes(version.sizeBytes)}</p>
              </div>
              {isPending ? (
                <span className="h-5 w-14 animate-pulse rounded-[2px] bg-surface-2" aria-hidden="true" />
              ) : (
                <VirusTotalBadge
                  report={report}
                  gameId={gameId}
                  sizeBytes={version.sizeBytes}
                  fileName={version.fileName}
                  showDate
                />
              )}
            </li>
          );
        })}
      </ul>
      <p className="mt-2 text-[11px] leading-relaxed text-ink-3">
        {t("No verdict is a verdict: an unchecked or unknown archive is not a safe archive.")}
      </p>
    </SettingsGroup>
  );
}
