import { useNavigate } from "react-router-dom";
import { Plus } from "lucide-react";
import { formatPercent } from "@/lib/format";
import type { GameSummary } from "@/lib/types";
import { useInstallingPercent } from "@/state/downloads";
import { launcherLabel, type LauncherSourceId } from "@/lib/library-entries";
import { EmptyState } from "@/components/page";
import { GameCover } from "@/components/art/cover";
import { Badge, Button, Skeleton } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

export type LibraryMode = "library" | "recent" | "bookmarks" | "index";

export function GameCardCoverThumb({ game }: { game: GameSummary }) {
  return <GameCover title={game.title} coverUrl={game.coverUrl} showType={false} />;
}

export function LibrarySkeleton({ view }: { view: "grid" | "list" }) {
  if (view === "list") {
    return (
      <div className="border-t border-line">
        {Array.from({ length: 9 }).map((_, i) => (
          <div key={i} className="flex items-center gap-4 border-b border-line py-3">
            <Skeleton className="h-3 w-6" />
            <Skeleton className="h-12 w-9" />
            <div className="flex-1 space-y-2">
              <Skeleton className="h-3.5 w-1/3" />
              <Skeleton className="h-2.5 w-1/5" />
            </div>
            <Skeleton className="hidden h-3 w-16 sm:block" />
          </div>
        ))}
      </div>
    );
  }
  return (
    <div className="grid grid-cols-2 gap-x-6 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6">
      {Array.from({ length: 10 }).map((_, i) => (
        <div key={i}>
          <Skeleton className="aspect-[2/3] w-full" />
          <div className="mt-3 space-y-2">
            <Skeleton className="h-3.5 w-4/5" />
            <Skeleton className="h-2.5 w-2/5" />
          </div>
        </div>
      ))}
    </div>
  );
}

export function LibraryEmpty({
  mode,
  hasFilters,
  installedOnly,
  isEditor,
  signedIn,
  onClear,
}: {
  mode: LibraryMode;
  hasFilters: boolean;
  installedOnly: boolean;
  isEditor: boolean;
  signedIn: boolean;
  onClear: () => void;
}) {
  const navigate = useNavigate();
  if (mode === "library" && hasFilters && installedOnly) {
    return (
      <EmptyState
        title={t("No installed games")}
        description={t("Nothing from this library is installed on this machine yet. Install a game from its page, or show the full library.")}
      >
        <Button variant="default" onClick={onClear}>
          {t("Show all games")}
        </Button>
      </EmptyState>
    );
  }
  if (hasFilters) {
    return (
      <EmptyState
        title={t("No games match")}
        description={t("Nothing in the library fits those filters. Widen the search, or clear them and start again.")}
      >
        <Button variant="default" onClick={onClear}>
          {t("Clear filters")}
        </Button>
      </EmptyState>
    );
  }

  if (mode === "bookmarks") {
    return (
      <EmptyState title={t("No bookmarks yet")} description={t("Bookmark a game from the library and it will wait for you here.")}>
        <Button variant="default" onClick={() => navigate("/library")}>
          {t("Browse the library")}
        </Button>
      </EmptyState>
    );
  }

  if (mode === "recent") {
    return (
      <EmptyState
        title={t("Nothing played yet")}
        description={t("Launch a game from the desktop app and it will show up here with its playtime.")}
      >
        <Button variant="default" onClick={() => navigate("/library")}>
          {t("Browse the library")}
        </Button>
      </EmptyState>
    );
  }

  return (
    <EmptyState
      className="max-w-[760px]"
      title={t("Nothing on the shelf yet")}
      description={
        isEditor
          ? t("The games folder is empty. Paste a download link and Ludex will fetch it, verify the archive and index it for you.")
          : t("This library is empty. An administrator can add games from the server, or drop archives into the games folder.")
      }
    >
      {isEditor ? (
        <Button variant="primary" onClick={() => navigate("/admin/ingest")} icon={<Plus className="size-4" />}>
          {t("Add games")}
        </Button>
      ) : (
        signedIn && <span className="text-body text-ink-3">{t("Ask an administrator to add the first game.")}</span>
      )}
    </EmptyState>
  );
}

/**
 * One hook per visible row (#237): installing progress lives in the card/row,
 * not in a `downloads` object read by the whole screen. A transfer tick then
 * re-renders this badge alone instead of the 200-row list.
 */
export function InstallStateBadges({
  gameId,
  installed,
  launcher,
}: {
  gameId: number;
  installed: boolean;
  launcher: LauncherSourceId | null;
}) {
  const percent = useInstallingPercent(gameId);
  if (percent !== null) {
    return (
      <Badge tone="signal" className="hidden shrink-0 sm:inline-flex">
        {t("Installing {percent}", { percent: formatPercent(percent) })}
      </Badge>
    );
  }
  if (!installed) return null;
  return (
    <span className="hidden shrink-0 items-center gap-1 sm:inline-flex">
      <Badge tone="outline">{t("Installed")}</Badge>
      {launcher && <Badge tone="signal">{launcherLabel(launcher)}</Badge>}
    </span>
  );
}
