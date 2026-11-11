import { t } from "@/lib/i18n";
import type { SettingsSectionId } from "@/lib/settings-sections";

/**
 * Translatable copy for the settings taxonomy, for the same reason as the
 * navigation labels: literal `t()` calls the extractor and the compiler can
 * check, resolved at render time so a language change applies immediately.
 */
export function settingsSectionLabel(id: SettingsSectionId): string {
  switch (id) {
    case "account":
      return t("Account");
    case "appearance":
      return t("Appearance");
    case "language":
      return t("Language");
    case "storage":
      return t("Storage");
    case "downloads":
      return t("Downloads");
    case "saves":
      return t("Saves and cloud");
    case "streaming":
      return t("Streaming");
    case "compatibility":
      return t("Compatibility");
    case "integrations":
      return t("Integrations");
    case "profiles":
      return t("Profiles");
    case "updates":
      return t("Updates");
    case "help":
      return t("Help and about");
  }
}

export function settingsSectionDescription(id: SettingsSectionId): string {
  switch (id) {
    case "account":
      return t("Your sign-in, your password and the profile other members can see.");
    case "appearance":
      return t("Themes, accent colour and the shape of the window on this device.");
    case "language":
      return t("The language of the interface. Community translations fall back to English.");
    case "storage":
      return t("Where games are installed, what this machine already has, and the folders Ludex can link.");
    case "downloads":
      return t("How archives arrive, where they are kept, and how quiet installers run.");
    case "saves":
      return t("Back up save folders to your server and teach Ludex where each game keeps its saves.");
    case "streaming":
      return t("Sunshine and Moonlight: publish the library and start a stream.");
    case "compatibility":
      return t("Steam, Proton, UMU and Wine detection for Linux launches.");
    case "integrations":
      return t("Import games from other launchers, talk to Discord, and keep several servers at hand.");
    case "profiles":
      return t("Controller mapping and the emulator commands used to launch ROMs.");
    case "updates":
      return t("Which build you run, how it updates itself, and when it checks.");
    case "help":
      return t("Version, the first-run checklist and the keyboard shortcuts.");
  }
}
