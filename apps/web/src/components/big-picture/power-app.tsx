import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { Glyph, type GlyphName } from "./glyphs";
import type { PowerAction, PowerActionId } from "./power";

/* --------------------------------------------------------------------------
   The Power surface (#196, #268).

   Four rows, not a tile grid: Sleep · Restart · Quit · Sign out, each with one
   line of what it does. The player is holding a controller and the one thing
   this screen must never do is make "Quit" look like "Cancel": the
   confirmation lives one level up and always starts on Cancel.

   No `01–04` index, no equal-card composition, no danger fill before the player
   has actually chosen the destructive action.
   -------------------------------------------------------------------------- */

const ACTION_GLYPHS: Record<PowerActionId, GlyphName> = {
  "end-session": "signout",
  sleep: "sleep",
  restart: "restart",
  quit: "power",
  "sign-out": "signout",
  "quit-game": "play",
  exit: "back",
};

export function PowerApp({
  actions,
  active,
  onHighlight,
  onChoose,
}: {
  actions: readonly PowerAction[];
  active: number;
  onHighlight: (index: number) => void;
  onChoose: (index: number) => void;
}) {
  return (
    <section
      className="flex min-h-0 flex-1 flex-col overflow-y-auto overscroll-contain py-5 bp-pad-x"
      aria-label={t("Power")}
    >
      <h1 className="text-bp-title font-semibold leading-[1.05] tracking-[-0.02em] text-ink">
        {t("Power")}
      </h1>
      <p className="mt-1 max-w-[62ch] text-bp-meta leading-relaxed text-ink-2">
        {t("Everything here works with the controller. B closes this menu.")}
      </p>

      <ul className="mt-5 max-w-[980px]">
        {actions.map((action, index) => {
          const selected = index === active;
          const danger = action.tone === "danger";
          return (
            <li key={action.id} className="min-w-0">
              <button
                type="button"
                data-system-active={selected ? "true" : undefined}
                data-gamepad-target="true"
                data-bp-settings-row="true"
                data-tone={danger ? "danger" : undefined}
                aria-current={selected ? "true" : undefined}
                aria-disabled={action.enabled ? undefined : "true"}
                aria-label={`${action.label} — ${action.enabled ? action.detail : action.reason ?? action.detail}`}
                onPointerEnter={() => onHighlight(index)}
                onClick={() => onChoose(index)}
                className={cn(
                  "bp-row relative mb-1 flex min-h-[calc(var(--bp-u)*88)] w-full items-center gap-5 px-4 py-3 text-start",
                  selected && "bg-surface-2",
                  !action.enabled && "opacity-70",
                )}
              >
                {selected && (
                  <span
                    aria-hidden="true"
                    className={cn(
                      "pointer-events-none absolute inset-y-3 start-0 w-[calc(var(--bp-u)*3)]",
                      danger ? "bg-danger" : "bg-signal",
                    )}
                  />
                )}
                <Glyph
                  name={ACTION_GLYPHS[action.id]}
                  className={cn(
                    "size-[calc(var(--bp-u)*32)] shrink-0",
                    danger ? "text-danger" : selected ? "text-signal-text" : "text-ink-2",
                  )}
                />
                <span className="min-w-0 flex-1">
                  <span className="block text-bp-body font-semibold leading-tight text-ink">
                    {action.label}
                  </span>
                  <span className="mt-1 block max-w-[58ch] text-bp-meta leading-relaxed text-ink-3">
                    {action.enabled ? action.detail : action.reason ?? action.detail}
                  </span>
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
