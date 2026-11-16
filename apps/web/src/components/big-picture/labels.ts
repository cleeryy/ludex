import { t } from "@/lib/i18n";
import type {
  ConsoleAccentSource,
  ConsoleCorners,
  ConsoleDensity,
  ConsoleHaptics,
  ConsoleMotion,
  ConsoleRoom,
  ConsoleSound,
  ConsoleWall,
} from "@/theme/console-room";
import type { BigPictureLayout } from "./layout";

/* --------------------------------------------------------------------------
   Console copy (#268). Resolved at render time, never at module load: a locale
   change remounts the tree and must re-translate these.

   The rules: sentence case for every word, no preset or implementation name in
   user copy, no reference console named, and one description that tells the
   player what the choice does rather than how it works.
   -------------------------------------------------------------------------- */

export function layoutLabel(layout: BigPictureLayout): string {
  switch (layout) {
    case "shelf":
      return t("Stage");
    case "grid":
      return t("Wall");
    case "compact":
      return t("List");
  }
}

export function layoutDescription(layout: BigPictureLayout): string {
  switch (layout) {
    case "shelf":
      return t("The selected game on the stage, with one rail of covers underneath.");
    case "grid":
      return t("The library as a wall of covers, filtered by shelf.");
    case "compact":
      return t("One line per game, for a large library or a small screen.");
  }
}

/* --------------------------------- rooms --------------------------------- */

export function consoleRoomLabel(room: ConsoleRoom): string {
  switch (room) {
    case "night":
      return t("Night");
    case "projector":
      return t("Projector");
    case "daylight":
      return t("Daylight");
    case "arcade":
      return t("Arcade");
  }
}

export function consoleRoomDescription(room: ConsoleRoom): string {
  switch (room) {
    case "night":
      return t("Warm charcoal, one lit game and the artwork wall. The room for a dark living room.");
    case "projector":
      return t("Pure black panels, no blur and thicker lines, for a long throw and a bright room.");
    case "daylight":
      return t("Paper and ink, flat panels, no blur. The room for daytime.");
    case "arcade":
      return t("Hard edges and dense covers, for the fun of it. The palette stays the same.");
  }
}

/* --------------------------------- knobs --------------------------------- */

export function consoleAccentLabel(accent: ConsoleAccentSource): string {
  switch (accent) {
    case "theme":
      return t("Theme");
    case "signal":
      return t("Signal");
    case "artwork":
      return t("From artwork");
  }
}

export function consoleWallLabel(wall: ConsoleWall): string {
  switch (wall) {
    case "artwork":
      return t("Artwork");
    case "solid":
      return t("Solid");
    case "gradient":
      return t("Gradient");
  }
}

export function consoleCornersLabel(corners: ConsoleCorners): string {
  switch (corners) {
    case "sharp":
      return t("Sharp");
    case "soft":
      return t("Soft");
    case "round":
      return t("Round");
  }
}

export function consoleDensityLabel(density: ConsoleDensity): string {
  switch (density) {
    case "comfortable":
      return t("Comfortable");
    case "compact":
      return t("Compact");
    case "dense":
      return t("Dense");
  }
}

export function consoleMotionLabel(motion: ConsoleMotion): string {
  switch (motion) {
    case "full":
      return t("Full");
    case "calm":
      return t("Calm");
    case "off":
      return t("Off");
  }
}

export function consoleSoundLabel(sound: ConsoleSound): string {
  switch (sound) {
    case "full":
      return t("Full");
    case "reduced":
      return t("Reduced");
    case "off":
      return t("Off");
  }
}

export function consoleHapticsLabel(haptics: ConsoleHaptics): string {
  switch (haptics) {
    case "off":
      return t("Off");
    case "standard":
      return t("Standard");
    case "strong":
      return t("Strong");
  }
}

/** One-line explanation per knob; the room description carries the rest. */
export function consoleKnobDescription(knob: string): string {
  switch (knob) {
    case "room":
      return t("A room sets the wall, corners, density, motion, sound and haptics at once.");
    case "accent":
      return t("What paints the signal colour. From artwork reads the selected cover.");
    case "wall":
      return t("What sits behind the shell. Artwork blurs the selected game's backdrop once.");
    case "corners":
      return t("How round the covers, objects, panels and sheets are. Nothing passes 24.");
    case "density":
      return t("How many covers the rail shows. The type never shrinks.");
    case "motion":
      return t("Full keeps the springs. Calm is a short fade. Off lands instantly.");
    case "sound":
      return t("Full plays every cue. Reduced keeps the essentials. Off is silent.");
    case "haptics":
      return t("Pad rumble on the band, launch and errors. Focus never rumbles.");
    default:
      return "";
  }
}

/* ------------------------------- artwork accent --------------------------- */

export function artworkAccentNote(valid: boolean): string {
  return valid
    ? t("Using a colour from this cover.")
    : t("Not enough colour in this cover. Signal is used instead.");
}
