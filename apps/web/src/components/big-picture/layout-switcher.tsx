import { motion } from "motion/react";
import { Check, ChevronDown, GalleryHorizontalEnd, LayoutGrid, List, PanelBottom, PanelTop, Sparkles } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { Kbd } from "@/components/ui/controls";
import { reveal } from "./entrance";
import { MOTION_DURATION, REDUCED_FADE, exitOf } from "./motion";
import { layoutDescription, layoutLabel } from "./labels";
import { BIG_PICTURE_LAYOUTS, type BigPictureLayout } from "./layout";
import { LayoutPreview } from "./layout-preview";

/* --------------------------------------------------------------------------
   The layout control (#141): one button in the Big Picture header, one quick
   switcher over the screen. Both read the same registry and the same store as
   Settings → Appearance, so a change applies everywhere without a reload.
   -------------------------------------------------------------------------- */

const LAYOUT_ICONS: Record<BigPictureLayout, typeof GalleryHorizontalEnd> = {
  shelf: GalleryHorizontalEnd,
  dock: PanelBottom,
  spotlight: Sparkles,
  hero: PanelTop,
  grid: LayoutGrid,
  compact: List,
};

export function LayoutTrigger({
  layout,
  open,
  onToggle,
  className,
}: {
  layout: BigPictureLayout;
  open: boolean;
  onToggle: () => void;
  className?: string;
}) {
  const Icon = LAYOUT_ICONS[layout];
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-haspopup="true"
      aria-expanded={open}
      aria-label={`${t("Big Picture layout")} — ${layoutLabel(layout)}`}
      className={cn(
        "inline-flex h-8 items-center gap-2 border border-line px-2.5 text-[11.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink",
        open && "border-line-2 text-ink",
        className,
      )}
    >
      <Icon className="size-3.5 text-signal-text" aria-hidden="true" />
      <span className="hidden sm:inline">{layoutLabel(layout)}</span>
      <ChevronDown
        className={cn("size-3 transition-transform duration-150", open && "rotate-180")}
        aria-hidden="true"
      />
    </button>
  );
}

export function LayoutPicker({
  layout,
  highlight,
  reduceMotion,
  onHighlight,
  onSelect,
  onClose,
}: {
  layout: BigPictureLayout;
  /** Card the arrows and the d-pad are on; A applies it. */
  highlight: number;
  reduceMotion: boolean | null;
  onHighlight: (position: number) => void;
  onSelect: (layout: BigPictureLayout) => void;
  onClose: () => void;
}) {
  return (
    <motion.div
      className="fixed inset-0 z-50 grid place-items-center bg-scrim/85 p-6"
      onClick={onClose}
      role="presentation"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0, pointerEvents: "none", transition: { duration: reduceMotion ? REDUCED_FADE : MOTION_DURATION.fast } }}
      transition={{ duration: reduceMotion ? REDUCED_FADE : MOTION_DURATION.fast }}
    >
      <motion.div
        className="w-[min(1000px,94vw)] border border-line-2 bg-bg-deep p-5 shadow-palette"
        onClick={(event) => event.stopPropagation()}
        role="group"
        aria-label={t("Big Picture layout")}
        {...reveal(reduceMotion, { y: 10, duration: MOTION_DURATION.base })}
        {...exitOf("overlayOpen", { reduce: reduceMotion })}
      >
        <div className="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-2">
          <p className="text-[13px] font-medium text-ink">{t("Big Picture layout")}</p>
          <span className="flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
            <span className="flex items-center gap-1.5">
              <Kbd>←</Kbd>
              <Kbd>→</Kbd>
              {t("choose")}
            </span>
            <span className="flex items-center gap-1.5">
              <Kbd>↵</Kbd>
              {t("apply")}
            </span>
            <span className="flex items-center gap-1.5">
              <Kbd>esc</Kbd>
              {t("close")}
            </span>
          </span>
        </div>

        <div className="mt-3 grid grid-cols-2 gap-2.5 sm:grid-cols-3 xl:grid-cols-6">
          {BIG_PICTURE_LAYOUTS.map((entry, position) => {
            const active = entry === layout;
            const focused = position === highlight;
            return (
              <button
                key={entry}
                type="button"
                onClick={() => onSelect(entry)}
                onPointerEnter={() => onHighlight(position)}
                aria-pressed={active}
                className={cn(
                  "flex flex-col gap-1.5 border p-1.5 text-start transition-colors duration-150",
                  focused ? "border-signal bg-surface-2" : "border-line hover:border-line-2 hover:bg-surface",
                )}
              >
                <LayoutPreview layout={entry} />
                <span className="flex items-baseline justify-between gap-2 px-0.5">
                  <span className="text-[12.5px] font-medium text-ink">{layoutLabel(entry)}</span>
                  {active && <Check className="size-3.5 shrink-0 text-signal-text" strokeWidth={3} aria-hidden="true" />}
                </span>
                <span className="line-clamp-3 px-0.5 text-[11px] leading-snug text-ink-3">
                  {layoutDescription(entry)}
                </span>
              </button>
            );
          })}
        </div>
      </motion.div>
    </motion.div>
  );
}
