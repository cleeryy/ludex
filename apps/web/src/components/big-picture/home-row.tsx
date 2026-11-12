import type { ComponentType } from "react";
import { Download, Layers, Library, Play, Power, Settings } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { HOME_ITEM_IDS, type HomeItemId } from "./system-zones";

/* --------------------------------------------------------------------------
   The home row (#196).

   One row of large system tiles above the shelf. It is a focus zone, not a
   toolbar: Left/Right wraps, Down returns to the same game, and a tile with no
   backing feature stays visible with the reason in its detail instead of
   disappearing under the ring.
   -------------------------------------------------------------------------- */

type Icon = ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" | "false" }>;

const ITEM_META: Record<HomeItemId, { icon: Icon; label: () => string }> = {
  continue: { icon: Play, label: () => t("Continue playing") },
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

export function HomeRow({
  active,
  focused,
  details,
  onHighlight,
  onChoose,
}: {
  active: number;
  focused: boolean;
  details: Record<HomeItemId, HomeItemDetail>;
  onHighlight: (index: number) => void;
  onChoose: (index: number) => void;
}) {
  return (
    <nav aria-label={t("System")} className="relative z-10 shrink-0 border-b border-line bg-bg-deep pb-2 pt-2 bp-pad-x">
      <ul className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6">
        {HOME_ITEM_IDS.map((id, index) => {
          const { icon: Icon, label } = ITEM_META[id];
          const item = details[id];
          const selected = focused && index === active;
          return (
            <li key={id} className="min-w-0">
              <button
                type="button"
                data-home-active={selected ? "true" : undefined}
                data-gamepad-target="true"
                aria-current={selected ? "true" : undefined}
                aria-disabled={item.disabled ? "true" : undefined}
                aria-label={`${label()} — ${item.detail}`}
                onPointerEnter={() => onHighlight(index)}
                onClick={() => onChoose(index)}
                className={cn(
                  "group relative flex h-[var(--bp-home-h)] w-full min-w-0 flex-col justify-between border p-2.5 text-start transition-[background-color,border-color,opacity] duration-150 sm:p-3",
                  selected
                    ? "border-signal bg-surface-2"
                    : "border-line bg-bg-deep/60 hover:border-line-2 hover:bg-surface/60",
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
                {selected && (
                  <span aria-hidden="true" className="pointer-events-none absolute inset-x-0 bottom-0 h-[2px] bg-signal" />
                )}
              </button>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
