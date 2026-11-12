import type { ReactNode } from "react";
import { Gamepad2 } from "lucide-react";
import { cn } from "@/lib/cn";
import {
  formatBytes,
  formatMinutes,
  formatPercent,
  formatRating,
  formatRelative,
  formatYear,
  stateLabel,
} from "@/lib/format";
import { t } from "@/lib/i18n";
import type { GameSummary, InstallJob } from "@/lib/types";
import { GameCover } from "@/components/art/cover";
import { Kbd } from "@/components/ui/controls";
import {
  hintsFor,
  padFamilyLabel,
  type HintContext,
  type HintId,
  type PadFamily,
} from "./controller";
import type { BigPictureLayout } from "./layout";
import type { BigPictureNavigation } from "./navigation";

/* --------------------------------------------------------------------------
   Pieces every Big Picture layout shares (#141): the selected game's meta
   line, its progress sentence, the shelf tabs, the three card shapes and the
   hint strip. Each layout composes these; none of them re-implements the
   information or the focus ring, so the four layouts stay one product.
   -------------------------------------------------------------------------- */

export function SelectedEyebrow({ game, className }: { game: GameSummary; className?: string }) {
  return (
    <span
      className={cn(
        "flex items-center gap-2.5 font-mono text-[10px] uppercase tracking-[0.16em] text-signal",
        className,
      )}
    >
      <span className="h-px w-8 bg-signal" aria-hidden="true" />
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
  return (
    <div className={cn("flex flex-wrap items-center gap-x-5 gap-y-1 font-mono text-[11px] text-stage-ink-2", className)}>
      <span>{formatYear(game.releaseDate)}</span>
      <span>{stateLabel(game.state)}</span>
      {formatRating(game.rating) && <span>{formatRating(game.rating)} / 100</span>}
      {minutes && <span>{t("{duration} played", { duration: minutes })}</span>}
      {game.lastPlayedAt && (
        <span>
          {t("Last played")} {formatRelative(game.lastPlayedAt)}
        </span>
      )}
      {installed && <span className="text-stage-ok">{t("Installed")}</span>}
    </div>
  );
}

export function ProgressLine({ job, percent, className }: { job: InstallJob; percent: number | null; className?: string }) {
  return (
    <p className={cn("flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-stage-ink-2", className)}>
      <span className="text-signal">{percent !== null ? formatPercent(percent) : t("Preparing")}</span>
      {job.bytesTotal && (
        <span>{t("{done} of {total}", { done: formatBytes(job.bytesDone), total: formatBytes(job.bytesTotal) })}</span>
      )}
      {job.speedBps > 0 && <span>{formatBytes(job.speedBps, 0)}/s</span>}
    </p>
  );
}

export function ActionRow({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn("flex flex-wrap items-center gap-2", className)}>{children}</div>;
}

/* ---------------------------------- tabs ---------------------------------- */

/**
 * Shelf tabs. Shoulders switch them, the arrows do in the rail layouts, and a
 * mouse can click one. They are not in the gamepad target list: the library's
 * focus model is one ring on one game, and moving it into the chrome would
 * make B ambiguous.
 */
export function ShelfTabs({
  nav,
  dense = false,
  className,
}: {
  nav: BigPictureNavigation;
  /** Rail layouts put the tabs inside the rail header: shorter, no extra rule. */
  dense?: boolean;
  className?: string;
}) {
  return (
    <div
      role="group"
      aria-label={t("Shelves")}
      className={cn(
        "nav-scroll flex shrink-0 items-stretch gap-1 overflow-x-auto",
        dense ? "h-9 px-0" : "h-11 border-b border-line bp-pad-x",
        className,
      )}
    >
      {nav.shelves.map((shelf, position) => {
        const active = position === nav.shelfIndex;
        return (
          <button
            key={shelf.id}
            type="button"
            aria-pressed={active}
            onClick={() => nav.selectShelf(position)}
            className={cn(
              "inline-flex shrink-0 items-center gap-2 border-b-2 transition-colors duration-150",
              dense ? "px-2 text-[12px]" : "h-full px-3 text-[12.5px]",
              active ? "border-signal text-ink" : "border-transparent text-ink-3 hover:text-ink-2",
            )}
          >
            <span>{shelf.label}</span>
            <span className={cn("num text-[10.5px]", active ? "text-signal-text" : "text-ink-3")}>
              {shelf.games.length}
            </span>
          </button>
        );
      })}
      {!dense && (
        <span className="ms-auto hidden items-center gap-1.5 self-center pe-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3 md:flex">
          <Kbd>LB</Kbd>
          <Kbd>RB</Kbd>
          {t("shelf")}
        </span>
      )}
    </div>
  );
}

/* --------------------------------- cards --------------------------------- */

function InstalledDot({ className }: { className?: string }) {
  return <span className={cn("absolute start-1.5 top-1.5 h-1.5 w-1.5 rounded-full bg-ok", className)} aria-hidden="true" />;
}

function FocusRing() {
  return (
    <span
      aria-hidden="true"
      className="pointer-events-none absolute -inset-[4px] z-10 rounded-[2px] ring-2 ring-signal ring-offset-2 ring-offset-stage"
    />
  );
}

/** Rail card, used by the shelf and hero layouts. */
export function RailCard({
  game,
  position,
  selected,
  nav,
  installed,
  className,
}: {
  game: GameSummary;
  position: number;
  selected: boolean;
  nav: BigPictureNavigation;
  installed: boolean;
  className?: string;
}) {
  return (
    <button
      type="button"
      ref={nav.cardRef(position)}
      data-gamepad-target="true"
      onClick={() => nav.setIndex(position)}
      aria-current={selected ? "true" : undefined}
      aria-label={game.title}
      className={cn(
        "group relative shrink-0 rounded-[2px] text-start transition-[transform,opacity] duration-150",
        selected ? "scale-[1.03] opacity-100" : "opacity-70 hover:opacity-100",
        className,
      )}
    >
      <div className="relative block">
        <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} showType={false} />
        {selected && <FocusRing />}
        {installed && <InstalledDot />}
      </div>
      <span className={cn("mt-2 block truncate text-[12.5px]", selected ? "text-ink" : "text-ink-3")}>
        {game.title}
      </span>
      <span className="num block truncate text-[9.5px] text-ink-3">
        {formatYear(game.releaseDate)}
        {selected ? ` · ${stateLabel(game.state)}` : ""}
      </span>
    </button>
  );
}

/** Cover grid card: the caption sits under the plate, the ring hugs the plate. */
export function GridCard({
  game,
  position,
  selected,
  nav,
  installed,
}: {
  game: GameSummary;
  position: number;
  selected: boolean;
  nav: BigPictureNavigation;
  installed: boolean;
}) {
  return (
    <button
      type="button"
      ref={nav.cardRef(position)}
      data-gamepad-target="true"
      onClick={() => nav.setIndex(position)}
      aria-current={selected ? "true" : undefined}
      aria-label={game.title}
      className="group relative flex w-full min-w-0 flex-col gap-1.5 text-start"
    >
      <div className="relative block">
        <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} showType={false} />
        {selected && <FocusRing />}
        {installed && <InstalledDot />}
      </div>
      <span className={cn("block truncate text-[12.5px] leading-tight", selected ? "text-ink" : "text-ink-2")}>
        {game.title}
      </span>
      <span className="num block truncate text-[9.5px] text-ink-3">
        {formatYear(game.releaseDate)}
        {selected ? ` · ${stateLabel(game.state)}` : ""}
      </span>
    </button>
  );
}

/** One dense list row: index, plate, title, state, live progress. */
export function CompactRow({
  game,
  position,
  selected,
  nav,
  installed,
  job,
  percent,
}: {
  game: GameSummary;
  position: number;
  selected: boolean;
  nav: BigPictureNavigation;
  installed: boolean;
  job: InstallJob | undefined;
  percent: number | null;
}) {
  const minutes = formatMinutes(game.minutesPlayed);
  return (
    <button
      type="button"
      ref={nav.cardRef(position)}
      data-gamepad-target="true"
      onClick={() => nav.setIndex(position)}
      aria-current={selected ? "true" : undefined}
      aria-label={game.title}
      className={cn(
        "group relative flex w-full items-center gap-3.5 border-b border-line px-4 py-2 text-start transition-colors duration-150 lg:gap-4 lg:px-5",
        selected ? "bg-surface-2" : "hover:bg-surface/70",
      )}
    >
      <span className={cn("num w-6 shrink-0 text-[10.5px]", selected ? "text-signal-text" : "text-ink-3")}>
        {String(position + 1).padStart(2, "0")}
      </span>
      <div className="relative block h-[46px] w-[34px] shrink-0">
        <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} showType={false} />
        {installed && <InstalledDot className="start-1 top-1" />}
      </div>
      <span className="min-w-0 flex-1">
        <span className={cn("block truncate text-[15px] leading-tight", selected ? "text-ink" : "text-ink-2")}>
          {game.title}
        </span>
        <span className="num mt-0.5 block truncate text-[10px] text-ink-3">
          {formatYear(game.releaseDate)} · {stateLabel(game.state)}
          {minutes ? ` · ${t("{duration} played", { duration: minutes })}` : ""}
        </span>
      </span>
      {job && (
        <span className="num shrink-0 text-[10.5px] text-signal">{percent !== null ? formatPercent(percent) : t("Preparing")}</span>
      )}
      {selected && (
        <>
          <span aria-hidden="true" className="pointer-events-none absolute inset-y-0 start-0 w-[3px] bg-signal" />
          <span aria-hidden="true" className="pointer-events-none absolute inset-0 ring-2 ring-inset ring-signal" />
        </>
      )}
    </button>
  );
}

/* --------------------------------- footer --------------------------------- */

function Hint({ keys, children }: { keys: ReactNode; children: ReactNode }) {
  return (
    <span className="flex items-center gap-1.5">
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

/**
 * The hint strip (#138). It reads the same registry as the input router, so a
 * context can never advertise a button it does not honour: the library, the
 * quick switcher and every overlay pass their own context, and a DualSense
 * names Cross where an Xbox names A.
 */
export function HintStrip({
  context,
  layout,
  connected,
  family,
  className,
}: {
  context: HintContext;
  layout: BigPictureLayout;
  connected: boolean;
  family: PadFamily;
  className?: string;
}) {
  const hints = hintsFor(context, { layout, family, connected });
  return (
    <div className={cn("flex flex-wrap items-center gap-x-4 gap-y-1", className)}>
      {hints.map((hint) => (
        <Hint key={hint.id} keys={hint.keys.map((key) => <Kbd key={key}>{key}</Kbd>)}>
          {HINT_COPY[hint.id]()}
        </Hint>
      ))}
    </div>
  );
}

/**
 * The Big Picture footer: contextual hints on the left, the active pad and
 * profile on the right — the profile indicator a player needs to know which
 * mapping is driving the screen (#138).
 */
export function BigPictureFooter({
  layout,
  connected,
  family,
  profileName,
  context,
  haptics,
  className,
}: {
  layout: BigPictureLayout;
  connected: boolean;
  family: PadFamily;
  /** Active controller profile name, shown as the indicator. */
  profileName: string;
  context: HintContext;
  haptics: boolean;
  className?: string;
}) {
  return (
    <footer
      className={cn(
        "relative flex min-h-10 shrink-0 flex-wrap items-center gap-x-5 gap-y-1 border-t border-line bg-bg-deep py-1.5 bp-pad-x text-[10.5px] text-ink-3",
        className,
      )}
    >
      <HintStrip context={context} layout={layout} connected={connected} family={family} />
      <span className="ms-auto hidden items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.12em] xl:flex">
        <Gamepad2 className="size-3.5" aria-hidden="true" />
        {connected ? (
          <>
            <span className="text-ink-2">{padFamilyLabel(family)}</span>
            <span aria-hidden="true">·</span>
            <span className="max-w-[18ch] truncate normal-case tracking-normal text-ink-3">{profileName}</span>
            {haptics && <span className="text-signal-text">{t("vibration")}</span>}
          </>
        ) : (
          t("Keyboard only — connect a controller for couch navigation")
        )}
      </span>
    </footer>
  );
}
