import { motion, useReducedMotion } from "motion/react";
import { Check, Gamepad2, Vibrate } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import type { ControllerProfile } from "@/lib/types";
import type { PadFamily, ProfileOption } from "./controller";
import { padFamilyLabel, profileSummary } from "./controller";
import { OverlayRow, OverlayShell } from "./overlays";

/* --------------------------------------------------------------------------
   Controller profiles inside Big Picture (#51 + #138).

   The same account/per-game profiles and built-in presets the settings screen
   edits, applied without leaving the couch: A switches the active profile, the
   last row toggles haptics. The selected profile keeps driving navigation
   because the screen hands it straight to `useGamepad`, so deadzone, curve,
   inversion, trigger threshold and remapping all take effect on the next
   press. The row summary shows the values that changed.
   -------------------------------------------------------------------------- */

export function ProfileOverlay({
  active,
  onChoose,
  onHighlight,
  options,
  selectedId,
  profile,
  haptics,
  family,
  ...shell
}: {
  active: number;
  onChoose: (index: number) => void;
  onHighlight: (index: number) => void;
  options: readonly ProfileOption[];
  selectedId: string;
  /** Effective profile for the current game, shown as the live summary. */
  profile: ControllerProfile;
  haptics: boolean;
  family: PadFamily;
} & Omit<Parameters<typeof OverlayShell>[0], "children" | "title" | "subtitle">) {
  const reduce = useReducedMotion();
  return (
    <OverlayShell
      title={t("Controller profile")}
      subtitle={t("Applies immediately. Remapping never takes navigation away: directions, A and B are always routed by action.")}
      family={family}
      {...shell}
    >
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 pb-2 font-mono text-[10.5px] text-ink-3">
        <Gamepad2 className="size-3.5 text-signal-text" aria-hidden="true" />
        <span className="text-ink-2">{padFamilyLabel(family)}</span>
        <span aria-hidden="true">·</span>
        <span className="truncate">{profileSummary(profile)}</span>
        {!profile.vibration && (
          <span className="text-ink-3">
            · {t("Vibration")}: {t("Disabled")}
          </span>
        )}
      </p>

      <ul className="flex flex-col gap-0.5">
        {options.map((option, index) => {
          const selected = option.id === selectedId;
          return (
            <li key={option.id}>
              <OverlayRow
                active={index === active}
                onChoose={() => onChoose(index)}
                onHighlight={() => onHighlight(index)}
                label={option.label}
                detail={`${option.detail} · ${profileSummary(option.profile)}`}
                trailing={
                  selected ? (
                    <Check className="size-4 text-signal-text" strokeWidth={3} aria-hidden="true" />
                  ) : undefined
                }
              />
            </li>
          );
        })}
      </ul>

      <div className="mt-2 border-t border-line pt-2">
        <OverlayRow
          active={active === options.length}
          onChoose={() => onChoose(options.length)}
          onHighlight={() => onHighlight(options.length)}
          icon={<Vibrate className={cn("size-4", haptics ? "text-signal-text" : "text-ink-3")} aria-hidden="true" />}
          label={t("Vibration")}
          detail={t("Vibrate on focus and confirm when the pad supports it.")}
          trailing={
            <span
              className={cn(
                "inline-flex h-5 w-9 items-center border px-0.5 transition-colors",
                haptics ? "justify-end border-signal bg-signal/20" : "justify-start border-line-2 bg-surface",
              )}
              aria-hidden="true"
            >
              <motion.span
                layout={!reduce}
                transition={{ duration: reduce ? 0 : 0.14 }}
                className={cn("block h-3.5 w-3.5", haptics ? "bg-signal" : "bg-ink-3")}
              />
            </span>
          }
        />
      </div>
    </OverlayShell>
  );
}
