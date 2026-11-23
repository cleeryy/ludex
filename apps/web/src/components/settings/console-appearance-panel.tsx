import { Check, RotateCcw } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { Button } from "@/components/ui/controls";
import {
  useConsoleRoom,
  setConsoleRoom,
} from "@/components/big-picture/use-console-room";
import {
  CONSOLE_ACCENT_SOURCES,
  CONSOLE_CORNERS,
  CONSOLE_DENSITIES,
  CONSOLE_HAPTICS,
  CONSOLE_MOTIONS,
  CONSOLE_ROOMS,
  CONSOLE_SOUNDS,
  CONSOLE_WALLS,
  resetConsoleRoom,
  withConsoleRoom,
  type ConsoleRoomSettings,
} from "@/theme/console-room";
import {
  consoleAccentLabel,
  consoleCornersLabel,
  consoleDensityLabel,
  consoleHapticsLabel,
  consoleMotionLabel,
  consoleRoomDescription,
  consoleRoomLabel,
  consoleSoundLabel,
  consoleWallLabel,
} from "@/components/big-picture/labels";

/* --------------------------------------------------------------------------
   Settings → Appearance → Console room (#268).

   The desktop counterpart of the Console's own Appearance section. Both read
   the same device-local record, so a choice here previews in a running console
   window immediately and survives the next launch. Rooms are chips; the eight
   knobs are chips too. There is no Apply button: the value is the preview.
   -------------------------------------------------------------------------- */

function Chip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "inline-flex h-8 items-center gap-2 rounded-[6px] border px-2.5 text-label transition-colors duration-150",
        active
          ? "border-signal bg-surface-2 text-ink"
          : "border-line text-ink-2 hover:border-line-2 hover:bg-surface",
      )}
    >
      {children}
      {active && <Check className="size-3 shrink-0 text-signal-text" strokeWidth={3} aria-hidden="true" />}
    </button>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-start gap-x-4 gap-y-2 border-t border-line py-3 first:border-t-0 first:pt-0">
      <span className="w-[120px] shrink-0 pt-1.5 text-meta text-ink-3">{label}</span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap gap-2">{children}</div>
        {hint && <p className="mt-1.5 max-w-[64ch] text-meta leading-relaxed text-ink-3">{hint}</p>}
      </div>
    </div>
  );
}

const KNOBS: ReadonlyArray<{
  knob: Exclude<keyof ConsoleRoomSettings, "room" | "accent"> | "accent";
  label: () => string;
  values: readonly string[];
  value: (settings: ConsoleRoomSettings) => string;
  format: (value: string) => string;
}> = [
  {
    knob: "accent",
    label: () => t("Accent"),
    values: CONSOLE_ACCENT_SOURCES,
    value: (settings) => settings.accent,
    format: (value) => consoleAccentLabel(value as ConsoleRoomSettings["accent"]),
  },
  {
    knob: "wall",
    label: () => t("Wall"),
    values: CONSOLE_WALLS,
    value: (settings) => settings.wall,
    format: (value) => consoleWallLabel(value as ConsoleRoomSettings["wall"]),
  },
  {
    knob: "corners",
    label: () => t("Corners"),
    values: CONSOLE_CORNERS,
    value: (settings) => settings.corners,
    format: (value) => consoleCornersLabel(value as ConsoleRoomSettings["corners"]),
  },
  {
    knob: "density",
    label: () => t("Density"),
    values: CONSOLE_DENSITIES,
    value: (settings) => settings.density,
    format: (value) => consoleDensityLabel(value as ConsoleRoomSettings["density"]),
  },
  {
    knob: "motion",
    label: () => t("Motion"),
    values: CONSOLE_MOTIONS,
    value: (settings) => settings.motion,
    format: (value) => consoleMotionLabel(value as ConsoleRoomSettings["motion"]),
  },
  {
    knob: "sound",
    label: () => t("Sound"),
    values: CONSOLE_SOUNDS,
    value: (settings) => settings.sound,
    format: (value) => consoleSoundLabel(value as ConsoleRoomSettings["sound"]),
  },
  {
    knob: "haptics",
    label: () => t("Haptics"),
    values: CONSOLE_HAPTICS,
    value: (settings) => settings.haptics,
    format: (value) => consoleHapticsLabel(value as ConsoleRoomSettings["haptics"]),
  },
];

export function ConsoleAppearancePanel() {
  const [room] = useConsoleRoom();

  const chooseRoom = (next: ConsoleRoomSettings["room"]) => {
    setConsoleRoom(withConsoleRoom(room, next));
  };
  const chooseKnob = (knob: (typeof KNOBS)[number]["knob"], value: string) => {
    setConsoleRoom({ ...room, [knob]: value } as ConsoleRoomSettings);
  };

  return (
    <section className="border border-line p-4" aria-label={t("Console room")}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <p className="text-body font-medium text-ink">{t("Console room")}</p>
          <p className="mt-1 max-w-[64ch] text-meta leading-relaxed text-ink-3">
            {t("How Console mode looks on this device. A room sets eight knobs at once; each one can then be changed on its own. The desktop keeps the app theme.")}
          </p>
        </div>
        <span className="text-meta text-ink-3">
          {t("Now: {values}", {
            values: [
              consoleRoomLabel(room.room),
              consoleWallLabel(room.wall),
              consoleCornersLabel(room.corners),
              consoleDensityLabel(room.density),
            ].join(" · "),
          })}
        </span>
      </div>

      <div className="mt-3">
        <Row label={t("Room")} hint={consoleRoomDescription(room.room)}>
          {CONSOLE_ROOMS.map((entry) => (
            <Chip key={entry} active={room.room === entry} onClick={() => chooseRoom(entry)}>
              {consoleRoomLabel(entry)}
            </Chip>
          ))}
        </Row>

        {KNOBS.map((knob) => (
          <Row key={knob.knob} label={knob.label()}>
            {knob.values.map((value) => (
              <Chip
                key={value}
                active={knob.value(room) === value}
                onClick={() => chooseKnob(knob.knob, value)}
              >
                {knob.format(value)}
              </Chip>
            ))}
          </Row>
        ))}
      </div>

      <div className="mt-1 flex flex-wrap items-center justify-between gap-3 border-t border-line pt-3">
        <p className="max-w-[64ch] text-meta leading-relaxed text-ink-3">
          {t("Changes apply immediately. Reset restores the room's own answers.")}
        </p>
        <Button
          size="sm"
          variant="ghost"
          icon={<RotateCcw className="size-3.5" />}
          onClick={() => setConsoleRoom(resetConsoleRoom(room))}
        >
          {t("Reset room")}
        </Button>
      </div>
    </section>
  );
}
