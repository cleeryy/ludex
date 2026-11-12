import { t } from "@/lib/i18n";
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
