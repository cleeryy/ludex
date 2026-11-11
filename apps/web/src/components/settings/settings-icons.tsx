import {
  Cast,
  CircleHelp,
  CloudUpload,
  Download,
  Gamepad2,
  HardDrive,
  Languages,
  Palette,
  Plug,
  RefreshCw,
  UserRound,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import type { SettingsIconName } from "@/lib/settings-sections";

/** Same contract as the navigation icons: names in the registry, JSX here. */
const ICONS: Record<SettingsIconName, LucideIcon> = {
  account: UserRound,
  appearance: Palette,
  language: Languages,
  storage: HardDrive,
  downloads: Download,
  saves: CloudUpload,
  streaming: Cast,
  compatibility: Wrench,
  integrations: Plug,
  profiles: Gamepad2,
  updates: RefreshCw,
  help: CircleHelp,
};

export function SettingsIcon({
  name,
  className = "size-4",
}: {
  name: SettingsIconName;
  className?: string;
}) {
  const Icon = ICONS[name];
  return <Icon className={className} aria-hidden="true" />;
}
