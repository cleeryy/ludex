import { motion } from "motion/react";
import { ArtPlate } from "@/components/art/cover";
import { t } from "@/lib/i18n";
import { BACKDROP_CUE, CARD_CUE, COPY_CUE, cardDelay, reveal } from "./entrance";
import type { BigPictureLayoutProps } from "./model";
import { ActionRow, MetaLine, ProgressLine, RailCard, SelectedEyebrow } from "./parts";

/**
 * Dock (#197) — the system row sits under one large rail, so the games and the
 * shell they belong to read as one block. The composition is bottom-anchored:
 * the title block grows upward from the tiles, the tiles grow upward from the
 * dock. No large plate, no description: this layout is for choosing.
 */
export function DockLayout({
  nav,
  reduceMotion,
  settled,
  detail,
  installed,
  job,
  percent,
  primary,
}: BigPictureLayoutProps) {
  const game = nav.selected;
  if (!game) return null;

  return (
    <section className="relative flex min-h-0 flex-1 flex-col overflow-hidden" aria-label={t("Games")}>
      <motion.div className="absolute inset-0" {...reveal(reduceMotion, BACKDROP_CUE)}>
        <ArtPlate
          title={game.title}
          artUrl={detail?.backgroundUrl ?? game.backgroundUrl}
          className="h-full w-full"
        />
      </motion.div>
      <div className="absolute inset-0 bp-shade" aria-hidden="true" />

      <div className="relative flex min-h-0 flex-1 flex-col justify-end gap-[clamp(0.5rem,1.8vh,1.5rem)] pb-2">
        <motion.div className="flex flex-col gap-2 bp-pad-x" {...reveal(reduceMotion, COPY_CUE)}>
          <div className="flex items-end justify-between gap-6">
            <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
              <SelectedEyebrow game={game} />
              {nav.shelf && (
                <span className="min-w-0 truncate font-mono text-[10px] uppercase tracking-[0.16em] text-stage-ink-2">
                  {nav.shelf.label} · {nav.games.length}
                </span>
              )}
            </div>
            <span className="num shrink-0 text-[11px] text-stage-ink-2">
              {String(nav.index + 1).padStart(2, "0")} / {String(nav.games.length).padStart(2, "0")}
            </span>
          </div>

          <h1 className="line-clamp-2 max-w-[24ch] text-[clamp(1.6rem,3.4vw,4.2rem)] font-semibold leading-[0.92] tracking-[-0.04em] text-stage-ink">
            {game.title}
          </h1>
          <MetaLine game={game} installed={installed.has(game.id)} />
          {job && <ProgressLine job={job} percent={percent} />}
          <ActionRow className="mt-0.5">{primary}</ActionRow>
        </motion.div>

        <div className="bp-pad-x">
          <div
            className="nav-scroll flex items-end gap-[clamp(0.75rem,1.5vw,1.75rem)] overflow-x-auto px-2 pb-2 pt-[calc(var(--bp-u)*52)] scroll-px-2"
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
                  variant="dock"
                  className="w-[calc(var(--bp-u)*252)]"
                />
              </motion.div>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
