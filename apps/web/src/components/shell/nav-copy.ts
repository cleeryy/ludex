import { t } from "@/lib/i18n";
import type { NavDestination, NavGroupId } from "@/lib/navigation";

/**
 * Translatable copy for the navigation table.
 *
 * `lib/navigation.ts` stays pure (ids, routes, groups) and the message ids are
 * resolved here, where each `t()` call is a literal: the catalog extractor can
 * see them, the compiler checks them, and switching language re-renders the
 * labels instead of freezing the language the app booted in.
 */
export function navLabel(destination: NavDestination): string {
  switch (destination.id) {
    case "home":
      return t("Home");
    case "library":
      return t("Library");
    case "recent":
      return t("Recently played");
    case "bookmarks":
      return t("Bookmarks");
    case "collections":
      return t("Collections");
    case "downloads":
      return t("Downloads");
    case "bigpicture":
      return t("Big Picture mode");
    case "updates":
      return t("Updates");
    case "community":
      return t("Community");
    case "news":
      return t("News");
    case "ingest":
      return t("Ingest");
    case "upload":
      return t("Upload game");
    case "scan":
      return t("Library scan");
    case "users":
      return t("Users");
    case "news-editor":
      return t("News editor");
    case "metadata":
      return t("Metadata providers");
    case "authentication":
      return t("Authentication");
    case "server":
      return t("Server");
    case "settings":
      return t("Settings");
    default:
      return destination.labelKey;
  }
}

export function navGroupLabel(group: NavGroupId): string {
  switch (group) {
    case "library":
      return t("Library");
    case "play":
      return t("Play");
    case "community":
      return t("Community");
    case "admin":
      return t("Admin");
    case "system":
      return t("Settings");
  }
}
