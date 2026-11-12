import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Download, HardDriveDownload, Play, RotateCcw, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatDate } from "@/lib/format";
import { isNative } from "@/lib/native";
import { t } from "@/lib/i18n";
import type { GameDetail, GameVersion, GameVersionInfo, InstalledGame } from "@/lib/types";
import {
  canDeleteVersion,
  freedVersionBytes,
  isRollbackInstall,
  isRollbackVersion,
  preferredDiffersFromInstalled,
  preferredVersion,
  sortVersionsNewestFirst,
  versionHashShort,
  versionLabel,
  versionState,
} from "@/lib/game-versions";
import { useToast } from "@/state/toasts";
import { Badge, Button } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";

/** A row from the server manager, or the public detail shape for readers. */
function asInfo(version: GameVersion): GameVersionInfo {
  return {
    ...version,
    indexedAt: version.updatedAt ?? null,
    missing: false,
    rootUnavailable: false,
  };
}

/**
 * Game page → Versions (#168).
 *
 * Readers see the available archives and which one downloads/updates use.
 * Editors additionally get the full row list (including missing archives),
 * the pin action (an older pin is a rollback) and archive deletion with an
 * explicit confirmation. The server still enforces every guard; the UI only
 * avoids offering an action that is known to fail.
 */
export function VersionManager({
  game,
  installed,
  isEditor,
  onInstall,
  onDownload,
}: {
  game: GameDetail;
  installed?: InstalledGame;
  isEditor: boolean;
  onInstall: (version: GameVersion) => void;
  onDownload: (version: GameVersion) => void;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [busyId, setBusyId] = useState<number | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<GameVersionInfo | null>(null);

  // Editors see rows the public detail hides (missing archives). The query is
  // disabled for readers: their copy of `game.versions` is enough and avoids a
  // guaranteed 403.
  const manager = useQuery({
    queryKey: ["game-versions", game.id],
    queryFn: () => api.gameVersions(game.id),
    enabled: isEditor,
    staleTime: 15_000,
  });

  const versions = sortVersionsNewestFirst(
    isEditor && manager.data ? manager.data.versions : game.versions.map(asInfo),
  );
  const defaultVersionId = (isEditor ? manager.data?.defaultVersionId : undefined) ?? game.defaultVersionId;
  const preferred = preferredVersion(versions, defaultVersionId);
  const installedVersion = preferredVersion(versions, installed?.versionId);
  const preferredIsOlder =
    installed && preferred ? isRollbackVersion(preferred, { id: installed.versionId }, versions) : false;

  const refresh = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["game", game.id] }),
      queryClient.invalidateQueries({ queryKey: ["games"] }),
      queryClient.invalidateQueries({ queryKey: ["game-updates"] }),
      queryClient.invalidateQueries({ queryKey: ["game-versions", game.id] }),
    ]);
  };

  const makeDefault = async (version: GameVersionInfo) => {
    setBusyId(version.id);
    try {
      await api.setDefaultGameVersion(game.id, version.id);
      await refresh();
      push({
        title: t("{version} is now the preferred version", { version: versionLabel(version) }),
        description: t("Downloads and update checks use it; the other archives stay available."),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not change the preferred version"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusyId(null);
    }
  };

  const remove = async () => {
    if (!deleteTarget) return;
    setBusyId(deleteTarget.id);
    try {
      const result = await api.deleteGameVersion(game.id, deleteTarget.id);
      await refresh();
      push({
        title: t("{size} freed", { size: formatBytes(result.freedBytes) }),
        description: t("{version} was removed. The game, its metadata and its saves are untouched.", {
          version: versionLabel(deleteTarget),
        }),
        tone: "success",
      });
      setDeleteTarget(null);
    } catch (cause) {
      push({ title: t("Could not delete this archive"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusyId(null);
    }
  };

  return (
    <section aria-labelledby="versions-heading">
      <SectionHeader
        title={t("Versions")}
        count={versions.length}
        description={t("Archives on the server, newest first")}
        actions={
          isEditor ? (
            <span className="text-[11px] text-ink-3">
              {manager.isFetching ? t("Refreshing…") : t("Preferred version drives downloads and updates")}
            </span>
          ) : undefined
        }
      />
      <div className="border-t border-line" id="versions-heading">
        {defaultVersionId != null && preferred && preferred.id !== defaultVersionId && (
          <p className="border-b border-line py-2.5 text-[12px] leading-relaxed text-signal-text">
            {t(
              "The pinned archive is missing. {version} is used for downloads and updates instead.",
              { version: versionLabel(preferred) },
            )}
          </p>
        )}

        {installed && preferred && preferredDiffersFromInstalled(preferred.id, installed.versionId) && (
          <p className="border-b border-line py-2.5 text-[12px] leading-relaxed text-ink-2">
            {preferredIsOlder
              ? t(
                  "Installed {installed} is newer than the preferred {preferred}. New downloads and update checks use {preferred} until another version is pinned.",
                  {
                    installed: installed.version ? `v${installed.version}` : t("this build"),
                    preferred: versionLabel(preferred),
                  },
                )
              : t(
                  "Installed {installed}; the preferred version is {preferred}. Updating installs it side-by-side and keeps this folder playable until it is ready.",
                  {
                    installed: installed.version ? `v${installed.version}` : t("this build"),
                    preferred: versionLabel(preferred),
                  },
                )}
          </p>
        )}

        {versions.map((version) => {
          const state = versionState(version, defaultVersionId);
          const rollback = isRollbackVersion(version, preferred, versions);
          const installRollback =
            installed && isRollbackInstall(version.id, installed.versionId, versions);
          const deletion = canDeleteVersion(versions, version);
          const isInstalled = installed?.versionId === version.id;
          const unavailable = state === "missing" || state === "unavailable";
          const shortHash = versionHashShort(version.archiveHash);
          return (
            <div
              key={version.id}
              className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2 border-b border-line py-3"
            >
              <div className="min-w-0 flex-1">
                <p className="flex flex-wrap items-baseline gap-2 text-[13.5px] text-ink">
                  <span className="truncate">{versionLabel(version)}</span>
                  {state === "default" && <Badge tone="solid">{t("Default")}</Badge>}
                  {isInstalled && <Badge tone="ok">{t("Installed")}</Badge>}
                  {state === "missing" && <Badge tone="danger">{t("Missing")}</Badge>}
                  {state === "unavailable" && <Badge tone="signal">{t("Root offline")}</Badge>}
                  {version.earlyAccess && <Badge tone="signal">{t("Early access")}</Badge>}
                </p>
                <p className="mt-1 flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
                  <span className="truncate">{version.fileName}</span>
                  <span>{formatBytes(version.size)}</span>
                  <span>{formatDate(version.releaseDate)}</span>
                  {version.indexedAt && <span>{t("indexed {date}", { date: formatDate(version.indexedAt) })}</span>}
                  {shortHash && (
                    <span title={version.archiveHash ?? undefined}>
                      sha256:{shortHash}
                    </span>
                  )}
                </p>
                {state === "missing" && (
                  <p className="mt-1 text-[11px] leading-relaxed text-ink-3">
                    {t("The archive is gone from disk; the row stays so saves and history keep their reference.")}
                  </p>
                )}
                {state === "unavailable" && (
                  <p className="mt-1 text-[11px] leading-relaxed text-ink-3">
                    {t("The library root holding this archive is offline. Restore it before deleting or pinning this version.")}
                  </p>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-1.5">
                {isEditor && state !== "default" && !unavailable && (
                  <Button
                    variant="ghost"
                    size="sm"
                    loading={busyId === version.id}
                    icon={rollback ? <RotateCcw className="size-3.5" /> : <Check className="size-3.5" />}
                    onClick={() => void makeDefault(version)}
                  >
                    {rollback ? t("Roll back to this version") : t("Set as default")}
                  </Button>
                )}
                {!unavailable && (
                  <Button
                    variant="ghost"
                    size="sm"
                    icon={<Download className="size-3.5" />}
                    onClick={() => onDownload(version)}
                  >
                    {t("Download")}
                  </Button>
                )}
                {!unavailable && isNative() && !isInstalled && (
                  <Button
                    variant="ghost"
                    size="sm"
                    icon={installRollback ? <RotateCcw className="size-3.5" /> : <HardDriveDownload className="size-3.5" />}
                    onClick={() => onInstall(version)}
                  >
                    {installRollback ? t("Roll back to this version") : t("Install this version")}
                  </Button>
                )}
                {isInstalled && isNative() && (
                  <Button variant="ghost" size="sm" icon={<Play className="size-3.5" />} disabled>
                    {t("Installed")}
                  </Button>
                )}
                {isEditor && (
                  <Button
                    variant="ghost"
                    size="sm"
                    icon={<Trash2 className="size-3.5" />}
                    className="text-danger"
                    disabled={!deletion.allowed}
                    title={
                      deletion.reason === "last"
                        ? t("Ludex never deletes the last available version")
                        : deletion.reason === "missing"
                          ? t("A missing archive cannot be deleted")
                          : deletion.reason === "unavailable"
                            ? t("Restore the library root first")
                            : undefined
                    }
                    onClick={() => setDeleteTarget(version)}
                  >
                    {t("Delete archive")}
                  </Button>
                )}
              </div>
            </div>
          );
        })}
        {versions.length === 0 && (
          <p className="py-4 text-[12.5px] text-ink-3">{t("No archive is indexed for this game yet.")}</p>
        )}
      </div>

      <Dialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
        title={deleteTarget ? t("Delete {version}?", { version: versionLabel(deleteTarget) }) : ""}
        description={
          installed && preferred && installedVersion
            ? t("The preferred version is {preferred}; {installed} is installed on this device.", {
                preferred: versionLabel(preferred),
                installed: installed.version ? `v${installed.version}` : t("this build"),
              })
            : undefined
        }
        size="sm"
        footer={
          <>
            <Button variant="ghost" onClick={() => setDeleteTarget(null)}>
              {t("Cancel")}
            </Button>
            <Button
              variant="danger"
              loading={busyId === deleteTarget?.id}
              icon={<Trash2 className="size-4" />}
              onClick={() => void remove()}
            >
              {t("Delete {size}", { size: deleteTarget ? formatBytes(freedVersionBytes(deleteTarget)) : "" })}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3 text-[12.5px] leading-relaxed text-ink-2">
          <p>
            {t(
              "This removes the archive from the server (and its object-storage copy). The game, its metadata, progress and cloud saves stay; only this version stops being downloadable.",
            )}
          </p>
          <p className="text-ink-3">
            {t("Deleting the archive cannot be undone. Ludex refuses to delete the last available version, an archive in use by a download or one on an offline root.")}
          </p>
          {deleteTarget && versionState(deleteTarget, defaultVersionId) === "default" && (
            <p className="border-l-2 border-signal pl-3 text-signal-text">
              {t("This is the preferred version. The newest remaining archive becomes the default.")}
            </p>
          )}
        </div>
      </Dialog>
    </section>
  );
}
