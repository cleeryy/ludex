import { useMemo } from "react";
import { Link } from "react-router-dom";
import { Check, FolderOpen, RefreshCw, X } from "lucide-react";
import { installsAvailable } from "@/lib/install";
import { isActiveJob, useDownloads } from "@/state/downloads";
import { useAllGames } from "@/lib/hooks";
import { formatBytes, formatEta, formatSpeed, installPhaseLabel } from "@/lib/format";
import type { InstallJob } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { EmptyState, PageHeader, SectionHeader } from "@/components/page";
import { GameCover } from "@/components/art/cover";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { cn } from "@/lib/cn";

export function DownloadsScreen() {
  const downloads = useDownloads();
  const { push } = useToast();
  const { data } = useAllGames();
  const gamesById = useMemo(() => new Map((data?.items ?? []).map((game) => [game.id, game])), [data]);

  const coverFor = (job: InstallJob) => gamesById.get(job.gameId)?.coverUrl ?? null;
  const yearFor = (job: InstallJob) => gamesById.get(job.gameId)?.releaseDate ?? null;

  if (!installsAvailable()) {
    return (
      <div className="page page-in">
        <PageHeader title="Downloads" description="Install jobs run inside the Ludex desktop app." />
        <div className="mt-8">
          <EmptyState
            title="Downloads live on the desktop"
            description="The browser build can browse the library and download archives. Installing, launching and playtime need the Ludex desktop app."
          >
            <Link to="/library">
              <Button variant="default">Open the library</Button>
            </Link>
          </EmptyState>
        </div>
      </div>
    );
  }

  const { active, history, activeCount, aggregate, titleFor, cancel, resume, clearHistory, openFolder } = downloads;

  const clear = async () => {
    await clearHistory();
    push({ title: "Finished downloads cleared" });
  };

  return (
    <div className="page page-in">
      <PageHeader
        title="Downloads"
        meta={
          <>
            <span className="num">{activeCount} active</span>
            <span className="num">{history.length} in history</span>
            {aggregate?.speedBps ? <span className="num">{formatSpeed(aggregate.speedBps)}</span> : null}
            {aggregate?.eta ? <span className="num">{aggregate.eta} left</span> : null}
          </>
        }
        description="Installs keep running while you use the rest of the app. Closing the install window does not stop them."
        actions={
          <Link to="/library">
            <Button variant="ghost" size="sm">
              Browse the library
            </Button>
          </Link>
        }
      />

      <section className="mt-8" aria-labelledby="downloads-active">
        <SectionHeader
          title="Active"
          count={activeCount}
          actions={
            aggregate?.percent !== null && aggregate ? (
              <span className="num text-[11px] text-ink-3">{aggregate.percent.toFixed(0)}% overall</span>
            ) : null
          }
        />
        <div id="downloads-active" className="border-t border-line">
          {active.length === 0 && (
            <p className="py-6 text-[13px] text-ink-3">Nothing downloading. New installs appear here and in the status bar.</p>
          )}
          {active.map((job) => (
            <JobRow
              key={job.jobId}
              job={job}
              title={titleFor(job)}
              coverUrl={coverFor(job)}
              year={yearFor(job)}
              onCancel={() => void cancel(job.jobId)}
              onOpenFolder={job.installPath ? () => void openFolder(job.installPath!) : undefined}
              onResume={() => void resume(job, { title: titleFor(job) })}
            />
          ))}
        </div>
      </section>

      <section className="mt-10" aria-labelledby="downloads-history">
        <SectionHeader
          title="History"
          count={history.length}
          actions={
            history.length > 0 ? (
              <button
                type="button"
                onClick={() => void clear()}
                className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
              >
                Clear finished
              </button>
            ) : null
          }
        />
        <div id="downloads-history" className="border-t border-line">
          {history.length === 0 && (
            <p className="py-6 text-[13px] text-ink-3">No finished downloads yet.</p>
          )}
          {history.map((job) => (
            <JobRow
              key={job.jobId}
              job={job}
              title={titleFor(job)}
              coverUrl={coverFor(job)}
              year={yearFor(job)}
              onCancel={() => void cancel(job.jobId)}
              onOpenFolder={job.installPath ? () => void openFolder(job.installPath!) : undefined}
              onResume={() => void resume(job, { title: titleFor(job) })}
            />
          ))}
        </div>
      </section>

      <p className="mt-6 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
        Install folders, stream installs and archive passwords are set in Settings. Downloads are indexed by the desktop app
        and survive a restart.
      </p>
    </div>
  );
}

function JobRow({
  job,
  title,
  coverUrl,
  year,
  onCancel,
  onOpenFolder,
  onResume,
}: {
  job: InstallJob;
  title: string;
  coverUrl: string | null;
  year: string | null;
  onCancel: () => void;
  onOpenFolder?: () => void;
  onResume: () => void;
}) {
  const active = isActiveJob(job);
  const pct = job.bytesTotal ? Math.min(100, (job.bytesDone / job.bytesTotal) * 100) : null;
  const eta = active && job.bytesTotal && job.speedBps ? formatEta(job.bytesDone, job.bytesTotal, job.speedBps) : null;
  const resumable = job.phase === "error" || job.phase === "cancelled" || job.phase === "interrupted";
  const done = job.phase === "done";
  const showMessage = Boolean(job.message) && !["Downloading archive", "Queued", "Resuming"].includes(job.message ?? "");
  const starting = active && job.bytesDone === 0;
  const showBar = active || (!done && pct !== null && pct > 0);

  return (
    <article className="flex items-start gap-4 border-b border-line py-3.5" aria-label={`${title}, ${installPhaseLabel(job.phase)}`}>
      <div className="w-11 shrink-0 overflow-hidden rounded-[1px]">
        <GameCover title={title} coverUrl={coverUrl} showType={false} />
      </div>

      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
          <div className="flex min-w-0 items-baseline gap-2">
            <h3 className="truncate text-[14px] font-medium text-ink">{title}</h3>
            {year && <span className="num text-[10.5px] text-ink-3">{year.slice(0, 4)}</span>}
          </div>
          <Badge tone={done ? "ok" : job.phase === "error" ? "danger" : active ? "signal" : "outline"}>
            {installPhaseLabel(job.phase)}
          </Badge>
        </div>

        <div className="mt-1 flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
          {starting ? (
            <>
              <span>Starting</span>
              {job.bytesTotal && <span>{formatBytes(job.bytesTotal)}</span>}
            </>
          ) : (
            <>
              <span>{formatBytes(job.bytesDone)}</span>
              {job.bytesTotal && <span>of {formatBytes(job.bytesTotal)}</span>}
              {job.speedBps > 0 && <span>{formatSpeed(job.speedBps)}</span>}
              {eta && <span>{eta} left</span>}
              {pct !== null && <span>{pct.toFixed(1)}%</span>}
            </>
          )}
          {job.phase === "interrupted" && <span className="text-signal-text">resume keeps the bytes already received</span>}
        </div>

        {showBar && (
          <div className="mt-2 max-w-[520px]">
            <Progress
              value={starting ? null : pct}
              label={`Install progress for ${title}`}
              tone={job.phase === "error" ? "danger" : "signal"}
            />
          </div>
        )}

        {showMessage && (
          <p className={cn("mt-1.5 text-[12px] leading-snug", job.phase === "error" ? "text-danger" : "text-ink-3")}>
            {job.message}
          </p>
        )}

        <div className="mt-2 flex flex-wrap items-center gap-x-1 gap-y-1">
          {active && (
            <button
              type="button"
              onClick={onCancel}
              className="flex h-8 items-center gap-1.5 pr-2 text-[12px] text-danger underline-offset-4 hover:underline"
            >
              <X className="size-3.5" aria-hidden="true" />
              Cancel
            </button>
          )}
          {resumable && (
            <Button variant="ghost" size="sm" className="text-signal-text" icon={<RefreshCw className="size-3.5" />} onClick={onResume}>
              Resume
            </Button>
          )}
          {onOpenFolder && (
            <Button variant="ghost" size="sm" className="text-ink-3" icon={<FolderOpen className="size-3.5" />} onClick={onOpenFolder}>
              Open folder
            </Button>
          )}
          {done && (
            <span className="flex items-center gap-1.5 px-1 text-[11.5px] text-ok">
              <Check className="size-3.5" aria-hidden="true" />
              Ready to play
            </span>
          )}
          <Link
            to={`/game/${job.gameId}`}
            className="ml-auto flex h-8 items-center px-1 text-[12px] text-ink-3 hover:text-ink"
          >
            Game page
          </Link>
        </div>
      </div>
    </article>
  );
}
