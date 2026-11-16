import { memo, type ReactNode } from "react";
import { motion, useReducedMotion } from "motion/react";
import { cn } from "@/lib/cn";
import {
  formatBytes,
  formatMinutes,
  formatPercent,
  formatRating,
  formatRelative,
  formatSpeed,
  formatYear,
  stateLabel,
} from "@/lib/format";
import { t } from "@/lib/i18n";
import type { GameSummary, InstallJob } from "@/lib/types";
import { GameCover } from "@/components/art/cover";
import { Kbd } from "@/components/ui/controls";
import { hintsFor, padFamilyLabel, type HintContext, type HintId, type PadFamily } from "./controller";
import { reveal } from "./entrance";
import type { BigPictureLayout } from "./layout";
import { BOOT_DELAY, BOOT_DURATION } from "./motion";
import type { BigPictureNavigation } from "./navigation";

/* --------------------------------------------------------------------------
   Pieces every console composition shares (#141, #268): the stage copy, the
   chips, the card shapes, the bracket and the single hint strip.

   Cabinet rules applied here once, so six components cannot drift:
   - words are sentence case in the UI face; mono is only numerals and units;
   - one bracket per selected object: a signal block on the leading edge and a
     1px warm rim. No second outline, no glow;
   - the caption sits under the selected cover only;
   - `text-bp-meta` (24u) is the floor.
   -------------------------------------------------------------------------- */

export function SelectedEyebrow({ game, className }: { game: GameSummary; className?: string }) {
  return (
    <span className={cn("flex items-center gap-2.5 text-bp-meta text-ink-2", className)}>
      <span className="bp-dot" aria-hidden="true" />
      {game.lastPlayedAt ? t("Continue playing") : t("From the library")}
    </span>
  );
}

export function MetaLine({
  game,
  installed,
  className,
}: {
  game: GameSummary;
  installed: boolean;
  className?: string;
}) {
  const minutes = formatMinutes(game.minutesPlayed);
  const rating = formatRating(game.rating);
  return (
    <div
      className={cn(
        "flex flex-wrap items-center gap-x-5 gap-y-1 text-bp-meta text-stage-ink-2",
        className,
      )}
    >
      <span className="num">{formatYear(game.releaseDate)}</span>
      <span>{stateLabel(game.state)}</span>
      {rating && (
        <span>
          <span className="num">{rating}</span> / <span className="num">100</span>
        </span>
      )}
      {minutes && (
        <span>
          <span className="num">{minutes}</span> {t("played")}
        </span>
      )}
      {game.lastPlayedAt && (
        <span>
          {t("Last played")} <span className="num">{formatRelative(game.lastPlayedAt)}</span>
        </span>
      )}
      {installed && <span>{t("Installed")}</span>}
    </div>
  );
}

export function ProgressLine({
  job,
  percent,
  className,
}: {
  job: InstallJob;
  percent: number | null;
  className?: string;
}) {
  return (
    <p className={cn("flex flex-wrap items-baseline gap-x-3 text-bp-meta text-stage-ink-2", className)}>
      <span className="num text-signal-text">
        {percent !== null ? formatPercent(percent) : t("Preparing")}
      </span>
      {job.bytesTotal && (
        <span>
          <span className="num">{formatBytes(job.bytesDone)}</span> {t("of")}{" "}
          <span className="num">{formatBytes(job.bytesTotal)}</span>
        </span>
      )}
      {job.speedBps > 0 && <span className="num">{formatSpeed(job.speedBps)}</span>}
    </p>
  );
}

export function ActionRow({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("flex flex-wrap items-center gap-3", className)}>{children}</div>;
}

/* ---------------------------------- chips --------------------------------- */

/**
 * Shelf chips: Recent, All games, then Collections. They are not in the gamepad
 * target list — the library's focus model is one ring on one game — but they
 * are real buttons for a mouse and a keyboard, and shoulders still switch them.
 */
export function ShelfChips({
  nav,
  onOpenCollections,
  className,
}: {
  nav: BigPictureNavigation;
  onOpenCollections?: () => void;
  className?: string;
}) {
  return (
    <div
      role="group"
      aria-label={t("Shelves")}
      className={cn("nav-scroll flex shrink-0 items-center gap-2 overflow-x-auto", className)}
    >
      {nav.shelves.map((shelf, position) => {
        const active = position === nav.shelfIndex;
        return (
          <button
            key={shelf.id}
            type="button"
            aria-pressed={active}
            data-bp-tab-active={active ? "true" : undefined}
            onClick={() => nav.selectShelf(position)}
            className={cn(
              "bp-chip inline-flex shrink-0 items-center gap-2",
              active
                ? "border-line-2 bg-surface-2 text-ink"
                : "border-transparent text-ink-3 hover:text-ink-2",
            )}
          >
            <span className="truncate">{shelf.label}</span>
            <span className={cn("num", active ? "text-signal-text" : "text-ink-3")}>
              {shelf.games.length}
            </span>
          </button>
        );
      })}
      {onOpenCollections && (
        <button
          type="button"
          onClick={onOpenCollections}
          className="bp-chip inline-flex shrink-0 items-center gap-2 border-transparent text-ink-3 hover:text-ink-2"
        >
          {t("Collections")}
        </button>
      )}
    </div>
  );
}

/* --------------------------------- cards --------------------------------- */

/**
 * The bracket (#268): a 6u signal block on the leading edge and a 1px warm rim.
 * It is drawn by the selected object only, so a pad move can never paint a
 * second outline over it.
 */
export function Bracket({ inset = false }: { inset?: boolean }) {
  return (
    <>
      <span aria-hidden="true" data-bp-bracket="true" />
      <span aria-hidden="true" data-bp-rim="true" data-bp-rim-inset={inset ? "true" : undefined} />
    </>
  );
}

/** Rail card: cover, and a caption under the selected cover only. */
function RailCardBase({
  game,
  position,
  selected,
  nav,
  className,
}: {
  game: GameSummary;
  position: number;
  selected: boolean;
  nav: BigPictureNavigation;
  className?: string;
}) {
  return (
    <button
      type="button"
      ref={nav.cardRef(position)}
      data-bp-card="true"
      data-gamepad-target="true"
      onClick={() => nav.setIndex(position)}
      aria-current={selected ? "true" : undefined}
      aria-label={game.title}
      className={cn(
        "group relative shrink-0 text-start bp-focus-move bp-rail-card",
        selected ? "opacity-100" : "opacity-70 hover:opacity-100",
        className,
      )}
    >
      <div className="relative block">
        <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} showType={false} />
        {selected && <Bracket />}
      </div>
      <span
        aria-hidden={!selected}
        className={cn(
          "mt-2 block truncate text-bp-meta leading-snug",
          selected ? "text-ink" : "invisible",
        )}
      >
        {game.title}
      </span>
    </button>
  );
}

/**
 * Memoised (#237): the screen re-renders on every download sample while an
 * install runs. A card whose game/selection state did not change keeps its DOM
 * and its inline SVG cover.
 */
export const RailCard = memo(RailCardBase);

/** Wall card: the caption sits under the plate, the bracket hugs the plate. */
function GridCardBase({
  game,
  position,
  selected,
  nav,
}: {
  game: GameSummary;
  position: number;
  selected: boolean;
  nav: BigPictureNavigation;
}) {
  return (
    <button
      type="button"
      ref={nav.cardRef(position)}
      data-bp-card="true"
      data-gamepad-target="true"
      onClick={() => nav.setIndex(position)}
      aria-current={selected ? "true" : undefined}
      aria-label={game.title}
      className="group relative flex w-full min-w-0 flex-col gap-1.5 text-start"
    >
      <div className="relative block" data-bp-zone-item="true" data-bp-zone-active={selected ? "true" : undefined}>
        <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} showType={false} />
        {selected && <Bracket />}
      </div>
      <span
        className={cn(
          "line-clamp-2 min-h-[2.5em] text-bp-meta leading-snug",
          selected ? "text-ink" : "text-ink-2",
        )}
      >
        {game.title}
      </span>
      <span className="num block truncate text-bp-meta text-ink-3">
        {formatYear(game.releaseDate)}
        {selected ? ` · ${stateLabel(game.state)}` : ""}
      </span>
    </button>
  );
}

export const GridCard = memo(GridCardBase);

/** One list row: plate, title, state, live progress. No index, no second ring. */
function ListRowBase({
  game,
  position,
  selected,
  nav,
  job,
  percent,
}: {
  game: GameSummary;
  position: number;
  selected: boolean;
  nav: BigPictureNavigation;
  job: InstallJob | undefined;
  percent: number | null;
}) {
  const minutes = formatMinutes(game.minutesPlayed);
  return (
    <button
      type="button"
      ref={nav.cardRef(position)}
      data-bp-card="true"
      data-gamepad-target="true"
      onClick={() => nav.setIndex(position)}
      aria-current={selected ? "true" : undefined}
      aria-label={game.title}
      data-bp-row="true"
      className={cn(
        "group relative flex w-full items-center gap-4 px-4 py-2.5 text-start",
        selected ? "bg-surface-2" : "hover:bg-surface/70",
      )}
    >
      <div className="relative block h-[calc(var(--bp-u)*52)] w-[calc(var(--bp-u)*39)] shrink-0">
        <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} showType={false} />
      </div>
      <span className="min-w-0 flex-1">
        <span className={cn("block truncate text-bp-body leading-tight", selected ? "text-ink" : "text-ink-2")}>
          {game.title}
        </span>
        <span className="mt-1 block truncate text-bp-meta text-ink-3">
          <span className="num">{formatYear(game.releaseDate)}</span> · {stateLabel(game.state)}
          {minutes ? (
            <>
              {" · "}
              <span className="num">{minutes}</span> {t("played")}
            </>
          ) : null}
        </span>
      </span>
      {job && (
        <span className="num shrink-0 text-bp-meta text-signal-text">
          {percent !== null ? formatPercent(percent) : t("Preparing")}
        </span>
      )}
      {selected && (
        <span aria-hidden="true" className="pointer-events-none absolute inset-y-0 start-0 w-[calc(var(--bp-u)*3)] bg-signal" />
      )}
    </button>
  );
}

export const ListRow = memo(ListRowBase);

/* --------------------------------- footer --------------------------------- */

function Hint({ keys, children }: { keys: ReactNode; children: ReactNode }) {
  return (
    <span className="flex items-center gap-2 text-bp-meta">
      {keys}
      {children}
    </span>
  );
}

const HINT_COPY: Record<HintId, () => string> = {
  navigate: () => t("navigate"),
  select: () => t("select"),
  play: () => t("play or install"),
  shelf: () => t("shelf"),
  page: () => t("page"),
  search: () => t("search"),
  downloads: () => t("downloads"),
  menu: () => t("menu"),
  exit: () => t("exit"),
  choose: () => t("choose"),
  apply: () => t("apply"),
  close: () => t("close"),
  type: () => t("type"),
  delete: () => t("delete"),
  space: () => t("space"),
  clear: () => t("clear"),
  open: () => t("open"),
  cancel: () => t("cancel"),
  back: () => t("back"),
  adjust: () => t("Change"),
  tabs: () => t("Tabs"),
};

/** One hint strip, max three hints, ordered by frequency (A first). */
export function HintStrip({
  context,
  layout,
  connected,
  family,
  swapAB = false,
  className,
}: {
  context: HintContext;
  layout: BigPictureLayout;
  connected: boolean;
  family: PadFamily;
  swapAB?: boolean;
  className?: string;
}) {
  const hints = hintsFor(context, { layout, family, connected, swapAB }).slice(0, 3);
  return (
    <div className={cn("flex flex-wrap items-center gap-x-5 gap-y-1", className)}>
      {hints.map((hint) => (
        <Hint key={hint.id} keys={hint.keys.map((key) => <Kbd key={key}>{key}</Kbd>)}>
          {HINT_COPY[hint.id]()}
        </Hint>
      ))}
    </div>
  );
}

/**
 * The console footer: one hint strip on the left, the active pad on the right.
 * When a sheet is open the screen hides this footer, so a surface can never show
 * two strips.
 */
export function BigPictureFooter({
  layout,
  connected,
  family,
  swapAB = false,
  profileName,
  context,
  haptics,
  className,
}: {
  layout: BigPictureLayout;
  connected: boolean;
  family: PadFamily;
  swapAB?: boolean;
  profileName: string;
  context: HintContext;
  haptics: boolean;
  className?: string;
}) {
  const reduceMotion = useReducedMotion();
  return (
    <motion.footer
      data-bp-footer="true"
      data-bp-bleed-bottom="true"
      className={cn(
        "relative z-10 flex min-h-10 shrink-0 flex-wrap items-center gap-x-6 gap-y-1 border-t pt-2 text-bp-meta text-ink-3 bp-chrome bp-pad-x",
        className,
      )}
      {...reveal(reduceMotion, { y: 6, delay: BOOT_DELAY.footer, duration: BOOT_DURATION })}
    >
      <HintStrip context={context} layout={layout} connected={connected} family={family} swapAB={swapAB} />
      <span className="ms-auto hidden items-center gap-2 xl:flex">
        {connected ? (
          <>
            <span className="bp-dot" aria-hidden="true" />
            <span className="text-ink-2">{padFamilyLabel(family)}</span>
            <span aria-hidden="true">·</span>
            <span className="max-w-[18ch] truncate">{profileName}</span>
            {haptics && <span className="text-signal-text">{t("vibration")}</span>}
          </>
        ) : (
          t("Keyboard only — connect a controller for couch navigation")
        )}
      </span>
    </motion.footer>
  );
}
