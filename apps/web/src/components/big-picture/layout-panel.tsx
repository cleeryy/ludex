import { Check } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { layoutDescription, layoutLabel } from "./labels";
import { BIG_PICTURE_LAYOUTS } from "./layout";
import { LayoutPreview } from "./layout-preview";
import { useBigPictureLayout } from "./use-layout";

/**
 * Settings → Appearance → Big Picture layout (#141).
 *
 * The same store the screen uses: picking a card applies the layout to a
 * running Big Picture window immediately, and to the next launch because the
 * choice is written to `localStorage` for this device.
 */
export function BigPictureLayoutPanel() {
  const [layout, setLayout] = useBigPictureLayout();

  return (
    <section className="border border-line p-4" aria-label={t("Big Picture layout")}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <p className="text-[13px] font-medium text-ink">{t("Big Picture layout")}</p>
          <p className="mt-1 max-w-[64ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("Big Picture opens in this layout. Changes apply immediately, here and in a running screen.")}
          </p>
        </div>
        <span className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
          {t("Now: {layout}", { layout: layoutLabel(layout) })}
        </span>
      </div>

      <div
        className="mt-3 grid grid-cols-2 gap-2.5 sm:grid-cols-3 xl:grid-cols-6"
        role="group"
        aria-label={t("Big Picture layout")}
      >
        {BIG_PICTURE_LAYOUTS.map((entry) => {
          const active = entry === layout;
          return (
            <button
              key={entry}
              type="button"
              onClick={() => setLayout(entry)}
              aria-pressed={active}
              className={cn(
                "flex flex-col gap-1.5 border p-1.5 text-start transition-colors duration-150",
                active ? "border-signal bg-surface-2" : "border-line hover:border-line-2 hover:bg-surface",
              )}
            >
              <LayoutPreview layout={entry} />
              <span className="flex items-baseline justify-between gap-2 px-0.5">
                <span className="text-[12.5px] font-medium text-ink">{layoutLabel(entry)}</span>
                <span
                  className={cn(
                    "grid size-4 shrink-0 place-items-center rounded-full border",
                    active ? "border-signal bg-signal text-on-signal" : "border-line-2 text-transparent",
                  )}
                >
                  <Check className="size-2.5" strokeWidth={3} aria-hidden="true" />
                </span>
              </span>
              <span className="line-clamp-3 px-0.5 text-[11px] leading-snug text-ink-3">
                {layoutDescription(entry)}
              </span>
            </button>
          );
        })}
      </div>

      <p className="mt-2.5 text-[11px] leading-relaxed text-ink-3">
        {t("Saved on this device. Big Picture switches with L on a keyboard, or Menu → Layout on a controller.")}
      </p>
    </section>
  );
}
