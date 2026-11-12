import {
  Activity,
  Archive,
  Bookmark,
  Clock,
  Download,
  Film,
  Gamepad2,
  House,
  KeyRound,
  Library,
  ListPlus,
  MailPlus,
  PlugZap,
  RefreshCw,
  Server,
  Settings,
  UploadCloud,
  Users,
  type LucideIcon,
} from "lucide-react";
import type { NavIconName } from "@/lib/navigation";

/**
 * Icon per navigation destination. The registry stores names, not components,
 * so `lib/navigation.ts` stays free of JSX and can be tested on its own.
 */
const ICONS: Record<NavIconName, LucideIcon> = {
  home: House,
  library: Library,
  clock: Clock,
  bookmark: Bookmark,
  collections: ListPlus,
  download: Download,
  gamepad: Gamepad2,
  refresh: RefreshCw,
  community: Users,
  news: Film,
  users: Users,
  invitations: MailPlus,
  ingest: Download,
  upload: UploadCloud,
  health: Activity,
  metadata: PlugZap,
  authentication: KeyRound,
  server: Server,
  activity: Activity,
  backup: Archive,
  settings: Settings,
};

export function NavIcon({ name, className = "size-4" }: { name: NavIconName; className?: string }) {
  const Icon = ICONS[name];
  return <Icon className={className} aria-hidden="true" />;
}
