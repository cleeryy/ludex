import { motion } from "motion/react";
import { ArtPlate } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { t } from "@/lib/i18n";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, cardDelay, reveal } from "./entrance";
import type { BigPictureLayoutProps } from "./model";
import { transitionOf } from "./motion";
import { useChangeWitness } from "./use-change-witness";
import { ActionRow, MetaLine, ProgressLine, RailCard, SelectedEyebrow } from "./parts";

/**
 * Stage (#268) — the Cabinet home.
 *
 * One lit object: the selected game fills the stage, its title is the largest
 * thing on screen, and the primary action sits directly under it. The rail
 * below shows 5.5 covers at comfortable density and captions the selected one
 * only. There is no KPI grid, no index, no second composition competing for the
 * eye: the stage is the first read, the rail is the second.
 *
 * The stage art is the one plate in the composition; the wallpaper behind the
 * shell is the only blurred layer and it never moves with the selection.
 */
export function ShelfLayout({
  nav,
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
      <section
        className="bp-stage flex min-h-0 flex-1 items-end"
        aria-label={t("Selected game")}
      >
        <motion.div className="absolute inset-0" {...reveal(reduceMotion, BACKDROP_CUE)}>
          <ArtPlate
            title={game.title}
            artUrl={detail?.backgroundUrl ?? game.backgroundUrl}
            className="h-full w-full"
          />
        </motion.div>
        <div className="absolute inset-0 bp-stage-wash" aria-hidden="true" />

        <motion.div
          className="relative flex w-full flex-col gap-3 pb-6 pt-8 bp-pad-x"
          {...reveal(reduceMotion, COPY_CUE)}
        >
          <SelectedEyebrow game={game} />
          <h1 className="line-clamp-2 max-w-[20ch] text-bp-hero font-semibold leading-[1.02] tracking-[-0.02em] text-stage-ink">
            {game.title}
          </h1>
          <MetaLine game={game} installed={installed.has(game.id)} />
          {job && <ProgressLine job={job} percent={percent} />}
          <ActionRow className="mt-2">
            <span data-bp-action="true">{primary}</span>
            <span data-bp-action="true">
              <Button variant="default" className="bp-button" onClick={() => onOpenGame(game.id)}>
                {t("Manage")}
              </Button>
            </span>
          </ActionRow>
        </motion.div>
      </section>

      <section className="relative shrink-0 bp-pad-x" aria-label={t("Games")}>
        <motion.div
          key={nav.shelf?.id ?? "shelf"}
          className="bp-rail nav-scroll"
          {...shelfFrame}
          initial={shelfChanged ? shelfFrame.initial : false}
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
              />
            </motion.div>
          ))}
        </motion.div>
      </section>
    </>
  );
}
