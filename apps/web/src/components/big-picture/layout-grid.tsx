import { motion } from "motion/react";
import { ArtPlate, GameCover } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { formatYear } from "@/lib/format";
import { t } from "@/lib/i18n";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, HERO_CUE, cardDelay, reveal } from "./entrance";
import type { BigPictureLayoutProps } from "./model";
import { transitionOf } from "./motion";
import { useChangeWitness } from "./use-change-witness";
import { ActionRow, GridCard, MetaLine, ProgressLine, SelectedEyebrow, ShelfChips } from "./parts";

/**
 * Wall (#268) — the library as plates.
 *
 * A shorter stage keeps the selected game's title and its one primary action in
 * reach while the grid below shows many more covers than a rail can. Chips sit
 * between the two, because they filter the wall. Columns come from
 * `gridColumns(width)`, the same number the keyboard uses for its row step.
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
  onOpenCollections,
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
      <section className="bp-stage relative shrink-0 overflow-hidden border-b border-line">
        <motion.div className="absolute inset-0" {...reveal(reduceMotion, BACKDROP_CUE)}>
          <ArtPlate
            title={game.title}
            artUrl={detail?.backgroundUrl ?? game.backgroundUrl}
            className="h-full w-full"
          />
        </motion.div>
        <div className="absolute inset-0 bp-stage-wash" aria-hidden="true" />

        <div className="relative flex items-end gap-6 py-5 bp-pad-x">
          <motion.div
            className="w-[calc(var(--bp-u)*92)] shrink-0"
            {...reveal(reduceMotion, HERO_CUE)}
          >
            <GameCover
              title={game.title}
              coverUrl={game.coverUrl}
              index={game.id}
              year={formatYear(game.releaseDate)}
              genre={game.genres[0]}
            />
          </motion.div>

          <motion.div className="flex min-w-0 flex-1 flex-col gap-2" {...reveal(reduceMotion, COPY_CUE)}>
            <SelectedEyebrow game={game} />
            <h1 className="line-clamp-1 max-w-[30ch] text-bp-title font-semibold leading-[1.05] tracking-[-0.02em] text-stage-ink">
              {game.title}
            </h1>
            <MetaLine game={game} installed={installed.has(game.id)} />
            {job && <ProgressLine job={job} percent={percent} />}
            <ActionRow className="mt-1">
              <span data-bp-action="true">{primary}</span>
              <span data-bp-action="true">
                <Button variant="default" className="bp-button" onClick={() => onOpenGame(game.id)}>
                  {t("Manage")}
                </Button>
              </span>
            </ActionRow>
          </motion.div>
        </div>
      </section>

      <div className="shrink-0 px-0 pb-2 pt-3 bp-pad-x">
        <ShelfChips nav={nav} onOpenCollections={onOpenCollections} />
      </div>

      <section
        className="bp-content bp-grid min-h-0 flex-1 overflow-y-auto overscroll-contain pb-6 pt-1 bp-pad-x"
        aria-label={t("Games")}
      >
        <motion.div
          key={nav.shelf?.id ?? "shelf"}
          className="grid gap-4"
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
              />
            </motion.div>
          ))}
        </motion.div>
      </section>
    </>
  );
}
