import { motion } from "motion/react";
import { GameCover } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { formatYear } from "@/lib/format";
import { t } from "@/lib/i18n";
import { COPY_CUE, HERO_CUE, cardDelay, reveal } from "./entrance";
import { jobPercent, type BigPictureLayoutProps } from "./model";
import {
  ActionRow,
  CompactRow,
  MetaLine,
  ProgressLine,
  SelectedEyebrow,
  ShelfTabs,
} from "./parts";

/**
 * Compact (#141) — a dense list for big libraries and fast stepping, with the
 * selected game's plate and actions pinned beside it. One row is one game:
 * index, plate, title, state, and live progress on the right.
 */
export function CompactLayout({
  nav,
  reduceMotion,
  settled,
  detail,
  installed,
  job,
  percent,
  primary,
  onOpenGame,
  jobFor,
}: BigPictureLayoutProps) {
  const game = nav.selected;
  if (!game) return null;

  return (
    <>
      <ShelfTabs nav={nav} />

      <div className="flex min-h-0 flex-1 flex-col md:flex-row">
        <section
          className="nav-scroll min-h-0 flex-1 overflow-y-auto md:border-e md:border-line"
          aria-label={t("Games")}
        >
          {nav.games.map((entry, position) => {
            const entryJob = jobFor(entry.id);
            return (
              <motion.div
                key={entry.id}
                {...reveal(reduceMotion, {
                  ...COPY_CUE,
                  delay: cardDelay(position),
                  settled,
                })}
              >
                <CompactRow
                  game={entry}
                  position={position}
                  selected={position === nav.index}
                  nav={nav}
                  installed={installed.has(entry.id)}
                  job={entryJob}
                  percent={jobPercent(entryJob)}
                />
              </motion.div>
            );
          })}
        </section>

        <motion.aside
          className="flex max-h-[44%] shrink-0 flex-col gap-3 overflow-y-auto overscroll-contain border-t border-line bg-stage p-4 md:max-h-none md:w-[clamp(260px,30vw,420px)] md:border-t-0 lg:gap-4 lg:p-6"
          aria-label={t("Selected game")}
          {...reveal(reduceMotion, HERO_CUE)}
        >
          <div className="flex gap-4">
            <div className="w-[clamp(84px,8vw,150px)] shrink-0 shadow-hero">
              <GameCover
                title={game.title}
                coverUrl={game.coverUrl}
                index={game.id}
                year={formatYear(game.releaseDate)}
                genre={game.genres[0]}
              />
            </div>
            <div className="flex min-w-0 flex-col gap-1.5">
              <SelectedEyebrow game={game} />
              <h1 className="max-w-[18ch] text-[clamp(1.15rem,2vw,2rem)] font-semibold leading-[0.98] tracking-[-0.03em] text-stage-ink">
                {game.title}
              </h1>
              <MetaLine game={game} installed={installed.has(game.id)} className="flex-col items-start gap-y-0.5" />
            </div>
          </div>

          {detail?.description && (
            <p className="line-clamp-4 text-[12.5px] leading-relaxed text-stage-ink-2">{detail.description}</p>
          )}

          {job && <ProgressLine job={job} percent={percent} />}

          <ActionRow>
            {primary}
            <Button variant="default" onClick={() => onOpenGame(game.id)}>
              {t("Game page")}
            </Button>
          </ActionRow>
        </motion.aside>
      </div>
    </>
  );
}
