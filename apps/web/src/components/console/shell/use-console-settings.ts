import { useMemo } from "react";
import { localeMeta, selectableLocales, t } from "@/lib/i18n";
import { uiSound } from "@/lib/ui-sound";
import { clampVolume, stepVolume, type CueId } from "@/lib/ui-sound-specs";
import { useAuth } from "@/state/auth";
import { useI18n } from "@/lib/i18n";
import type { SettingsRow } from "@/components/big-picture/settings-app";
import { cycleHapticsStrength, hapticsStrengthLabel, type BigPictureControllerPrefs } from "@/components/big-picture/controller";
import {
  consoleAccentLabel,
  consoleBackgroundLabel,
  consolePresetDescription,
  consolePresetLabel,
  consoleRadiusLabel,
  layoutLabel,
} from "@/components/big-picture/labels";
import { wrapIndex, type SettingsSectionId } from "@/components/big-picture/system-zones";
import { BIG_PICTURE_LAYOUTS, type BigPictureLayout } from "@/components/big-picture/layout";
import {
  CONSOLE_ACCENT_IDS,
  CONSOLE_BACKGROUNDS,
  CONSOLE_PRESETS,
  CONSOLE_RADII,
  DEFAULT_CONSOLE_APPEARANCE,
  cycleChoice,
  withConsolePreset,
  type ConsoleAppearance,
} from "@/theme/console-appearance";
import type { ControllerProfile } from "@/lib/types";
import type { ThemeSelection } from "@/theme/types";

/**
 * The Console settings app's model (#267).
 *
 * Four rooms — language, appearance, controllers, streaming — displayed as
 * eight knobs each in the shell, no Apply: every change lands immediately and
 * is read back by the shell. The logic used to be three hundred lines inside
 * the screen; it is the same logic, with the devices and the appearance store
 * injected so it can be tested and read on its own.
 */
export interface ConsoleSettingsInput {
  section: SettingsSectionId;
  layout: BigPictureLayout;
  setLayout: (layout: BigPictureLayout) => void;
  markLayoutTouched: () => void;
  appearance: ConsoleAppearance;
  setAppearance: (appearance: ConsoleAppearance) => void;
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
    appearance,
    setAppearance,
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
      return [
        {
          id: "layout",
          label: t("Layout"),
          detail: t("Change how the library is presented."),
          value: layoutLabel(layout),
          kind: "choice",
        },
        {
          id: "console-preset",
          label: t("Console style"),
          detail: consolePresetDescription(appearance.preset),
          value: consolePresetLabel(appearance.preset),
          kind: "choice",
        },
        {
          id: "console-accent",
          label: t("Accent"),
          detail: t("A curated console accent. Theme accent follows the appearance settings."),
          value: consoleAccentLabel(appearance.accent),
          kind: "choice",
        },
        {
          id: "console-background",
          label: t("Background"),
          detail: t("What the console floats over. Artwork keeps the selected game behind the chrome."),
          value: consoleBackgroundLabel(appearance.background),
          kind: "choice",
        },
        {
          id: "console-radius",
          label: t("Corners"),
          detail: t("How round the tiles, panels and sheets are. Sharp keeps the archive's 2 px edge."),
          value: consoleRadiusLabel(appearance.radius),
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
          label: t("Reset console appearance"),
          detail: t("Back to Studio, artwork, theme accent and sharp corners."),
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
          value: prefs.haptics ? t("On") : t("Off"),
          kind: "toggle",
        },
        {
          id: "haptics-strength",
          label: t("Haptics strength"),
          detail: t("Standard or strong rumble on this device. Off silences the pad."),
          value: hapticsStrengthLabel(prefs.hapticsStrength),
          kind: "choice",
        },
        {
          id: "sound",
          label: t("UI sounds"),
          detail: t("Short cues for movement, confirmations and launches. Silent while a game runs."),
          value: prefs.sound.enabled ? t("On") : t("Off"),
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
          disabled: !prefs.sound.enabled || clampVolume(prefs.sound.volume) <= 0,
          reason: prefs.sound.enabled ? t("Volume is at 0 %.") : t("UI sounds are off."),
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
      case "console-preset":
        emit("confirm");
        setAppearance(withConsolePreset(appearance, cycleChoice(CONSOLE_PRESETS, appearance.preset, 1)));
        return;
      case "console-accent":
        emit("confirm");
        setAppearance({ ...appearance, accent: cycleChoice(CONSOLE_ACCENT_IDS, appearance.accent, 1) });
        return;
      case "console-background":
        emit("confirm");
        setAppearance({ ...appearance, background: cycleChoice(CONSOLE_BACKGROUNDS, appearance.background, 1) });
        return;
      case "console-radius":
        emit("confirm");
        setAppearance({ ...appearance, radius: cycleChoice(CONSOLE_RADII, appearance.radius, 1) });
        return;
      case "console-reset":
        emit("confirm");
        setAppearance(DEFAULT_CONSOLE_APPEARANCE);
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
        const next = !prefs.haptics;
        updatePrefs({
          haptics: next,
          hapticsStrength: next && prefs.hapticsStrength === "off" ? "standard" : prefs.hapticsStrength,
        });
        return;
      }
      case "haptics-strength": {
        emit("confirm");
        const next = cycleHapticsStrength(prefs.hapticsStrength, 1);
        updatePrefs({ hapticsStrength: next, haptics: next !== "off" });
        return;
      }
      case "sound": {
        const next = { ...prefs.sound, enabled: !prefs.sound.enabled };
        // Apply before the cue: turning sound on should click once, turning it
        // off should go quiet immediately.
        uiSound.setPrefs(next);
        emit("confirm");
        updatePrefs({ sound: next });
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
      case "console-preset":
        emit("move");
        setAppearance(withConsolePreset(appearance, cycleChoice(CONSOLE_PRESETS, appearance.preset, delta)));
        return;
      case "console-accent":
        emit("move");
        setAppearance({ ...appearance, accent: cycleChoice(CONSOLE_ACCENT_IDS, appearance.accent, delta) });
        return;
      case "console-background":
        emit("move");
        setAppearance({ ...appearance, background: cycleChoice(CONSOLE_BACKGROUNDS, appearance.background, delta) });
        return;
      case "console-radius":
        emit("move");
        setAppearance({ ...appearance, radius: cycleChoice(CONSOLE_RADII, appearance.radius, delta) });
        return;
      case "theme":
        emit("move");
        cycleTheme(delta);
        return;
      case "sound-volume": {
        const next = { ...prefs.sound, volume: stepVolume(prefs.sound.volume, delta) };
        uiSound.setPrefs(next);
        emit("move");
        updatePrefs({ sound: next });
        return;
      }
      case "haptics-strength": {
        emit("move");
        const next = cycleHapticsStrength(prefs.hapticsStrength, delta);
        updatePrefs({ hapticsStrength: next, haptics: next !== "off" });
        return;
      }
      default:
        return;
    }
  };

  return { rows, activate, adjust, localeLabel: localeMeta(locale).label };
}
