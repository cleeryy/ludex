import { Link } from "react-router-dom";
import { Bookmark, BookmarkCheck, Play, RefreshCw } from "lucide-react";
import type { GameSummary, GameUpdate, InstalledGame } from "@/lib/types";
import { cn } from "@/lib/cn";
import { formatBytes, formatMinutes, formatRating, formatYear } from "@/lib/format";
import { GameCover } from "@/components/art/cover";
import { STATE_TONE } from "@/components/game/state";
import { Tooltip } from "@/components/ui/tooltip";

export function GameCard({
  game,
  index,
  installed,
  installing,
  update,
  onBookmark,
  onPlay,
  onUpdate,
}: {
  game: GameSummary;
  index: number;
  installed?: InstalledGame;
  /** Percent complete while an install job is active. */
  installing?: number | null;
  /** Newer version available on the server (#30). */
  update?: GameUpdate;
  onBookmark: (next: boolean) => void;
  onPlay?: () => void;
  onUpdate?: () => void;
}) {
  const played = formatMinutes(game.minutesPlayed);

  return (
    <article className="cover-host group relative flex flex-col">
      <div className="relative transition-transform duration-[200ms] ease-out group-hover:-translate-y-[3px]">
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
          className="absolute inset-0 z-[4] rounded-[2px]"
          aria-label={`Open ${game.title}`}
        />
        {installing !== null && installing !== undefined ? (
          <span className="absolute left-2 top-2 z-[5] inline-flex h-5 items-center rounded-[2px] bg-signal px-1.5 font-mono text-[9.5px] uppercase tracking-[0.1em] text-on-signal">
            Installing {installing > 0 ? `${installing.toFixed(0)}%` : ""}
          </span>
        ) : update ? (
          <span
            className="absolute left-2 top-2 z-[5] inline-flex h-5 items-center rounded-[2px] bg-signal px-1.5 font-mono text-[9.5px] uppercase tracking-[0.1em] text-on-signal"
            title={update.latestVersionLabel ? `Version ${update.latestVersionLabel} is available` : "An update is available"}
          >
            Update
          </span>
        ) : installed ? (
          <span className="absolute left-2 top-2 z-[5] inline-flex h-5 items-center rounded-[2px] bg-ink px-1.5 font-mono text-[9.5px] uppercase tracking-[0.1em] text-bg">
            Installed
          </span>
        ) : null}
        {installing !== null && installing !== undefined && (
          <span className="absolute inset-x-0 bottom-0 z-[5] h-[3px] bg-line/60" aria-hidden="true">
            <span className="block h-full bg-signal transition-[width] duration-500" style={{ width: `${installing}%` }} />
          </span>
        )}
        <div
          className="pointer-events-none absolute inset-0 z-[3] rounded-[2px] opacity-0 transition-opacity duration-[200ms] group-hover:opacity-100"
          style={{ boxShadow: "0 22px 40px -24px rgba(0,0,0,0.75), 0 0 0 1px rgba(255,255,255,0.08)" }}
          aria-hidden="true"
        />
      </div>

      <div className="mt-3 flex items-start gap-2">
        <span className={cn("mt-[5px] h-3.5 w-[2px] shrink-0", STATE_TONE[game.state].bar)} aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-[13.5px] font-medium leading-snug text-ink">
            <Link to={`/game/${game.id}`} className="hover:text-ink">
              {game.title}
            </Link>
          </h3>
          <div className="mt-1 flex flex-wrap items-baseline gap-x-2.5 gap-y-0.5 font-mono text-[10.5px] leading-none text-ink-3">
            <span>{formatYear(game.releaseDate)}</span>
            <span>{formatBytes(game.totalSize)}</span>
            {played && <span className="text-ink-2">{played}</span>}
            {formatRating(game.rating) && (
              <span className="ml-auto text-ink-2" aria-label={`Rating ${formatRating(game.rating)} out of 100`}>
                {formatRating(game.rating)}
              </span>
            )}
          </div>
        </div>
        <div className="-my-2 -mr-2 flex shrink-0 items-center">
          {update && onUpdate && !installing && (
            <Tooltip label={update.latestVersionLabel ? `Update to ${update.latestVersionLabel}` : "Update"}>
              <button
                type="button"
                onClick={onUpdate}
                aria-label={`Update ${game.title}`}
                className="grid size-10 place-items-center rounded-[2px] text-signal-text transition-colors hover:bg-surface hover:text-signal-text max-sm:size-11"
              >
                <RefreshCw className="size-[15px]" />
              </button>
            </Tooltip>
          )}
          {installed && !installing && onPlay && (
            <Tooltip label="Play">
              <button
                type="button"
                onClick={onPlay}
                aria-label={`Play ${game.title}`}
                className="grid size-10 place-items-center rounded-[2px] text-ink-3 transition-colors hover:bg-surface hover:text-ink max-sm:size-11"
              >
                <Play className="size-[15px]" />
              </button>
            </Tooltip>
          )}
          <Tooltip label={game.bookmarked ? "Remove bookmark" : "Bookmark"}>
            <button
              type="button"
              onClick={() => onBookmark(!game.bookmarked)}
              aria-pressed={game.bookmarked}
              aria-label={game.bookmarked ? `Remove bookmark for ${game.title}` : `Bookmark ${game.title}`}
              className={cn(
                "grid size-10 place-items-center rounded-[2px] transition-colors max-sm:size-11",
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
