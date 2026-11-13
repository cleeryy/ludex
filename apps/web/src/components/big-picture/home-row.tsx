import type { ComponentType } from "react";
import { motion, useReducedMotion } from "motion/react";
import { Download, Layers, Library, Play, Power, Settings } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { reveal } from "./entrance";
import { BOOT_DELAY, BOOT_DURATION } from "./motion";
import { HOME_ITEM_IDS, type HomeItemId } from "./system-zones";

/* --------------------------------------------------------------------------
   The home row (#196, #197).

   One row of system tiles, one focus zone, three placements. Above the games
   it is the console header (shelf, hero, grid, compact); under the games it is
   the Dock; over the artwork it is the Spotlight's function row. Left/Right
   wraps in every placement, the direction back to the games follows the
   layout, and a tile with no backing feature stays visible with the reason in
   its detail instead of disappearing under the ring.
   -------------------------------------------------------------------------- */

type Icon = ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" | "false" }>;

const ITEM_META: Record<HomeItemId, { icon: Icon; label: () => string; short?: () => string }> = {
  continue: { icon: Play, label: () => t("Continue playing"), short: () => t("Continue") },
  library: { icon: Library, label: () => t("Library") },
  downloads: { icon: Download, label: () => t("Downloads") },
  collections: { icon: Layers, label: () => t("Collections") },
  settings: { icon: Settings, label: () => t("Settings") },
  power: { icon: Power, label: () => t("Power") },
};

export interface HomeItemDetail {
  detail: string;
  disabled?: boolean;
}

export type HomeRowVariant = "row" | "dock" | "overlay";

function HomeButton({
  id,
  index,
  selected,
  item,
  variant,
  onHighlight,
  onChoose,
}: {
  id: HomeItemId;
  index: number;
  selected: boolean;
  item: HomeItemDetail;
  variant: HomeRowVariant;
  onHighlight: (index: number) => void;
  onChoose: (index: number) => void;
}) {
  const { icon: Icon, label } = ITEM_META[id];
  const shortLabel = ITEM_META[id].short ?? label;
  const common = {
    type: "button" as const,
    "data-home-active": selected ? "true" : undefined,
    "data-gamepad-target": "true",
    "data-bp-zone-item": "true",
    "data-bp-zone-active": selected ? "true" : undefined,
    "aria-current": selected ? ("true" as const) : undefined,
    "aria-disabled": item.disabled ? ("true" as const) : undefined,
    "aria-label": `${label()} — ${item.detail}`,
    onPointerEnter: () => onHighlight(index),
    onClick: () => onChoose(index),
  };

  if (variant === "dock") {
    return (
      <button
        {...common}
        className={cn(
          "inline-flex h-[max(40px,calc(var(--bp-u)*48))] w-full items-center gap-2 border px-3 text-start",
          selected
            ? "border-signal bg-surface-2 text-ink"
            : "border-transparent text-ink-2 hover:border-line hover:bg-surface/60",
          item.disabled && "opacity-60",
        )}
      >
        <Icon className={cn("size-[18px] shrink-0", selected ? "text-signal-text" : "text-ink-3")} aria-hidden="true" />
        <span className="min-w-0 truncate text-[clamp(0.78rem,0.95vw,1rem)] font-medium leading-none">{label()}</span>
      </button>
    );
  }

  if (variant === "overlay") {
    return (
      <button
        {...common}
        className={cn(
          "group flex min-w-[64px] flex-col items-center gap-1 px-2 py-1.5",
          selected ? "text-signal-text" : "text-stage-ink-2 hover:text-stage-ink",
          item.disabled && "opacity-60",
        )}
      >
        <Icon className="size-[22px] shrink-0" aria-hidden="true" />
        <span className="max-w-[14ch] truncate font-mono text-[10px] uppercase tracking-[0.08em]">{shortLabel()}</span>
        <span
          aria-hidden="true"
          data-bp-zone-underline="true"
          className="h-[2px] w-6 origin-center bg-signal"
        />
      </button>
    );
  }

  return (
    <button
      {...common}
      className={cn(
        "group relative flex h-[var(--bp-home-h)] w-full min-w-0 flex-col justify-between border p-2.5 text-start sm:p-3",
        selected ? "border-signal bg-surface-2" : "border-line bg-bg-deep/60 hover:border-line-2 hover:bg-surface/60",
        item.disabled && "opacity-60",
      )}
    >
      <span className="flex w-full items-start justify-between gap-2">
        <span className="num shrink-0 font-mono text-[9.5px] leading-none text-ink-3">
          {String(index + 1).padStart(2, "0")}
        </span>
        <Icon className={cn("size-5 shrink-0 sm:size-6", selected ? "text-signal-text" : "text-ink-3")} aria-hidden="true" />
      </span>
      <span className="min-w-0">
        <span className="block truncate text-[clamp(0.78rem,1.05vw,1.05rem)] font-semibold leading-tight text-ink">
          {label()}
        </span>
        <span className="mt-0.5 block truncate font-mono text-[9.5px] uppercase tracking-[0.08em] text-ink-3">
          {item.detail}
        </span>
      </span>
      <span
        aria-hidden="true"
        data-bp-zone-underline="true"
        className="pointer-events-none absolute inset-x-0 bottom-0 h-[2px] origin-center bg-signal"
      />
    </button>
  );
}

export function HomeRow({
  active,
  focused,
  details,
  onHighlight,
  onChoose,
  placement = "row",
  settled = false,
}: {
  active: number;
  focused: boolean;
  details: Record<HomeItemId, HomeItemDetail>;
  onHighlight: (index: number) => void;
  onChoose: (index: number) => void;
  /** Where the shell draws the row; the layout decides, not the row. */
  placement?: HomeRowVariant;
  /** The #142 cascade already played: re-enter without replaying it. */
  settled?: boolean;
}) {
  const reduceMotion = useReducedMotion();
  const boot = reveal(reduceMotion, { y: 6, delay: BOOT_DELAY.home, duration: BOOT_DURATION, settled });
  if (placement === "dock") {
    return (
      <motion.nav
        aria-label={t("System")}
        className="relative z-10 shrink-0 border-t border-line bg-bg-deep bp-pad-x"
        {...boot}
      >
        <ul className="nav-scroll flex items-stretch gap-1 overflow-x-auto py-1.5">
          {HOME_ITEM_IDS.map((id, index) => (
            <li key={id} className="min-w-0 shrink-0">
              <HomeButton
                id={id}
                index={index}
                selected={focused && index === active}
                item={details[id]}
                variant="dock"
                onHighlight={onHighlight}
                onChoose={onChoose}
              />
            </li>
          ))}
        </ul>
      </motion.nav>
    );
  }

  if (placement === "overlay") {
    return (
      <motion.nav aria-label={t("System")} className="relative z-10" {...boot}>
        <ul className="flex items-center gap-0.5">
          {HOME_ITEM_IDS.map((id, index) => (
            <li key={id} className="min-w-0">
              <HomeButton
                id={id}
                index={index}
                selected={focused && index === active}
                item={details[id]}
                variant="overlay"
                onHighlight={onHighlight}
                onChoose={onChoose}
              />
            </li>
          ))}
        </ul>
      </motion.nav>
    );
  }

  return (
    <motion.nav
      aria-label={t("System")}
      className="relative z-10 shrink-0 border-b border-line bg-bg-deep pb-2 pt-2 bp-pad-x"
      {...boot}
    >
      <ul className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6">
        {HOME_ITEM_IDS.map((id, index) => (
          <li key={id} className="min-w-0">
            <HomeButton
              id={id}
              index={index}
              selected={focused && index === active}
              item={details[id]}
              variant="row"
              onHighlight={onHighlight}
              onChoose={onChoose}
            />
          </li>
        ))}
      </ul>
    </motion.nav>
  );
}
