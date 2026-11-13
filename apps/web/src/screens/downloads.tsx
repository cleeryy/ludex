import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowDown, ArrowUp, ArrowUpToLine, Check, Download, FolderOpen, Pause, Play, RefreshCw, Trash2, X } from "lucide-react";
import { installsAvailable } from "@/lib/install";
import { isActiveJob, useDownloads, type Aggregate } from "@/state/downloads";
import { canRetryJob, canRetryNow, isTrulyResumable, secondsUntilRetry } from "@/lib/install-retry";
import {
  isVerifyingJob,
  isWaitingForSpaceJob,
  jobDisplaySpeed,
  jobProgressPercent,
  prefetchLabel,
  spaceWaitLabel,
  verifiedDigestLabel,
  verifyingElapsedLabel,
  verifyingLabel,
} from "@/lib/install-phase";
import { canMoveDown, canMoveUp, isReorderableJob, waitingQueue } from "@/lib/install-queue";
import { useAllGames } from "@/lib/hooks";
import { formatBytes, formatDuration, formatSpeed, installPhaseLabel, formatPercent } from "@/lib/format";
import { HISTORY_WINDOW_MS, type JobMeter, type SpeedPoint } from "@/lib/eta";
import type { InstallJob } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { RemoteQueueStrip } from "@/components/game/remote-queue-strip";
import { EmptyState, PageHeader, SectionHeader } from "@/components/page";
import { GameCover } from "@/components/art/cover";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { cn } from "@/lib/cn";
import { tr, t, tn } from "@/lib/i18n";

/** Rows whose history entry may be deleted (#100); active and paused rows cannot. */
const REMOVABLE_PHASES: ReadonlySet<InstallJob["phase"]> = new Set(["done", "cancelled", "error", "interrupted"]);

const GRAPH_WIDTH = 240;
const GRAPH_HEIGHT = 40;
const GRAPH_PAD = 3;
/** Shortest x-axis so the first points do not collapse into the right edge. */
const MIN_GRAPH_SPAN_MS = 5_000;

/**
 * Aggregate status for the header: a gated ETA while a download moves, the
 * phase plus elapsed time while extracting, and "—" when a running download
 * does not report a total size (#102).
 */
function aggregateStatus(aggregate: Aggregate): { text: string; hint: string | null } | null {
  if (aggregate.downloading > 0) {
    if (!aggregate.etaKnown) {
      return { text: "—", hint: t("At least one running download does not report a total size.") };
    }
    if (aggregate.estimating) return { text: t("Estimating…"), hint: t("Collecting the first samples.") };
    if (aggregate.etaSeconds !== null) return { text: t("{duration} left", { duration: formatDuration(aggregate.etaSeconds) }), hint: null };
    return null;
  }
  // No download is moving: narrate the disk phase with elapsed time, and stay
  // quiet for queued/paused queues that have no estimate at all. Verification
  // (#223) has no ETA: it reports how long the read has been running.
  if (
    aggregate.phase !== "verifying" &&
    aggregate.phase !== "extracting" &&
    aggregate.phase !== "finalizing"
  ) {
    return null;
  }
  const label = installPhaseLabel(aggregate.phase);
  if (aggregate.phaseElapsedMs === null) return { text: `${label}…`, hint: null };
  return { text: `${label}… ${formatDuration(aggregate.phaseElapsedMs / 1000)}`, hint: null };
}

/**
 * Compact SVG area chart of the aggregate download rate (#102). No chart
 * dependency, theme tokens via `currentColor`, and no motion at all so
 * `prefers-reduced-motion` needs no extra branch.
 */
function TransferGraph({ points, now }: { points: SpeedPoint[]; now: number }) {
  const visible = useMemo(() => points.filter((point) => point.at >= now - HISTORY_WINDOW_MS), [points, now]);
  const current = visible.length > 0 ? visible[visible.length - 1].speedBps : 0;
  const peak = visible.reduce((max, point) => Math.max(max, point.speedBps), 0);
  const average = visible.length > 0 ? visible.reduce((sum, point) => sum + point.speedBps, 0) / visible.length : 0;
  const scale = Math.max(peak, 1);
  // Fill the width while history is still shorter than two minutes, then
  // scroll a fixed window. Without this the first samples pile up at the right.
  const oldest = visible.length > 0 ? visible[0].at : now;
  const span = Math.min(HISTORY_WINDOW_MS, Math.max(MIN_GRAPH_SPAN_MS, now - oldest));
  const start = now - span;
  const xFor = (at: number) => ((Math.min(Math.max(at, start), now) - start) / span) * GRAPH_WIDTH;
  const yFor = (speed: number) =>
    GRAPH_HEIGHT - GRAPH_PAD - (Math.min(speed, scale) / scale) * (GRAPH_HEIGHT - GRAPH_PAD * 2);
  const coords = visible.map((point) => [xFor(point.at), yFor(point.speedBps)] as const);
  const line = coords.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
  const area =
    coords.length > 1
      ? `M${coords[0][0].toFixed(1)},${GRAPH_HEIGHT} ${coords
          .map(([x, y]) => `L${x.toFixed(1)},${y.toFixed(1)}`)
          .join(" ")} L${coords[coords.length - 1][0].toFixed(1)},${GRAPH_HEIGHT} Z`
      : "";

  const speedLabel = (value: number) => formatSpeed(value);
  const summary = t("Download speed over the last two minutes: {current} now, peak {peak}, average {average}.", {
    current: speedLabel(current),
    peak: speedLabel(peak),
    average: speedLabel(average),
  });

  return (
    <figure className="flex items-center gap-3">
      <svg
        viewBox={`0 0 ${GRAPH_WIDTH} ${GRAPH_HEIGHT}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={summary}
        className="h-10 w-[160px] text-signal sm:w-[240px]"
      >
        <title>{summary}</title>
        <line
          x1="0"
          y1={GRAPH_HEIGHT - 0.5}
          x2={GRAPH_WIDTH}
          y2={GRAPH_HEIGHT - 0.5}
          stroke="currentColor"
          strokeOpacity="0.25"
          strokeWidth="1"
          vectorEffect="non-scaling-stroke"
        />
        {area !== "" && <path d={area} fill="currentColor" fillOpacity="0.14" />}
        {coords.length > 1 && (
          <polyline
            points={line}
            fill="none"
            stroke="currentColor"
            strokeWidth="1.25"
            strokeLinejoin="round"
            strokeLinecap="round"
            vectorEffect="non-scaling-stroke"
          />
        )}
      </svg>
      <figcaption className="flex flex-col gap-0.5 font-mono text-[10px] leading-tight text-ink-3">
        <span>
          {t("Now")} <span className="text-ink-2">{speedLabel(current)}</span>
        </span>
        <span>
          {t("peak")} <span className="text-ink-2">{speedLabel(peak)}</span>
        </span>
        <span>
          {t("Avg")} <span className="text-ink-2">{speedLabel(average)}</span>
        </span>
      </figcaption>
      <span className="sr-only">{summary}</span>
    </figure>
  );
}

export function DownloadsScreen() {
  const downloads = useDownloads();
  const { push } = useToast();
  const { data } = useAllGames();
  const gamesById = useMemo(() => new Map((data?.items ?? []).map((game) => [game.id, game])), [data]);
  // Failed entries ask first: removing them is the only way to lose sight of
  // the resume/retry action, even though the files stay on disk.
  const [removeTarget, setRemoveTarget] = useState<InstallJob | null>(null);
  // A retry without a checkpoint restarts from zero: say so and confirm (#205).
  const [restartTarget, setRestartTarget] = useState<InstallJob | null>(null);

  const coverFor = (job: InstallJob) => gamesById.get(job.gameId)?.coverUrl ?? null;
  const yearFor = (job: InstallJob) => gamesById.get(job.gameId)?.releaseDate ?? null;

  if (!installsAvailable()) {
    return (
      <div className="page page-in">
        <PageHeader title={t("Downloads")} description={t("Install jobs run inside the Ludex desktop app.")} />
        <RemoteQueueStrip />
        <div className="mt-8">
          <EmptyState
            title={t("Downloads live on the desktop")}
            description={t("The browser build can browse the library and download archives. Installing, launching and playtime need the Ludex desktop app.")}
          >
            <Link to="/remote">
              <Button variant="default">{t("Queue an install on the desktop")}</Button>
            </Link>
            <Link to="/library">
              <Button variant="ghost">{t("Open the library")}</Button>
            </Link>
          </EmptyState>
        </div>
      </div>
    );
  }

  const {
    active,
    history,
    activeCount,
    aggregate,
    meters,
    speedHistory,
    speedHistoryAt,
    titleFor,
    cancel,
    pause,
    resume,
    retryAsDownload,
    retryStream,
    retryNow,
    checkSpace,
    reorder,
    remove,
    clearHistory,
    openFolder,
  } = downloads;

  // #202: the waiting queue in start order; the running install is pinned
  // above it and never reordered. Cheap enough to recompute per render (a
  // handful of rows) and this stays below the early return.
  const waiting = waitingQueue(active);
  const queueRank = (job: InstallJob): number | null => {
    if (!isReorderableJob(job)) return null;
    const index = waiting.findIndex((entry) => entry.jobId === job.jobId);
    return index >= 0 ? index + 1 : null;
  };

  const status = aggregate ? aggregateStatus(aggregate) : null;

  const clear = async () => {
    const cleared = await clearHistory();
    if (cleared > 0) {
      push({
        title: t("Finished downloads cleared"),
        description: tn(cleared, "1 entry removed from history. Install folders are kept.", "{count} entries removed from history. Install folders are kept."),
      });
    }
  };

  const requestRemove = (job: InstallJob) => {
    if (job.phase === "error" || job.phase === "interrupted") {
      setRemoveTarget(job);
      return;
    }
    void remove(job.jobId);
  };

  const confirmRemove = () => {
    const target = removeTarget;
    setRemoveTarget(null);
    if (target) void remove(target.jobId);
  };

  const requestRestart = (job: InstallJob) => setRestartTarget(job);

  const confirmRestart = () => {
    const target = restartTarget;
    setRestartTarget(null);
    if (!target) return;
    // Same job id, same history row: only the transport is chosen here.
    if (target.stream) {
      void retryStream(target, { title: titleFor(target) });
    } else {
      void resume(target, { title: titleFor(target) });
    }
  };

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Downloads")}
        meta={
          <>
            <span className="num">{t("{count} active", { count: activeCount })}</span>
            <span className="num">{t("{count} in history", { count: history.length })}</span>
            {aggregate?.speedBps ? <span className="num">{formatSpeed(aggregate.speedBps)}</span> : null}
            {status ? (
              <span className="num" title={status.hint ?? undefined}>
                {status.text}
              </span>
            ) : null}
          </>
        }
        description={t("Installs keep running while you use the rest of the app. Closing the install window does not stop them.")}
        actions={
          <Link to="/library">
            <Button variant="ghost" size="sm">
              {t("Browse the library")}
            </Button>
          </Link>
        }
      />

      <section className="mt-8" aria-labelledby="downloads-active">
        <SectionHeader
          title={t("Active")}
          count={activeCount}
          actions={
            <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
              {aggregate && aggregate.percent !== null ? (
                <span className="num text-[11px] text-ink-3">{t("{percent} overall", { percent: formatPercent(aggregate.percent) })}</span>
              ) : null}
              {active.length > 0 ? <TransferGraph points={speedHistory} now={speedHistoryAt} /> : null}
            </div>
          }
        />
        <div id="downloads-active" className="border-t border-line">
          {active.length === 0 && (
            <p className="py-6 text-[13px] text-ink-3">{t("Nothing downloading. New installs appear here and in the status bar.")}</p>
          )}
          {active.map((job) => (
            <JobRow
              key={job.jobId}
              job={job}
              meter={meters[job.jobId]}
              title={titleFor(job)}
              coverUrl={coverFor(job)}
              year={yearFor(job)}
              queueRank={queueRank(job)}
              reorderable={isReorderableJob(job)}
              onCancel={() => void cancel(job.jobId)}
              onPause={() => void pause(job.jobId)}
              onOpenFolder={job.installPath ? () => void openFolder(job.installPath!) : undefined}
              onResume={() => void resume(job, { title: titleFor(job) })}
              onRestart={() => requestRestart(job)}
              onRetryNow={() => void retryNow(job.jobId)}
              onCheckSpace={() => void checkSpace(job.jobId)}
              onMoveTop={canMoveUp(active, job.jobId) ? () => void reorder(job.jobId, "top") : undefined}
              onMoveUp={canMoveUp(active, job.jobId) ? () => void reorder(job.jobId, "up") : undefined}
              onMoveDown={canMoveDown(active, job.jobId) ? () => void reorder(job.jobId, "down") : undefined}
            />
          ))}
          {waiting.length > 1 ? (
            <p className="py-2.5 text-[11.5px] leading-relaxed text-ink-3">
              {t("Queued installs start in this order. The running install keeps its place until it finishes, pauses or is cancelled.")}
            </p>
          ) : null}
        </div>
      </section>

      <section className="mt-10" aria-labelledby="downloads-history">
        <SectionHeader
          title={t("History")}
          count={history.length}
          actions={
            history.length > 0 ? (
              <button
                type="button"
                onClick={() => void clear()}
                className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
              >
                {t("Clear finished")}
              </button>
            ) : null
          }
        />
        <div id="downloads-history" className="border-t border-line">
          {history.length === 0 && (
            <p className="py-6 text-[13px] text-ink-3">{t("No finished downloads yet.")}</p>
          )}
          {history.map((job) => (
            <JobRow
              key={job.jobId}
              job={job}
              title={titleFor(job)}
              coverUrl={coverFor(job)}
              year={yearFor(job)}
              onCancel={() => void cancel(job.jobId)}
              onPause={() => void pause(job.jobId)}
              onOpenFolder={job.installPath ? () => void openFolder(job.installPath!) : undefined}
              onResume={() => void resume(job, { title: titleFor(job) })}
              onRestart={() => requestRestart(job)}
              onRetryAsDownload={() => void retryAsDownload(job, { title: titleFor(job) })}
              onRemove={REMOVABLE_PHASES.has(job.phase) ? () => requestRemove(job) : undefined}
            />
          ))}
        </div>
      </section>

      <p className="mt-6 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
        {tr("Install folders, stream installs and archive passwords are set in Settings. Downloads are indexed by the desktop app and survive a restart. Removing an entry from history never deletes the installed game, its folder or a partial {part} download.", {
          part: <span className="font-mono">.part</span>,
        })}
      </p>

      <ConfirmDialog
        open={restartTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRestartTarget(null);
        }}
        title={t("Restart the install?")}
        description={
          restartTarget
            ? t("{title} has no saved checkpoint, so restarting starts it from zero. The same Downloads row is reused; no second job is created.", {
                title: titleFor(restartTarget),
              })
            : ""
        }
        confirmLabel={t("Restart")}
        onConfirm={confirmRestart}
      />

      <ConfirmDialog
        open={removeTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRemoveTarget(null);
        }}
        title={t("Remove from Download history?")}
        description={
          removeTarget
            ? `${titleFor(removeTarget)} stays installed and its files are kept, including a partial .part download. Only this Downloads entry is removed.`
            : ""
        }
        confirmLabel={t("Remove from history")}
        onConfirm={confirmRemove}
      />
    </div>
  );
}

/** Per-job ETA label: estimating, gated duration, or "—" for an unknown total. */
function jobEta(job: InstallJob, meter?: JobMeter): string | null {
  if (!meter || meter.estimating) return "Estimating…";
  if (meter.etaSeconds !== null) return t("{duration} left", { duration: formatDuration(meter.etaSeconds) });
  if (!job.bytesTotal) return "—";
  return null;
}

/** Ticks once per second only while a retry is scheduled (#212). */
function useRetrySeconds(nextRetryAt: string | null): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!nextRetryAt) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [nextRetryAt]);
  return secondsUntilRetry(nextRetryAt, now);
}

function JobRow({
  job,
  meter,
  title,
  coverUrl,
  year,
  queueRank,
  reorderable,
  onCancel,
  onPause,
  onOpenFolder,
  onResume,
  onRestart,
  onRetryNow,
  onCheckSpace,
  onRetryAsDownload,
  onRemove,
  onMoveTop,
  onMoveUp,
  onMoveDown,
}: {
  job: InstallJob;
  meter?: JobMeter;
  title: string;
  coverUrl: string | null;
  year: string | null;
  /** 1-based place in the waiting queue; null for running/finished rows. */
  queueRank?: number | null;
  /** True for queued/paused rows: they can be moved, running/finished cannot. */
  reorderable?: boolean;
  onCancel: () => void;
  onPause: () => void;
  onOpenFolder?: () => void;
  onResume: () => void;
  onRestart: () => void;
  onRetryNow?: () => void;
  /** "Check again" for a job parked on disk space (#222). */
  onCheckSpace?: () => void;
  onRetryAsDownload?: () => void;
  onRemove?: () => void;
  onMoveTop?: () => void;
  onMoveUp?: () => void;
  onMoveDown?: () => void;
}) {
  const active = isActiveJob(job);
  const paused = job.phase === "paused";
  const removable = REMOVABLE_PHASES.has(job.phase);
  // #223: `null` while verifying turns the bar indeterminate instead of
  // freezing it at 100 % with the download's last rate.
  const pct = jobProgressPercent(job);
  const starting = active && !paused && job.bytesDone === 0 && !isVerifyingJob(job);
  const downloading = job.phase === "downloading";
  const verifying = isVerifyingJob(job);
  const waitingForSpace = isWaitingForSpaceJob(job);
  // Smoothed speed from the sampling layer; raw only until the first sample.
  // Verification and extraction report 0: a leftover download rate would look
  // like a frozen transfer (#223).
  const speed = jobDisplaySpeed(job, meter?.speedBps);
  const eta = downloading && !starting ? jobEta(job, meter) : null;
  // Verification/extraction/finalizing have no meaningful ETA: narrate the
  // phase and how long it has been running instead.
  const phaseElapsed =
    (verifying || job.phase === "extracting" || job.phase === "finalizing") && meter
      ? formatDuration(meter.elapsedMs / 1000)
      : null;
  // "Resume" only when the checkpoint really continues the job; a failed job
  // without one is an explicit "Restart" with a confirmation (#205/#208).
  const retryable = canRetryJob(job);
  const resumes = isTrulyResumable(job);
  const restartLabel = job.stream ? t("Restart stream") : t("Restart");
  const pausable =
    (job.phase === "queued" || job.phase === "downloading" || verifying) &&
    (job.stream !== true || job.resumable);
  const retryIn = useRetrySeconds(job.nextRetryAt);
  const done = job.phase === "done";
  const showMessage =
    Boolean(job.message) && !["Downloading archive", "Queued", "Resuming", "Paused"].includes(job.message ?? "");
  // #223: the bar is indeterminate while the archive is re-hashed; #222: a job
  // waiting for room shows no progress bar at all.
  const showBar = (active && !waitingForSpace) || (!done && pct !== null && pct > 0);
  const prefetch = prefetchLabel(job);
  const verifyLabel = verifyingLabel(job);
  const spaceLabel = spaceWaitLabel(job);
  const digestLabel = verifiedDigestLabel(job);

  return (
    <article className="flex items-start gap-4 border-b border-line py-3.5" aria-label={t("{title}, {phase}", { title, phase: installPhaseLabel(job.phase) })}>
      <div className="w-11 shrink-0 overflow-hidden rounded-[1px]">
        <GameCover title={title} coverUrl={coverUrl} showType={false} />
      </div>

      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
          <div className="flex min-w-0 items-baseline gap-2">
            <h3 className="truncate text-[14px] font-medium text-ink">{title}</h3>
            {year && <span className="num text-[10.5px] text-ink-3">{year.slice(0, 4)}</span>}
          </div>
          <div className="flex items-center gap-2">
            {queueRank ? (
              <span className="num text-[10.5px] text-ink-3" title={t("Place in the install queue")}>
                {t("Queue #{rank}", { rank: queueRank })}
              </span>
            ) : null}
            <Badge tone={done ? "ok" : job.phase === "error" ? "danger" : paused ? "outline" : active ? "signal" : "outline"}>
              {installPhaseLabel(job.phase)}
            </Badge>
          </div>
        </div>

        <div className="mt-1 flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
          {starting ? (
            <>
              <span>{t("Starting")}</span>
              {job.bytesTotal && <span>{formatBytes(job.bytesTotal)}</span>}
            </>
          ) : waitingForSpace ? (
            <span className="text-signal-text">{spaceLabel ?? t("Waiting for disk space")}</span>
          ) : (
            <>
              <span>{formatBytes(job.bytesDone)}</span>
              {job.bytesTotal && <span>{t("{done} of {total}", { done: formatBytes(job.bytesDone), total: formatBytes(job.bytesTotal) })}</span>}
              {job.reconnecting ? (
                <span className="text-signal-text" title={job.lastError ?? undefined}>
                  {job.waitingForNetwork
                    ? t("Waiting for the network")
                    : retryIn !== null
                      ? t("Retrying in {seconds} s, attempt {attempt}", { seconds: retryIn, attempt: job.attempt })
                      : t("Connection lost, resuming from {percent}", { percent: formatPercent(pct ?? 0) })}
                </span>
              ) : (
                speed > 0 && <span>{formatSpeed(speed)}</span>
              )}
              {eta && <span>{eta}</span>}
              {verifying && <span className="text-signal-text">{verifyLabel}</span>}
              {phaseElapsed && !verifying && (
                <span>{t("{phase} for {duration}", { phase: installPhaseLabel(job.phase), duration: phaseElapsed })}</span>
              )}
              {verifying && phaseElapsed && <span>{verifyingElapsedLabel((meter?.elapsedMs ?? 0))}</span>}
              {pct !== null && <span>{formatPercent(pct, 1)}</span>}
              {job.entriesTotal ? (
                <span>{t("{done}/{total} files", { done: job.entriesDone ?? 0, total: job.entriesTotal })}</span>
              ) : null}
            </>
          )}
          {job.phase === "interrupted" && <span className="text-signal-text">{t("resume keeps the bytes already received")}</span>}
        </div>

        {showBar && (
          <div className="mt-2 max-w-[520px]">
            <Progress
              value={starting ? null : pct}
              label={t("Install progress for {title}", { title })}
              tone={job.phase === "error" ? "danger" : "signal"}
            />
          </div>
        )}

        {showMessage && (
          <p className={cn("mt-1.5 text-[12px] leading-snug", job.phase === "error" ? "text-danger" : "text-ink-3")}>
            {job.message}
          </p>
        )}

        {prefetch && (
          <p className="mt-1 text-[11px] leading-snug text-signal-text">{prefetch}</p>
        )}

        {digestLabel && (
          <p className="mt-1 font-mono text-[10.5px] text-ink-3" title={job.verifiedSha256 ?? undefined}>
            {t("Archive verified · {digest}", { digest: digestLabel })}
          </p>
        )}

        <div className="mt-2 flex flex-wrap items-center gap-x-1 gap-y-1">
          {reorderable ? (
            <div
              className="flex items-center gap-0.5"
              role="group"
              aria-label={t("Queue order for {title}", { title })}
            >
              <Button
                variant="ghost"
                size="sm"
                className="px-2 text-ink-2"
                icon={<ArrowUpToLine className="size-3.5" aria-hidden="true" />}
                disabled={!onMoveTop}
                onClick={onMoveTop}
                aria-label={t("Move {title} to the top of the queue", { title })}
                title={t("Move {title} to the top of the queue", { title })}
              />
              <Button
                variant="ghost"
                size="sm"
                className="px-2 text-ink-2"
                icon={<ArrowUp className="size-3.5" aria-hidden="true" />}
                disabled={!onMoveUp}
                onClick={onMoveUp}
                aria-label={t("Move {title} up one place in the queue", { title })}
                title={t("Move {title} up one place in the queue", { title })}
              />
              <Button
                variant="ghost"
                size="sm"
                className="px-2 text-ink-2"
                icon={<ArrowDown className="size-3.5" aria-hidden="true" />}
                disabled={!onMoveDown}
                onClick={onMoveDown}
                aria-label={t("Move {title} down one place in the queue", { title })}
                title={t("Move {title} down one place in the queue", { title })}
              />
            </div>
          ) : null}
          {waitingForSpace && onCheckSpace && (
            <Button
              variant="ghost"
              size="sm"
              className="text-signal-text"
              icon={<RefreshCw className="size-3.5" />}
              onClick={onCheckSpace}
              title={t("Re-evaluates the disk reservation of this install")}
            >
              {t("Check again")}
            </Button>
          )}
          {active && (
            <button
              type="button"
              onClick={onCancel}
              className="flex h-8 items-center gap-1.5 pr-2 text-[12px] text-danger underline-offset-4 hover:underline"
            >
              <X className="size-3.5" aria-hidden="true" />
              {t("Cancel")}
            </button>
          )}
          {pausable && (
            <Button variant="ghost" size="sm" className="text-ink-2" icon={<Pause className="size-3.5" />} onClick={onPause}>
              {t("Pause")}
            </Button>
          )}
          {active && canRetryNow(job) && onRetryNow && (
            <Button
              variant="ghost"
              size="sm"
              className="text-signal-text"
              icon={<RefreshCw className="size-3.5" />}
              onClick={onRetryNow}
              title={t("Skips the current wait and retries immediately")}
            >
              {t("Retry now")}
            </Button>
          )}
          {retryable && (
            <Button
              variant="ghost"
              size="sm"
              className="text-signal-text"
              icon={paused ? <Play className="size-3.5" /> : <RefreshCw className="size-3.5" />}
              onClick={resumes ? onResume : onRestart}
              title={
                resumes
                  ? t("Continues from the saved checkpoint with the same job")
                  : t("Starts from zero and reuses the same Downloads row")
              }
            >
              {resumes ? t("Resume") : restartLabel}
            </Button>
          )}
          {retryable && job.stream && onRetryAsDownload && (
            <Button
              variant="ghost"
              size="sm"
              className="text-signal-text"
              icon={<Download className="size-3.5" />}
              onClick={onRetryAsDownload}
              title={t("Downloads the archive instead and resumes with the same job")}
            >
              {t("Retry as download")}
            </Button>
          )}
          {retryable && !resumes && (
            <span className="px-1 text-[11px] leading-snug text-ink-3">
              {t("No saved checkpoint: restarting starts from zero and keeps one Downloads row.")}
            </span>
          )}
          {onOpenFolder && (
            <Button variant="ghost" size="sm" className="text-ink-3" icon={<FolderOpen className="size-3.5" />} onClick={onOpenFolder}>
              {t("Open folder")}
            </Button>
          )}
          {removable && onRemove && (
            <button
              type="button"
              onClick={onRemove}
              aria-label={t("Remove {title} from history", { title })}
              title={t("Only the history entry is deleted; the installed files and any .part download are kept")}
              className="flex h-8 items-center gap-1.5 px-1 text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
            >
              <Trash2 className="size-3.5" aria-hidden="true" />
              {t("Remove from history")}
            </button>
          )}
          {done && (
            <span className="flex items-center gap-1.5 px-1 text-[11.5px] text-ok">
              <Check className="size-3.5" aria-hidden="true" />
              {t("Ready to play")}
            </span>
          )}
          <Link
            to={`/game/${job.gameId}`}
            className="ml-auto flex h-8 items-center px-1 text-[12px] text-ink-3 hover:text-ink"
          >
            {t("Game page")}
          </Link>
        </div>
      </div>
    </article>
  );
}
