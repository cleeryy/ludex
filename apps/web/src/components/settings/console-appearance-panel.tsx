import { Check, RotateCcw } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { useSettings } from "@/state/settings";
import { Button } from "@/components/ui/controls";
import {
  CONSOLE_ACCENT_IDS,
  CONSOLE_BACKGROUNDS,
  CONSOLE_PRESETS,
  CONSOLE_RADII,
  DEFAULT_CONSOLE_APPEARANCE,
  consoleAccentColor,
  withConsolePreset,
  type ConsoleAccentId,
  type ConsoleBackground,
  type ConsolePreset,
  type ConsoleRadius,
} from "@/theme/console-appearance";
import {
  consoleAccentLabel,
  consoleBackgroundLabel,
  consolePresetDescription,
  consolePresetLabel,
  consoleRadiusLabel,
} from "@/components/big-picture/labels";
import { useConsoleAppearance } from "@/components/big-picture/use-console-appearance";

/* --------------------------------------------------------------------------
   Settings → Appearance → Console appearance (#229).

   The desktop counterpart of Console mode's own Appearance section. The same
   device-local store drives both, so a choice made here previews in a running
   console window immediately and survives the next launch. Colours still come
   from the theme pipeline: the swatches read `consoleAccentColor`, never a
   literal.
   -------------------------------------------------------------------------- */

function Chip({
  active,
  onClick,
  children,
  swatch,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
  swatch?: string | null;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "inline-flex h-8 items-center gap-2 rounded-[2px] border px-2.5 text-[12px] transition-colors duration-150",
        active
          ? "border-signal bg-surface-2 text-ink"
          : "border-line text-ink-2 hover:border-line-2 hover:bg-surface",
      )}
    >
      {swatch !== undefined && (
        <span
          aria-hidden="true"
          className={cn(
            "size-3.5 shrink-0 rounded-full border",
            swatch === null ? "border-line-2 bg-signal" : "border-line-2",
          )}
          style={swatch === null ? undefined : { backgroundColor: swatch }}
        />
      )}
      {children}
      {active && <Check className="size-3 shrink-0 text-signal-text" strokeWidth={3} aria-hidden="true" />}
    </button>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-start gap-x-4 gap-y-2 border-t border-line py-3 first:border-t-0 first:pt-0">
      <span className="w-[120px] shrink-0 pt-1.5 font-mono text-[9.5px] uppercase tracking-[0.14em] text-ink-3">
        {label}
      </span>
      <div className="flex min-w-0 flex-1 flex-wrap gap-2">{children}</div>
    </div>
  );
}

export function ConsoleAppearancePanel() {
  const [appearance, setAppearance] = useConsoleAppearance();
  const { resolved } = useSettings();

  const setPreset = (preset: ConsolePreset) => setAppearance(withConsolePreset(appearance, preset));
  const setAccent = (accent: ConsoleAccentId) => setAppearance({ ...appearance, accent });
  const setBackground = (background: ConsoleBackground) => setAppearance({ ...appearance, background });
  const setRadius = (radius: ConsoleRadius) => setAppearance({ ...appearance, radius });
  // The Soft palette is light by construction; the swatch must show the tone
  // the console will actually use, not the desktop's polarity.
  const swatchMode = appearance.preset === "soft" ? "light" : resolved;

  return (
    <section className="border border-line p-4" aria-label={t("Console appearance")}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <p className="text-[13px] font-medium text-ink">{t("Console appearance")}</p>
          <p className="mt-1 max-w-[64ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("How Console mode looks on this device. Changes preview immediately, here and in a running console window. The desktop keeps the app theme.")}
          </p>
        </div>
        <span className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
          {t("Now: {values}", {
            values: [
              consolePresetLabel(appearance.preset),
              consoleBackgroundLabel(appearance.background),
              consoleRadiusLabel(appearance.radius),
            ].join(" · "),
          })}
        </span>
      </div>

      <div className="mt-3">
        <Row label={t("Console style")}>
          {CONSOLE_PRESETS.map((preset) => (
            <Chip key={preset} active={appearance.preset === preset} onClick={() => setPreset(preset)}>
              {consolePresetLabel(preset)}
            </Chip>
          ))}
          <span className="w-full pt-1 text-[11px] leading-relaxed text-ink-3">
            {consolePresetDescription(appearance.preset)}
          </span>
        </Row>

        <Row label={t("Accent")}>
          {CONSOLE_ACCENT_IDS.map((accent) => (
            <Chip
              key={accent}
              active={appearance.accent === accent}
              onClick={() => setAccent(accent)}
              swatch={consoleAccentColor(accent, swatchMode)}
            >
              {consoleAccentLabel(accent)}
            </Chip>
          ))}
        </Row>

        <Row label={t("Background")}>
          {CONSOLE_BACKGROUNDS.map((background) => (
            <Chip
              key={background}
              active={appearance.background === background}
              onClick={() => setBackground(background)}
            >
              {consoleBackgroundLabel(background)}
            </Chip>
          ))}
        </Row>

        <Row label={t("Corners")}>
          {CONSOLE_RADII.map((radius) => (
            <Chip
              key={radius}
              active={appearance.radius === radius}
              onClick={() => setRadius(radius)}
            >
              {consoleRadiusLabel(radius)}
            </Chip>
          ))}
        </Row>
      </div>

      <div className="mt-1 flex flex-wrap items-center justify-between gap-3 border-t border-line pt-3">
        <p className="max-w-[64ch] text-[11px] leading-relaxed text-ink-3">
          {t("Saved on this device. A full theme editor and per-element overrides are not built yet.")}
        </p>
        <Button
          size="sm"
          variant="ghost"
          icon={<RotateCcw className="size-3.5" />}
          onClick={() => setAppearance(DEFAULT_CONSOLE_APPEARANCE)}
        >
          {t("Reset console appearance")}
        </Button>
      </div>
    </section>
  );
}
