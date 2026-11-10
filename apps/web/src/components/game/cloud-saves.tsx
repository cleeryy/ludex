import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Cloud,
  CloudOff,
  Download,
  FileArchive,
  RefreshCw,
  RotateCcw,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import {
  CLOUD_SAVES_DESKTOP_ONLY,
  cloudSaveReasonText,
  cloudSaves,
  cloudSavesAvailable,
  shortDeviceId,
} from "@/lib/cloud-saves";
import { errorMessage } from "@/lib/http";
import { cn } from "@/lib/cn";
import { formatBytes, formatDate, formatPercent } from "@/lib/format";
import type { CloudSaveStatus, SaveSnapshot } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Button, IconButton, Input, Progress, Skeleton, Switch } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { tr, t } from "@/lib/i18n";

const MAX_SAVE_BYTES = 2 * 1024 ** 3;

/**
 * Per-user save snapshots plus automatic sync (#108).
 *
 * Manual upload/download works in the browser. Automation reads and writes
 * save files on this machine, so the browser build explains that it needs the
 * desktop app instead of pretending to sync.
 */
export function CloudSaves({ gameId, title }: { gameId: number; title: string }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const native = cloudSavesAvailable();
  const saves = useQuery({
    queryKey: ["saves", gameId],
    queryFn: () => api.listSaves(gameId),
  });
  const cloud = useQuery({
    queryKey: ["cloud-save-status", gameId],
    queryFn: () => cloudSaves.status(gameId),
    enabled: native,
  });
  const status = cloud.data ?? null;

  const [composerOpen, setComposerOpen] = useState(false);
  const [file, setFile] = useState<File | null>(null);
  const [label, setLabel] = useState("");
  const [dragging, setDragging] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [progress, setProgress] = useState<{ loaded: number; total: number } | null>(null);
  const [downloadingId, setDownloadingId] = useState<number | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<SaveSnapshot | null>(null);
  const [deleting, setDeleting] = useState(false);

  const [toggling, setToggling] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [conflictOpen, setConflictOpen] = useState(false);
  const [conflictBusy, setConflictBusy] = useState<"remote" | "keep_local" | null>(null);

  const fileInput = useRef<HTMLInputElement>(null);
  const labelInput = useRef<HTMLInputElement>(null);

  const rows = [...(saves.data ?? [])].sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
  const pct = progress && progress.total > 0 ? Math.min(100, (progress.loaded / progress.total) * 100) : null;
  const conflict = status?.conflict ?? null;

  useEffect(() => {
    if (conflict) setConflictOpen(true);
  }, [conflict?.remoteSha256, conflict?.remoteCreatedAt]); // eslint-disable-line react-hooks/exhaustive-deps

  // Reflect post-session uploads while the page stays open.
  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    const unlisteners: Array<() => void> = [];
    const refresh = (id: number) => {
      if (id === gameId) {
        void queryClient.invalidateQueries({ queryKey: ["cloud-save-status", gameId] });
      }
    };
    void Promise.all([cloudSaves.onSynced(refresh), cloudSaves.onPending(refresh)]).then((offs) => {
      if (cancelled) offs.forEach((off) => off());
      else unlisteners.push(...offs);
    });
    return () => {
      cancelled = true;
      unlisteners.forEach((off) => off());
    };
  }, [native, gameId]); // eslint-disable-line react-hooks/exhaustive-deps

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["saves", gameId] });
  const invalidateStatus = () => queryClient.invalidateQueries({ queryKey: ["cloud-save-status", gameId] });

  const resetComposer = () => {
    setFile(null);
    setLabel("");
    setProgress(null);
    if (fileInput.current) fileInput.current.value = "";
  };

  const pick = (picked: File | undefined) => {
    if (!picked) return;
    if (!/\.zip$/i.test(picked.name)) {
      if (fileInput.current) fileInput.current.value = "";
      push({ title: t("Use a .zip archive"), description: t("Save snapshots are stored as zip files."), tone: "danger" });
      return;
    }
    if (picked.size > MAX_SAVE_BYTES) {
      if (fileInput.current) fileInput.current.value = "";
      push({ title: t("Archive too large"), description: t("Saves are limited to 2 GiB."), tone: "danger" });
      return;
    }
    setFile(picked);
    requestAnimationFrame(() => labelInput.current?.focus());
  };

  const upload = async () => {
    if (!file || uploading) return;
    setUploading(true);
    setProgress(null);
    try {
      const snapshot = await api.uploadSave(gameId, file, label.trim() || undefined, (loaded, total) =>
        setProgress({ loaded, total }),
      );
      await invalidate();
      push({
        title: t("Save uploaded"),
        description: `${snapshot.label} · ${formatBytes(snapshot.size)}`,
        tone: "success",
      });
      resetComposer();
      setComposerOpen(false);
    } catch (cause) {
      push({ title: t("Upload did not finish"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setUploading(false);
      setProgress(null);
    }
  };

  const download = async (snapshot: SaveSnapshot) => {
    setDownloadingId(snapshot.id);
    try {
      await api.downloadSave(snapshot.id);
      if (USING_MOCKS) {
        push({ title: t("Demo build"), description: t("The archive is a placeholder; a real server streams the .zip.") });
      } else {
        push({ title: t("Save downloaded"), description: snapshot.fileName });
      }
    } catch (cause) {
      push({ title: t("Download did not start"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setDownloadingId(null);
    }
  };

  const remove = async () => {
    if (!deleteTarget || deleting) return;
    setDeleting(true);
    try {
      await api.deleteSave(deleteTarget.id);
      await invalidate();
      push({ title: t("Save deleted"), description: t("{label} is no longer on your account.", { label: deleteTarget.label }), tone: "success" });
      setDeleteTarget(null);
    } catch (cause) {
      push({ title: t("Could not delete the save"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setDeleting(false);
    }
  };

  const toggleSync = async (enabled: boolean) => {
    if (toggling) return;
    setToggling(true);
    try {
      await cloudSaves.setSync(gameId, enabled);
      await invalidateStatus();
      await queryClient.invalidateQueries({ queryKey: ["desktop-settings"] });
      push({
        title: enabled ? "Automatic backup on" : "Automatic backup off",
        description: enabled ? "Save files are archived after each session." : "No automatic uploads for this game.",
        tone: "success",
      });
    } catch (cause) {
      push({ title: "Could not change automatic backup", description: errorMessage(cause), tone: "danger" });
    } finally {
      setToggling(false);
    }
  };

  const describeSync = (next: CloudSaveStatus) => {
    if (next.pending.error) {
      push({
        title: "Cloud save sync failed",
        description: next.pending.nextAttemptAt
          ? `${next.pending.error} Retrying automatically.`
          : next.pending.error,
        tone: "danger",
      });
      return;
    }
    if (!next.latest) {
      push({ title: "Cloud save sync", description: next.message ?? "Nothing to upload yet." });
      return;
    }
    push({
      title: "Cloud save synced",
      description: `Latest snapshot ${formatBytes(next.latest.size)} · ${formatDate(next.latest.createdAt)}`,
      tone: "success",
    });
  };

  const syncNow = async () => {
    if (syncing) return;
    setSyncing(true);
    try {
      const next = await cloudSaves.syncNow(gameId);
      queryClient.setQueryData(["cloud-save-status", gameId], next);
      await invalidate();
      describeSync(next);
    } catch (cause) {
      push({ title: "Could not sync the save", description: errorMessage(cause), tone: "danger" });
    } finally {
      setSyncing(false);
    }
  };

  const restoreRemote = async () => {
    setRestoring(true);
    try {
      const outcome = await cloudSaves.restore(gameId, "remote");
      await Promise.all([invalidate(), invalidateStatus()]);
      push({
        title: outcome.restored ? "Server snapshot restored" : "Nothing restored",
        description: outcome.backupPath
          ? `${outcome.message} Local backup: ${outcome.backupPath}`
          : outcome.message,
        tone: outcome.restored ? "success" : "default",
      });
      setRestoreOpen(false);
      setConflictOpen(false);
    } catch (cause) {
      push({ title: "Could not restore the snapshot", description: errorMessage(cause), tone: "danger" });
    } finally {
      setRestoring(false);
    }
  };

  const resolveConflict = async (strategy: "remote" | "keep_local") => {
    if (strategy === "remote") {
      setConflictOpen(false);
      setRestoreOpen(true);
      return;
    }
    setConflictBusy("keep_local");
    try {
      const outcome = await cloudSaves.resolveConflict(gameId, "keep_local");
      await Promise.all([invalidate(), invalidateStatus()]);
      push({ title: "Local saves kept", description: outcome.message, tone: "success" });
      setConflictOpen(false);
    } catch (cause) {
      push({ title: "Could not keep the local saves", description: errorMessage(cause), tone: "danger" });
    } finally {
      setConflictBusy(null);
    }
  };

  return (
    <section className="mt-12" aria-label={t("Cloud saves")}>
      <SectionHeader
        title={t("Cloud saves")}
        count={rows.length > 0 ? rows.length : undefined}
        description={t("Restore points stored against your account")}
        actions={
          <Button
            variant={composerOpen ? "ghost" : "default"}
            size="sm"
            icon={composerOpen ? <X className="size-3.5" /> : <Upload className="size-3.5" />}
            aria-expanded={composerOpen}
            aria-controls="cloud-saves-composer"
            onClick={() => {
              if (composerOpen) resetComposer();
              setComposerOpen((open) => !open);
            }}
          >
            {composerOpen ? t("Close") : t("Upload .zip")}
          </Button>
        }
      />

      <div className="border-t border-line">
        {/* Automatic sync (#108) */}
        <div className="border-b border-line px-4 py-4">
          <div className="flex flex-wrap items-start justify-between gap-4">
            <div className="flex min-w-0 items-start gap-3">
              <span
                className="mt-0.5 grid size-8 shrink-0 place-items-center border border-line bg-surface/60"
                aria-hidden="true"
              >
                {status?.toggle.enabled ? (
                  <Cloud className="size-4 text-signal-text" />
                ) : (
                  <CloudOff className="size-4 text-ink-3" />
                )}
              </span>
              <div className="min-w-0">
                <p className="flex flex-wrap items-center gap-x-2 text-[13px] font-medium text-ink">
                  Automatic backup
                  {native && status && (
                    <span
                      className={cn(
                        "font-mono text-[9.5px] uppercase tracking-[0.08em]",
                        status.toggle.enabled ? "text-signal-text" : "text-ink-3",
                      )}
                    >
                      {status.toggle.enabled ? "on" : status.toggle.reason === "game_off" ? "off · this game" : "off · global"}
                    </span>
                  )}
                </p>
                <p className="mt-0.5 max-w-[62ch] text-[11.5px] leading-relaxed text-ink-3">
                  {native
                    ? status
                      ? cloudSaveReasonText(status.toggle)
                      : "Archive the detected save folders after each session."
                    : CLOUD_SAVES_DESKTOP_ONLY}
                </p>
              </div>
            </div>
            {native && status && (
              <Switch
                checked={status.toggle.enabled}
                disabled={toggling}
                onCheckedChange={(enabled) => void toggleSync(enabled)}
                label="Automatic cloud save backup"
              />
            )}
          </div>

          {native && cloud.isPending && (
            <div className="mt-4 flex flex-col gap-2">
              <Skeleton className="h-8 w-3/4" />
              <Skeleton className="h-8 w-1/2" />
            </div>
          )}

          {native && cloud.isError && (
            <div className="mt-3 flex flex-wrap items-center justify-between gap-2" role="alert">
              <p className="text-[12.5px] text-ink-2">Cloud save status did not load. {errorMessage(cloud.error)}</p>
              <Button variant="ghost" size="sm" onClick={() => void cloud.refetch()}>
                Try again
              </Button>
            </div>
          )}

          {native && status && (
            <>
              {!status.serverConfigured && (
                <p className="mt-3 border-l-2 border-line-2 pl-3 text-[12px] leading-relaxed text-ink-3">
                  Connect the desktop app to your Ludex server to upload and restore snapshots.
                </p>
              )}

              <dl className="mt-4 grid gap-x-8 gap-y-2 font-mono text-[10.5px] text-ink-3 sm:grid-cols-2">
                <div className="flex min-w-0 items-baseline justify-between gap-3 border-b border-line/60 pb-1.5">
                  <dt>Last backup</dt>
                  <dd className="truncate text-ink-2" title={status.latest?.createdAt}>
                    {status.latest
                      ? `${formatDate(status.latest.createdAt)} · ${formatBytes(status.latest.size)}`
                      : "Never"}
                  </dd>
                </div>
                <div className="flex min-w-0 items-baseline justify-between gap-3 border-b border-line/60 pb-1.5">
                  <dt>Device</dt>
                  <dd className="truncate text-ink-2" title={status.latest?.deviceId ?? undefined}>
                    {status.latest
                      ? status.latest.source === "auto"
                        ? shortDeviceId(status.latest.deviceId)
                        : "manual upload"
                      : "—"}
                  </dd>
                </div>
                <div className="flex min-w-0 items-baseline justify-between gap-3 border-b border-line/60 pb-1.5">
                  <dt>Local saves</dt>
                  <dd className="text-ink-2">
                    {status.localFiles === 0
                      ? "No files yet"
                      : `${status.localFiles} file${status.localFiles === 1 ? "" : "s"} · ${formatBytes(status.localBytes)}`}
                  </dd>
                </div>
                <div className="flex min-w-0 items-baseline justify-between gap-3 border-b border-line/60 pb-1.5">
                  <dt>Last restore</dt>
                  <dd className="truncate text-ink-2" title={status.lastRestoreAt ?? undefined}>
                    {status.lastRestoreAt ? formatDate(status.lastRestoreAt) : "Never"}
                  </dd>
                </div>
              </dl>

              {status.pending.pending && status.pending.error && (
                <div className="mt-3 flex items-start gap-2 border-l-2 border-warning/60 pl-3" role="status">
                  <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-warning" aria-hidden="true" />
                  <p className="text-[12px] leading-relaxed text-ink-2">
                    {status.pending.error}
                    {status.pending.nextAttemptAt && (
                      <span className="text-ink-3"> Retrying after {formatDate(status.pending.nextAttemptAt)}.</span>
                    )}
                  </p>
                </div>
              )}

              {status.message && !status.pending.error && (
                <p className="mt-3 text-[12px] leading-relaxed text-ink-3">{status.message}</p>
              )}

              {conflict && (
                <div className="mt-4 border border-warning/40 bg-warning/[0.05] px-3 py-3">
                  <p className="flex items-center gap-2 text-[12.5px] font-medium text-ink">
                    <AlertTriangle className="size-3.5 text-warning" aria-hidden="true" />
                    Local and server saves both changed
                  </p>
                  <p className="mt-1 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-2">
                    The local files are newer than the last upload from this machine, and the newest server snapshot is
                    different. Keeping local uploads it as a new snapshot; restoring the server copy writes a local
                    backup first.
                  </p>
                  <div className="mt-2.5 flex flex-wrap gap-2">
                    <Button
                      variant="default"
                      size="sm"
                      loading={conflictBusy === "keep_local"}
                      onClick={() => void resolveConflict("keep_local")}
                    >
                      Keep local
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      loading={conflictBusy === "remote"}
                      onClick={() => void resolveConflict("remote")}
                    >
                      Restore server copy
                    </Button>
                  </div>
                </div>
              )}

              <div className="mt-3 flex flex-wrap gap-2">
                <Button
                  variant="default"
                  size="sm"
                  icon={<RefreshCw className="size-3.5" />}
                  loading={syncing}
                  disabled={!status.serverConfigured}
                  onClick={() => void syncNow()}
                >
                  Sync now
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<RotateCcw className="size-3.5" />}
                  disabled={!status.latest || !status.serverConfigured}
                  onClick={() => setRestoreOpen(true)}
                >
                  Restore latest
                </Button>
              </div>
            </>
          )}
        </div>

        {composerOpen && (
          <div id="cloud-saves-composer" className="flex flex-col gap-3 border-b border-line bg-surface/30 px-4 py-4">
            <label
              onDragOver={(event) => {
                event.preventDefault();
                if (!uploading) setDragging(true);
              }}
              onDragLeave={(event) => {
                if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false);
              }}
              onDrop={(event) => {
                event.preventDefault();
                setDragging(false);
                if (!uploading) pick(event.dataTransfer.files?.[0]);
              }}
              className={cn(
                "group relative flex cursor-pointer items-center justify-between gap-4 border border-dashed px-3 py-2.5 transition-colors duration-[120ms]",
                "focus-within:border-signal-text/70",
                dragging ? "border-signal/70 bg-signal/[0.06]" : "border-line-2 bg-bg/40 hover:border-ink-3",
                uploading && "pointer-events-none opacity-60",
              )}
            >
              <input
                ref={fileInput}
                type="file"
                accept=".zip,application/zip"
                aria-label={t("Save archive for {title}", { title })}
                className="absolute inset-0 cursor-pointer opacity-0"
                disabled={uploading}
                onChange={(event) => pick(event.target.files?.[0])}
              />
              <span className="flex min-w-0 items-center gap-3">
                <FileArchive className="size-4 shrink-0 text-ink-3 transition-colors group-hover:text-ink-2" aria-hidden="true" />
                <span className="min-w-0">
                  <span className="block truncate text-[13px] text-ink">
                    {file ? file.name : t("Choose a .zip archive")}
                  </span>
                  <span className="mt-0.5 block font-mono text-[10.5px] text-ink-3">
                    {file ? `${formatBytes(file.size)} · ready to upload` : t("Drag it here or browse · up to 2 GiB")}
                  </span>
                </span>
              </span>
              <span className="shrink-0 text-[12px] text-ink-2 underline-offset-4 group-hover:text-ink group-hover:underline">
                {file ? t("Change") : t("Browse")}
              </span>
            </label>

            <div className="grid gap-2 sm:grid-cols-[1fr_auto]">
              <label className="flex flex-col gap-1.5">
                <span className="sr-only">{t("Snapshot label")}</span>
                <Input
                  ref={labelInput}
                  value={label}
                  maxLength={80}
                  disabled={uploading}
                  onChange={(event) => setLabel(event.target.value)}
                  placeholder={t("Label (optional) — e.g. Before the final boss")}
                  className="h-10 text-[13px]"
                />
              </label>
              <Button
                variant={file ? "primary" : "default"}
                loading={uploading}
                disabled={!file}
                onClick={() => void upload()}
              >
                {t("Upload save")}
              </Button>
            </div>

            {uploading && (
              <div aria-busy="true">
                <Progress value={pct} label={t("Save upload progress")} />
                <p className="mt-1.5 flex flex-wrap items-baseline gap-x-2.5 font-mono text-[10.5px] text-ink-3">
                  <span className="text-ink-2">{t("Uploading")}</span>
                  {pct !== null && <span>{formatPercent(pct)}</span>}
                  {progress && (
                    <span>
                      {formatBytes(progress.loaded)} / {formatBytes(progress.total)}
                    </span>
                  )}
                </p>
              </div>
            )}
          </div>
        )}

        {saves.isPending ? (
          <div className="flex flex-col gap-3 py-4">
            <Skeleton className="h-9 w-full" />
            <Skeleton className="h-9 w-4/5" />
          </div>
        ) : saves.isError ? (
          <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line py-5" role="alert">
            <p className="text-[13px] text-ink-2">{t("Cloud saves didn’t load.")} {errorMessage(saves.error)}</p>
            <Button variant="default" size="sm" onClick={() => void saves.refetch()}>
              {t("Try again")}
            </Button>
          </div>
        ) : rows.length === 0 ? (
          <div className="flex flex-col gap-4 border-b border-line py-7">
            <div className="flex items-start gap-3.5">
              <span
                className="mt-0.5 grid size-9 shrink-0 place-items-center border border-line bg-surface/60"
                aria-hidden="true"
              >
                <FileArchive className="size-4 text-ink-3" />
              </span>
              <div className="flex flex-col gap-1.5">
                <p className="text-[14px] text-ink">{t("No snapshots for this game yet")}</p>
                <p className="measure text-[13px] leading-relaxed text-ink-2">
                  {tr("Saves belong to your account: only you can list, download or delete them. Upload a {zip} of your save folder to keep a restore point.", {
                    zip: <span className="font-mono text-[12px] text-ink-2">.zip</span>,
                  })}
                </p>
                <p className="measure font-mono text-[10.5px] leading-relaxed text-ink-3">
                  {native
                    ? "Launch the game once, then Sync now to create the first automatic snapshot."
                    : "Automatic snapshots are created by the desktop app after each session."}
                </p>
              </div>
            </div>
            {!composerOpen && (
              <div className="sm:pl-[50px]">
                <Button
                  variant="default"
                  size="sm"
                  icon={<Upload className="size-3.5" />}
                  onClick={() => {
                    setComposerOpen(true);
                    requestAnimationFrame(() => fileInput.current?.focus());
                  }}
                >
                  {t("Upload a save")}
                </Button>
              </div>
            )}
          </div>
        ) : (
          <div>
            {rows.map((snapshot, index) => (
              <div
                key={snapshot.id}
                className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-line py-3"
              >
                <div className="min-w-0">
                  <p className="flex items-center gap-2 truncate text-[13.5px] text-ink">
                    <span className="truncate">{snapshot.label}</span>
                    {snapshot.source === "auto" && (
                      <span className="shrink-0 border border-line-2 px-1 py-0.5 font-mono text-[9px] uppercase tracking-[0.06em] text-ink-3">
                        auto
                      </span>
                    )}
                    {index === 0 && (
                      <span className="shrink-0 font-mono text-[9.5px] uppercase tracking-[0.08em] text-signal-text">
                        {t("latest")}
                      </span>
                    )}
                  </p>
                  <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-ink-3">
                    <span className="truncate">{snapshot.fileName}</span>
                    <span className="num">{formatBytes(snapshot.size)}</span>
                    <span title={new Date(snapshot.createdAt).toLocaleString("en-GB")}>
                      {formatDate(snapshot.createdAt)}
                    </span>
                    {snapshot.deviceId && <span className="hidden sm:inline">dev {shortDeviceId(snapshot.deviceId)}</span>}
                    <span className="hidden sm:inline" title={snapshot.sha256}>
                      {t("sha {hash}", { hash: snapshot.sha256.slice(0, 10) })}
                    </span>
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-1">
                  <Button
                    variant="ghost"
                    size="sm"
                    icon={<Download className="size-3.5" />}
                    loading={downloadingId === snapshot.id}
                    onClick={() => void download(snapshot)}
                    className="text-ink-2"
                  >
                    {t("Download")}
                  </Button>
                  <IconButton
                    label={t("Delete {label}", { label: snapshot.label })}
                    variant="ghost"
                    className="size-8 text-ink-3 hover:text-danger"
                    onClick={() => setDeleteTarget(snapshot)}
                  >
                    <Trash2 className="size-3.5" />
                  </IconButton>
                </div>
              </div>
            ))}
            <p className="pt-3 text-[11.5px] leading-relaxed text-ink-3">
              {t("Snapshots are private to your account and stored as-is; the server never opens them.")}
            </p>
          </div>
        )}
      </div>

      <ConfirmDialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("Delete this save?")}
        description={
          deleteTarget
            ? `${deleteTarget.label} · ${formatBytes(deleteTarget.size)}. The archive is removed from the server for good.`
            : ""
        }
        confirmLabel={t("Delete save")}
        loading={deleting}
        onConfirm={() => void remove()}
      />

      <ConfirmDialog
        open={restoreOpen}
        onOpenChange={setRestoreOpen}
        title="Restore the server snapshot?"
        description={
          status && status.localFiles > 0
            ? `The newest server snapshot replaces the local save files. Ludex writes a timestamped backup of the current local saves first and returns its path.`
            : "The newest server snapshot is written into the detected save folders. They are currently empty."
        }
        confirmLabel={status && status.localFiles > 0 ? "Back up and restore" : "Restore snapshot"}
        tone="primary"
        loading={restoring}
        onConfirm={() => void restoreRemote()}
      />

      <Dialog
        open={conflictOpen && Boolean(conflict)}
        onOpenChange={setConflictOpen}
        title="Local and server saves both changed"
        description="Nothing is overwritten until you choose. Hashes are SHA-256; timestamps are local time."
        size="md"
        footer={
          <>
            <Button
              variant="ghost"
              loading={conflictBusy === "keep_local"}
              onClick={() => void resolveConflict("keep_local")}
            >
              Keep local
            </Button>
            <Button variant="primary" onClick={() => void resolveConflict("remote")}>
              Restore server copy
            </Button>
          </>
        }
      >
        {conflict && (
          <dl className="grid gap-x-6 gap-y-2 text-[12.5px]">
            <div className="flex items-baseline justify-between gap-4 border-b border-line pb-1.5">
              <dt className="text-ink-3">Local newest change</dt>
              <dd className="font-mono text-[11px] text-ink-2" title={conflict.localNewestMtime ?? undefined}>
                {conflict.localNewestMtime
                  ? new Date(conflict.localNewestMtime).toLocaleString("en-GB")
                  : "unknown"}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-4 border-b border-line pb-1.5">
              <dt className="text-ink-3">Last upload from this device</dt>
              <dd className="font-mono text-[11px] text-ink-2" title={conflict.lastUploadAt ?? undefined}>
                {conflict.lastUploadAt ? new Date(conflict.lastUploadAt).toLocaleString("en-GB") : "never"}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-4 border-b border-line pb-1.5">
              <dt className="text-ink-3">Server snapshot</dt>
              <dd className="font-mono text-[11px] text-ink-2">
                {new Date(conflict.remoteCreatedAt).toLocaleString("en-GB")} ·{" "}
                {shortDeviceId(conflict.remoteDeviceId)}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-4 border-b border-line pb-1.5">
              <dt className="text-ink-3">Last uploaded hash</dt>
              <dd className="truncate font-mono text-[11px] text-ink-2" title={conflict.lastHash ?? undefined}>
                {conflict.lastHash ? conflict.lastHash.slice(0, 16) : "none"}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-4">
              <dt className="text-ink-3">Server hash</dt>
              <dd className="truncate font-mono text-[11px] text-ink-2" title={conflict.remoteSha256}>
                {conflict.remoteSha256.slice(0, 16)}
              </dd>
            </div>
          </dl>
        )}
      </Dialog>
    </section>
  );
}
