import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { AlertTriangle, RefreshCw, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatRelative } from "@/lib/format";
import type { GameDetail } from "@/lib/types";
import { useIsEditor } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { Button } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

/**
 * "No archive available" state (#210).
 *
 * A game whose archives were deleted outside Ludex (or whose root is offline)
 * keeps its metadata, progress and saves, but offers no download or install.
 * The only two actions are a rescan — which restores the same row when the
 * file is back — and an explicit, confirmed removal from the library that
 * deletes the database rows only.
 */
export function MissingArchiveNotice({ game }: { game: GameDetail }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const isEditor = useIsEditor();
  const { push } = useToast();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const versions = game.missingVersions ?? [];

  const rescan = useMutation({
    mutationFn: () => api.startScan(),
    onSuccess: (result) => {
      push({
        title: result.started ? t("Rescan queued") : t("A scan is already running"),
        description: t("When the file is back, the next scan restores this game and its progress."),
      });
    },
    onError: (cause) =>
      push({ title: t("Could not start the scan"), description: errorMessage(cause), tone: "danger" }),
  });

  const remove = useMutation({
    mutationFn: () => api.deleteGame(game.id, "library"),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["games"] });
      void queryClient.invalidateQueries({ queryKey: ["games-missing-count"] });
      void queryClient.invalidateQueries({ queryKey: ["stats"] });
      void queryClient.invalidateQueries({ queryKey: ["game", game.id] });
      push({
        title: t("{title} removed from the library", { title: game.title }),
        description: t("Only the database row was deleted; no file was touched."),
      });
      setConfirmOpen(false);
      navigate("/library");
    },
    onError: (cause) =>
      push({ title: t("Could not remove the game"), description: errorMessage(cause), tone: "danger" }),
  });

  return (
    <section
      aria-labelledby="missing-archive-title"
      className="mt-6 max-w-[640px] rounded-[3px] border border-danger/45 bg-danger/10 p-4"
    >
      <p className="flex items-center gap-2 font-mono text-[10.5px] uppercase tracking-[0.12em] text-danger">
        <AlertTriangle className="size-4" aria-hidden="true" />
        {t("No archive available")}
      </p>
      <h2 id="missing-archive-title" className="mt-2 text-[16px] font-semibold">
        {t("Ludex kept the metadata, but there is nothing to download or install.")}
      </h2>
      <p className="mt-1.5 text-[12.5px] leading-relaxed text-ink-2">
        {t("The archive was deleted outside Ludex, or the library root that holds it is offline. Progress, saves, achievements and collections are untouched; a scan never deletes a game.")}
      </p>

      {versions.length > 0 && (
        <ul className="mt-3 space-y-1.5 border-t border-danger/25 pt-3">
          {versions.map((version) => (
            <li key={version.id} className="flex flex-wrap items-baseline gap-x-2 gap-y-1 text-[12px]">
              <span className="font-mono text-[11.5px] text-ink">{version.fileName}</span>
              <span className="text-danger">
                {version.rootUnavailable ? t("library root offline") : t("file missing")}
              </span>
              {version.updatedAt && (
                <span className="text-ink-3">
                  {t("last seen {when}", { when: formatRelative(version.updatedAt) })}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}

      <div className="mt-3 flex flex-wrap items-center gap-2">
        {isEditor && (
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            loading={rescan.isPending}
            onClick={() => rescan.mutate()}
          >
            {t("Relink / rescan")}
          </Button>
        )}
        {isEditor && (
          <Button
            variant="danger"
            size="sm"
            icon={<Trash2 className="size-3.5" />}
            onClick={() => setConfirmOpen(true)}
          >
            {t("Remove from library")}
          </Button>
        )}
        <span className="text-[11.5px] text-ink-3">
          {t("Files are never deleted from this page.")}
        </span>
      </div>

      <ConfirmDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        title={t("Remove {title} from the library?", { title: game.title })}
        description={t("This deletes the game row, its progress, bookmarks, achievements and cloud-save snapshots from the server. Archive files are not touched. A later scan can re-add the game, but the deleted history will not come back.")}
        confirmLabel={t("Remove from library")}
        loading={remove.isPending}
        onConfirm={() => remove.mutate()}
      />
    </section>
  );
}
