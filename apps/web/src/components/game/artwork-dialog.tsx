import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ImagePlus, RotateCcw, Upload } from "lucide-react";

import { ARTWORK_ACCEPT, ARTWORK_MAX_BYTES, resetArtwork, uploadArtwork, type ArtworkKind } from "@/lib/artwork";
import { assetUrl } from "@/lib/api";
import { toSafeUrl, type SafeUrl } from "@/lib/safe-url";
import { errorMessage } from "@/lib/http";
import type { GameDetail } from "@/lib/types";
import { Button } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { useToast } from "@/state/toasts";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/**
 * Editor-only dialog replacing a game's cover or background (#38).
 *
 * The preview is local (object URL) until the upload succeeds; the server
 * sniffs the real format and stores it under a cache-busting name. Reset clears
 * the custom art and falls back to provider artwork.
 */
export function ArtworkDialog({
  open,
  onOpenChange,
  game,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  game: GameDetail;
}) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const [files, setFiles] = useState<Partial<Record<ArtworkKind, File>>>({});
  const [previews, setPreviews] = useState<Partial<Record<ArtworkKind, SafeUrl>>>({});
  const [busy, setBusy] = useState<ArtworkKind | null>(null);
  const [resetTarget, setResetTarget] = useState<ArtworkKind | null>(null);
  const [error, setError] = useState<string | null>(null);
  const urls = useRef<string[]>([]);

  useEffect(
    () => () => {
      for (const url of urls.current) URL.revokeObjectURL(url);
      urls.current = [];
    },
    [],
  );

  const choose = (kind: ArtworkKind, file: File | undefined) => {
    if (!file) return;
    setError(null);
    if (file.size > ARTWORK_MAX_BYTES) {
      setError(t("{name} is larger than {max} MiB.", { name: file.name, max: ARTWORK_MAX_BYTES / (1024 * 1024) }));
      return;
    }
    if (!["image/png", "image/jpeg", "image/webp", "image/gif"].includes(file.type)) {
      setError(t("Use a PNG, JPEG, WebP or GIF image."));
      return;
    }
    const url = URL.createObjectURL(file);
    urls.current.push(url);
    setFiles((current) => ({ ...current, [kind]: file }));
    // Object URLs are `blob:` and pass the same boundary as server artwork, so
    // the preview sink never receives a raw string (#266).
    setPreviews((current) => ({ ...current, [kind]: toSafeUrl(url) ?? undefined }));
  };

  const upload = async (kind: ArtworkKind) => {
    const file = files[kind];
    if (!file) return;
    setBusy(kind);
    setError(null);
    try {
      await uploadArtwork(game.id, kind, file);
      push({ title: kind === "cover" ? t("Cover updated") : t("Background updated"), tone: "success" });
      setFiles((current) => ({ ...current, [kind]: undefined }));
      setPreviews((current) => ({ ...current, [kind]: undefined }));
      await queryClient.invalidateQueries({ queryKey: qk.game.detail(game.id) });
      await queryClient.invalidateQueries({ queryKey: qk.games.root });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const confirmReset = async () => {
    const kind = resetTarget;
    setResetTarget(null);
    if (!kind) return;
    setBusy(kind);
    setError(null);
    try {
      await resetArtwork(game.id, kind);
      push({
        title: kind === "cover" ? t("Cover reset") : t("Background reset"),
        description: t("Provider artwork or the placeholder is used again."),
        tone: "success",
      });
      await queryClient.invalidateQueries({ queryKey: qk.game.detail(game.id) });
      await queryClient.invalidateQueries({ queryKey: qk.games.root });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const slot = (kind: ArtworkKind) => {
    const current = kind === "cover" ? game.coverUrl : game.backgroundUrl;
    const rawPreview = previews[kind] ?? assetUrl(current);
    const preview = rawPreview ? rawPreview.replace(/[<>"']/g, (ch) => encodeURIComponent(ch)) : null;
    const kindLabel = kind === "cover" ? t("Cover") : t("Background");
    return (
      <section className="flex flex-col gap-2 border-t border-line pt-4 first:border-t-0 first:pt-0">
        <div className="flex items-baseline justify-between gap-3">
          <h3 className="text-body font-medium text-ink">{kindLabel}</h3>
          <span className="font-mono text-micro text-ink-3">{t("PNG · JPEG · WebP · GIF · max 8 MiB")}</span>
        </div>
        <div
          className={
            kind === "cover"
              ? "flex aspect-[2/3] w-[124px] items-center justify-center overflow-hidden rounded-[var(--r-sm)] border border-line bg-bg-2"
              : "flex aspect-[16/9] w-full max-w-[360px] items-center justify-center overflow-hidden rounded-[var(--r-sm)] border border-line bg-bg-2"
          }
        >
          {preview ? (
            <img src={preview} alt={`${game.title} ${kindLabel}`} className="h-full w-full object-cover" />
          ) : (
            <ImagePlus className="size-6 text-ink-3" aria-hidden="true" />
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <label className="inline-flex h-9 cursor-pointer items-center gap-2 rounded-[var(--r-sm)] border border-line px-3 text-label text-ink-2 hover:border-line-2 hover:text-ink">
            <ImagePlus className="size-3.5" aria-hidden="true" />
            {t("Choose image")}
            <input
              type="file"
              accept={ARTWORK_ACCEPT}
              className="sr-only"
              onChange={(event) => choose(kind, event.target.files?.[0])}
              aria-label={kind === "cover" ? t("Choose a new cover image") : t("Choose a new background image")}
            />
          </label>
          <Button
            variant="primary"
            size="sm"
            icon={<Upload className="size-3.5" />}
            loading={busy === kind}
            disabled={!files[kind] || (busy !== null && busy !== kind)}
            onClick={() => void upload(kind)}
          >
            {t("Upload")}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            icon={<RotateCcw className="size-3.5" />}
            disabled={busy !== null}
            onClick={() => setResetTarget(kind)}
          >
            {t("Reset to provider art")}
          </Button>
        </div>
      </section>
    );
  };

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={onOpenChange}
        title={t("Artwork for {title}", { title: game.title })}
        description={t("Replace the cover or background with your own image. The server validates the file and clients pick it up after a refresh.")}
        footer={
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Close")}
          </Button>
        }
      >
        <div className="flex flex-col gap-4">
          {slot("cover")}
          {slot("background")}
          {error && (
            <p role="alert" className="border-l-2 border-danger pl-3 text-label text-danger">
              {error}
            </p>
          )}
        </div>
      </Dialog>
      <ConfirmDialog
        open={resetTarget !== null}
        onOpenChange={(next) => {
          if (!next) setResetTarget(null);
        }}
        title={t("Reset custom artwork?")}
        description={t("The custom image is deleted and provider artwork or the placeholder is shown again. This cannot be undone.")}
        confirmLabel={t("Reset artwork")}
        onConfirm={() => void confirmReset()}
      />
    </>
  );
}
