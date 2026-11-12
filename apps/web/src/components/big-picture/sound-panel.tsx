import { Check, Volume2, VolumeX, Vibrate } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { firstRumblePad, pulsePad } from "@/lib/gamepad";
import { uiSound } from "@/lib/ui-sound";
import { Switch } from "@/components/ui/controls";
import {
  clampVolume,
  type CueId,
  type SoundPrefs,
} from "@/lib/ui-sound-specs";
import { hapticsActive, hapticsStrengthLabel } from "./controller";
import { useBigPicturePrefs } from "./use-controller-prefs";

/* --------------------------------------------------------------------------
   Settings → Appearance → Big Picture sound (#198).

   The same device store the console screen reads, so a change here applies to
   a running Big Picture immediately. Two controls, one test row: the sound
   toggle, the volume slider, and the haptics strength. Every cue is a generated
   WAV (`scripts/bp-sounds.mjs`), never a sample from a console.
   -------------------------------------------------------------------------- */

const TEST_CUES: ReadonlyArray<{ cue: CueId; label: () => string }> = [
  { cue: "move", label: () => t("Test move") },
  { cue: "confirm", label: () => t("Test confirm") },
  { cue: "error", label: () => t("Test error") },
];

function SoundToggle({
  enabled,
  onChange,
  label,
  detail,
  icon,
}: {
  enabled: boolean;
  onChange: (next: boolean) => void;
  label: string;
  detail: string;
  icon: "sound" | "haptics";
}) {
  const Icon = icon === "sound" ? (enabled ? Volume2 : VolumeX) : Vibrate;
  return (
    <div className="flex items-start justify-between gap-4 border-b border-line py-3">
      <div className="min-w-0">
        <p className="flex items-center gap-2 text-[13px] font-medium text-ink">
          <Icon className={cn("size-3.5", enabled ? "text-signal-text" : "text-ink-3")} aria-hidden="true" />
          {label}
        </p>
        <p className="mt-1 max-w-[58ch] text-[11.5px] leading-relaxed text-ink-3">{detail}</p>
      </div>
      <Switch checked={enabled} onCheckedChange={onChange} label={label} />
    </div>
  );
}

function StrengthChoice({
  value,
  onChange,
}: {
  value: "off" | "standard" | "strong";
  onChange: (next: "off" | "standard" | "strong") => void;
}) {
  const options = ["off", "standard", "strong"] as const;
  return (
    <div role="group" aria-label={t("Haptics strength")} className="flex flex-wrap items-center gap-1">
      {options.map((option) => {
        const active = option === value;
        return (
          <button
            key={option}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(option)}
            className={cn(
              "inline-flex h-8 items-center gap-1.5 border px-2.5 text-[11.5px] transition-colors duration-150",
              active
                ? "border-signal bg-surface-2 text-ink"
                : "border-line text-ink-3 hover:border-line-2 hover:text-ink-2",
            )}
          >
            {active && <Check className="size-3" strokeWidth={3} aria-hidden="true" />}
            {hapticsStrengthLabel(option)}
          </button>
        );
      })}
    </div>
  );
}

export function BigPictureSoundPanel() {
  const [prefs, update] = useBigPicturePrefs();
  const sound = prefs.sound;
  const percent = Math.round(clampVolume(sound.volume) * 100);
  const hapticsOn = hapticsActive(prefs, { vibration: true });

  const setSound = (next: Partial<SoundPrefs>) => {
    update({ sound: { ...sound, ...next } });
  };

  const testCue = (cue: CueId) => {
    if (!sound.enabled) return;
    uiSound.play(cue);
  };

  const testPulse = () => {
    if (!hapticsOn) return;
    pulsePad(firstRumblePad(), "confirm", prefs.hapticsStrength);
  };

  return (
    <section className="border border-line p-4" aria-label={t("Big Picture sound")}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <p className="text-[13px] font-medium text-ink">{t("Big Picture sound")}</p>
          <p className="mt-1 max-w-[64ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("Short cues for movement, confirmations and launches. Silent while a game runs.")}
          </p>
        </div>
        <span className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
          {sound.enabled ? t("Now: {value}", { value: `${percent}%` }) : t("Sound is off")}
        </span>
      </div>

      <div className="mt-2">
        <SoundToggle
          enabled={sound.enabled}
          onChange={(enabled) => setSound({ enabled })}
          label={t("UI sounds")}
          detail={t("Every press answers with a short, dry cue. Saved on this device.")}
          icon="sound"
        />

        <div className={cn("border-b border-line py-3", !sound.enabled && "opacity-55")}>
          <div className="flex flex-wrap items-baseline justify-between gap-3">
            <label htmlFor="bp-sound-volume" className="text-[13px] font-medium text-ink">
              {t("Volume")}
            </label>
            <span className="font-mono text-[12px] tabular-nums text-ink-2">{percent}%</span>
          </div>
          <input
            id="bp-sound-volume"
            type="range"
            min={0}
            max={100}
            step={5}
            value={percent}
            aria-label={t("Volume")}
            aria-valuetext={t("{value} percent", { value: percent })}
            onChange={(event) => setSound({ volume: clampVolume(Number(event.target.value) / 100) })}
            className="mt-3 w-full max-w-[420px] accent-signal"
          />
          <p className="mt-2 text-[11px] leading-relaxed text-ink-3">
            {t("Applies to this device. Volume is squared, so 45 % sits near half loudness.")}
          </p>
        </div>

        <SoundToggle
          enabled={prefs.haptics && prefs.hapticsStrength !== "off"}
          onChange={(enabled) =>
            update(
              enabled
                ? { haptics: true, hapticsStrength: prefs.hapticsStrength === "off" ? "standard" : prefs.hapticsStrength }
                : { haptics: false },
            )
          }
          label={t("Haptics")}
          detail={t("Vibrate on the cue a pad caused. Keyboard and mouse never rumble.")}
          icon="haptics"
        />

        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line py-3">
          <div className="min-w-0">
            <p className="text-[13px] font-medium text-ink">{t("Haptics strength")}</p>
            <p className="mt-1 max-w-[52ch] text-[11.5px] leading-relaxed text-ink-3">
              {t("Standard or strong rumble on this device. Off silences the pad.")}
            </p>
          </div>
          <StrengthChoice
            value={prefs.haptics ? prefs.hapticsStrength : "off"}
            onChange={(next) =>
              update({ hapticsStrength: next, haptics: next === "off" ? false : true })
            }
          />
        </div>
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <span className="font-mono text-[9.5px] uppercase tracking-[0.14em] text-ink-3">{t("Test")}</span>
        {TEST_CUES.map((entry) => (
          <button
            key={entry.cue}
            type="button"
            onClick={() => testCue(entry.cue)}
            disabled={!sound.enabled}
            className={cn(
              "inline-flex h-8 items-center border border-line px-2.5 text-[11.5px] text-ink-2 transition-colors duration-150",
              "hover:border-line-2 hover:text-ink disabled:opacity-55",
            )}
          >
            {entry.label()}
          </button>
        ))}
        <button
          type="button"
          onClick={testPulse}
          disabled={!hapticsOn}
          className={cn(
            "inline-flex h-8 items-center gap-1.5 border border-line px-2.5 text-[11.5px] text-ink-2 transition-colors duration-150",
            "hover:border-line-2 hover:text-ink disabled:opacity-55",
          )}
        >
          <Vibrate className="size-3.5" aria-hidden="true" />
          {t("Test pulse")}
        </button>
      </div>

      <p className="mt-3 text-[11px] leading-relaxed text-ink-3">
        {t("Saved on this device. Also in Big Picture → Settings → Controllers.")}
      </p>
    </section>
  );
}
