import { motion } from "motion/react";
import { ArtPlate } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { t } from "@/lib/i18n";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, cardDelay, reveal } from "./entrance";
import type { BigPictureLayoutProps } from "./model";
import { transitionOf } from "./motion";
import { useChangeWitness } from "./use-change-witness";
import { ActionRow, MetaLine, ProgressLine, RailCard, SelectedEyebrow, ShelfChips } from "./parts";

/**
 * Stage (#268) — the Cabinet home.
 *
 * Full-bleed selected game stage with directional vignette, rich metadata
 * (genres, synopsis, playtime, rating, installed badge), primary/manage
 * actions, interactive shelf tabs, and the 2:3 cover rail below.
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
      <section
        className="bp-stage flex min-h-0 flex-1 items-end"
        aria-label={t("Selected game")}
      >
        <motion.div className="absolute inset-0" {...reveal(reduceMotion, BACKDROP_CUE)}>
          <ArtPlate
            title={game.title}
            artUrl={detail?.backgroundUrl ?? game.backgroundUrl}
            fallbackUrls={[detail?.screenshots?.[0], game.coverUrl]}
            className="h-full w-full"
          />
        </motion.div>
        <div className="absolute inset-0 bp-stage-wash" aria-hidden="true" />

        <motion.div
          className="relative flex w-full flex-col gap-2 pb-4 pt-3 bp-pad-x"
          {...reveal(reduceMotion, COPY_CUE)}
        >
          <SelectedEyebrow game={game} />
          <h1 className="line-clamp-1 max-w-[24ch] text-bp-hero font-semibold leading-[1.04] tracking-[-0.02em] text-stage-ink">
            {game.title}
          </h1>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5">
            <MetaLine game={game} installed={installed.has(game.id)} />
            {game.genres.length > 0 && (
              <div className="flex flex-wrap items-center gap-1.5">
                {game.genres.slice(0, 2).map((genre) => (
                  <span
                    key={genre}
                    className="rounded-[var(--bp-radius-cover)] border border-sheen/15 bg-stage/65 px-2.5 py-0.5 text-bp-meta text-stage-ink-2"
                  >
                    {genre}
                  </span>
                ))}
              </div>
            )}
          </div>
          {detail?.description && (
            <p className="line-clamp-1 max-w-[60ch] text-bp-meta leading-snug text-stage-ink-2">
              {detail.description}
            </p>
          )}
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
      </section>

      <section className="relative shrink-0 bg-bg-deep pt-1.5 bp-pad-x" aria-label={t("Games")}>
        <div className="pb-1">
          <ShelfChips nav={nav} onOpenCollections={onOpenCollections} />
        </div>
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
                installed={installed.has(entry.id)}
                nav={nav}
              />
            </motion.div>
          ))}
        </motion.div>
      </section>
    </>
  );
}
