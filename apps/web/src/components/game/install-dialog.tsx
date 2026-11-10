import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Download, FolderOpen } from "lucide-react";
import { installBridge } from "@/lib/install";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatEta, formatSpeed, installPhaseLabel } from "@/lib/format";
import type { GameDetail, GameVersion } from "@/lib/types";
import { useDownloads } from "@/state/downloads";
import { Button, Progress } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";

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

  const pct = job?.bytesTotal ? Math.min(100, (job.bytesDone / job.bytesTotal) * 100) : null;
  const paused = job?.phase === "paused";
  const active = job && ["queued", "downloading", "extracting", "finalizing"].includes(job.phase);
  const failed = job && ["error", "cancelled", "interrupted"].includes(job.phase);
  const resumable = paused || failed;
  const eta = job && active && job.bytesTotal ? formatEta(job.bytesDone, job.bytesTotal, job.speedBps) : null;

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Install ${game.title}`}
      description={version.version ? `Version ${version.version} · ${formatBytes(version.size)}` : formatBytes(version.size)}
      size="sm"
      footer={
        job?.phase === "done" ? (
          <Button variant="primary" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        ) : active ? (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              Continue in background
            </Button>
            {job.phase === "queued" || job.phase === "downloading" ? (
              <Button variant="default" onClick={() => void downloads.pause(job.jobId)}>
                Pause
              </Button>
            ) : null}
            <Button variant="danger" onClick={() => void downloads.cancel(job.jobId)}>
              Cancel
            </Button>
          </>
        ) : resumable ? (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              Close
            </Button>
            <Button variant="primary" loading={starting} onClick={() => void resume()}>
              Resume
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button variant="primary" loading={starting} onClick={() => void start()} icon={<Download className="size-4" />}>
              Install
            </Button>
          </>
        )
      }
    >
      <div className="flex flex-col gap-4">
        <dl className="flex flex-col gap-2 text-[12.5px]">
          <div className="flex items-baseline justify-between gap-4">
            <dt className="text-ink-3">Install folder</dt>
            <dd className="truncate font-mono text-[11px] text-ink-2">{settings?.installDir || "Set in Settings"}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-4">
            <dt className="text-ink-3">Download folder</dt>
            <dd className="truncate font-mono text-[11px] text-ink-2">
              {settings?.streamInstall ? "Streamed, not stored" : settings?.downloadDir || "Set in Settings"}
            </dd>
          </div>
          <div className="flex items-baseline justify-between gap-4">
            <dt className="text-ink-3">Archive</dt>
            <dd className="truncate font-mono text-[11px] text-ink-2">{version.fileName}</dd>
          </div>
        </dl>

        {job && (
          <div className="flex flex-col gap-2 border-t border-line pt-4">
            <div className="flex items-baseline justify-between gap-3">
              <span className="text-[12.5px] text-ink">{installPhaseLabel(job.phase)}</span>
              {pct !== null && <span className="num text-[11.5px] text-ink-3">{pct.toFixed(0)}%</span>}
            </div>
            <Progress value={pct} label="Install progress" />
            <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
              <span>{formatBytes(job.bytesDone)}</span>
              {job.bytesTotal && <span>of {formatBytes(job.bytesTotal)}</span>}
              {job.speedBps > 0 && <span>{formatSpeed(job.speedBps)}</span>}
              {eta && <span>{eta} left</span>}
              {job.message && <span className="text-ink-2">{job.message}</span>}
            </div>
            {active && (
              <p className="text-[11.5px] leading-relaxed text-ink-3">
                You can close this window. The install keeps running and stays in Downloads.
              </p>
            )}
            {job.installPath && (
              <button
                type="button"
                onClick={() => void downloads.openFolder(job.installPath!)}
                className="flex w-fit items-center gap-1.5 text-[11.5px] text-ink-3 hover:text-ink"
              >
                <FolderOpen className="size-3.5" aria-hidden="true" />
                Open install folder
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
