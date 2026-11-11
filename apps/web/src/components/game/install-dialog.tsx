import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Download, FolderOpen } from "lucide-react";
import { installBridge } from "@/lib/install";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatEta, formatSpeed, installPhaseLabel, formatPercent } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import type { GameDetail, GameVersion, LocationStatus } from "@/lib/types";
import { isStreamFailedJob, useDownloads } from "@/state/downloads";
import { Button, Progress } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { Dialog } from "@/components/ui/dialog";
import { t, tn } from "@/lib/i18n";

/** Keep in sync with `locations::FREE_SPACE_RESERVE_BYTES` (#99). */
const FREE_SPACE_RESERVE = 256 * 1024 * 1024;

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
  const [installLocationId, setInstallLocationId] = useState<string>("");
  const [downloadLocationId, setDownloadLocationId] = useState<string>("");

  const { data: settings } = useQuery({
    queryKey: ["desktop-settings"],
    queryFn: () => installBridge.settings(),
    enabled: open,
    staleTime: 30_000,
  });

  const { data: locations } = useQuery<LocationStatus[]>({
    queryKey: ["desktop-locations"],
    queryFn: () => native.listLocations(),
    enabled: open && isNative(),
    staleTime: 20_000,
  });

  const installLocations = useMemo(
    () => (locations ?? []).filter((location) => location.kind === "install"),
    [locations],
  );
  const downloadLocations = useMemo(
    () => (locations ?? []).filter((location) => location.kind === "download" || location.kind === "temp"),
    [locations],
  );

  const defaultInstallId =
    installLocations.find((location) => location.default)?.id ?? installLocations[0]?.id ?? "";

  useEffect(() => {
    if (!open) {
      setError(null);
      setStarting(false);
      return;
    }
    // Preselect the default location every time the dialog opens (and once the
    // probe list finishes loading).
    setInstallLocationId(defaultInstallId);
    setDownloadLocationId("");
  }, [open, defaultInstallId]);

  const installStatus = installLocations.find((location) => location.id === installLocationId) ?? null;
  const downloadStatus = downloadLocations.find((location) => location.id === downloadLocationId) ?? null;
  const sameVolume = !downloadStatus || !installStatus || downloadStatus.path === installStatus.path;
  const requiredBytes = version.size * (sameVolume ? 2 : 1) + FREE_SPACE_RESERVE;
  const spaceError = installStatus?.freeBytes != null && installStatus.freeBytes < requiredBytes
    ? t("Not enough free space on {label}: {free} available, about {needed} needed.", {
        label: installStatus.label,
        free: formatBytes(installStatus.freeBytes),
        needed: formatBytes(requiredBytes),
      })
    : downloadStatus?.freeBytes != null && !sameVolume && downloadStatus.freeBytes < version.size + FREE_SPACE_RESERVE
      ? t("The download location {label} is too small for this archive.", { label: downloadStatus.label })
      : null;
  const blocked = Boolean(spaceError) || Boolean(installStatus && !installStatus.writable);

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
        locationId: installLocationId || undefined,
        downloadLocationId: downloadLocationId || undefined,
        expectedBytes: version.size,
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
            <Button
              variant="primary"
              loading={starting}
              disabled={blocked}
              onClick={() => void start()}
              icon={<Download className="size-4" />}
            >
              {t("Install")}
            </Button>
          </>
        )
      }
    >
      <div className="flex flex-col gap-4">
        <div className="grid gap-4 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-2">{t("Install location")}</span>
            {installLocations.length > 0 ? (
              <Select
                label={t("Install location")}
                value={installLocationId}
                onChange={setInstallLocationId}
                options={installLocations.map((location) => ({
                  value: location.id,
                  label: location.default ? t("{label} (default)", { label: location.label }) : location.label,
                  hint: location.freeBytes != null ? t("{free} free", { free: formatBytes(location.freeBytes) }) : location.note ?? undefined,
                }))}
                className="h-10 text-[12px]"
              />
            ) : (
              <p className="truncate font-mono text-[11px] text-ink-2">{settings?.installDir || t("Set in Settings")}</p>
            )}
            {installStatus && (
              <p className="font-mono text-[10.5px] text-ink-3">
                {installStatus.writable ? t("Writable") : t("Not writable")} · {installStatus.path}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-2">{t("Download location")}</span>
            <Select
              label={t("Download location")}
              value={downloadLocationId}
              onChange={setDownloadLocationId}
              options={[
                { value: "", label: t("Same as install"), hint: t("Archive deleted after extraction") },
                ...downloadLocations.map((location) => ({
                  value: location.id,
                  label: location.label,
                  hint: location.freeBytes != null ? t("{free} free", { free: formatBytes(location.freeBytes) }) : location.note ?? undefined,
                })),
              ]}
              className="h-10 text-[12px]"
            />
            <p className="text-[10.5px] leading-relaxed text-ink-3">
              {settings?.streamInstall
                ? t("Streamed, not stored (resumes per file)")
                : sameVolume
                  ? t("Peak usage is roughly twice the archive size.")
                  : t("The archive is stored on another volume, then unpacked into the install location.")}
            </p>
          </div>
        </div>

        <dl className="flex flex-col gap-2 border-t border-line pt-3 text-[12.5px]">
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

        {spaceError && (
          <p className="border-l-2 border-signal pl-3 text-[12px] text-signal-text">{spaceError}</p>
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
