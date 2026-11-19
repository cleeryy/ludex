import { AnimatePresence, motion } from "motion/react";
import { t } from "@/lib/i18n";
import { volumeOsdLabel, type VolumeOsd } from "@/lib/volume-osd";
import { Glyph } from "@/components/big-picture/glyphs";

/**
 * The console's own volume OSD (#289).
 *
 * A 10-foot session must not depend on the desktop's overlay: it has a
 * different scale, it is not reachable from a pad, and it can be disabled in
 * the desktop session. This bar is drawn by Ludex, above the band, and fades
 * itself out. It never takes the ring.
 */
export function ConsoleVolumeOsd({ osd, reduceMotion }: { osd: VolumeOsd | null; reduceMotion: boolean }) {
  return (
    <div
      className="pointer-events-none fixed inset-x-0 bottom-[calc(var(--bp-band-h)+var(--bp-safe-y)+1.25rem)] z-50 flex justify-center bp-pad-x"
      aria-live="polite"
      aria-atomic="true"
    >
      <AnimatePresence initial={false}>
        {osd && (
          <motion.div
            key={`${osd.sink}\u0000${osd.percent}\u0000${osd.muted}\u0000${osd.at}`}
            role="status"
            data-console-volume-osd=""
            className="bp-notice flex min-w-[min(22rem,72vw)] max-w-[34rem] items-center gap-4 px-5 py-3"
            initial={{ opacity: 0, y: reduceMotion ? 0 : 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: reduceMotion ? 0 : 4 }}
            transition={{ duration: reduceMotion ? 0 : 0.14 }}
          >
            <span className="shrink-0 text-ink-2" aria-hidden="true">
              <Glyph name={osd.muted ? "volumeOff" : "volume"} size={22} />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-bp-meta leading-tight text-ink-2">{osd.label}</span>
              <span className="mt-1.5 block h-1 w-full overflow-hidden rounded-full bg-line-2" aria-hidden="true">
                <span
                  className="block h-full rounded-full bg-ink transition-[width] duration-150"
                  style={{ width: `${osd.muted ? 0 : osd.percent}%` }}
                />
              </span>
            </span>
            <span className="shrink-0 text-bp-meta font-mono tabular-nums text-ink">
              {osd.muted ? t("Muted") : volumeOsdLabel(osd)}
            </span>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
