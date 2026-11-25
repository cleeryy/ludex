import { type ComponentProps } from "react";
import { motion } from "motion/react";
import { ArtPlate } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { t } from "@/lib/i18n";
import { useJob } from "@/state/downloads";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, cardDelay, reveal } from "./entrance";
import { jobPercent, type BigPictureLayoutProps } from "./model";
import { transitionOf } from "./motion";
import { useChangeWitness } from "./use-change-witness";
import { ActionRow, ListRow, MetaLine, ProgressLine, SelectedEyebrow, ShelfChips } from "./parts";

/**
 * One live row (#267): the row subscribes to its own job, so a progress tick
 * never repaints the shell or the rest of the list.
 */
function LiveListRow({ ...props }: Omit<ComponentProps<typeof ListRow>, "job" | "percent">) {
  const job = useJob(props.game.id);
  return <ListRow {...props} job={job} percent={jobPercent(job)} />;
}

/**
 * List (#268) — the third composition, and the accessible one.
 *
 * Dense rows, one line of metadata, no index column and no second ring: the
 * selected row is a surface step plus a 3u signal bar on its leading edge. It
 * answers "show me the whole shelf as text" without becoming a desktop table:
 * the title stays 28u, the metadata 24u, and the stage header keeps the actions
 * in reach.
 */
export function CompactLayout({
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
      <section className="bp-stage relative shrink-0 overflow-hidden border-b border-line">
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
          className="relative flex flex-wrap items-end gap-x-6 gap-y-2 py-4 bp-pad-x"
          {...reveal(reduceMotion, COPY_CUE)}
        >
          <div className="flex min-w-0 flex-col gap-1">
            <SelectedEyebrow game={game} />
            <h1 className="line-clamp-1 max-w-[36ch] text-bp-title font-semibold leading-[1.05] tracking-[-0.02em] text-stage-ink">
              {game.title}
            </h1>
            <MetaLine game={game} installed={installed.has(game.id)} />
            {job && <ProgressLine job={job} percent={percent} />}
          </div>
          <ActionRow className="ms-auto">
            <span data-bp-action="true">{primary}</span>
            <span data-bp-action="true">
              <Button variant="default" className="bp-button" onClick={() => onOpenGame(game.id)}>
                {t("Manage")}
              </Button>
            </span>
          </ActionRow>
        </motion.div>
      </section>

      <div className="shrink-0 pb-2 pt-3 bp-pad-x">
        <ShelfChips nav={nav} onOpenCollections={onOpenCollections} />
      </div>

      <section
        className="bp-content min-h-0 flex-1 overflow-y-auto overscroll-contain pb-6 bp-pad-x"
        aria-label={t("Games")}
      >
        <motion.div
          key={nav.shelf?.id ?? "shelf"}
          {...shelfFrame}
          initial={shelfChanged ? shelfFrame.initial : false}
        >
          {nav.games.map((entry, position) => (
            <motion.div
              key={entry.id}
              {...reveal(reduceMotion, {
                ...CARD_CUE,
                delay: cardDelay(position),
                settled,
              })}
            >
              <LiveListRow
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
