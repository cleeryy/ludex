import { useEffect, useRef, useState } from "react";
import { motion } from "motion/react";
import { ArtPlate } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { formatMinutes, formatRating, formatRelative, formatYear, stateLabel } from "@/lib/format";
import { t } from "@/lib/i18n";
import type { GameDetail, GameSummary } from "@/lib/types";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, cardDelay, reveal } from "./entrance";
import type { BigPictureLayoutProps } from "./model";
import { ProgressLine, RailCard } from "./parts";

/* --------------------------------------------------------------------------
   Spotlight (#197) — the selected game is the stage. Full-bleed artwork
   crossfades under the title, one activity line says what the player did here,
   quick actions are a real focus zone, and the row is a strip of covers at the
   bottom. The system icons float over the art at the top, drawn by the shell's
   own HomeRow with `placement="overlay"`.
   -------------------------------------------------------------------------- */

interface BackdropLayer {
  id: number;
  title: string;
  artUrl: string | null;
}

/** Two layers at most: the outgoing art fades out under the incoming one. */
function SpotlightBackdrop({
  game,
  detail,
  reduceMotion,
}: {
  game: GameSummary;
  detail: GameDetail | undefined;
  reduceMotion: boolean | null;
}) {
  const current: BackdropLayer = {
    id: game.id,
    title: game.title,
    artUrl: detail?.backgroundUrl ?? game.backgroundUrl,
  };
  const [outgoing, setOutgoing] = useState<BackdropLayer | null>(null);
  const last = useRef(current);

  useEffect(() => {
    const previous = last.current;
    last.current = current;
    if (previous.id === current.id || reduceMotion) {
      setOutgoing(null);
      return;
    }
    setOutgoing(previous);
    const timer = window.setTimeout(() => setOutgoing(null), 240);
    return () => window.clearTimeout(timer);
    // The identity that matters is the game and its art; a re-render with the
    // same values must not restart the fade.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current.id, current.artUrl, reduceMotion]);

  return (
    <motion.div className="absolute inset-0" {...reveal(reduceMotion, BACKDROP_CUE)}>
      {outgoing && (
        <ArtPlate
          key={`out-${outgoing.id}`}
          title={outgoing.title}
          artUrl={outgoing.artUrl}
          className="bp-crossfade-out absolute inset-0 h-full w-full"
        />
      )}
      <ArtPlate
        key={`in-${current.id}`}
        title={current.title}
        artUrl={current.artUrl}
        className="bp-crossfade-in absolute inset-0 h-full w-full"
      />
    </motion.div>
  );
}

/**
 * One honest line of recent activity. Built from real fields only; when the
 * game has never been played the line is omitted rather than filled with a
 * placeholder.
 */
function ActivityLine({ game }: { game: GameSummary }) {
  const minutes = formatMinutes(game.minutesPlayed);
  const parts = [
    game.lastPlayedAt ? `${t("Last played")} ${formatRelative(game.lastPlayedAt)}` : null,
    minutes ? t("{duration} played", { duration: minutes }) : null,
  ].filter((part): part is string => Boolean(part));
  if (parts.length === 0) return null;
  return (
    <p className="flex flex-wrap items-center gap-x-2.5 gap-y-1 font-mono text-[10px] uppercase tracking-[0.14em] text-stage-ink-2">
      {parts.map((part, index) => (
        <span key={part} className="flex items-center gap-2.5">
          {index > 0 && (
            <span aria-hidden="true" className="text-stage-ink-2/60">
              ·
            </span>
          )}
          {part}
        </span>
      ))}
    </p>
  );
}

/** The compact meta line: year, state, rating, installed. */
function SpotlightMeta({ game, installed }: { game: GameSummary; installed: boolean }) {
  const rating = formatRating(game.rating);
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-[11px] text-stage-ink-2">
      <span>{formatYear(game.releaseDate)}</span>
      <span>{stateLabel(game.state)}</span>
      {rating && <span>{rating} / 100</span>}
      {installed && <span className="text-stage-ok">{t("Installed")}</span>}
    </div>
  );
}

export function SpotlightLayout({
  nav,
  reduceMotion,
  settled,
  detail,
  installed,
  job,
  percent,
  primary,
  onOpenGame,
  onOpenMenu,
}: BigPictureLayoutProps) {
  const game = nav.selected;
  if (!game) return null;

  return (
    <section className="relative flex min-h-0 flex-1 flex-col overflow-hidden" aria-label={t("Games")}>
      <SpotlightBackdrop game={game} detail={detail} reduceMotion={reduceMotion} />
      <div className="absolute inset-0 hero-shade" aria-hidden="true" />
      <div className="bp-top-shade absolute inset-x-0 top-0 h-[calc(var(--bp-u)*150)]" aria-hidden="true" />

      <div className="relative flex min-h-0 flex-1 flex-col justify-end gap-[clamp(0.5rem,1.4vh,1.25rem)] pb-2">
        <motion.div className="flex flex-col gap-2 bp-pad-x" {...reveal(reduceMotion, COPY_CUE)}>
          <ActivityLine game={game} />
          <h1 className="line-clamp-2 max-w-[26ch] text-[clamp(1.8rem,3.8vw,4.6rem)] font-semibold leading-[0.9] tracking-[-0.045em] text-stage-ink">
            {game.title}
          </h1>
          <SpotlightMeta game={game} installed={installed.has(game.id)} />
          {job && <ProgressLine job={job} percent={percent} />}

          <div
            className="mt-1 flex flex-wrap items-center gap-2"
            data-bp-actions="true"
            role="group"
            aria-label={t("Actions")}
          >
            <span data-bp-action="true" className="inline-flex">
              {primary}
            </span>
            <span data-bp-action="true" className="inline-flex">
              <Button variant="default" data-gamepad-target="true" onClick={() => onOpenGame(game.id)}>
                {t("Game page")}
              </Button>
            </span>
            <span data-bp-action="true" className="inline-flex">
              <Button variant="default" data-gamepad-target="true" onClick={onOpenMenu}>
                {t("Options")}
              </Button>
            </span>
          </div>
        </motion.div>

        <div className="bp-pad-x">
          <div
            className="nav-scroll flex items-end gap-[clamp(0.75rem,1.5vw,1.75rem)] overflow-x-auto px-2 pb-2 pt-[calc(var(--bp-u)*36)] scroll-px-2"
          >
            {nav.games.map((entry, position) => (
              <motion.div
                key={entry.id}
                className="shrink-0"
                {...reveal(reduceMotion, {
                  ...CARD_CUE,
                  delay: cardDelay(position),
                  settled,
                })}
              >
                <RailCard
                  game={entry}
                  position={position}
                  selected={position === nav.index}
                  nav={nav}
                  installed={installed.has(entry.id)}
                  variant="spotlight"
                  className="w-[calc(var(--bp-u)*285)]"
                />
              </motion.div>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
