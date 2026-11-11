import { t } from "@/lib/i18n";
import type { BigPictureLayout } from "./layout";

/* --------------------------------------------------------------------------
   Copy for the four layouts (#141). Resolved at render time, never at module
   load: a locale change remounts the tree and must re-translate these.
   -------------------------------------------------------------------------- */

export function layoutLabel(layout: BigPictureLayout): string {
  switch (layout) {
    case "shelf":
      return t("Shelf");
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
    case "hero":
      return t("A full-screen backdrop, a big title and one centred rail.");
    case "grid":
      return t("Many covers at once, under a focused header. The fastest way to browse a big library.");
    case "compact":
      return t("Dense rows with a details pane, for large libraries and quick stepping.");
  }
}
