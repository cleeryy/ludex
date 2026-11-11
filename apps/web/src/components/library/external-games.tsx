import { ExternalLink, FolderOpen, Play, Trash2 } from "lucide-react";
import { cn } from "@/lib/cn";
import { formatBytes } from "@/lib/format";
import { launcherLabel, type ImportLibraryEntry } from "@/lib/library-entries";
import { GameCover } from "@/components/art/cover";
import { Badge, Button } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { Tooltip } from "@/components/ui/tooltip";
import { t } from "@/lib/i18n";

/**
 * Launcher imports that are not in the server library (#154). They use their
 * negative local id, so they never become a `/game/<id>` link: the card opens a
 * local dialog with the actions the desktop index can actually perform.
 */

export function LauncherBadge({ entry, className }: { entry: ImportLibraryEntry; className?: string }) {
  if (!entry.launcher) return null;
  return (
    <Badge tone="signal" className={className}>
      {launcherLabel(entry.launcher)}
    </Badge>
  );
}

function folderLabel(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export function ExternalGameCard({
  entry,
  onOpen,
  onLaunch,
  onOpenFolder,
}: {
  entry: ImportLibraryEntry;
  onOpen: () => void;
  onLaunch: () => void;
  onOpenFolder: () => void;
}) {
  return (
    <article className="cover-host group relative flex flex-col">
      <div className="relative transition-transform duration-[200ms] ease-out group-hover:-translate-y-[3px]">
        <GameCover title={entry.title} coverUrl={null} />
        <button
          type="button"
          onClick={onOpen}
          data-gamepad-target="true"
          className="absolute inset-0 z-[4] rounded-[2px]"
          aria-label={t("Open {title}", { title: entry.title })}
        />
        <span className="absolute left-2 top-2 z-[5] inline-flex items-center gap-1">
          <LauncherBadge entry={entry} />
          <span className="inline-flex h-5 items-center rounded-[2px] bg-stage/70 px-1.5 font-mono text-[9.5px] uppercase tracking-[0.1em] text-sheen backdrop-blur-sm">
            {t("Import")}
          </span>
        </span>
        <div
          className="shadow-cover pointer-events-none absolute inset-0 z-[3] rounded-[2px] opacity-0 transition-opacity duration-[200ms] group-hover:opacity-100"
          aria-hidden="true"
        />
      </div>

      <div className="mt-3 flex items-start gap-2">
        <span className="mt-[5px] h-3.5 w-[2px] shrink-0 bg-line-2" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-[13.5px] font-medium leading-snug text-ink">
            <button type="button" onClick={onOpen} className="text-left hover:text-ink">
              {entry.title}
            </button>
          </h3>
          <div className="mt-1 flex flex-wrap items-baseline gap-x-2.5 gap-y-0.5 font-mono text-[10.5px] leading-none text-ink-3">
            <span className="num">{formatBytes(entry.sizeBytes)}</span>
            {entry.install?.installPath && (
              <span className="max-w-[14ch] truncate" title={entry.install.installPath}>
                {folderLabel(entry.install.installPath)}
              </span>
            )}
          </div>
        </div>
        <div className="-my-2 -mr-2 flex shrink-0 items-center">
          <Tooltip label={t("Play through {source}", { source: entry.launcher ? launcherLabel(entry.launcher) : t("the launcher") })}>
            <button
              type="button"
              onClick={onLaunch}
              aria-label={t("Play {title}", { title: entry.title })}
              className="grid size-10 place-items-center rounded-[2px] text-ink-3 transition-colors hover:bg-surface hover:text-ink max-sm:size-11"
            >
              <Play className="size-[15px]" />
            </button>
          </Tooltip>
          {entry.install?.installPath && (
            <Tooltip label={t("Open folder")}>
              <button
                type="button"
                onClick={onOpenFolder}
                aria-label={t("Open folder for {title}", { title: entry.title })}
                className="grid size-10 place-items-center rounded-[2px] text-ink-3 transition-colors hover:bg-surface hover:text-ink max-sm:size-11"
              >
                <FolderOpen className="size-[15px]" />
              </button>
            </Tooltip>
          )}
        </div>
      </div>
    </article>
  );
}

export function ExternalGameRow({
  entry,
  onOpen,
  onLaunch,
  onOpenFolder,
}: {
  entry: ImportLibraryEntry;
  onOpen: () => void;
  onLaunch: () => void;
  onOpenFolder: () => void;
}) {
  return (
    <div
      data-gamepad-target="true"
      onClick={(event) => {
        // The row itself opens the local sheet; its Play/Folder buttons keep
        // their own actions (#154).
        if (!(event.target as HTMLElement).closest("button")) onOpen();
      }}
      className="row-hover group grid grid-cols-[2rem_1fr_auto] items-center gap-4 border-t border-line py-2.5 sm:grid-cols-[2rem_1fr_4rem_5rem_3rem] md:grid-cols-[2rem_1fr_4rem_5rem_3rem_6rem]"
    >
      <span className="num text-[10.5px] text-ink-3">—</span>
      <div className="flex min-w-0 items-center gap-3">
        <div className="w-9 shrink-0 overflow-hidden rounded-[1px]">
          <GameCover title={entry.title} coverUrl={null} showType={false} />
        </div>
        <div className="min-w-0">
          <button type="button" onClick={onOpen} className="block max-w-full truncate text-left text-[14px] text-ink">
            {entry.title}
          </button>
          <p className="flex min-w-0 items-center gap-2 truncate text-[11.5px] text-ink-3">
            <LauncherBadge entry={entry} />
            <span className="truncate font-mono" title={entry.install?.installPath ?? undefined}>
              {entry.install?.installPath ? folderLabel(entry.install.installPath) : t("Launcher import")}
            </span>
          </p>
        </div>
      </div>
      <span className="num hidden text-right text-[11.5px] text-ink-3 sm:block">—</span>
      <span className="num text-right text-[11.5px] text-ink-3">{formatBytes(entry.sizeBytes)}</span>
      <span className="num hidden text-right text-[11.5px] text-ink-2 md:block">—</span>
      <span className="hidden justify-end gap-1 sm:flex">
        <Button
          variant="ghost"
          size="sm"
          className="h-7 px-2"
          aria-label={t("Play {title}", { title: entry.title })}
          onClick={onLaunch}
        >
          <Play className="size-3.5" aria-hidden="true" />
        </Button>
        {entry.install?.installPath && (
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2"
            aria-label={t("Open folder for {title}", { title: entry.title })}
            onClick={onOpenFolder}
          >
            <FolderOpen className="size-3.5" aria-hidden="true" />
          </Button>
        )}
      </span>
    </div>
  );
}

export function ExternalImportDialog({
  entry,
  open,
  onOpenChange,
  onLaunch,
  onOpenFolder,
  onRemove,
  removing = false,
}: {
  entry: ImportLibraryEntry | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onLaunch: (entry: ImportLibraryEntry) => void;
  onOpenFolder: (entry: ImportLibraryEntry) => void;
  onRemove: (entry: ImportLibraryEntry) => void;
  removing?: boolean;
}) {
  if (!entry) return null;
  const launcher = entry.launcher ? launcherLabel(entry.launcher) : t("the launcher");
  const path = entry.install?.installPath ?? "";
  const uri = entry.install?.launchUri ?? null;

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={entry.title}
      description={t("Imported from {source}. Ludex keeps an index row only.", { source: launcher })}
      footer={
        <>
          <Button
            variant="ghost"
            icon={<Trash2 className="size-3.5" />}
            loading={removing}
            onClick={() => onRemove(entry)}
          >
            {t("Remove from Ludex")}
          </Button>
          <Button variant="primary" icon={<Play className="size-3.5" />} onClick={() => onLaunch(entry)}>
            {t("Play")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="flex flex-wrap items-center gap-2">
          <LauncherBadge entry={entry} />
          <Badge tone="outline">{t("Launcher import")}</Badge>
          <span className="num text-[11.5px] text-ink-3">{formatBytes(entry.sizeBytes)}</span>
        </div>

        <dl className="grid grid-cols-[6rem_1fr] gap-x-3 gap-y-2 text-[12px]">
          <dt className="text-ink-3">{t("Folder")}</dt>
          <dd className="min-w-0 truncate font-mono text-[11.5px] text-ink-2" title={path || undefined}>
            {path || t("No folder reported by the launcher")}
          </dd>
          <dt className="text-ink-3">{t("Launch URI")}</dt>
          <dd className="min-w-0 truncate font-mono text-[11.5px] text-ink-2" title={uri ?? undefined}>
            {uri ?? t("Not reported")}
          </dd>
        </dl>

        <p className="border-l-2 border-line-2 pl-3 text-[11.5px] leading-relaxed text-ink-3">
          {t("Removing it from Ludex deletes the index row only. The launcher's files stay where they are and the game keeps working there.")}
        </p>

        <div className="flex flex-wrap gap-2">
          {path && (
            <Button
              variant="default"
              size="sm"
              icon={<FolderOpen className="size-3.5" />}
              onClick={() => onOpenFolder(entry)}
            >
              {t("Open folder")}
            </Button>
          )}
          {uri && (
            <a
              href={uri}
              target="_blank"
              rel="noreferrer"
              className={cn(
                "inline-flex h-8 items-center gap-1.5 rounded-[2px] border border-line-2 px-2.5 text-[12.5px] text-ink-2",
                "transition-colors hover:border-ink-3 hover:text-ink",
              )}
            >
              {t("Open in {launcher}", { launcher })}
              <ExternalLink className="size-3" aria-hidden="true" />
            </a>
          )}
        </div>
      </div>
    </Dialog>
  );
}
