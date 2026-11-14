/**
 * Admin → Games: the VirusTotal column and the throttled scan queue (#209/#224).
 *
 * The public API allows 4 requests per minute, so a bulk action must not fire
 * one request per row in a burst. The queue schedules each scan with
 * `scanQueueSchedule`, waits for its slot, then polls the report cache by
 * invalidating the game's query — never a tight loop.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import type { ScanReport } from "@/lib/types";
import { scanQueueEtaLabel, scanQueueSchedule, summaryFor, verdictLabel, worstVerdict } from "@/lib/virustotal";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

export interface ScanQueueState {
  ids: number[];
  /** Scans that came back with a report. */
  done: number;
  /** Scans that failed; the strip reports the first message. */
  failed: number;
  startedAt: number;
}

export interface VirusTotalQueueController {
  queue: ScanQueueState | null;
  start: (ids: number[]) => void;
  cancel: () => void;
  error: string | null;
  /** Queue length left to fire. */
  remaining: number;
}

/**
 * Runs one scan per slot of the 4/minute budget. Each step invalidates that
 * game's cached report (and the table when the queue drains), so the UI reads
 * verdicts from the cache instead of re-scanning.
 */
export function useVirusTotalQueue(): VirusTotalQueueController {
  const queryClient = useQueryClient();
  const [queue, setQueue] = useState<ScanQueueState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const running = useRef(false);

  const start = useCallback((ids: number[]) => {
    if (ids.length === 0) return;
    setError(null);
    setQueue({ ids, done: 0, failed: 0, startedAt: Date.now() });
  }, []);

  const cancel = useCallback(() => setQueue(null), []);

  const index = queue ? queue.done + queue.failed : 0;
  const finished = queue ? index >= queue.ids.length : true;

  useEffect(() => {
    if (!queue || finished || running.current) return;
    const schedule = scanQueueSchedule(queue.ids.length);
    const slot = schedule.slots[index];
    if (!slot) return;
    const delay = Math.max(0, queue.startedAt + slot.startsAtMs - Date.now());
    let cancelled = false;
    const timer = setTimeout(async () => {
      if (cancelled) return;
      running.current = true;
      const gameId = queue.ids[index];
      try {
        await api.scanVirusTotal({ gameId });
        await queryClient.invalidateQueries({ queryKey: ["game-virustotal", gameId] });
        setQueue((current) => (current ? { ...current, done: current.done + 1 } : current));
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
        setQueue((current) => (current ? { ...current, failed: current.failed + 1 } : current));
      } finally {
        running.current = false;
      }
    }, delay);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [queryClient, finished, index, queue]);

  // The table and every cell read from the cache; refresh them once the queue
  // has fired its last request.
  useEffect(() => {
    if (!queue || !finished) return;
    void queryClient.invalidateQueries({ queryKey: ["admin-games"] });
    void queryClient.invalidateQueries({ queryKey: ["admin-virustotal"] });
  }, [finished, queryClient, queue]);

  const remaining = queue ? Math.max(0, queue.ids.length - index) : 0;
  return { queue, start, cancel, error, remaining };
}

/** Header strip: queue position, ETA at 4 requests/minute and a stop button. */
export function VirusTotalQueueStrip({
  queue,
  onCancel,
  error,
}: {
  queue: ScanQueueState;
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
