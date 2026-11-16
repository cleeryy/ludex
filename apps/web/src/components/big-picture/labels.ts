import { t } from "@/lib/i18n";
import type {
  ConsoleAccentId,
  ConsoleBackground,
  ConsolePreset,
  ConsoleRadius,
} from "@/theme/console-appearance";
import type { BigPictureLayout } from "./layout";

/* --------------------------------------------------------------------------
   Copy for the six layouts (#141, #197). Resolved at render time, never at
   module load: a locale change remounts the tree and must re-translate these.
   No reference console is named here: the labels describe the composition.
   -------------------------------------------------------------------------- */

export function layoutLabel(layout: BigPictureLayout): string {
  switch (layout) {
    case "shelf":
      return t("Shelf");
    case "dock":
      return t("Dock");
    case "spotlight":
      return t("Spotlight");
    case "hero":
      return t("Hero");
    case "grid":
      return t("Grid");
    case "compact":
      return t("Compact");
  }
}

export function layoutDescription(layout: BigPictureLayout): string {
  switch (layout) {
    case "shelf":
      return t("One shelf at a time: a large plate, its title and a caption rail.");
    case "dock":
      return t("One large row of games over the system dock. The selected tile leads the screen.");
    case "spotlight":
      return t("The selected game takes over the backdrop, with quick actions and a row underneath.");
    case "hero":
      return t("A full-screen backdrop, a big title and one centred rail.");
    case "grid":
      return t("Many covers at once, under a focused header. The fastest way to browse a big library.");
    case "compact":
      return t("Dense rows with a details pane, for large libraries and quick stepping.");
  }
}

/* ------------------------- console appearance (#229) ------------------------ */

export function consolePresetLabel(preset: ConsolePreset): string {
  return preset === "soft" ? t("Soft") : t("Studio");
}

export function consolePresetDescription(preset: ConsolePreset): string {
  return preset === "soft"
    ? t("Soft corners and raised plates over the game's wallpaper. Colours still come from the app theme.")
    : t("The original archive look: crisp hairlines, flat panels and the app theme.");
}

export function consoleAccentLabel(accent: ConsoleAccentId): string {
  switch (accent) {
    case "theme":
      return t("Theme accent");
    case "coral":
      return t("Coral");
    case "mint":
      return t("Mint");
    case "sky":
      return t("Sky");
    case "lilac":
      return t("Lilac");
    case "sun":
      return t("Sun");
  }
}

export function consoleBackgroundLabel(background: ConsoleBackground): string {
  switch (background) {
    case "artwork":
      return t("Artwork");
    case "solid":
      return t("Solid");
    case "gradient":
      return t("Soft gradient");
  }
}

export function consoleRadiusLabel(radius: ConsoleRadius): string {
  switch (radius) {
    case "soft":
      return t("Soft");
    case "round":
      return t("Round");
    case "sharp":
      return t("Sharp");
  }
}
