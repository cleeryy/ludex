import { motion } from "motion/react";
import { ArtPlate, GameCover } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { formatYear } from "@/lib/format";
import { t } from "@/lib/i18n";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, HERO_CUE, cardDelay, reveal } from "./entrance";
import type { BigPictureLayoutProps } from "./model";
import {
  ActionRow,
  MetaLine,
  ProgressLine,
  RailCard,
  SelectedEyebrow,
  ShelfTabs,
} from "./parts";

/**
 * Shelf (#141) — the layout Big Picture shipped with, kept as the default.
 * A large plate and its copy on the stage, one captioned shelf underneath.
 */
export function ShelfLayout({
  nav,
  reduceMotion,
  settled,
  detail,
  installed,
  job,
  percent,
  primary,
  onOpenGame,
}: BigPictureLayoutProps) {
  const game = nav.selected;
  if (!game) return null;

  return (
    <>
      <section className="relative flex min-h-0 flex-1 items-center overflow-hidden">
        <motion.div className="absolute inset-0" {...reveal(reduceMotion, BACKDROP_CUE)}>
          <ArtPlate
            title={game.title}
            artUrl={detail?.backgroundUrl ?? game.backgroundUrl}
            className="h-full w-full"
          />
        </motion.div>
        <div className="absolute inset-0 bp-shade" aria-hidden="true" />

        <div className="relative flex w-full items-center gap-[clamp(1rem,2.5vw,2.5rem)] py-5 bp-pad-x">
          <motion.div
            className="w-[clamp(132px,12vw,420px)] shrink-0"
            {...reveal(reduceMotion, HERO_CUE)}
          >
            <div className="shadow-hero">
              <GameCover
                title={game.title}
                coverUrl={game.coverUrl}
                index={game.id}
                year={formatYear(game.releaseDate)}
                genre={game.genres[0]}
              />
            </div>
          </motion.div>

          <motion.div className="flex min-w-0 flex-1 flex-col gap-3" {...reveal(reduceMotion, COPY_CUE)}>
            <SelectedEyebrow game={game} />
            <h1 className="max-w-[20ch] text-[clamp(1.7rem,4vw,4.35rem)] font-semibold leading-[0.92] tracking-[-0.04em] text-stage-ink">
              {game.title}
            </h1>
            <MetaLine game={game} installed={installed.has(game.id)} />
            {detail?.description && (
              <p className="line-clamp-2 max-w-[64ch] text-[14px] leading-relaxed text-stage-ink-2">
                {detail.description}
              </p>
            )}
            {job && <ProgressLine job={job} percent={percent} />}
            <ActionRow className="mt-1">
              {primary}
              <Button variant="default" onClick={() => onOpenGame(game.id)}>
                {t("Game page")}
              </Button>
            </ActionRow>
          </motion.div>
        </div>
      </section>

      <section
        className="relative shrink-0 border-t border-line pb-3 pt-1.5 bp-pad-x"
        aria-label={t("Games")}
      >
        <div className="flex items-center justify-between gap-4">
          <ShelfTabs nav={nav} dense className="min-w-0 flex-1" />
          <span className="num shrink-0 pe-1 text-[10.5px] text-ink-3">
            {nav.index + 1} / {nav.games.length}
          </span>
        </div>
        <div className="bp-rail bp-rail-shelf nav-scroll flex gap-3 overflow-x-auto">
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
                className="w-[var(--bp-rail-card)]"
              />
            </motion.div>
          ))}
        </div>
      </section>
    </>
  );
}
