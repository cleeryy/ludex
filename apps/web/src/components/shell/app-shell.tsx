import { useEffect, useRef, useState } from "react";
import { Outlet, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowUpDown,
  Bookmark,
  ChevronDown,
  Clock,
  Dice5,
  Download,
  Film,
  ListOrdered,
  LogOut,
  Moon,
  MoreHorizontal,
  RefreshCw,
  Search,
  Server,
  Settings,
  ShieldCheck,
  Sun,
  Users,
  Library as LibraryIcon,
} from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { cn } from "@/lib/cn";
import { useAuth, useIsAdmin, useIsEditor } from "@/state/auth";
import type { ProgressState } from "@/lib/types";
import { useSettings } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { Wordmark } from "@/components/brand";
import { Badge, IconButton, Kbd } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { DropdownMenu, useDropdown, type MenuEntry } from "@/components/ui/select";
import { CommandPalette } from "@/components/shell/command-palette";
import { useGenres, useNativeEvents } from "@/lib/hooks";

const SORTS = [
  { value: "title", label: "Title" },
  { value: "releaseDate", label: "Release date" },
  { value: "rating", label: "Rating" },
  { value: "addedAt", label: "Date added" },
  { value: "lastPlayed", label: "Last played" },
];

function RailLink({
  label,
  icon,
  active,
  count,
  onClick,
}: {
  label: string;
  icon: React.ReactNode;
  active: boolean;
  count?: number;
  onClick: () => void;
}) {
  return (
    <button type="button" className="rail-link w-full text-left" data-active={active} onClick={onClick} aria-current={active ? "page" : undefined}>
      <span className="grid size-4 place-items-center text-current opacity-80">{icon}</span>
      <span className="truncate">{label}</span>
      {typeof count === "number" && <span className="rail-count">{count}</span>}
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
  const { data: stats } = useQuery({ queryKey: ["stats"], queryFn: () => api.stats(), staleTime: 30_000 });

  const path = location.pathname;
  const genre = params.get("genre");
  const go = (to: string) => navigate(to);

  return (
    <div className="flex h-full flex-col">
      <nav aria-label="Library" className="flex flex-1 flex-col gap-0.5 overflow-y-auto py-5">
        <p className="px-5 pb-2 text-[11.5px] text-ink-3">Browse</p>
        <RailLink label="Library" icon={<LibraryIcon className="size-4" />} active={path === "/" && !genre} count={stats?.games} onClick={() => go("/")} />
        <RailLink label="Recently played" icon={<Clock className="size-4" />} active={path === "/recent"} onClick={() => go("/recent")} />
        <RailLink label="Bookmarks" icon={<Bookmark className="size-4" />} active={path === "/bookmarks"} onClick={() => go("/bookmarks")} />
        <RailLink label="Index" icon={<ListOrdered className="size-4" />} active={path === "/all"} onClick={() => go("/all")} />
        <RailLink label="News" icon={<Film className="size-4" />} active={path === "/news"} onClick={() => go("/news")} />

        {genres.length > 0 && (
          <>
            <div className="mx-5 my-4 h-px bg-line" />
            <p className="px-5 pb-2 text-[11.5px] text-ink-3">Genres</p>
          </>
        )}
        {genres.map((item) => (
          <RailLink
            key={item}
            label={item}
            icon={<span className="size-1.5 rounded-full bg-current opacity-60" />}
            active={path === "/" && genre === item}
            onClick={() => go(`/?genre=${encodeURIComponent(item)}`)}
          />
        ))}

        {isEditor && (
          <>
            <div className="mx-5 my-4 h-px bg-line" />
            <p className="px-5 pb-2 text-[11.5px] text-ink-3">Admin</p>
            <RailLink label="Ingest" icon={<Download className="size-4" />} active={path === "/admin/ingest"} onClick={() => go("/admin/ingest")} />
            <RailLink label="Library scan" icon={<RefreshCw className="size-4" />} active={path === "/admin/scan"} onClick={() => go("/admin/scan")} />
            {isAdmin && (
              <>
                <RailLink label="Users" icon={<Users className="size-4" />} active={path === "/admin/users"} onClick={() => go("/admin/users")} />
                <RailLink label="News editor" icon={<ShieldCheck className="size-4" />} active={path === "/admin/news"} onClick={() => go("/admin/news")} />
                <RailLink label="Server" icon={<Server className="size-4" />} active={path === "/admin/server"} onClick={() => go("/admin/server")} />
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
          <span>Search</span>
          <Kbd>⌘K</Kbd>
        </button>
        <div className="flex items-center gap-2 text-[12px] text-ink-3">
          <span className="size-1.5 rounded-full bg-ok" aria-hidden="true" />
          <span className="truncate">{setup?.serverName ?? "Ludex"}</span>
          <span className="num ml-auto shrink-0">v{setup?.version ?? "0.1.0"}</span>
        </div>
        {USING_MOCKS && (
          <div className="mt-2.5">
            <Badge tone="outline">Demo data</Badge>
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
  const current = SORTS.find((option) => option.value === sort) ?? SORTS[0];

  const setParam = (key: string, value: string) => {
    const next = new URLSearchParams(params);
    next.set(key, value);
    navigate({ pathname: location.pathname, search: next.toString() });
  };

  const items: MenuEntry[] = [
    { heading: "Sort by" },
    ...SORTS.map((option) => ({
      label: option.label,
      selected: option.value === sort,
      onSelect: () => setParam("sort", option.value),
    })),
    "separator",
    { label: "Ascending", selected: order === "asc", onSelect: () => setParam("order", "asc") },
    { label: "Descending", selected: order === "desc", onSelect: () => setParam("order", "desc") },
  ];

  return (
    <>
      <button
        {...menu.triggerProps}
        aria-label={`Sort: ${current.label}`}
        className="hidden h-9 items-center gap-2 rounded-[2px] px-2.5 text-[13px] text-ink-2 transition-colors hover:bg-surface hover:text-ink md:inline-flex"
      >
        <ArrowUpDown className="size-3.5 text-ink-3" aria-hidden="true" />
        <span>{current.label}</span>
        <ChevronDown className="size-3.5 text-ink-3" aria-hidden="true" />
      </button>
      <DropdownMenu {...menu.menuProps} items={items} label="Sort games" />
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
    { heading: `${user.username} — ${user.role === "admin" ? "Administrator" : user.role === "editor" ? "Editor" : "Member"}` },
    { label: "Settings", icon: <Settings className="size-3.5" />, onSelect: () => navigate("/settings") },
    {
      label: resolved === "dark" ? "Light theme" : "Dark theme",
      icon: resolved === "dark" ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />,
      onSelect: () => setTheme(resolved === "dark" ? "light" : "dark"),
    },
    "separator",
    { label: "Sign out", icon: <LogOut className="size-3.5" />, danger: true, onSelect: () => void signOut() },
  ];

  return (
    <>
      <button
        {...menu.triggerProps}
        aria-label={`Account menu for ${user.username}`}
        className="flex h-9 items-center gap-2 rounded-[2px] pl-1 pr-1.5 transition-colors hover:bg-surface"
      >
        <span className="grid size-7 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-[10.5px] text-ink">
          {user.username.slice(0, 2).toUpperCase()}
        </span>
        <span className="hidden text-[13px] text-ink-2 sm:block">{user.username}</span>
        <ChevronDown className="hidden size-3.5 text-ink-3 sm:block" aria-hidden="true" />
      </button>
      <DropdownMenu {...menu.menuProps} items={items} label="Account" />
    </>
  );
}

function Topbar({ onOpenPalette }: { onOpenPalette: () => void }) {
  const navigate = useNavigate();
  const location = useLocation();
  const { push } = useToast();
  const [randomizing, setRandomizing] = useState(false);
  const [params] = useSearchParams();
  const showSort = location.pathname === "/" || location.pathname === "/bookmarks";
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
      push({ title: "Nothing to pick", description: "The library is empty, or no game matches the filters." });
    } finally {
      setRandomizing(false);
    }
  };

  return (
    <header className="flex h-[var(--bar-h)] shrink-0 items-stretch border-b border-line">
      <div className="hidden w-[var(--rail-w)] shrink-0 items-center border-r border-line px-5 lg:flex">
        <button type="button" onClick={() => navigate("/")} aria-label="Ludex home" className="py-2">
          <Wordmark height={17} />
        </button>
      </div>

      <div className="flex min-w-0 flex-1 items-center gap-2 px-[var(--page-x)] lg:px-5">
        <button type="button" onClick={() => navigate("/")} aria-label="Ludex home" className="mr-1 lg:hidden">
          <Wordmark height={15} withDot={false} />
        </button>

        <button
          type="button"
          onClick={onOpenPalette}
          className="hidden h-9 w-[300px] items-center gap-2.5 rounded-[2px] border border-line bg-surface px-3 text-left text-[13px] text-ink-3 transition-colors hover:border-line-2 hover:text-ink-2 sm:flex"
        >
          <Search className="size-4 shrink-0" aria-hidden="true" />
          <span className="flex-1 truncate">Search games</span>
          <Kbd>⌘K</Kbd>
        </button>
        <div className="ml-auto flex items-center gap-1">
          <IconButton label="Search" variant="ghost" size="icon" className="sm:hidden" onClick={onOpenPalette}>
            <Search className="size-4" />
          </IconButton>
          <IconButton label="Pick a random game" variant="ghost" onClick={random} loading={randomizing}>
            <Dice5 className="size-4" />
          </IconButton>
          {showSort && <SortMenu />}
          <div className="mx-1.5 hidden h-5 w-px bg-line sm:block" aria-hidden="true" />
          <UserMenu />
        </div>
      </div>
    </header>
  );
}

function MobileNav({ onMore }: { onMore: () => void }) {
  const navigate = useNavigate();
  const location = useLocation();
  const path = location.pathname;

  const items = [
    { label: "Library", icon: <LibraryIcon className="size-4" />, to: "/", active: path === "/" },
    { label: "Recent", icon: <Clock className="size-4" />, to: "/recent", active: path === "/recent" },
    { label: "Saved", icon: <Bookmark className="size-4" />, to: "/bookmarks", active: path === "/bookmarks" },
    { label: "Index", icon: <ListOrdered className="size-4" />, to: "/all", active: path === "/all" },
  ];

  return (
    <nav
      aria-label="Primary"
      className="fixed inset-x-0 bottom-0 z-40 flex h-[calc(56px+env(safe-area-inset-bottom))] items-stretch border-t border-line bg-bg pb-[env(safe-area-inset-bottom)] lg:hidden"
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
        More
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
    <Dialog open={open} onOpenChange={onOpenChange} title="Menu" description={user ? `Signed in as ${user.username}` : undefined} size="sm">
      <div className="max-h-[60vh] overflow-y-auto">
        <Row label="News" icon={<Film className="size-3.5" />} onClick={() => go("/news")} />
        <Row label="Settings" icon={<Settings className="size-3.5" />} onClick={() => go("/settings")} />
        <div className="pb-1 pt-4 text-[11.5px] text-ink-3">Genres</div>
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
            <div className="pb-1 text-[11.5px] text-ink-3">Admin</div>
            <Row label="Ingest" icon={<Download className="size-3.5" />} onClick={() => go("/admin/ingest")} />
            <Row label="Library scan" icon={<RefreshCw className="size-3.5" />} onClick={() => go("/admin/scan")} />
            {isAdmin && <Row label="Users" icon={<Users className="size-3.5" />} onClick={() => go("/admin/users")} />}
            {isAdmin && <Row label="Server" icon={<Server className="size-3.5" />} onClick={() => go("/admin/server")} />}
          </>
        )}
        <Row
          label={resolved === "dark" ? "Switch to the light theme" : "Switch to the dark theme"}
          icon={resolved === "dark" ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />}
          onClick={() => setTheme(resolved === "dark" ? "light" : "dark")}
        />
        <Row label="Sign out" icon={<LogOut className="size-3.5" />} onClick={() => void signOut()} />
      </div>
    </Dialog>
  );
}

export function AppShell() {
  const location = useLocation();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  useNativeEvents();
  const scrollRef = useRef<HTMLElement>(null);
  const positions = useRef(new Map<string, number>());

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const typing = Boolean(target?.closest('input, textarea, select, [contenteditable="true"]'));
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
        return;
      }
      if (event.key === "/" && !typing) {
        if (document.querySelector('[role="dialog"][aria-modal="true"]')) return;
        event.preventDefault();
        setPaletteOpen(true);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = positions.current.get(location.key) ?? 0;
  }, [location.key]);

  return (
    <div className="flex h-dvh flex-col bg-bg text-ink">
      <div className="grain" aria-hidden="true" />
      <Topbar onOpenPalette={() => setPaletteOpen(true)} />
      <div className="flex min-h-0 flex-1">
        <aside className="hidden w-[var(--rail-w)] shrink-0 border-r border-line lg:block" aria-label="Sidebar">
          <Rail onOpenPalette={() => setPaletteOpen(true)} />
        </aside>
        <main
          ref={scrollRef}
          data-scroll-root
          className="min-w-0 flex-1 overflow-y-auto overscroll-contain pb-24 lg:pb-0"
          onScroll={(event) => positions.current.set(location.key, event.currentTarget.scrollTop)}
        >
          <Outlet />
        </main>
      </div>
      <MobileNav onMore={() => setMoreOpen(true)} />
      <MobileMenu open={moreOpen} onOpenChange={setMoreOpen} />
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
    </div>
  );
}
