import { motion } from "motion/react";
import { ArtPlate, GameCover } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { formatYear } from "@/lib/format";
import { t } from "@/lib/i18n";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, HERO_CUE, cardDelay, reveal } from "./entrance";
import type { BigPictureLayoutProps } from "./model";
import { transitionOf } from "./motion";
import { useChangeWitness } from "./use-change-witness";
import { ActionRow, GridCard, MetaLine, ProgressLine, SelectedEyebrow, ShelfTabs } from "./parts";

/**
 * Grid (#141) — a library wall. A focused header keeps the selected game's
 * actions in reach while the grid below shows many more covers than a rail
 * can. Columns come from `gridColumns(width)`, the same number the keyboard
 * uses for its up/down step.
 */
export function GridLayout({
  nav,
  columns,
  reduceMotion,
  instant,
  settled,
  detail,
  installed,
  job,
  percent,
  primary,
  onOpenGame,
}: BigPictureLayoutProps) {
  const shelfChanged = useChangeWitness(nav.shelf?.id);
  const game = nav.selected;
  if (!game) return null;

  const shelfFrame = transitionOf("shelf", {
    reduce: reduceMotion,
    instant,
    direction: nav.shelfDirection,
  });

  return (
    <>
      <section className="relative shrink-0 overflow-hidden border-b border-line">
        <motion.div className="absolute inset-0" {...reveal(reduceMotion, BACKDROP_CUE)}>
          <ArtPlate
            title={game.title}
            artUrl={detail?.backgroundUrl ?? game.backgroundUrl}
            className="h-full w-full"
          />
        </motion.div>
        <div className="absolute inset-0 backdrop-shade" aria-hidden="true" />

        <div className="relative flex min-h-[clamp(148px,19vh,320px)] items-center gap-5 py-5 lg:gap-7 bp-pad-x">
          <motion.div
            className="w-[clamp(78px,7vw,200px)] shrink-0"
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

          <motion.div className="flex min-w-0 flex-1 flex-col gap-2" {...reveal(reduceMotion, COPY_CUE)}>
            <SelectedEyebrow game={game} />
            <h1 className="line-clamp-2 max-w-[30ch] text-[clamp(1.4rem,2.6vw,2.9rem)] font-semibold leading-[0.95] tracking-[-0.035em] text-stage-ink">
              {game.title}
            </h1>
            <MetaLine game={game} installed={installed.has(game.id)} />
            {detail?.description && (
              <p className="line-clamp-1 max-w-[58ch] text-[12.5px] leading-relaxed text-stage-ink-2">
                {detail.description}
              </p>
            )}
            {job && <ProgressLine job={job} percent={percent} />}
            <ActionRow className="mt-0.5">
              {primary}
              <Button variant="default" onClick={() => onOpenGame(game.id)}>
                {t("Game page")}
              </Button>
            </ActionRow>
          </motion.div>
        </div>
      </section>

      <ShelfTabs nav={nav} />

      <section className="bp-grid min-h-0 flex-1 overflow-y-auto overscroll-contain pb-6 pt-4 scroll-pb-8 bp-pad-x" aria-label={t("Games")}>
        <motion.div
          key={nav.shelf?.id ?? "shelf"}
          className="grid gap-3 xl:gap-4"
          style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
          {...shelfFrame}
          initial={shelfChanged ? shelfFrame.initial : false}
        >
          {nav.games.map((entry, position) => (
            <motion.div
              key={entry.id}
              className="min-w-0"
              {...reveal(reduceMotion, {
                ...CARD_CUE,
                delay: cardDelay(position),
                settled,
              })}
            >
              <GridCard
                game={entry}
                position={position}
                selected={position === nav.index}
                nav={nav}
                installed={installed.has(entry.id)}
              />
            </motion.div>
          ))}
        </motion.div>
      </section>
    </>
  );
}
