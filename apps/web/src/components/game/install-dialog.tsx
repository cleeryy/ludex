import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Download, FolderOpen } from "lucide-react";
import { installBridge } from "@/lib/install";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatEta, formatSpeed, installPhaseLabel, formatPercent } from "@/lib/format";
import type { GameDetail, GameVersion } from "@/lib/types";
import { isStreamFailedJob, useDownloads } from "@/state/downloads";
import { Button, Progress } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { t, tn } from "@/lib/i18n";

/**
 * The install dialog starts a background job and then gets out of the way: the
 * job lives in DownloadsProvider, so closing this window does not stop it.
 */
export function InstallDialog({
  open,
  onOpenChange,
  game,
  version,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  game: GameDetail;
  version: GameVersion;
}) {
  const downloads = useDownloads();
  const job = downloads.jobForGame(game.id) ?? downloads.resumableForGame(game.id);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { data: settings } = useQuery({
    queryKey: ["desktop-settings"],
    queryFn: () => installBridge.settings(),
    enabled: open,
    staleTime: 30_000,
  });

  useEffect(() => {
    if (!open) {
      setError(null);
      setStarting(false);
    }
  }, [open]);

  const start = async () => {
    setError(null);
    setStarting(true);
    try {
      await downloads.startInstall({
        gameId: game.id,
        versionId: version.id,
        title: game.title,
        version: version.version,
        executable: game.launch.executable,
      });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setStarting(false);
    }
  };

  const resume = async () => {
    if (!job) return;
    setError(null);
    setStarting(true);
    try {
      await downloads.resume(job, {
        title: game.title,
        version: version.version,
        executable: game.launch.executable,
      });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setStarting(false);
    }
  };

  const retryAsDownload = async () => {
    if (!job) return;
    setError(null);
    setStarting(true);
    try {
      await downloads.retryAsDownload(job, {
        title: game.title,
        version: version.version,
        executable: game.launch.executable,
      });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setStarting(false);
    }
  };

  const pct = job?.bytesTotal ? Math.min(100, (job.bytesDone / job.bytesTotal) * 100) : null;
  const paused = job?.phase === "paused";
  const active = job && ["queued", "downloading", "extracting", "finalizing"].includes(job.phase);
  const failed = job && ["error", "cancelled", "interrupted"].includes(job.phase);
  // A stream install cannot resume: offer a normal download retry instead of a
  // Resume button that would restart the stream from zero (#114).
  const streamFailed = job ? isStreamFailedJob(job) : false;
  const resumable = !streamFailed && (paused || failed);
  const eta = job && active && job.bytesTotal ? formatEta(job.bytesDone, job.bytesTotal, job.speedBps) : null;

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("Install {title}", { title: game.title })}
      description={version.version ? `Version ${version.version} · ${formatBytes(version.size)}` : formatBytes(version.size)}
      size="sm"
      footer={
        job?.phase === "done" ? (
          <Button variant="primary" onClick={() => onOpenChange(false)}>
            {t("Done")}
          </Button>
        ) : active ? (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              {t("Continue in background")}
            </Button>
            {(job.phase === "queued" || job.phase === "downloading") &&
            (job.stream !== true || job.resumable) ? (
              <Button variant="default" onClick={() => void downloads.pause(job.jobId)}>
                {t("Pause")}
              </Button>
            ) : null}
            <Button variant="danger" onClick={() => void downloads.cancel(job.jobId)}>
              {t("Cancel")}
            </Button>
          </>
        ) : streamFailed ? (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              {t("Close")}
            </Button>
            <Button
              variant="ghost"
              onClick={() =>
                job &&
                void downloads.retryStream(job, { title: game.title, version: version.version, executable: game.launch.executable })
              }
            >
              {t("Retry stream")}
            </Button>
            <Button variant="primary" loading={starting} onClick={() => void retryAsDownload()} icon={<Download className="size-4" />}>
              {t("Retry as download")}
            </Button>
          </>
        ) : resumable ? (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              {t("Close")}
            </Button>
            <Button variant="primary" loading={starting} onClick={() => void resume()}>
              {t("Resume")}
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" loading={starting} onClick={() => void start()} icon={<Download className="size-4" />}>
              {t("Install")}
            </Button>
          </>
        )
      }
    >
      <div className="flex flex-col gap-4">
        <dl className="flex flex-col gap-2 text-[12.5px]">
          <div className="flex items-baseline justify-between gap-4">
            <dt className="text-ink-3">{t("Install folder")}</dt>
            <dd className="truncate font-mono text-[11px] text-ink-2">{settings?.installDir || t("Set in Settings")}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-4">
            <dt className="text-ink-3">{t("Download folder")}</dt>
            <dd className="truncate font-mono text-[11px] text-ink-2">
              {settings?.streamInstall
                ? t("Streamed, not stored (resumes per file)")
                : settings?.downloadDir || t("Set in Settings")}
            </dd>
          </div>
          <div className="flex items-baseline justify-between gap-4">
            <dt className="text-ink-3">{t("Archive")}</dt>
            <dd className="truncate font-mono text-[11px] text-ink-2">{version.fileName}</dd>
          </div>
        </dl>

        {job && (
          <div className="flex flex-col gap-2 border-t border-line pt-4">
            <div className="flex items-baseline justify-between gap-3">
              <span className="text-[12.5px] text-ink">{installPhaseLabel(job.phase)}</span>
              {pct !== null && <span className="num text-[11.5px] text-ink-3">{formatPercent(pct)}</span>}
            </div>
            <Progress value={pct} label={t("Install progress")} />
            <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
              <span>{formatBytes(job.bytesDone)}</span>
              {job.bytesTotal && <span>{t("{done} of {total}", { done: formatBytes(job.bytesDone), total: formatBytes(job.bytesTotal) })}</span>}
              {job.speedBps > 0 && <span>{formatSpeed(job.speedBps)}</span>}
              {eta && <span>{t("{duration} left", { duration: eta })}</span>}
              {job.entriesTotal ? (
                <span>
                  {t("{done}/{total} files", {
                    done: job.entriesDone ?? 0,
                    total: job.entriesTotal,
                  })}
                </span>
              ) : null}
              {job.message && <span className="text-ink-2">{job.message}</span>}
            </div>
            {job.currentEntry && (
              <p className="truncate font-mono text-[10.5px] text-ink-3" title={job.currentEntry}>
                {job.currentEntry}
              </p>
            )}
            {job.resumedEntries !== null && job.resumedEntries > 0 && (
              <p className="text-[11.5px] leading-relaxed text-ink-3">
                {tn(
                  job.resumedEntries,
                  "Resumed {count} file already on disk.",
                  "Resumed {count} files already on disk.",
                  { count: job.resumedEntries },
                )}
              </p>
            )}
            {active && (
              <p className="text-[11.5px] leading-relaxed text-ink-3">
                {t("You can close this window. The install keeps running and stays in Downloads.")}
              </p>
            )}
            {streamFailed && (
              <p className="text-[11.5px] leading-relaxed text-ink-3">
                {t(
                  "This stream has no per-file checkpoint and cannot be resumed; retrying as a download keeps the archive so it can resume.",
                )}
              </p>
            )}
            {job.stream && job.resumable && !streamFailed && (
              <p className="text-[11.5px] leading-relaxed text-ink-3">
                {t(
                  "Stream installs resume file by file: closing or losing the connection never re-downloads completed files.",
                )}
              </p>
            )}
            {job.installPath && (
              <button
                type="button"
                onClick={() => void downloads.openFolder(job.installPath!)}
                className="flex w-fit items-center gap-1.5 text-[11.5px] text-ink-3 hover:text-ink"
              >
                <FolderOpen className="size-3.5" aria-hidden="true" />
                {t("Open install folder")}
              </button>
            )}
          </div>
        )}

        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
            {error}
          </p>
        )}
      </div>
    </Dialog>
  );
}
