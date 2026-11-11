import { useState, type ReactNode } from "react";
import { Link, useLocation } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ChevronRight, CircleHelp } from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import {
  isNavActive,
  navGroups,
  systemNav,
  type NavContext,
  type NavDestination,
} from "@/lib/navigation";
import { useDownloads } from "@/state/downloads";
import { useAuth } from "@/state/auth";
import { Badge } from "@/components/ui/controls";
import { NavIcon } from "@/components/shell/nav-icons";
import { navGroupLabel, navLabel } from "@/components/shell/nav-copy";

/** Which rail groups the user folded away, remembered per device. */
const COLLAPSE_KEY = "ludex.nav.collapsed";
const DEFAULT_COLLAPSED = ["admin"];

function readCollapsedGroups(): string[] {
  if (typeof window === "undefined") return [...DEFAULT_COLLAPSED];
  try {
    const raw = window.localStorage.getItem(COLLAPSE_KEY);
    if (!raw) return [...DEFAULT_COLLAPSED];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [...DEFAULT_COLLAPSED];
    return parsed.filter((entry): entry is string => typeof entry === "string");
  } catch {
    return [...DEFAULT_COLLAPSED];
  }
}

function storeCollapsedGroups(groups: string[]): void {
  try {
    window.localStorage.setItem(COLLAPSE_KEY, JSON.stringify(groups));
  } catch {
    /* private mode: the rail simply forgets the fold */
  }
}

/**
 * A labelled, foldable group. The `<ul>` is named by its label, and the
 * disclosure button owns `aria-expanded` / `aria-controls`, so the group is
 * legible to a screen reader with or without the chevron.
 */
function RailGroup({
  id,
  label,
  collapsed,
  active,
  onToggle,
  children,
}: {
  id: string;
  label: string;
  collapsed: boolean;
  active: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  const listId = `rail-group-${id}`;
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={!collapsed}
        aria-controls={listId}
        className="flex h-8 w-full items-center gap-1.5 px-5 text-[11.5px] text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight
          className={cn("size-3 transition-transform duration-150 motion-reduce:transition-none", !collapsed && "rotate-90")}
          aria-hidden="true"
        />
        <span>{label}</span>
        {active && collapsed && <span className="ms-auto me-4 size-1.5 rounded-full bg-signal" aria-hidden="true" />}
      </button>
      <ul id={listId} aria-label={label} hidden={collapsed} className="flex flex-col">
        {children}
      </ul>
    </div>
  );
}

function RailLink({ destination, count, kbd }: { destination: NavDestination; count?: number; kbd?: string }) {
  const { pathname } = useLocation();
  const active = isNavActive(destination, pathname);
  const label = navLabel(destination);
  return (
    <li>
      <Link
        to={destination.to}
        aria-current={active ? "page" : undefined}
        data-active={active}
        className="rail-link h-11 w-full"
      >
        <NavIcon name={destination.icon} className="size-4" />
        <span className="truncate">{label}</span>
        {typeof count === "number" && count > 0 && (
          <span className={cn("rail-count", destination.id === "downloads" && "text-signal-text")}>{count}</span>
        )}
        {kbd && !count && <span className="rail-count">{kbd}</span>}
      </Link>
    </li>
  );
}

/**
 * Desktop rail (#174): Library, Play, Community and Admin, each a labelled
 * group, plus the account-level destinations in the footer. Every entry is a
 * `<Link>` — Cmd/Ctrl-click and middle-click work — and the active state comes
 * from the same table as the mobile bar and the command palette.
 */
export function NavRail({ context, onOpenHelp }: { context: NavContext; onOpenHelp: () => void }) {
  const location = useLocation();
  const pathname = location.pathname;
  const { setup } = useAuth();
  const downloads = useDownloads();
  const { data: stats } = useQuery({ queryKey: ["stats"], queryFn: () => api.stats(), staleTime: 30_000 });
  const [collapsed, setCollapsed] = useState<string[]>(() => readCollapsedGroups());

  const toggle = (id: string) => {
    setCollapsed((current) => {
      const next = current.includes(id) ? current.filter((entry) => entry !== id) : [...current, id];
      storeCollapsedGroups(next);
      return next;
    });
  };

  const groups = navGroups(context);
  const footerLinks = systemNav(context);
  const counts: Record<string, number | undefined> = {
    library: stats?.games,
    downloads: downloads.activeCount,
  };

  return (
    <div className="flex h-full flex-col">
      <nav aria-label={t("Main navigation")} className="flex flex-1 flex-col overflow-y-auto py-4">
        {groups.map((entry, index) => (
          <div key={entry.group.id} className={index === 0 ? "" : "mt-2 border-t border-line pt-1"}>
            <RailGroup
              id={entry.group.id}
              label={navGroupLabel(entry.group.id)}
              collapsed={collapsed.includes(entry.group.id)}
              active={entry.items.some((destination) => isNavActive(destination, pathname))}
              onToggle={() => toggle(entry.group.id)}
            >
              {entry.items.map((destination) => (
                <RailLink
                  key={destination.id}
                  destination={destination}
                  count={counts[destination.id]}
                  kbd={destination.kbd}
                />
              ))}
            </RailGroup>
          </div>
        ))}

      </nav>

      <div className="border-t border-line py-3">
        <ul className="flex flex-col" aria-label={t("Settings and help")}>
          {footerLinks.map((destination) => (
            <RailLink key={destination.id} destination={destination} kbd={destination.kbd} />
          ))}
          <li>
            <button type="button" onClick={onOpenHelp} className="rail-link h-11 w-full">
              <CircleHelp className="size-4" aria-hidden="true" />
              <span className="truncate">{t("Help")}</span>
              <span className="rail-count">?</span>
            </button>
          </li>
        </ul>

        <div className="mt-1 flex items-center gap-2 px-5 text-[12px] text-ink-3">
          <span className="size-1.5 shrink-0 rounded-full bg-ok" aria-hidden="true" />
          <span className="truncate">{setup?.serverName ?? "Ludex"}</span>
          <span className="num shrink-0">{`v${setup?.version ?? "0.1.0"}`}</span>
          {USING_MOCKS && (
            <Badge tone="outline" className="ms-auto shrink-0">
              {t("Demo")}
            </Badge>
          )}
        </div>
      </div>
    </div>
  );
}
