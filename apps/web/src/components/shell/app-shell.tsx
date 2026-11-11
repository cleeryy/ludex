import { useEffect, useRef, useState } from "react";
import { Link, Outlet, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowUpDown,
  Bookmark,
  ChevronDown,
  ChevronRight,
  Clock,
  Dice5,
  Download,
  Film,
  HardDrive,
  House,
  KeyRound,
  ListOrdered,
  ListPlus,
  PlugZap,
  LogOut,
  Moon,
  MoreHorizontal,
  RefreshCw,
  Search,
  Server,
  Settings,
  ShieldCheck,
  Sun,
  UploadCloud,
  Users,
  Library as LibraryIcon,
  Gamepad2,
} from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { cn } from "@/lib/cn";
import { useAuth, useIsAdmin, useIsEditor } from "@/state/auth";
import type { ProgressState } from "@/lib/types";
import { useSettings } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { Wordmark } from "@/components/brand";
import { WindowControls, useWindowChrome, type WindowChrome } from "@/components/shell/window-controls";
import { Badge, IconButton, Kbd } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { DropdownMenu, useDropdown, type MenuEntry } from "@/components/ui/select";
import { CommandPalette } from "@/components/shell/command-palette";
import { ServerSwitcher } from "@/components/shell/server-switcher";
import { ShortcutsDialog } from "@/components/shell/shortcuts-dialog";
import { OnboardingWizard } from "@/components/onboarding/onboarding-wizard";
import { useGenres, useInstalledGames, useNativeEvents } from "@/lib/hooks";
import { OPEN_SHORTCUTS_EVENT, openFiltersPanel, requestSearchFocus } from "@/lib/ui-events";
import { useDownloads } from "@/state/downloads";
import { formatSpeed, sortLabel, formatPercent } from "@/lib/format";
import { t } from "@/lib/i18n";

const SORTS = ["title", "releaseDate", "rating", "addedAt", "lastPlayed", "playtime"] as const;

function RailLink({
  label,
  icon,
  active,
  count,
  highlight,
  onClick,
}: {
  label: string;
  icon: React.ReactNode;
  active: boolean;
  count?: number;
  highlight?: boolean;
  onClick: () => void;
}) {
  return (
    <button type="button" className="rail-link w-full text-left" data-active={active} onClick={onClick} aria-current={active ? "page" : undefined}>
      <span className="grid size-4 place-items-center text-current opacity-80">{icon}</span>
      <span className="truncate">{label}</span>
      {typeof count === "number" && count > 0 && (
        <span className={highlight ? "rail-count text-signal-text" : "rail-count"}>{count}</span>
      )}
    </button>
  );
}

function Rail({ onOpenPalette }: { onOpenPalette: () => void }) {
  const navigate = useNavigate();
  const location = useLocation();
  const [params] = useSearchParams();
  const isEditor = useIsEditor();
  const isAdmin = useIsAdmin();
  const { setup } = useAuth();
  const genres = useGenres();
  const downloads = useDownloads();
  const { data: installedGames } = useInstalledGames();
  const { data: stats } = useQuery({ queryKey: ["stats"], queryFn: () => api.stats(), staleTime: 30_000 });

  const path = location.pathname;
  const genre = params.get("genre");
  const installedOnly = params.get("installed") === "true";
  const go = (to: string) => navigate(to);

  return (
    <div className="flex h-full flex-col">
      <nav aria-label={t("Library")} className="flex flex-1 flex-col gap-0.5 overflow-y-auto py-5">
        <p className="px-5 pb-2 text-[11.5px] text-ink-3">{t("Browse")}</p>
        <RailLink label={t("Home")} icon={<House className="size-4" />} active={path === "/"} onClick={() => go("/")} />
        <RailLink
          label={t("Library")}
          icon={<LibraryIcon className="size-4" />}
          active={path === "/library" && !genre && !installedOnly}
          count={stats?.games}
          onClick={() => go("/library")}
        />
        <RailLink label={t("Recently played")} icon={<Clock className="size-4" />} active={path === "/recent"} onClick={() => go("/recent")} />
        <RailLink label={t("Bookmarks")} icon={<Bookmark className="size-4" />} active={path === "/bookmarks"} onClick={() => go("/bookmarks")} />
        <RailLink label={t("Index")} icon={<ListOrdered className="size-4" />} active={path === "/all"} onClick={() => go("/all")} />
        <RailLink label={t("Collections")} icon={<ListPlus className="size-4" />} active={path.startsWith("/collections")} onClick={() => go("/collections")} />
        <RailLink label={t("Community")} icon={<Users className="size-4" />} active={path.startsWith("/community")} onClick={() => go("/community")} />
        {downloads.available && (
          <RailLink
            label={t("Installed")}
            icon={<HardDrive className="size-4" />}
            active={path === "/library" && installedOnly}
            count={installedGames?.length}
            onClick={() => go("/library?installed=true")}
          />
        )}
        {downloads.available && (
          <RailLink
            label={t("Downloads")}
            icon={<Download className="size-4" />}
            active={path === "/downloads"}
            count={downloads.activeCount}
            highlight
            onClick={() => go("/downloads")}
          />
        )}
        <RailLink label={t("News")} icon={<Film className="size-4" />} active={path === "/news"} onClick={() => go("/news")} />

        {genres.length > 0 && (
          <>
            <div className="mx-5 my-4 h-px bg-line" />
            <p className="px-5 pb-2 text-[11.5px] text-ink-3">{t("Genres")}</p>
          </>
        )}
        {genres.map((item) => (
          <RailLink
            key={item}
            label={item}
            icon={<span className="size-1.5 rounded-full bg-current opacity-60" />}
            active={path === "/library" && genre === item}
            onClick={() => go(`/library?genre=${encodeURIComponent(item)}`)}
          />
        ))}

        {isEditor && (
          <>
            <div className="mx-5 my-4 h-px bg-line" />
            <p className="px-5 pb-2 text-[11.5px] text-ink-3">{t("Admin")}</p>
            <RailLink label={t("Ingest")} icon={<Download className="size-4" />} active={path === "/admin/ingest"} onClick={() => go("/admin/ingest")} />
            <RailLink label={t("Upload game")} icon={<UploadCloud className="size-4" />} active={path === "/admin/upload"} onClick={() => go("/admin/upload")} />
            <RailLink label={t("Library scan")} icon={<RefreshCw className="size-4" />} active={path === "/admin/scan"} onClick={() => go("/admin/scan")} />
            {isAdmin && (
              <>
                <RailLink label={t("Users")} icon={<Users className="size-4" />} active={path === "/admin/users"} onClick={() => go("/admin/users")} />
                <RailLink label={t("News editor")} icon={<ShieldCheck className="size-4" />} active={path === "/admin/news"} onClick={() => go("/admin/news")} />
                <RailLink label={t("Server")} icon={<Server className="size-4" />} active={path === "/admin/server"} onClick={() => go("/admin/server")} />
                <RailLink label={t("Authentication")} icon={<KeyRound className="size-4" />} active={path === "/admin/authentication"} onClick={() => go("/admin/authentication")} />
                <RailLink label={t("Metadata providers")} icon={<PlugZap className="size-4" />} active={path === "/admin/metadata"} onClick={() => go("/admin/metadata")} />
              </>
            )}
          </>
        )}
      </nav>

      <div className="border-t border-line px-5 py-4">
        <button
          type="button"
          onClick={onOpenPalette}
          className="mb-4 flex w-full items-center gap-2 text-left text-[12px] text-ink-3 transition-colors hover:text-ink-2"
        >
          <Search className="size-3.5" aria-hidden="true" />
          <span>{t("Search")}</span>
          <Kbd>⌘K</Kbd>
        </button>
        <div className="flex items-center gap-2 text-[12px] text-ink-3">
          <span className="size-1.5 rounded-full bg-ok" aria-hidden="true" />
          <span className="truncate">{setup?.serverName ?? "Ludex"}</span>
          <span className="num ml-auto shrink-0">{`v${setup?.version ?? "0.1.0"}`}</span>
        </div>
        {USING_MOCKS && (
          <div className="mt-2.5">
            <Badge tone="outline">{t("Demo data")}</Badge>
          </div>
        )}
      </div>
    </div>
  );
}

function SortMenu() {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const location = useLocation();
  const menu = useDropdown();
  const sort = params.get("sort") ?? "title";
  const order = params.get("order") ?? (sort === "title" ? "asc" : "desc");
  const currentSort = SORTS.find((option) => option === sort) ?? SORTS[0];
  const current = { value: currentSort, label: sortLabel(currentSort) };

  const setParam = (key: string, value: string) => {
    const next = new URLSearchParams(params);
    next.set(key, value);
    navigate({ pathname: location.pathname, search: next.toString() });
  };

  const items: MenuEntry[] = [
    { heading: t("Sort by") },
    ...SORTS.map((option) => ({
      label: sortLabel(option),
      selected: option === sort,
      onSelect: () => setParam("sort", option),
    })),
    "separator",
    { label: t("Ascending"), selected: order === "asc", onSelect: () => setParam("order", "asc") },
    { label: t("Descending"), selected: order === "desc", onSelect: () => setParam("order", "desc") },
  ];

  return (
    <>
      <button
        {...menu.triggerProps}
        aria-label={t("Sort: {sort}", { sort: current.label })}
        className="hidden h-9 items-center gap-2 rounded-[2px] px-2.5 text-[13px] text-ink-2 transition-colors hover:bg-surface hover:text-ink md:inline-flex"
      >
        <ArrowUpDown className="size-3.5 text-ink-3" aria-hidden="true" />
        <span>{current.label}</span>
        <ChevronDown className="size-3.5 text-ink-3" aria-hidden="true" />
      </button>
      <DropdownMenu {...menu.menuProps} items={items} label={t("Sort games")} />
    </>
  );
}

function UserMenu() {
  const { user, signOut } = useAuth();
  const { resolved, setTheme } = useSettings();
  const navigate = useNavigate();
  const menu = useDropdown();
  if (!user) return null;

  const items: MenuEntry[] = [
    { heading: `${user.username} — ${user.role === "admin" ? t("Administrator") : user.role === "editor" ? t("Editor") : t("Member")}` },
    { label: t("Settings"), icon: <Settings className="size-3.5" />, onSelect: () => navigate("/settings") },
    {
      label: resolved === "dark" ? "Light theme" : "Dark theme",
      icon: resolved === "dark" ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />,
      onSelect: () => setTheme(resolved === "dark" ? "light" : "dark"),
    },
    "separator",
    { label: t("Sign out"), icon: <LogOut className="size-3.5" />, danger: true, onSelect: () => void signOut() },
  ];

  return (
    <>
      <button
        {...menu.triggerProps}
        aria-label={t("Account menu for {name}", { name: user.username })}
        className="flex h-9 items-center gap-2 rounded-[2px] pl-1 pr-1.5 transition-colors hover:bg-surface"
      >
        <span className="grid size-7 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-[10.5px] text-ink">
          {user.username.slice(0, 2).toUpperCase()}
        </span>
        <span className="hidden text-[13px] text-ink-2 sm:block">{user.username}</span>
        <ChevronDown className="hidden size-3.5 text-ink-3 sm:block" aria-hidden="true" />
      </button>
      <DropdownMenu {...menu.menuProps} items={items} label={t("Account")} />
    </>
  );
}

function Topbar({ onOpenPalette, chrome }: { onOpenPalette: () => void; chrome: WindowChrome }) {
  const navigate = useNavigate();
  const location = useLocation();
  const { push } = useToast();
  const [randomizing, setRandomizing] = useState(false);
  const [params] = useSearchParams();
  const showSort = location.pathname === "/library" || location.pathname === "/bookmarks";
  const onLibrary = location.pathname === "/";

  const random = async () => {
    setRandomizing(true);
    try {
      const game = await api.randomGame(
        onLibrary
          ? {
              genre: params.get("genre") ?? undefined,
              state: (params.get("state") as ProgressState | null) ?? undefined,
              earlyAccess: params.get("earlyAccess") === "true" || undefined,
            }
          : {},
      );
      navigate(`/game/${game.id}`);
    } catch {
      push({ title: t("Nothing to pick"), description: t("The library is empty, or no game matches the filters.") });
    } finally {
      setRandomizing(false);
    }
  };

  return (
    <header
      // With client-side decorations the empty parts of the bar move the
      // window; buttons and inputs block it themselves, and the trailing
      // clusters opt out explicitly.
      data-tauri-drag-region={chrome.custom ? "deep" : undefined}
      className="flex h-[calc(var(--bar-h)+env(safe-area-inset-top))] shrink-0 items-stretch border-b border-line pt-[env(safe-area-inset-top)]"
    >
      <div className="hidden w-[var(--rail-w)] shrink-0 items-center border-r border-line px-5 lg:flex">
        <button type="button" onClick={() => navigate("/")} aria-label={t("Ludex home")} className="py-2">
          <Wordmark height={17} />
        </button>
      </div>

      <div className={cn("flex min-w-0 flex-1 items-center gap-2 px-[var(--page-x)] lg:px-5", chrome.custom && "lg:pr-3")}>
        <button type="button" onClick={() => navigate("/")} aria-label={t("Ludex home")} className="mr-1 lg:hidden">
          <Wordmark height={15} withDot={false} />
        </button>

        <button
          type="button"
          onClick={onOpenPalette}
          className="hidden h-9 w-[300px] items-center gap-2.5 rounded-[2px] border border-line bg-surface px-3 text-left text-[13px] text-ink-3 transition-colors hover:border-line-2 hover:text-ink-2 sm:flex"
        >
          <Search className="size-4 shrink-0" aria-hidden="true" />
          <span className="flex-1 truncate">{t("Search games")}</span>
          <Kbd>⌘K</Kbd>
        </button>
        <div data-tauri-drag-region={chrome.custom ? "false" : undefined} className="ml-auto flex items-center gap-1">
          <IconButton label={t("Search")} variant="ghost" size="icon" className="sm:hidden" onClick={onOpenPalette}>
            <Search className="size-4" />
          </IconButton>
          <IconButton label={t("Pick a random game")} variant="ghost" onClick={random} loading={randomizing}>
            <Dice5 className="size-4" />
          </IconButton>
          <span className="hidden sm:block">
            <IconButton label={t("Big Picture mode")} variant="ghost" onClick={() => navigate("/bigpicture")}>
              <Gamepad2 className="size-4" />
            </IconButton>
          </span>
          {showSort && <SortMenu />}
          <div className="mx-1.5 hidden h-5 w-px bg-line sm:block" aria-hidden="true" />
          <span className="hidden sm:block">
            <ServerSwitcher />
          </span>
          <UserMenu />
        </div>
      </div>

      {chrome.custom && <WindowControls chrome={chrome} height="bar" />}
    </header>
  );
}

function MobileNav({ onMore }: { onMore: () => void }) {
  const navigate = useNavigate();
  const location = useLocation();
  const path = location.pathname;

  const items = [
    { label: t("Home"), icon: <House className="size-4" />, to: "/", active: path === "/" },
    { label: t("Library"), icon: <LibraryIcon className="size-4" />, to: "/library", active: path === "/library" },
    { label: t("Recent"), icon: <Clock className="size-4" />, to: "/recent", active: path === "/recent" },
    { label: t("Saved"), icon: <Bookmark className="size-4" />, to: "/bookmarks", active: path === "/bookmarks" },
  ];

  return (
    <nav
      aria-label={t("Primary")}
      className="flex h-[calc(56px+env(safe-area-inset-bottom))] shrink-0 items-stretch border-t border-line bg-bg pb-[env(safe-area-inset-bottom)] lg:hidden"
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          onClick={() => navigate(item.to)}
          aria-current={item.active ? "page" : undefined}
          className={cn(
            "relative flex flex-1 flex-col items-center justify-center gap-1 text-[10px] transition-colors",
            item.active ? "text-ink" : "text-ink-3",
          )}
        >
          {item.active && <span className="absolute inset-x-5 top-0 h-[2px] bg-signal" aria-hidden="true" />}
          {item.icon}
          {item.label}
        </button>
      ))}
      <button
        type="button"
        onClick={onMore}
        className="flex flex-1 flex-col items-center justify-center gap-1 text-[10px] text-ink-3"
      >
        <MoreHorizontal className="size-4" />
        {t("More")}
      </button>
    </nav>
  );
}

function MobileMenu({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const navigate = useNavigate();
  const genres = useGenres();
  const isEditor = useIsEditor();
  const isAdmin = useIsAdmin();
  const { resolved, setTheme } = useSettings();
  const { signOut, user } = useAuth();
  const downloads = useDownloads();
  const go = (to: string) => {
    onOpenChange(false);
    navigate(to);
  };

  const Row = ({ label, onClick, icon }: { label: string; onClick: () => void; icon?: React.ReactNode }) => (
    <button
      type="button"
      onClick={onClick}
      className="flex h-11 w-full items-center gap-3 border-b border-line text-left text-[14px] text-ink-2 last:border-0 hover:text-ink"
    >
      <span className="grid size-4 place-items-center text-ink-3">{icon}</span>
      {label}
    </button>
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={t("Menu")} description={user ? t("Signed in as {name}", { name: user.username }) : undefined} size="sm">
      <div className="max-h-[60vh] overflow-y-auto">
        <Row label={t("News")} icon={<Film className="size-3.5" />} onClick={() => go("/news")} />
        {downloads.available && (
          <Row label={t("Downloads")} icon={<Download className="size-3.5" />} onClick={() => go("/downloads")} />
        )}
        {downloads.available && (
          <Row label={t("Installed games")} icon={<HardDrive className="size-3.5" />} onClick={() => go("/library?installed=true")} />
        )}
        <Row label={t("Collections")} icon={<ListPlus className="size-3.5" />} onClick={() => go("/collections")} />
        <Row label={t("Community")} icon={<Users className="size-3.5" />} onClick={() => go("/community")} />
        <Row label={t("Big Picture mode")} icon={<Gamepad2 className="size-3.5" />} onClick={() => go("/bigpicture")} />
        <Row label={t("Servers")} icon={<Server className="size-3.5" />} onClick={() => go("/settings")} />
        <Row label={t("Settings")} icon={<Settings className="size-3.5" />} onClick={() => go("/settings")} />
        <div className="pb-1 pt-4 text-[11.5px] text-ink-3">{t("Genres")}</div>
        <div className="flex flex-wrap gap-1.5 pb-4">
          {genres.map((genre) => (
            <button
              key={genre}
              type="button"
              onClick={() => go(`/?genre=${encodeURIComponent(genre)}`)}
              className="h-8 rounded-[2px] border border-line px-2.5 text-[12.5px] text-ink-2"
            >
              {genre}
            </button>
          ))}
        </div>
        {isEditor && (
          <>
            <div className="pb-1 text-[11.5px] text-ink-3">{t("Admin")}</div>
            <Row label={t("Ingest")} icon={<Download className="size-3.5" />} onClick={() => go("/admin/ingest")} />
            <Row label={t("Library scan")} icon={<RefreshCw className="size-3.5" />} onClick={() => go("/admin/scan")} />
            {isAdmin && <Row label={t("Users")} icon={<Users className="size-3.5" />} onClick={() => go("/admin/users")} />}
            {isAdmin && <Row label={t("Server")} icon={<Server className="size-3.5" />} onClick={() => go("/admin/server")} />}
            {isAdmin && <Row label={t("Authentication")} icon={<KeyRound className="size-3.5" />} onClick={() => go("/admin/authentication")} />}
            {isAdmin && <Row label={t("Metadata providers")} icon={<PlugZap className="size-3.5" />} onClick={() => go("/admin/metadata")} />}
          </>
        )}
        <Row
          label={resolved === "dark" ? t("Switch to the light theme") : t("Switch to the dark theme")}
          icon={resolved === "dark" ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />}
          onClick={() => setTheme(resolved === "dark" ? "light" : "dark")}
        />
        <Row label={t("Sign out")} icon={<LogOut className="size-3.5" />} onClick={() => void signOut()} />
      </div>
    </Dialog>
  );
}

function DownloadsStrip() {
  const { active, activeCount, aggregate, titleFor } = useDownloads();
  if (active.length === 0) return null;
  const job = active[0];
  const paused = job.phase === "paused";
  const jobPct = job.bytesTotal ? Math.min(100, (job.bytesDone / job.bytesTotal) * 100) : null;
  const pct = paused ? jobPct : (aggregate?.percent ?? null);

  return (
    <Link
      to="/downloads"
      aria-label={t("Downloads: {title}, {percent} percent", { title: titleFor(job), percent: pct !== null ? pct.toFixed(0) : "" })}
      className="relative flex h-10 shrink-0 items-center gap-3 border-t border-line bg-surface-2 px-4 text-[12px] text-ink-2 transition-colors hover:bg-surface"
    >
      <span className="absolute inset-x-0 top-0 h-[2px] bg-line" aria-hidden="true">
        <span
          className="block h-full bg-signal transition-[width] duration-500"
          style={{ width: pct !== null ? `${pct}%` : "30%" }}
        />
      </span>
      <Download className="size-3.5 shrink-0 text-signal-text" aria-hidden="true" />
      <span className="min-w-0 truncate text-ink">{titleFor(job)}</span>
      {activeCount > 1 && <span className="num shrink-0 text-ink-3">+{activeCount - 1}</span>}
      <span className="num ml-auto shrink-0">{paused ? t("Paused") : pct !== null ? formatPercent(pct) : t("Preparing")}</span>
      {aggregate?.speedBps ? (
        <span className="num hidden shrink-0 text-ink-3 sm:inline">{formatSpeed(aggregate.speedBps)}</span>
      ) : null}
      {aggregate?.eta ? <span className="num hidden shrink-0 text-ink-3 md:inline">{t("{duration} left", { duration: aggregate.eta })}</span> : null}
      <span className="hidden shrink-0 items-center gap-1 text-ink-3 sm:flex">
        {t("Downloads")}
        <ChevronRight className="rtl:-scale-x-100 size-3" aria-hidden="true" />
      </span>
    </Link>
  );
}

export function AppShell() {
  const location = useLocation();
  const navigate = useNavigate();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const chrome = useWindowChrome();
  useNativeEvents();
  const scrollRef = useRef<HTMLElement>(null);
  const positions = useRef(new Map<string, number>());

  useEffect(() => {
    const onOpenHelp = () => setHelpOpen(true);
    window.addEventListener(OPEN_SHORTCUTS_EVENT, onOpenHelp);
    return () => window.removeEventListener(OPEN_SHORTCUTS_EVENT, onOpenHelp);
  }, []);

  useEffect(() => {
    let gPressedAt = 0;
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const typing = Boolean(target?.closest('input, textarea, select, [contenteditable="true"]'));
      const modal = Boolean(document.querySelector('[role="dialog"][aria-modal="true"]'));

      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
        return;
      }
      if (event.key === "Escape") {
        if (paletteOpen) setPaletteOpen(false);
        return;
      }
      if (typing || event.metaKey || event.ctrlKey || event.altKey) return;
      if (modal) return;

      const key = event.key.toLowerCase();
      if (key === "g") {
        gPressedAt = Date.now();
        return;
      }
      if (gPressedAt && Date.now() - gPressedAt < 1200) {
        const routes: Record<string, string> = {
          h: "/",
          l: "/library",
          d: "/downloads",
          s: "/settings",
          b: "/bookmarks",
          i: "/all",
        };
        if (routes[key]) {
          event.preventDefault();
          gPressedAt = 0;
          navigate(routes[key]);
          return;
        }
        gPressedAt = 0;
      }

      if (event.key === "/") {
        event.preventDefault();
        navigate("/library");
        requestSearchFocus();
      } else if (event.key === "?") {
        event.preventDefault();
        setHelpOpen(true);
      } else if (key === "d") {
        event.preventDefault();
        navigate("/downloads");
      } else if (key === "f" && location.pathname === "/library") {
        event.preventDefault();
        openFiltersPanel();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [location.pathname, navigate, paletteOpen]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = positions.current.get(location.key) ?? 0;
  }, [location.key]);

  return (
    <div className="flex h-dvh flex-col bg-bg text-ink">
      <div className="grain" aria-hidden="true" />
      {/* Fullscreen has no title bar: the custom controls and the bar step
          aside so Big Picture and full-screen sessions stay chrome-free. */}
      {!chrome.fullscreen && <Topbar onOpenPalette={() => setPaletteOpen(true)} chrome={chrome} />}
      <div className="flex min-h-0 flex-1">
        <aside className="hidden w-[var(--rail-w)] shrink-0 border-r border-line lg:block" aria-label={t("Sidebar")}>
          <Rail onOpenPalette={() => setPaletteOpen(true)} />
        </aside>
        <main
          ref={scrollRef}
          data-scroll-root
          className="min-w-0 flex-1 overflow-y-auto overscroll-contain pb-8"
          onScroll={(event) => positions.current.set(location.key, event.currentTarget.scrollTop)}
        >
          <Outlet />
        </main>
      </div>
      <DownloadsStrip />
      <MobileNav onMore={() => setMoreOpen(true)} />
      <MobileMenu open={moreOpen} onOpenChange={setMoreOpen} />
      <OnboardingWizard />
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
      <ShortcutsDialog open={helpOpen} onOpenChange={setHelpOpen} />
    </div>
  );
}
