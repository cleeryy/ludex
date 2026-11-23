import { memo } from "react";
import { Link } from "react-router-dom";
import { Bookmark, BookmarkCheck, Play, RefreshCw } from "lucide-react";
import type { GameSummary, GameUpdate, InstalledGame } from "@/lib/types";
import { cn } from "@/lib/cn";
import { formatBytes, formatMinutes, formatRating, formatYear, formatPercent } from "@/lib/format";
import { GameCover } from "@/components/art/cover";
import { STATE_TONE } from "@/components/game/state";
import { Led } from "@/components/ui/controls";
import { Tooltip } from "@/components/ui/tooltip";
import { t } from "@/lib/i18n";
import { useInstallAction } from "@/lib/use-install-action";

function GameCardBase({
  game,
  index,
  installed,
  update,
  source,
  onBookmark,
  onPlay,
  onUpdate,
}: {
  game: GameSummary;
  index: number;
  installed?: InstalledGame;
  /** Newer version available on the server (#30). */
  update?: GameUpdate;
  /** Launcher that owns the install, shown on matched imports (#154). */
  source?: string | null;
  onBookmark: (next: boolean) => void;
  onPlay?: () => void;
  onUpdate?: () => void;
}) {
  // Subscribed per card (#237): the shared action matrix decides Play/Update,
  // and a progress tick only repaints the card being installed.
  const action = useInstallAction(game.id, {
    installed: Boolean(installed),
    update: Boolean(update),
  });
  const installing = action.percent;
  const played = formatMinutes(game.minutesPlayed);
  const rating = formatRating(game.rating);

  return (
    <article className="cover-host group relative flex flex-col">
      {/* 3:4 matte poster cover (#333): border line->ink and -2px lift on hover, zero stacked badges over artwork. */}
      <div className="relative transition-transform duration-[180ms] ease-out group-hover:-translate-y-[2px]">
        <GameCover
          title={game.title}
          coverUrl={game.coverUrl}
          index={index}
          year={formatYear(game.releaseDate)}
          genre={game.genres[0] ?? null}
        />
        <Link
          to={`/game/${game.id}`}
          data-gamepad-target="true"
          className="absolute inset-0 z-[4] rounded-[var(--r-md)]"
          aria-label={t("Open {title}", { title: game.title })}
        />
        {installing !== null && (
          <span className="absolute inset-x-0 bottom-0 z-[5] h-[3px] overflow-hidden rounded-b-[var(--r-md)] bg-line/70" aria-hidden="true">
            <span className="block h-full bg-signal transition-[width] duration-500" style={{ width: `${installing}%` }} />
          </span>
        )}
      </div>

      <div className="mt-3 flex items-start gap-2">
        <span className={cn("mt-[5px] h-3.5 w-[2px] shrink-0", STATE_TONE[game.state].bar)} aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-body font-medium leading-snug text-ink">
            <Link to={`/game/${game.id}`} className="hover:text-ink">
              {game.title}
            </Link>
          </h3>

          <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-meta leading-none text-ink-3">
            {installing !== null ? (
              <span className="inline-flex items-center gap-1.5 text-signal-text">
                <Led tone="info" pulse />
                <span>
                  {t("Installing {percent}", { percent: installing > 0 ? formatPercent(installing) : "" })}
                </span>
              </span>
            ) : game.versionCount === 0 ? (
              <span
                className="inline-flex items-center gap-1.5 text-danger"
                title={t("No archive available: the file was deleted or its root is offline.")}
              >
                <Led tone="danger" />
                <span>{t("Missing")}</span>
              </span>
            ) : update ? (
              <span
                className="inline-flex items-center gap-1.5 text-signal-text"
                title={
                  update.latestVersionLabel
                    ? t("Version {version} is available", { version: update.latestVersionLabel })
                    : t("An update is available")
                }
              >
                <Led tone="signal" />
                <span>{t("Update")}</span>
              </span>
            ) : installed ? (
              <span className="inline-flex items-center gap-1.5 text-ok">
                <Led tone="ok" />
                <span>{source ? `${t("Installed")} · ${source}` : t("Installed")}</span>
              </span>
            ) : null}

            <span className="num">{formatYear(game.releaseDate)}</span>
            {game.kind === "rom" && game.platform && (
              <span className="font-medium uppercase text-signal-text">{game.platform}</span>
            )}
            <span className="num">{formatBytes(game.totalSize)}</span>
            {played && <span className="num text-ink-2">{played}</span>}
            {rating && (
              <span
                className="num ml-auto text-ink-2"
                aria-label={t("Rating {rating} out of 100", { rating })}
              >
                {rating}
              </span>
            )}
          </div>
        </div>
        <div className="-my-2 -mr-2 flex shrink-0 items-center">
          {update && action.update && onUpdate && !installing && (
            <Tooltip label={update.latestVersionLabel ? t("Update to {version}", { version: update.latestVersionLabel }) : t("Update")}>
              <button
                type="button"
                onClick={onUpdate}
                aria-label={t("Update {title}", { title: game.title })}
                className="grid size-9 place-items-center rounded-[var(--r-sm)] text-signal-text transition-colors hover:bg-surface hover:text-signal-text max-sm:size-11"
              >
                <RefreshCw className="size-[15px]" />
              </button>
            </Tooltip>
          )}
          {action.kind === "play" && installing === null && onPlay && (
            <Tooltip label={t("Play")}>
              <button
                type="button"
                onClick={onPlay}
                aria-label={t("Play {title}", { title: game.title })}
                className="grid size-9 place-items-center rounded-[var(--r-sm)] text-ink-3 transition-colors hover:bg-surface hover:text-ink max-sm:size-11"
              >
                <Play className="size-[15px]" />
              </button>
            </Tooltip>
          )}
          <Tooltip label={game.bookmarked ? t("Remove bookmark") : t("Bookmark")}>
            <button
              type="button"
              onClick={() => onBookmark(!game.bookmarked)}
              aria-pressed={game.bookmarked}
              aria-label={
                game.bookmarked
                  ? t("Remove bookmark for {title}", { title: game.title })
                  : t("Bookmark {title}", { title: game.title })
              }
              className={cn(
                "grid size-9 place-items-center rounded-[var(--r-sm)] transition-colors max-sm:size-11",
                game.bookmarked ? "text-signal-text" : "text-ink-3 hover:bg-surface hover:text-ink",
              )}
            >
              {game.bookmarked ? <BookmarkCheck className="size-[15px]" /> : <Bookmark className="size-[15px]" />}
            </button>
          </Tooltip>
        </div>
      </div>
    </article>
  );
}

/**
 * Memoised (#237): the library re-renders when filters, data or one card's
 * install progress changes, and every other card keeps its DOM. The inline
 * handlers are intentionally ignored by the comparator: they only close over
 * `bookmark`/`launch` mutations (stable) and the same `game` object, so a
 * changed callback identity with an unchanged game cannot change the output.
 */
export const GameCard = memo(GameCardBase, (previous, next) => {
  return (
    previous.game === next.game &&
    previous.index === next.index &&
    previous.installed === next.installed &&
    previous.update === next.update &&
    previous.source === next.source
  );
});
