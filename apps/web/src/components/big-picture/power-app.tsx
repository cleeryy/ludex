import type { ComponentType } from "react";
import { Gamepad2, LogOut, Moon, Power, RotateCcw } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import type { PowerAction, PowerActionId } from "./power";

/* --------------------------------------------------------------------------
   The Power surface (#196).

   Big tiles, not a dropdown: the player is holding a controller and the one
   thing this screen must never do is make "Quit" look like "Cancel". The
   confirmation lives one level up and always starts on Cancel.
   -------------------------------------------------------------------------- */

type Icon = ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" | "false" }>;

const ACTION_ICONS: Record<PowerActionId, Icon> = {
  "end-session": LogOut,
  sleep: Moon,
  restart: RotateCcw,
  quit: Power,
  "quit-game": Gamepad2,
  exit: LogOut,
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
    <section className="flex min-h-0 flex-1 flex-col overflow-y-auto overscroll-contain py-5 bp-pad-x" aria-label={t("Power")}>
      <p className="font-mono text-[9.5px] uppercase tracking-[0.18em] text-signal">{t("System")}</p>
      <h1 className="mt-1 text-[clamp(1.8rem,3.2vw,3rem)] font-semibold leading-[0.95] tracking-[-0.04em] text-ink">
        {t("Power")}
      </h1>
      <p className="mt-2 max-w-[62ch] text-[12.5px] leading-relaxed text-ink-2">
        {t("Everything here works with the controller. B closes this menu.")}
      </p>

      <ul className="mt-5 grid max-w-[980px] grid-cols-1 gap-2.5 sm:grid-cols-2">
        {actions.map((action, index) => {
          const Icon = ACTION_ICONS[action.id];
          const selected = index === active;
          const danger = action.tone === "danger";
          return (
            <li key={action.id} className="min-w-0">
              <button
                type="button"
                data-system-active={selected ? "true" : undefined}
                data-gamepad-target="true"
                data-bp-zone-item="true"
                data-bp-zone-active={selected ? "true" : undefined}
                aria-current={selected ? "true" : undefined}
                aria-disabled={action.enabled ? undefined : "true"}
                aria-label={`${action.label} — ${action.enabled ? action.detail : action.reason ?? action.detail}`}
                onPointerEnter={() => onHighlight(index)}
                onClick={() => onChoose(index)}
                className={cn(
                  "group relative flex min-h-[128px] w-full flex-col justify-between gap-4 border p-4 text-start",
                  selected
                    ? danger
                      ? "border-danger bg-surface-2"
                      : "border-signal bg-surface-2"
                    : "border-line bg-bg-deep/60 hover:border-line-2 hover:bg-surface/60",
                  !action.enabled && "opacity-70",
                )}
              >
                <span className="flex w-full items-start justify-between gap-3">
                  <Icon
                    className={cn(
                      "size-7 shrink-0",
                      danger ? "text-danger" : selected ? "text-signal-text" : "text-ink-2",
                    )}
                    aria-hidden="true"
                  />
                  <span className="num font-mono text-[9.5px] uppercase tracking-[0.12em] text-ink-3">
                    {String(index + 1).padStart(2, "0")}
                  </span>
                </span>
                <span className="min-w-0">
                  <span className="block text-[clamp(1rem,1.35vw,1.35rem)] font-semibold leading-tight text-ink">
                    {action.label}
                  </span>
                  <span className="mt-1 block max-w-[58ch] text-[11px] leading-relaxed text-ink-3">
                    {action.enabled ? action.detail : action.reason ?? action.detail}
                  </span>
                </span>
                {selected && (
                  <span
                    aria-hidden="true"
                    className={cn("pointer-events-none absolute inset-x-0 bottom-0 h-[2px]", danger ? "bg-danger" : "bg-signal")}
                  />
                )}
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
