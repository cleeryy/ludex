import { useState, type ReactNode } from "react";
import { Link, useLocation } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ChevronRight, CircleHelp, Download } from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { cn } from "@/lib/cn";
import { formatPercent, formatSpeed } from "@/lib/format";
import { t } from "@/lib/i18n";
import {
  isNavActive,
  navGroups,
  systemNav,
  type NavContext,
  type NavDestination,
} from "@/lib/navigation";
import { useActiveCount, useActiveJobs, useQueueSummary, useTitleFor } from "@/state/downloads";
import { useAuth } from "@/state/auth";
import { Badge, Led, SegmentedGauge } from "@/components/ui/controls";
import { NavIcon } from "@/components/shell/nav-icons";
import { navGroupLabel, navLabel } from "@/components/shell/nav-copy";
import { qk } from "@/lib/query-keys";

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
        className="flex h-8 w-full items-center gap-1.5 px-5 text-meta font-medium text-ink-3 transition-colors hover:text-ink-2"
      >
        <ChevronRight
          className={cn("size-3 transition-transform duration-150 motion-reduce:transition-none", !collapsed && "rotate-90")}
          aria-hidden="true"
        />
        <span>{label}</span>
        {active && collapsed && <Led tone="signal" className="ms-auto me-1" />}
      </button>
      <ul id={listId} aria-label={label} hidden={collapsed} className="flex flex-col gap-0.5">
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
        className="rail-link"
      >
        <NavIcon name={destination.icon} className="size-4 shrink-0" />
        <span className="truncate">{label}</span>
        {typeof count === "number" && count > 0 && (
          <span className={cn("rail-count", destination.id === "downloads" && "text-signal-text")}>{count}</span>
        )}
        {kbd && !count && <span className="rail-count">{kbd}</span>}
        {active && <Led tone="signal" className={count || kbd ? "ms-1.5" : "ms-auto"} />}
      </Link>
    </li>
  );
}

/**
 * Mini active-download widget anchored above the rail footer (#333): renders a
 * 16-segment dual-color gauge (`--signal` progress + `--info` live network head)
 * so transfers stay glanceable without cluttering the library canvas.
 */
function RailTransferWidget() {
  const active = useActiveJobs();
  const { activeCount, percent: aggregatePercent, speedBps } = useQueueSummary();
  const titleFor = useTitleFor();
  if (active.length === 0) return null;

  const job = active[0];
  const paused = job.phase === "paused";
  const jobPct = job.bytesTotal ? Math.min(100, (job.bytesDone / job.bytesTotal) * 100) : null;
  const pct = paused ? jobPct : aggregatePercent;

  return (
    <div className="mx-2.5 mb-2.5">
      <Link
        to="/downloads"
        className="flex flex-col gap-2 rounded-[var(--r-md)] border border-line bg-surface p-3 transition-colors hover:border-line-2 hover:bg-surface-2"
      >
        <div className="flex items-center gap-2">
          <Led tone={paused ? "warn" : "info"} pulse={!paused} />
          <Download className="size-3.5 shrink-0 text-ink-3" aria-hidden="true" />
          <span className="min-w-0 flex-1 truncate text-label font-medium text-ink">{titleFor(job)}</span>
          {activeCount > 1 && <span className="num text-micro text-ink-3">+{activeCount - 1}</span>}
        </div>
        <SegmentedGauge
          value={pct}
          segments={16}
          active={!paused}
          size="sm"
          label={t("Downloads: {title}, {percent} percent", {
            title: titleFor(job),
            percent: pct !== null ? pct.toFixed(0) : "",
          })}
        />
        <div className="flex items-baseline justify-between gap-2 text-micro text-ink-3">
          <span className="num text-ink-2">
            {paused ? t("Paused") : pct !== null ? formatPercent(pct) : t("Preparing")}
          </span>
          {speedBps ? <span className="num text-info">{formatSpeed(speedBps)}</span> : null}
        </div>
      </Link>
    </div>
  );
}

/**
 * Desktop rail (#174, #333): 236px sidebar on `bg-deep` with square 7px LEDs on
 * active states and a mini 16-segment active-download widget in the footer.
 */
export function NavRail({ context, onOpenHelp }: { context: NavContext; onOpenHelp: () => void }) {
  const location = useLocation();
  const pathname = location.pathname;
  const { setup } = useAuth();
  const activeDownloads = useActiveCount();
  const { data: stats } = useQuery({ queryKey: qk.stats, queryFn: () => api.stats(), staleTime: 30_000 });
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
    downloads: activeDownloads,
  };

  return (
    <div className="flex h-full flex-col bg-bg-deep">
      <nav aria-label={t("Main navigation")} className="flex flex-1 flex-col overflow-y-auto py-3">
        {groups.map((entry, index) => (
          <div key={entry.group.id} className={index === 0 ? "" : "mt-2 border-t border-line pt-1.5"}>
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

      <RailTransferWidget />

      <div className="border-t border-line py-2.5">
        <ul className="flex flex-col gap-0.5" aria-label={t("Settings and help")}>
          {footerLinks.map((destination) => (
            <RailLink key={destination.id} destination={destination} kbd={destination.kbd} />
          ))}
          <li>
            <button type="button" onClick={onOpenHelp} className="rail-link w-[calc(100%-20px)]">
              <CircleHelp className="size-4 shrink-0" aria-hidden="true" />
              <span className="truncate">{t("Help")}</span>
              <span className="rail-count">?</span>
            </button>
          </li>
        </ul>

        <div className="mt-2 flex items-center gap-2 px-5 text-meta text-ink-3">
          <Led tone="ok" />
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
