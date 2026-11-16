import { motion, useReducedMotion } from "motion/react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { reveal } from "./entrance";
import { Glyph, type GlyphName } from "./glyphs";
import { BOOT_DELAY, BOOT_DURATION } from "./motion";
import { HOME_ITEM_IDS, type HomeItemId } from "./system-zones";

/* --------------------------------------------------------------------------
   The console band (#268).

   Five destinations, one row, text and glyphs — not six equal tiles with corner
   icons and `01–06` indexes. State lives in the bar; destinations live here; no
   item appears twice. The band is the bottom zone of the home axis, so Down
   from the games reaches it and Up returns, exactly like the rail.

   A destination with a live count (Downloads) shows the count in mono and the
   dot as its unread badge. Nothing else carries an ornament.
   -------------------------------------------------------------------------- */

const BAND_META: Record<HomeItemId, { glyph: GlyphName; label: () => string }> = {
  search: { glyph: "search", label: () => t("Search") },
  library: { glyph: "library", label: () => t("Library") },
  downloads: { glyph: "downloads", label: () => t("Downloads") },
  settings: { glyph: "settings", label: () => t("Settings") },
  power: { glyph: "power", label: () => t("Power") },
};

export interface BandItemDetail {
  /** Sentence-case reason or state; never a second label. */
  detail?: string;
  /** Mono count beside the label (downloads). */
  count?: number;
  disabled?: boolean;
  /** The dot: an unread or in-flight event on this destination. */
  dot?: boolean;
}

export function HomeBand({
  active,
  focused,
  details,
  onHighlight,
  onChoose,
  settled = false,
}: {
  active: number;
  focused: boolean;
  details: Record<HomeItemId, BandItemDetail>;
  onHighlight: (index: number) => void;
  onChoose: (index: number) => void;
  /** The boot cascade already played: re-enter without replaying it. */
  settled?: boolean;
}) {
  const reduceMotion = useReducedMotion();
  const boot = reveal(reduceMotion, { y: 6, delay: BOOT_DELAY.home, duration: BOOT_DURATION, settled });
  return (
    <motion.nav
      aria-label={t("System")}
      className="bp-band relative z-10 shrink-0 border-t bp-pad-x"
      {...boot}
    >
      <ul className="flex items-stretch gap-1 overflow-x-auto py-2 nav-scroll">
        {HOME_ITEM_IDS.map((id, index) => {
          const meta = BAND_META[id];
          const item = details[id];
          const selected = focused && index === active;
          return (
            <li key={id} className="min-w-0 shrink-0">
              <button
                type="button"
                data-home-active={selected ? "true" : undefined}
                data-gamepad-target="true"
                data-bp-zone-item="true"
                data-bp-zone-active={selected ? "true" : undefined}
                data-bp-band-item="true"
                aria-current={selected ? "true" : undefined}
                aria-disabled={item.disabled ? "true" : undefined}
                aria-label={item.detail ? `${meta.label()} — ${item.detail}` : meta.label()}
                onPointerEnter={() => onHighlight(index)}
                onClick={() => onChoose(index)}
                className={cn("bp-band-item", item.disabled && "opacity-60")}
              >
                <Glyph name={meta.glyph} className="size-[calc(var(--bp-u)*28)] shrink-0" />
                <span className="truncate text-bp-body font-medium leading-none">
                  {meta.label()}
                </span>
                {typeof item.count === "number" && item.count > 0 && (
                  <span className="num text-bp-meta leading-none text-ink-3">{item.count}</span>
                )}
                {item.dot && <span className="bp-dot" aria-hidden="true" />}
              </button>
            </li>
          );
        })}
      </ul>
    </motion.nav>
  );
}
