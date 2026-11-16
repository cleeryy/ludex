/**
 * Admin → Games: the VirusTotal column and the throttled scan queue (#209/#224).
 *
 * The public API allows 4 requests per minute, so a bulk action must not fire
 * one request per row in a burst. #238 moved the scheduler to
 * `state/virustotal-queue.tsx`, mounted above the router: this module only
 * renders the queue strip, the badge cell and the per-row action, and the
 * Games screen reads the live queue from the provider.
 */

import { useMemo } from "react";
import { RefreshCw } from "lucide-react";
import { cn } from "@/lib/cn";
import type { ScanReport } from "@/lib/types";
import { scanQueueEtaLabel, summaryFor, verdictLabel, worstVerdict } from "@/lib/virustotal";
import type { VirusTotalQueueRun } from "@/lib/virustotal-queue";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

/** Header strip: queue position, ETA at 4 requests/minute and a stop button. */
export function VirusTotalQueueStrip({
  queue,
  onCancel,
  error,
}: {
  queue: VirusTotalQueueRun;
  onCancel: () => void;
  error: string | null;
}) {
  const total = queue.ids.length;
  const handled = queue.done + queue.failed;
  const remaining = Math.max(0, total - handled);
  const label = scanQueueEtaLabel(remaining);
  const pct = total === 0 ? 0 : (handled / total) * 100;

  return (
    <div className="mt-3 border border-line-2 bg-surface/60 p-3" role="status" aria-live="polite">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <p className="text-[12.5px] text-ink">
          {t("VirusTotal queue: {done} of {total} scanned", { done: queue.done, total })}
          {queue.failed > 0 && (
            <span className="ms-2 text-danger">{t("{count} failed", { count: queue.failed })}</span>
          )}
        </p>
        <div className="flex items-center gap-2">
          {label && <span className="font-mono text-[10.5px] text-ink-3">{label}</span>}
          <Button variant="ghost" size="sm" onClick={onCancel}>
            {t("Stop")}
          </Button>
        </div>
      </div>
      <Progress className="mt-2" value={pct} label={t("VirusTotal queue progress")} />
      {error && <p className="mt-2 text-[11.5px] text-danger">{error}</p>}
      <p className="mt-1.5 text-[11px] leading-relaxed text-ink-3">
        {t("Verdicts are read from the report cache as each scan lands. Nothing is uploaded.")}
      </p>
    </div>
  );
}

/** Worst report of a game, the one the table column shows. */
export function worstReport(reports: ScanReport[] | undefined): ScanReport | null {
  if (!reports || reports.length === 0) return null;
  const verdict = worstVerdict(reports);
  return reports.find((report) => report.verdict === verdict) ?? reports[0];
}

/** Compact table cell: the badge for the worst verdict, or a dash. */
export function VirusTotalCell({
  reports,
  onScan,
  scanning,
}: {
  reports: ScanReport[] | undefined;
  onScan: () => void;
  scanning: boolean;
}) {
  const report = useMemo(() => worstReport(reports), [reports]);
  if (!report) {
    return (
      <button
        type="button"
        onClick={onScan}
        disabled={scanning}
        className="inline-flex h-5 items-center gap-1 rounded-[2px] border border-line-2 px-1.5 text-[11px] text-ink-3 transition-colors hover:border-ink-3 hover:text-ink-2 disabled:opacity-40"
        title={t("Not checked yet: scan this game")}
      >
        {t("Not checked")}
      </button>
    );
  }
  const summary = summaryFor(report);
  const tones: Record<string, string> = {
    ok: "border-ok/40 bg-ok/10 text-ok",
    signal: "border-signal/50 bg-signal/12 text-signal-text",
    danger: "border-danger/45 bg-danger/10 text-danger",
    outline: "border-line-2 text-ink-3",
  };
  return (
    <span className="inline-flex items-center gap-1.5">
      <span
        className={cn(
          "inline-flex h-5 items-center rounded-[2px] border px-1.5 text-[11px] font-medium leading-none",
          tones[summary?.tone ?? "outline"],
        )}
        title={`${summary?.label}: ${summary?.explanation}`}
      >
        {summary?.tone === "outline" ? summary.label : summary?.badge}
      </span>
      <span className="sr-only">{verdictLabel(report.verdict)}</span>
    </span>
  );
}

/** Per-row scan action; the label stays short so the column does not grow. */
export function VirusTotalScanButton({
  scanning,
  disabled,
  onClick,
}: {
  scanning: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <Button
      variant="ghost"
      size="sm"
      loading={scanning}
      disabled={disabled}
      icon={<RefreshCw className="size-3.5" />}
      onClick={onClick}
      title={t("Scan this game with VirusTotal")}
    >
      {t("Scan")}
    </Button>
  );
}

/** Small legend under the table so the colours mean something. */
export function VirusTotalLegend() {
  return (
    <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px] text-ink-3">
      <Badge tone="ok">{t("Clean")}</Badge>
      <Badge tone="signal">{t("Suspicious")}</Badge>
      <Badge tone="danger">{t("Malicious")}</Badge>
      <Badge tone="outline">{t("Never seen / not checked")}</Badge>
      <span>{t("Hash lookups only; the column is advisory.")}</span>
    </div>
  );
}
