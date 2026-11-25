import { memo } from "react";
import { Link } from "react-router-dom";
import { Bookmark, BookmarkCheck, ChevronRight, Play, RefreshCw } from "lucide-react";
import type { GameSummary, GameUpdate, InstalledGame } from "@/lib/types";
import { cn } from "@/lib/cn";
import { formatBytes, formatMinutes, formatPercent, formatRating, formatYear } from "@/lib/format";
import { GameCover } from "@/components/art/cover";
import { STATE_TONE } from "@/components/game/state";
import { Led } from "@/components/ui/controls";
import { Tooltip } from "@/components/ui/tooltip";
import { t } from "@/lib/i18n";
import { useInstallAction } from "@/lib/use-install-action";

function GameCardBase({
  game,
  index,
  variant = "poster",
  installed,
  update,
  source,
  onBookmark,
  onPlay,
  onUpdate,
}: {
  game: GameSummary;
  index: number;
  /** `poster` renders native 2:3 box art; `wide` renders 16:9 Steam/GOG banners. */
  variant?: "poster" | "wide";
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
  const canPlay = action.kind === "play" && installing === null && Boolean(onPlay);
  const canUpdate = Boolean(update && action.update && onUpdate && installing === null);
  const genres = game.genres.slice(0, 2).join(" · ");

  return (
    <article className="cover-host group relative flex flex-col">
      <div className="relative overflow-hidden rounded-[var(--r-md)] transition-all duration-[200ms] ease-out group-hover:-translate-y-[3px] group-hover:shadow-cover">
        <GameCover
          title={game.title}
          coverUrl={variant === "wide" ? (game.backgroundUrl ?? game.coverUrl) : game.coverUrl}
          fallbackUrl={variant === "wide" ? game.coverUrl : undefined}
          variant={variant}
          index={index}
          year={formatYear(game.releaseDate)}
          genre={game.genres[0] ?? null}
          showIndex={false}
        />

        <Link
          to={`/game/${game.id}`}
          data-gamepad-target="true"
          className="absolute inset-0 z-[4] rounded-[var(--r-md)]"
          aria-label={t("Open {title}", { title: game.title })}
        />

        {/* Single high-signal status badge (top-left) only when installed, updating, installing, or missing. */}
        {installing !== null ? (
          <span className="pointer-events-none absolute left-2 top-2 z-[5] inline-flex items-center gap-1.5 rounded-[var(--r-xs)] border border-sheen/15 bg-stage/85 px-2 py-0.5 text-micro font-medium text-signal">
            <Led tone="info" pulse />
            <span className="num">{installing > 0 ? formatPercent(installing) : t("Preparing")}</span>
          </span>
        ) : game.versionCount === 0 ? (
          <span
            className="pointer-events-none absolute left-2 top-2 z-[5] inline-flex items-center gap-1.5 rounded-[var(--r-xs)] border border-danger/45 bg-stage/85 px-2 py-0.5 text-micro font-medium text-danger"
            title={t("No archive available: the file was deleted or its root is offline.")}
          >
            <Led tone="danger" />
            <span>{t("Missing")}</span>
          </span>
        ) : update ? (
          <span
            className="pointer-events-none absolute left-2 top-2 z-[5] inline-flex items-center gap-1.5 rounded-[var(--r-xs)] border border-signal/50 bg-stage/85 px-2 py-0.5 text-micro font-medium text-signal"
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
          <span className="pointer-events-none absolute left-2 top-2 z-[5] inline-flex items-center gap-1.5 rounded-[var(--r-xs)] border border-sheen/15 bg-stage/85 px-2 py-0.5 text-micro font-medium text-stage-ok">
            <Led tone="ok" />
            <span>{source ?? t("Installed")}</span>
          </span>
        ) : null}

        {/* Floating quick-bookmark toggle (top-right): always visible when bookmarked, revealed on hover otherwise. */}
        <div
          className={cn(
            "absolute right-2 top-2 z-[6] transition-opacity duration-[160ms]",
            game.bookmarked ? "opacity-100" : "opacity-0 group-focus-within:opacity-100 group-hover:opacity-100",
          )}
        >
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
                "grid size-8 place-items-center rounded-[var(--r-sm)] border transition-colors",
                game.bookmarked
                  ? "border-signal/60 bg-stage/90 text-signal"
                  : "border-sheen/20 bg-stage/80 text-stage-ink hover:border-sheen/40 hover:bg-stage",
              )}
            >
              {game.bookmarked ? <BookmarkCheck className="size-4" /> : <Bookmark className="size-4" />}
            </button>
          </Tooltip>
        </div>

        {/* Hover action veil (bottom): instant Play/Update CTA or genre teaser like Steam/Epic/GOG. */}
        <div className="pointer-events-none absolute inset-x-0 bottom-0 z-[6] flex flex-col justify-end bg-gradient-to-t from-stage via-stage/75 to-transparent px-2.5 pb-2.5 pt-10 opacity-0 transition-opacity duration-[180ms] group-focus-within:opacity-100 group-hover:opacity-100">
          {canPlay ? (
            <div className="pointer-events-auto flex items-center gap-1.5">
              <button
                type="button"
                onClick={onPlay}
                aria-label={t("Play {title}", { title: game.title })}
                className="inline-flex h-8 flex-1 items-center justify-center gap-1.5 rounded-[var(--r-sm)] bg-signal px-3 text-label font-semibold text-on-signal transition-opacity hover:opacity-90"
              >
                <Play className="size-3.5 fill-current" aria-hidden="true" />
                <span>{t("Play")}</span>
              </button>
              {canUpdate && (
                <Tooltip
                  label={
                    update?.latestVersionLabel
                      ? t("Update to {version}", { version: update.latestVersionLabel })
                      : t("Update")
                  }
                >
                  <button
                    type="button"
                    onClick={onUpdate}
                    aria-label={t("Update {title}", { title: game.title })}
                    className="grid size-8 shrink-0 place-items-center rounded-[var(--r-sm)] border border-sheen/25 bg-stage/90 text-stage-ink transition-colors hover:border-signal hover:text-signal"
                  >
                    <RefreshCw className="size-3.5" aria-hidden="true" />
                  </button>
                </Tooltip>
              )}
            </div>
          ) : canUpdate ? (
            <button
              type="button"
              onClick={onUpdate}
              aria-label={t("Update {title}", { title: game.title })}
              className="pointer-events-auto inline-flex h-8 w-full items-center justify-center gap-1.5 rounded-[var(--r-sm)] bg-signal px-3 text-label font-semibold text-on-signal transition-opacity hover:opacity-90"
            >
              <RefreshCw className="size-3.5" aria-hidden="true" />
              <span>{t("Update")}</span>
            </button>
          ) : (
            <div className="flex items-center justify-between gap-2 text-micro text-stage-ink-2">
              <span className="truncate">{genres || formatBytes(game.totalSize)}</span>
              <ChevronRight className="rtl:-scale-x-100 size-3.5 shrink-0 text-stage-ink" aria-hidden="true" />
            </div>
          )}
        </div>

        {installing !== null && (
          <span
            className="absolute inset-x-0 bottom-0 z-[7] h-[3px] overflow-hidden rounded-b-[var(--r-md)] bg-line/70"
            aria-hidden="true"
          >
            <span
              className="block h-full bg-signal transition-[width] duration-500"
              style={{ width: `${installing}%` }}
            />
          </span>
        )}
      </div>

      <div className="mt-2.5 flex items-start gap-2">
        <span
          className={cn("mt-[5px] h-3.5 w-[2px] shrink-0 rounded-full", STATE_TONE[game.state].bar)}
          aria-hidden="true"
        />
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-body font-semibold leading-snug text-ink">
            <Link to={`/game/${game.id}`} className="transition-colors hover:text-signal-text">
              {game.title}
            </Link>
          </h3>

          <div className="mt-1 flex items-center justify-between gap-2 text-meta leading-none text-ink-3">
            <div className="flex min-w-0 items-center gap-x-2 truncate">
              <span className="num shrink-0">{formatYear(game.releaseDate)}</span>
              {game.kind === "rom" && game.platform ? (
                <span className="shrink-0 font-medium uppercase text-signal-text">{game.platform}</span>
              ) : played ? (
                <span className="num truncate text-ink-2">{played}</span>
              ) : (
                <span className="num truncate">{formatBytes(game.totalSize)}</span>
              )}
            </div>
            {rating && (
              <span
                className="num shrink-0 rounded-[var(--r-xs)] border border-line bg-surface px-1.5 py-0.5 text-micro font-medium text-ink-2"
                aria-label={t("Rating {rating} out of 100", { rating })}
              >
                {rating}
              </span>
            )}
          </div>
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
    previous.variant === next.variant &&
    previous.installed === next.installed &&
    previous.update === next.update &&
    previous.source === next.source
  );
});
