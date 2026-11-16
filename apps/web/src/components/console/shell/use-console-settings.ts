import { useMemo } from "react";
import { localeMeta, selectableLocales, t } from "@/lib/i18n";
import { uiSound } from "@/lib/ui-sound";
import { clampVolume, stepVolume, type CueId } from "@/lib/ui-sound-specs";
import { useAuth } from "@/state/auth";
import { useI18n } from "@/lib/i18n";
import type { SettingsRow } from "@/components/big-picture/settings-app";
import { hapticsStrengthLabel, type BigPictureControllerPrefs } from "@/components/big-picture/controller";
import {
  artworkAccentNote,
  consoleAccentLabel,
  consoleCornersLabel,
  consoleDensityLabel,
  consoleHapticsLabel,
  consoleKnobDescription,
  consoleMotionLabel,
  consoleRoomDescription,
  consoleRoomLabel,
  consoleSoundLabel,
  consoleWallLabel,
  layoutDescription,
  layoutLabel,
} from "@/components/big-picture/labels";
import { wrapIndex, type SettingsSectionId } from "@/components/big-picture/system-zones";
import { BIG_PICTURE_LAYOUTS, type BigPictureLayout } from "@/components/big-picture/layout";
import {
  KNOB_VALUES,
  cycleChoice,
  resetConsoleRoom,
  withConsoleRoom,
  type ConsoleRoomSettings,
} from "@/theme/console-room";
import type { ControllerProfile } from "@/lib/types";
import type { ThemeSelection } from "@/theme/types";

/**
 * The Console settings app's model (#267, #268).
 *
 * Four sections — language, appearance, controllers, streaming — one row per
 * knob, no Apply: every change lands immediately and is read back by the shell.
 * Appearance is the Cabinet room: four rooms and eight knobs, with the
 * composition toggle and a reset that restores the room's own answers.
 */
export interface ConsoleSettingsInput {
  section: SettingsSectionId;
  layout: BigPictureLayout;
  setLayout: (layout: BigPictureLayout) => void;
  markLayoutTouched: () => void;
  room: ConsoleRoomSettings;
  setRoom: (room: ConsoleRoomSettings) => void;
  /** A validation-ready artwork accent, or null when the cover has none. */
  artworkAccent: string | null;
  theme: ThemeSelection;
  resolved: "dark" | "light";
  setTheme: (theme: ThemeSelection) => void;
  prefs: BigPictureControllerPrefs;
  updatePrefs: (patch: Partial<BigPictureControllerPrefs>) => void;
  deviceOnly: boolean;
  setDeviceOnly: (value: boolean) => void;
  padConnected: boolean;
  padFamilyName: string;
  hapticsActive: boolean;
  activeProfile: ControllerProfile;
  emit: (cue: CueId | null) => void;
  pulse: (cue: CueId) => void;
  /** Opens the controller profile sheet, owned by the shell. */
  openProfiles: () => void;
  /** Leaves for the desktop remote-play page. */
  openRemotePlay: () => void;
}

export interface ConsoleSettingsModel {
  rows: SettingsRow[];
  activate: (index: number) => void;
  adjust: (index: number, delta: -1 | 1) => void;
  /** The code of the account/device language, for the shell's own header. */
  localeLabel: string;
}

export function useConsoleSettings(input: ConsoleSettingsInput): ConsoleSettingsModel {
  const { user, updateLocale } = useAuth();
  const { locale, deviceOverride, setLocale, clearDeviceOverride } = useI18n();
  const {
    section,
    layout,
    setLayout,
    markLayoutTouched,
    room,
    setRoom,
    artworkAccent,
    theme,
    resolved,
    setTheme,
    prefs,
    updatePrefs,
    deviceOnly,
    setDeviceOnly,
    padConnected,
    padFamilyName,
    hapticsActive,
    activeProfile,
    emit,
    pulse,
    openProfiles,
    openRemotePlay,
  } = input;

  const localeChoices = useMemo(() => selectableLocales(), []);

  const cycleLocale = async (delta: -1 | 1) => {
    const current = Math.max(0, localeChoices.findIndex((entry) => entry.code === locale));
    const next = localeChoices[wrapIndex(current, delta, localeChoices.length)];
    if (!next) return;
    try {
      if (!user || deviceOnly) await setLocale(next.code);
      else await updateLocale(next.code);
    } catch {
      /* the shell keeps the current language and says nothing new */
    }
  };

  const applyAccountDefault = async () => {
    try {
      await clearDeviceOverride();
      setDeviceOnly(false);
    } catch {
      /* the device override stays; nothing else changes */
    }
  };

  const themeChoices = ["system", "dark", "light"] as const;
  const themeChoice = theme === "system" ? "system" : resolved === "dark" ? "dark" : "light";
  const cycleTheme = (delta: -1 | 1) => {
    const next = themeChoices[wrapIndex(themeChoices.indexOf(themeChoice), delta, themeChoices.length)];
    setTheme(next === "system" ? "system" : next === "dark" ? "midnight" : "light");
  };

  const cycleLayout = (delta: -1 | 1) => {
    const next = BIG_PICTURE_LAYOUTS[wrapIndex(BIG_PICTURE_LAYOUTS.indexOf(layout), delta, BIG_PICTURE_LAYOUTS.length)];
    if (!next || next === layout) return;
    setLayout(next);
    markLayoutTouched();
  };

  const rows: SettingsRow[] = (() => {
    if (section === "language") {
      const entries: SettingsRow[] = [
        {
          id: "language",
          label: t("Language"),
          detail: t("Applies to your account on every device. A device override stays on this device."),
          value: localeMeta(locale).label,
          kind: "choice",
        },
        {
          id: "device-only",
          label: t("Only this device"),
          detail: t("Changes apply immediately."),
          value: deviceOnly ? t("On") : t("Off"),
          kind: "toggle",
        },
      ];
      if (deviceOverride) {
        entries.push({
          id: "account-default",
          label: t("Use account default"),
          detail: t("Applies to your account on every device. A device override stays on this device."),
          value: t("Select"),
          kind: "action",
        });
      }
      return entries;
    }
    if (section === "appearance") {
      const themeValue =
        themeChoice === "system" ? t("Follow the system") : themeChoice === "dark" ? t("Dark") : t("Light");
      const accentNote =
        room.accent === "artwork" && artworkAccent !== null
          ? artworkAccentNote(true)
          : room.accent === "artwork"
            ? artworkAccentNote(false)
            : null;
      return [
        {
          id: "layout",
          label: t("Composition"),
          detail: layoutDescription(layout),
          value: layoutLabel(layout),
          kind: "choice",
        },
        {
          id: "knob-room",
          label: t("Room"),
          detail: consoleRoomDescription(room.room),
          value: consoleRoomLabel(room.room),
          kind: "choice",
        },
        {
          id: "knob-accent",
          label: t("Accent"),
          detail: accentNote ?? consoleKnobDescription("accent"),
          value: consoleAccentLabel(room.accent),
          kind: "choice",
        },
        {
          id: "knob-wall",
          label: t("Wall"),
          detail: consoleKnobDescription("wall"),
          value: consoleWallLabel(room.wall),
          kind: "choice",
        },
        {
          id: "knob-corners",
          label: t("Corners"),
          detail: consoleKnobDescription("corners"),
          value: consoleCornersLabel(room.corners),
          kind: "choice",
        },
        {
          id: "knob-density",
          label: t("Density"),
          detail: consoleKnobDescription("density"),
          value: consoleDensityLabel(room.density),
          kind: "choice",
        },
        {
          id: "knob-motion",
          label: t("Motion"),
          detail: consoleKnobDescription("motion"),
          value: consoleMotionLabel(room.motion),
          kind: "choice",
        },
        {
          id: "knob-sound",
          label: t("Sound"),
          detail: consoleKnobDescription("sound"),
          value: consoleSoundLabel(room.sound),
          kind: "choice",
        },
        {
          id: "knob-haptics",
          label: t("Haptics"),
          detail: consoleKnobDescription("haptics"),
          value: consoleHapticsLabel(room.haptics),
          kind: "choice",
        },
        {
          id: "theme",
          label: t("Theme"),
          detail: t("Theme applies to this browser; language is saved to your account."),
          value: themeValue,
          kind: "choice",
        },
        {
          id: "console-reset",
          label: t("Reset section"),
          detail: t("Back to the room's own wall, corners, density, motion, sound and haptics."),
          value: t("Reset"),
          kind: "action",
        },
      ];
    }
    if (section === "controllers") {
      return [
        {
          id: "profile",
          label: t("Controller profile"),
          detail: padConnected ? `${padFamilyName} · ${activeProfile.name}` : t("No controller detected."),
          value: activeProfile.name,
          kind: "action",
        },
        {
          id: "vibration",
          label: t("Vibration"),
          detail: t("Switch the pad setup and vibration."),
          value: room.haptics !== "off" ? t("On") : t("Off"),
          kind: "toggle",
        },
        {
          id: "haptics-strength",
          label: t("Haptics strength"),
          detail: t("Standard or strong rumble on this device. Off silences the pad."),
          value: hapticsStrengthLabel(room.haptics),
          kind: "choice",
        },
        {
          id: "sound",
          label: t("UI sounds"),
          detail: t("Short cues for movement, confirmations and launches. Silent while a game runs."),
          value: room.sound !== "off" ? t("On") : t("Off"),
          kind: "toggle",
        },
        {
          id: "sound-volume",
          label: t("Volume"),
          detail: t("Applies to this device. 0 % is silent even when the cues are on."),
          value: `${Math.round(clampVolume(prefs.sound.volume) * 100)}%`,
          kind: "choice",
        },
        {
          id: "sound-test",
          label: t("Test sound"),
          detail: t("Plays the confirm cue."),
          value: t("Play"),
          kind: "action",
          disabled: room.sound === "off" || clampVolume(prefs.sound.volume) <= 0,
          reason: room.sound === "off" ? t("UI sounds are off.") : t("Volume is at 0 %."),
        },
        {
          id: "haptics-test",
          label: t("Test pulse"),
          detail: t("Rumbles once on the connected controller."),
          value: t("Play"),
          kind: "action",
          disabled: !hapticsActive || !padConnected,
          reason: padConnected ? t("Haptics are off.") : t("No controller detected."),
        },
      ];
    }
    return [
      {
        id: "remote-play",
        label: t("Remote play"),
        detail: t(
          "Publish the Ludex library as Sunshine apps and start streams from Moonlight. Ludex never bundles either application.",
        ),
        value: t("Set up remote play"),
        kind: "action",
      },
    ];
  })();

  const activate = (index: number) => {
    const row = rows[index];
    if (!row) return;
    if (row.disabled) {
      // #229: a disabled row still answers the press, with the dry chrome click
      // instead of silence — dead controls are how a console feels broken.
      emit("click");
      return;
    }
    switch (row.id) {
      case "language":
        emit("confirm");
        void cycleLocale(1);
        return;
      case "device-only":
        emit("confirm");
        setDeviceOnly(!deviceOnly);
        return;
      case "account-default":
        emit("confirm");
        void applyAccountDefault();
        return;
      case "layout":
        emit("confirm");
        cycleLayout(1);
        return;
      case "knob-room":
        emit("confirm");
        setRoom(withConsoleRoom(room, cycleChoice(KNOB_VALUES.room, room.room, 1)));
        return;
      case "knob-accent":
        emit("confirm");
        setRoom({ ...room, accent: cycleChoice(KNOB_VALUES.accent, room.accent, 1) });
        return;
      case "knob-wall":
        emit("confirm");
        setRoom({ ...room, wall: cycleChoice(KNOB_VALUES.wall, room.wall, 1) });
        return;
      case "knob-corners":
        emit("confirm");
        setRoom({ ...room, corners: cycleChoice(KNOB_VALUES.corners, room.corners, 1) });
        return;
      case "knob-density":
        emit("confirm");
        setRoom({ ...room, density: cycleChoice(KNOB_VALUES.density, room.density, 1) });
        return;
      case "knob-motion":
        emit("confirm");
        setRoom({ ...room, motion: cycleChoice(KNOB_VALUES.motion, room.motion, 1) });
        return;
      case "knob-sound":
        emit("confirm");
        setRoom({ ...room, sound: cycleChoice(KNOB_VALUES.sound, room.sound, 1) });
        return;
      case "knob-haptics":
        emit("confirm");
        setRoom({ ...room, haptics: cycleChoice(KNOB_VALUES.haptics, room.haptics, 1) });
        return;
      case "console-reset":
        emit("confirm");
        setRoom(resetConsoleRoom(room));
        return;
      case "theme":
        emit("confirm");
        cycleTheme(1);
        return;
      case "profile":
        openProfiles();
        return;
      case "vibration": {
        emit("confirm");
        setRoom({ ...room, haptics: room.haptics === "off" ? "standard" : "off" });
        return;
      }
      case "haptics-strength": {
        emit("confirm");
        setRoom({ ...room, haptics: cycleChoice(KNOB_VALUES.haptics, room.haptics, 1) });
        return;
      }
      case "sound": {
        // The sound knob owns the on/off and the reduced set; the cue plays
        // after the new preference is applied, so muting is immediate.
        const next = cycleChoice(KNOB_VALUES.sound, room.sound, 1);
        uiSound.setPrefs({ ...prefs.sound, enabled: next !== "off" });
        emit("confirm");
        setRoom({ ...room, sound: next });
        return;
      }
      case "sound-volume": {
        const next = { ...prefs.sound, volume: stepVolume(prefs.sound.volume, 1) };
        uiSound.setPrefs(next);
        emit("confirm");
        updatePrefs({ sound: next });
        return;
      }
      case "sound-test":
        // The cue is the feedback; a pointer press must not rumble.
        uiSound.play("confirm");
        return;
      case "haptics-test":
        pulse("confirm");
        return;
      case "remote-play":
        emit("confirm");
        openRemotePlay();
        return;
    }
  };

  const adjust = (index: number, delta: -1 | 1) => {
    const row = rows[index];
    if (!row) return;
    if (row.disabled) {
      emit("click");
      return;
    }
    switch (row.id) {
      case "language":
        emit("move");
        void cycleLocale(delta);
        return;
      case "layout":
        emit("move");
        cycleLayout(delta);
        return;
      case "knob-room":
        emit("move");
        setRoom(withConsoleRoom(room, cycleChoice(KNOB_VALUES.room, room.room, delta)));
        return;
      case "knob-accent":
        emit("move");
        setRoom({ ...room, accent: cycleChoice(KNOB_VALUES.accent, room.accent, delta) });
        return;
      case "knob-wall":
        emit("move");
        setRoom({ ...room, wall: cycleChoice(KNOB_VALUES.wall, room.wall, delta) });
        return;
      case "knob-corners":
        emit("move");
        setRoom({ ...room, corners: cycleChoice(KNOB_VALUES.corners, room.corners, delta) });
        return;
      case "knob-density":
        emit("move");
        setRoom({ ...room, density: cycleChoice(KNOB_VALUES.density, room.density, delta) });
        return;
      case "knob-motion":
        emit("move");
        setRoom({ ...room, motion: cycleChoice(KNOB_VALUES.motion, room.motion, delta) });
        return;
      case "knob-sound":
        emit("move");
        setRoom({ ...room, sound: cycleChoice(KNOB_VALUES.sound, room.sound, delta) });
        return;
      case "knob-haptics":
        emit("move");
        setRoom({ ...room, haptics: cycleChoice(KNOB_VALUES.haptics, room.haptics, delta) });
        return;
      case "theme":
        emit("move");
        cycleTheme(delta);
        return;
      case "sound-volume": {
        const next = { ...prefs.sound, volume: stepVolume(prefs.sound.volume, delta) };
        uiSound.setPrefs({ ...next, enabled: room.sound !== "off" });
        emit("move");
        updatePrefs({ sound: next });
        return;
      }
      case "haptics-strength": {
        emit("move");
        setRoom({ ...room, haptics: cycleChoice(KNOB_VALUES.haptics, room.haptics, delta) });
        return;
      }
      default:
        return;
    }
  };

  return { rows, activate, adjust, localeLabel: localeMeta(locale).label };
}
