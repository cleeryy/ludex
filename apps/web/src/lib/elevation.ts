/**
 * Elevation prompt helpers (#192).
 *
 * A privileged action is a whitelisted operation plus a target and a reason;
 * the desktop app builds the fixed elevated command from it. The UI never sends
 * a command or an executable name. These helpers keep the operation copy in one
 * place so every panel explains the same thing.
 */
import { t } from "@/lib/i18n";

export type ElevationOperation =
  | "grantPathAcl"
  | "runInstaller"
  | "installService"
  | "installDriver"
  | "addFirewallRule"
  | "removeFirewallRule"
  | "writeConfigFile";

export interface ElevationPrompt {
  operation: ElevationOperation;
  /** Absolute path or rule the operation changes. */
  target: string;
  /** One sentence explaining why administrator rights are required. */
  reason: string;
}

/** Every whitelisted operation, in the order the dialog may show them. */
export const ELEVATION_OPERATIONS: ElevationOperation[] = [
  "grantPathAcl",
  "runInstaller",
  "installService",
  "installDriver",
  "addFirewallRule",
  "removeFirewallRule",
  "writeConfigFile",
];

/** Plain-language "what will change" line for the operation. */
export function elevationOperationLabel(operation: ElevationOperation): string {
  switch (operation) {
    case "grantPathAcl":
      return t("Grant modify rights on a folder");
    case "runInstaller":
      return t("Run a verified installer");
    case "installService":
      return t("Install a Windows service");
    case "installDriver":
      return t("Install a driver");
    case "addFirewallRule":
      return t("Add a firewall rule");
    case "removeFirewallRule":
      return t("Remove a firewall rule");
    case "writeConfigFile":
      return t("Write a configuration file");
    default:
      return t("Run a privileged operation");
  }
}

/**
 * Config folder shown in the elevation dialog: the parent of the detected
 * `apps.json`. Kept pure so Windows and POSIX separators are both tested.
 */
export function configFolderFromAppsPath(appsPath: string | null | undefined): string | null {
  if (!appsPath) return null;
  const trimmed = appsPath.replace(/[\\/]+$/, "");
  const separator = Math.max(trimmed.lastIndexOf("\\"), trimmed.lastIndexOf("/"));
  if (separator <= 0) return null;
  return trimmed.slice(0, separator);
}

/** `cancelled` means UAC was declined and nothing changed. */
export function isElevationCancelled(status: string | null | undefined): boolean {
  return status === "cancelled";
}
