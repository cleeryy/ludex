import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, FileArchive, Trash2, Upload, X } from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { cn } from "@/lib/cn";
import { formatBytes, formatDate, formatPercent } from "@/lib/format";
import type { SaveSnapshot } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Button, IconButton, Input, Progress, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { tr, t } from "@/lib/i18n";

const MAX_SAVE_BYTES = 2 * 1024 ** 3;

/**
 * Per-user save snapshots. The server stores opaque archives and never reads
 * them; the browser only lists, uploads, downloads and deletes. Works in both
 * the browser and the Tauri shell: a file input for upload, a Blob download
 * for restore, no native command.
 */
export function CloudSaves({ gameId, title }: { gameId: number; title: string }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const saves = useQuery({
    queryKey: ["saves", gameId],
    queryFn: () => api.listSaves(gameId),
  });

  const [composerOpen, setComposerOpen] = useState(false);
  const [file, setFile] = useState<File | null>(null);
  const [label, setLabel] = useState("");
  const [dragging, setDragging] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [progress, setProgress] = useState<{ loaded: number; total: number } | null>(null);
  const [downloadingId, setDownloadingId] = useState<number | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<SaveSnapshot | null>(null);
  const [deleting, setDeleting] = useState(false);

  const fileInput = useRef<HTMLInputElement>(null);
  const labelInput = useRef<HTMLInputElement>(null);

  const rows = [...(saves.data ?? [])].sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
  const pct = progress && progress.total > 0 ? Math.min(100, (progress.loaded / progress.total) * 100) : null;

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["saves", gameId] });

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
                  {t("Ludusavi automation is on the roadmap.")}
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
    </section>
  );
}
