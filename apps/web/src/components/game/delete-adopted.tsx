import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Trash2 } from "lucide-react";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import type { DeleteAdoptedPreview } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Button, Field, Input } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { t, tn } from "@/lib/i18n";

/** Last path segment of a folder, for the typed confirmation. */
export function folderBaseName(folder: string): string {
  const trimmed = folder.replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] ?? trimmed;
}

/**
 * "Uninstall and delete files" (#129).
 *
 * The desktop counts the files first and shows the canonical path, size and file
 * count; the destructive action only runs after the folder name is typed. The
 * backend refuses any path that is not exactly the stored adopted folder.
 */
export function DeleteAdoptedDialog({
  open,
  onOpenChange,
  gameId,
  title,
  onDeleted,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  gameId: number;
  title: string;
  onDeleted?: () => void;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [preview, setPreview] = useState<DeleteAdoptedPreview | null>(null);
  const [typed, setTyped] = useState("");
  const [loading, setLoading] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setPreview(null);
      setTyped("");
      setLoading(false);
      setDeleting(false);
      setError(null);
      return;
    }
    if (!isNative()) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    void native
      .deleteAdoptedInstallation(gameId)
      .then((next) => {
        if (!cancelled) setPreview(next);
      })
      .catch((cause: Error) => {
        if (!cancelled) setError(errorMessage(cause));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, gameId]);

  const expected = preview ? folderBaseName(preview.folder) : "";
  const matches = Boolean(expected) && typed.trim() === expected;

  const remove = async () => {
    if (!preview || !matches) return;
    setDeleting(true);
    setError(null);
    try {
      const result = await native.deleteAdoptedInstallation(gameId, preview.folder);
      await queryClient.invalidateQueries({ queryKey: ["installed"] });
      await queryClient.invalidateQueries({ queryKey: ["recent-games"] });
      await queryClient.invalidateQueries({ queryKey: ["game", gameId] });
      await queryClient.invalidateQueries({ queryKey: ["games"] });
      push({
        title: result.deleted ? t("{title} deleted", { title }) : t("{title} removed from Ludex", { title }),
        description: result.deleted
          ? t("{size} and {files} were deleted from {folder}.", {
              size: formatBytes(result.sizeBytes),
              files: tn(result.fileCount, "1 file", "{count} files"),
              folder: result.folder,
            })
          : (result.warnings[0] ?? t("The folder was already gone.")),
        tone: "success",
      });
      onDeleted?.();
      onOpenChange(false);
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not delete the folder"), description: message, tone: "danger" });
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("Uninstall {title} and delete its files?", { title })}
      description={t("This deletes the game folder from this machine. It cannot be undone, and Ludex cannot download it again unless the server has an archive.")}
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={deleting}>
            {t("Cancel")}
          </Button>
          <Button
            variant="primary"
            className="bg-danger text-bg hover:bg-danger/90"
            icon={<Trash2 className="size-4" />}
            loading={deleting}
            disabled={!preview || !matches || loading}
            onClick={() => void remove()}
          >
            {preview?.missing ? t("Remove the stale entry") : t("Delete the folder")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="flex items-start gap-2 border-l-2 border-danger pl-3 text-[12px] leading-relaxed text-ink-2">
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden="true" />
          {t("Adopted folders are not managed by Ludex: there is no archive to download again. To remove the entry but keep the files, use Remove from Ludex instead.")}
        </p>

        {loading && <p className="text-[12.5px] text-ink-3">{t("Counting the files…")}</p>}

        {preview && (
          <dl className="flex flex-col gap-1.5 text-[12px]">
            <div className="flex items-baseline justify-between gap-4">
              <dt className="shrink-0 text-ink-3">{t("Folder")}</dt>
              <dd className="truncate font-mono text-[11px] text-ink-2" title={preview.folder}>
                {preview.folder}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-4">
              <dt className="shrink-0 text-ink-3">{t("Size on disk")}</dt>
              <dd className="num text-[12px] text-ink-2">
                {formatBytes(preview.sizeBytes)}
                <span className="ml-2 text-ink-3">{tn(preview.fileCount, "1 file", "{count} files")}</span>
              </dd>
            </div>
          </dl>
        )}

        {preview?.warnings.map((warning) => (
          <p key={warning} className="text-[11.5px] leading-relaxed text-signal-text">
            {warning}
          </p>
        ))}

        {preview && (
          <Field
            label={t("Type {name} to confirm", { name: expected })}
            htmlFor="delete-adopted-confirm"
            hint={t("The folder name must match exactly.")}
          >
            <Input
              id="delete-adopted-confirm"
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
              placeholder={expected}
              spellCheck={false}
              autoComplete="off"
              disabled={deleting}
              className="h-10 font-mono text-[12px]"
            />
          </Field>
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
