import { motion, useReducedMotion } from "motion/react";
import { cn } from "@/lib/cn";
import { t, tn } from "@/lib/i18n";
import type { HapticsStrength } from "@/lib/gamepad-core";
import type { ControllerProfile } from "@/lib/types";
import type { PadFamily, ProfileOption } from "./controller";
import { hapticsStrengthLabel, padFamilyLabel, profileSummary } from "./controller";
import { Glyph } from "./glyphs";
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
  strength,
  family,
  pad = null,
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
  /** Rumble intensity on this device (#198). */
  strength: HapticsStrength;
  family: PadFamily;
  /** Compact identity line for the pad driving the console (#230). */
  pad?: { label: string; fingerprint: string; count: number } | null;
} & Omit<Parameters<typeof OverlayShell>[0], "children" | "title" | "subtitle">) {
  const reduce = useReducedMotion();
  return (
    <OverlayShell
      title={t("Controller profile")}
      subtitle={t("Applies immediately. Remapping never takes navigation away: directions, A and B are always routed by action.")}
      family={family}
      {...shell}
    >
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 pb-2 font-mono text-bp-meta text-ink-3">
        <Glyph name="pad" className="size-[calc(var(--bp-u)*24)] text-signal-text" />
        <span className="text-ink-2">{padFamilyLabel(family)}</span>
        <span aria-hidden="true">·</span>
        <span className="truncate">{profileSummary(profile)}</span>
        {pad && (
          <>
            <span aria-hidden="true">·</span>
            <span className="truncate" title={pad.fingerprint}>
              {pad.label}
            </span>
            {pad.count > 1 && <span>· {tn(pad.count, "{count} pad", "{count} pads")}</span>}
          </>
        )}
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
                    <Glyph name="check" className="size-[calc(var(--bp-u)*24)] text-signal-text" />
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
          icon={<Glyph name="vibrate" className={cn("size-[calc(var(--bp-u)*24)]", haptics ? "text-signal-text" : "text-ink-3")} />}
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
        <OverlayRow
          active={active === options.length + 1}
          onChoose={() => onChoose(options.length + 1)}
          onHighlight={() => onHighlight(options.length + 1)}
          icon={<Glyph name="settings" className={cn("size-[calc(var(--bp-u)*24)]", strength !== "off" ? "text-signal-text" : "text-ink-3")} />}
          label={t("Haptics strength")}
          detail={t("Standard or strong rumble on this device.")}
          trailing={
            <span className="text-bp-meta text-ink-2">{hapticsStrengthLabel(strength)}</span>
          }
        />
      </div>
    </OverlayShell>
  );
}
