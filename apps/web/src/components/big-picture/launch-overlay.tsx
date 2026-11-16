import { motion } from "motion/react";
import { GameCover } from "@/components/art/cover";
import { Button } from "@/components/ui/controls";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import {
  LAUNCH_DIM_ALPHA,
  LAUNCH_FILAMENT_MS,
  flipFrom,
  launchPlateCopy,
  plateCanDismiss,
  type LaunchPlateStatus,
  type PlateRect,
} from "./launch-plate";
import { exitOf, transitionOf } from "./motion";

/* --------------------------------------------------------------------------
   Launch handoff, view half (#229).

   Play on an installed game opens this plate instead of a toast: the selected
   cover flies to full-bleed (transform only), the library dims to 0.35, and the
   hold frame carries the artwork, the title, "Launching {title}" and a 2 px
   signal filament until the launch promise settles. `error` closes it with the
   failure reason; 8 s swaps the line for the calm truth. `B` closes it once the
   promise has given up (see `launch-plate.ts`).

   The plate is inert for the pad loop on purpose: the screen routes B to
   `onDismiss` and swallows everything else, so a stray A cannot queue another
   launch behind it.
   -------------------------------------------------------------------------- */

export function LaunchPlate({
  gameId,
  title,
  coverUrl,
  rect,
  status,
  detail,
  reduceMotion,
  instant = false,
  onDismiss,
}: {
  gameId: number;
  title: string;
  coverUrl: string | null;
  rect: PlateRect | null;
  status: LaunchPlateStatus;
  /** The failure reason, when the launch rejected. */
  detail?: string | null;
  reduceMotion: boolean | null;
  /** A held key opened it: land on the final frame. */
  instant?: boolean;
  onDismiss: () => void;
}) {
  const enter = transitionOf("launch", { reduce: reduceMotion, instant });
  const leave = exitOf("launch", { reduce: reduceMotion, instant });
  const copy = launchPlateCopy(status, title);
  const dismissable = plateCanDismiss(status);
  // The FLIP is only measured in a browser; a null viewport falls back to the
  // table's own frame so the plate still arrives.
  const transform =
    typeof window === "undefined"
      ? null
      : flipFrom(rect, { width: window.innerWidth, height: window.innerHeight });
  const initial =
    instant || reduceMotion
      ? false
      : transform
        ? { x: transform.x, y: transform.y, scaleX: transform.scaleX, scaleY: transform.scaleY }
        : { opacity: 0.55, scale: 0.985 };
  // Motion only animates the keys in `animate`: without the target transform
  // the plate would stay pinned at the cover rect, and without the target
  // scale the no-cover fallback would stay shrunk.
  const animate = transform ? { x: 0, y: 0, scaleX: 1, scaleY: 1, opacity: 1 } : { opacity: 1, scale: 1 };
  const failed = status === "failed";

  return (
    <motion.div
      className="fixed inset-0 z-50 overflow-hidden"
      data-launch-plate={status}
      role="group"
      aria-label={copy.eyebrow}
      aria-busy={status === "launching" ? true : undefined}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      {...leave}
      transition={{ duration: reduceMotion ? 0.12 : 0.18 }}
    >
      {/* The library behind reads at 0.35: one static scrim, never an animated
          blur or a filter. */}
      <div
        className="absolute inset-0 bg-scrim"
        style={{ opacity: LAUNCH_DIM_ALPHA }}
        aria-hidden="true"
      />

      <motion.div
        className="absolute inset-0 overflow-hidden bg-bg-deep"
        style={{ transformOrigin: "0 0" }}
        initial={initial}
        animate={animate}
        transition={enter.transition}
      >
        <GameCover
          title={title}
          coverUrl={coverUrl}
          index={gameId}
          showType={false}
          className="absolute inset-0 h-full w-full rounded-none"
        />
        <div className="pointer-events-none absolute inset-0 z-[1] cover-shade" aria-hidden="true" />
        <div
          className="pointer-events-none absolute inset-x-0 bottom-0 z-[2] h-[52%]"
          style={{
            background:
              "linear-gradient(180deg, transparent 0%, color-mix(in oklab, var(--bg-deep) 68%, transparent) 58%, var(--bg-deep) 100%)",
          }}
          aria-hidden="true"
        />
      </motion.div>

      <div className="absolute inset-x-0 bottom-0 z-10 flex flex-col gap-3 p-6 sm:p-10">
        <p className="font-mono text-bp-meta uppercase tracking-[0.16em] text-signal">{copy.eyebrow}</p>
        <h2 className="max-w-[24ch] truncate text-bp-title font-semibold leading-[1.05] tracking-[-0.02em] text-ink sm:text-bp-hero">
          {title}
        </h2>

        {/* The signal filament: 2 px, one 1.1 s linear cycle. Under reduced
            motion it is a still segment, never a spinner. */}
        <div className="relative h-0.5 w-full max-w-[420px] overflow-hidden bg-line-2/60" aria-hidden="true">
          {reduceMotion ? (
            <span className={cn("absolute inset-y-0 left-1/3 w-1/3", failed ? "bg-danger" : "bg-signal")} />
          ) : (
            <motion.span
              className={cn("absolute inset-y-0 w-1/3", failed ? "bg-danger" : "bg-signal")}
              initial={{ x: "-120%" }}
              animate={failed ? { x: "0%" } : { x: ["-120%", "320%"] }}
              transition={
                failed
                  ? { duration: 0.2 }
                  : { duration: LAUNCH_FILAMENT_MS / 1_000, ease: "linear", repeat: Infinity }
              }
            />
          )}
        </div>

        <p className="max-w-[52ch] text-bp-body leading-relaxed text-ink-2" role="status" aria-live="polite">
          {copy.line}
        </p>
        {failed && detail && <p className="max-w-[52ch] text-bp-meta leading-relaxed text-ink-3">{detail}</p>}

        {dismissable && (
          <div>
            <Button variant="ghost" onClick={onDismiss}>
              {t("Back")}
            </Button>
          </div>
        )}
      </div>
    </motion.div>
  );
}
