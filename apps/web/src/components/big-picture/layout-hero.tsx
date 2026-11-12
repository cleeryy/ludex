import { motion } from "motion/react";
import { ArtPlate } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { t } from "@/lib/i18n";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, HERO_CUE, cardDelay, reveal } from "./entrance";
import type { BigPictureLayoutProps } from "./model";
import { ActionRow, MetaLine, ProgressLine, RailCard, SelectedEyebrow, ShelfTabs } from "./parts";

/**
 * Hero (#141) — console storefront: the selected game owns the screen, the
 * title is centred, and one rail of covers sits under it. No large plate: the
 * backdrop *is* the hero, which is what separates this layout from the shelf.
 */
export function HeroLayout({
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
      <section className="relative flex min-h-0 flex-1 flex-col justify-end overflow-hidden">
        <motion.div className="absolute inset-0" {...reveal(reduceMotion, BACKDROP_CUE)}>
          <ArtPlate
            title={game.title}
            artUrl={detail?.backgroundUrl ?? game.backgroundUrl}
            className="h-full w-full"
          />
        </motion.div>
        <div className="absolute inset-0 hero-shade" aria-hidden="true" />

        <motion.div
          className="relative mx-auto flex w-full max-w-[70rem] flex-col items-center gap-3 px-6 pb-[clamp(0.75rem,2.4vh,2.25rem)] text-center"
          {...reveal(reduceMotion, HERO_CUE)}
        >
          <SelectedEyebrow game={game} />
          <h1 className="max-w-[24ch] text-balance text-[clamp(1.9rem,4.8vw,6rem)] font-semibold leading-[0.9] tracking-[-0.045em] text-stage-ink">
            {game.title}
          </h1>
          <MetaLine game={game} installed={installed.has(game.id)} className="justify-center" />
          {detail?.description && (
            <p className="line-clamp-2 max-w-[62ch] text-[13.5px] leading-relaxed text-stage-ink-2">
              {detail.description}
            </p>
          )}
          {job && <ProgressLine job={job} percent={percent} className="justify-center" />}
          <ActionRow className="mt-1 justify-center">
            {primary}
            <Button variant="default" onClick={() => onOpenGame(game.id)}>
              {t("Game page")}
            </Button>
          </ActionRow>
        </motion.div>
      </section>

      <section
        className="relative shrink-0 border-t border-line pb-2.5 pt-1.5"
        aria-label={t("Games")}
      >
        <motion.div
          className="flex items-center justify-between gap-4 bp-pad-x"
          {...reveal(reduceMotion, { ...COPY_CUE, delay: 0.02, y: 6 })}
        >
          <ShelfTabs nav={nav} dense className="min-w-0 flex-1" />
          <span className="num shrink-0 pe-1 text-[10.5px] text-ink-3">
            {nav.index + 1} / {nav.games.length}
          </span>
        </motion.div>

        <div className="nav-scroll overflow-x-auto pb-2 pt-2.5 bp-pad-x">
          <div className="mx-auto flex w-max gap-3">
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
        </div>
      </section>
    </>
  );
}
