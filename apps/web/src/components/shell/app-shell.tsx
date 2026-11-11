import { useEffect, useRef, useState } from "react";
import { Link, Outlet, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import {
  ArrowUpDown,
  ChevronDown,
  ChevronRight,
  Dice5,
  Download,
  Gamepad2,
  LogOut,
  Moon,
  Search,
  Settings,
  Sun,
} from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { installsAvailable } from "@/lib/install";
import { navShortcutTarget, type NavContext } from "@/lib/navigation";
import { useAuth, useIsAdmin, useIsEditor } from "@/state/auth";
import type { ProgressState } from "@/lib/types";
import { otherModeTheme, useSettings } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { Wordmark } from "@/components/brand";
import { WindowControls, useWindowChrome, type WindowChrome } from "@/components/shell/window-controls";
import { IconButton, Kbd } from "@/components/ui/controls";
import { DropdownMenu, useDropdown, type MenuEntry } from "@/components/ui/select";
import { CommandPalette } from "@/components/shell/command-palette";
import { MobileBar, MoreSheet } from "@/components/shell/nav-mobile";
import { NavRail } from "@/components/shell/nav-rail";
import { ServerSwitcher } from "@/components/shell/server-switcher";
import { ShortcutsDialog } from "@/components/shell/shortcuts-dialog";
import { OnboardingWizard } from "@/components/onboarding/onboarding-wizard";
import { DesktopAppPrompt } from "@/components/desktop-app-prompt";
import { useNativeEvents } from "@/lib/hooks";
import { OPEN_SHORTCUTS_EVENT, openFiltersPanel, requestSearchFocus } from "@/lib/ui-events";
import { useDownloads } from "@/state/downloads";
import { formatSpeed, sortLabel, formatPercent } from "@/lib/format";
import { t } from "@/lib/i18n";

const SORTS = ["title", "releaseDate", "rating", "addedAt", "lastPlayed", "playtime"] as const;

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
      label: resolved === "dark" ? t("Light theme") : t("Dark theme"),
      icon: resolved === "dark" ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />,
      onSelect: () => setTheme(otherModeTheme(resolved)),
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
        <Link to="/" aria-label={t("Ludex home")} className="py-2">
          <Wordmark height={17} />
        </Link>
      </div>

      <div className={cn("flex min-w-0 flex-1 items-center gap-2 px-[var(--page-x)] lg:px-5", chrome.custom && "lg:pr-3")}>
        <Link to="/" aria-label={t("Ludex home")} className="mr-1 lg:hidden">
          <Wordmark height={15} withDot={false} />
        </Link>

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
  const isEditor = useIsEditor();
  const isAdmin = useIsAdmin();
  const desktop = installsAvailable();
  useNativeEvents();
  const scrollRef = useRef<HTMLElement>(null);
  const positions = useRef(new Map<string, number>());
  const context: NavContext = { editor: isEditor, admin: isAdmin, desktop };

  useEffect(() => {
    const onOpenHelp = () => setHelpOpen(true);
    window.addEventListener(OPEN_SHORTCUTS_EVENT, onOpenHelp);
    return () => window.removeEventListener(OPEN_SHORTCUTS_EVENT, onOpenHelp);
  }, []);

  useEffect(() => {
    let gPressedAt = 0;
    const shortcuts: NavContext = { editor: isEditor, admin: isAdmin, desktop };
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
        const route = navShortcutTarget(key, shortcuts);
        if (route) {
          event.preventDefault();
          gPressedAt = 0;
          navigate(route);
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
  }, [desktop, isAdmin, isEditor, location.pathname, navigate, paletteOpen]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = positions.current.get(location.key) ?? 0;
  }, [location.key]);

  return (
    <div className="flex h-dvh flex-col bg-bg text-ink">
      <div className="grain" aria-hidden="true" />
      <a
        href="#main-content"
        onClick={(event) => {
          // HashRouter owns the hash: focus the scroller instead of letting the
          // browser navigate to a route named after the anchor.
          event.preventDefault();
          scrollRef.current?.focus();
        }}
        className="sr-only focus:not-sr-only focus:absolute focus:left-4 focus:top-3 focus:z-[100] focus:rounded-[2px] focus:border focus:border-line-2 focus:bg-surface focus:px-3 focus:py-2 focus:text-[13px] focus:text-ink"
      >
        {t("Skip to content")}
      </a>
      {/* Fullscreen has no title bar: the custom controls and the bar step
          aside so Big Picture and full-screen sessions stay chrome-free. */}
      {!chrome.fullscreen && <Topbar onOpenPalette={() => setPaletteOpen(true)} chrome={chrome} />}
      <div className="flex min-h-0 flex-1">
        <aside className="hidden w-[var(--rail-w)] shrink-0 border-r border-line lg:block" aria-label={t("Sidebar")}>
          <NavRail context={context} onOpenHelp={() => setHelpOpen(true)} />
        </aside>
        <main
          id="main-content"
          tabIndex={-1}
          ref={scrollRef}
          data-scroll-root
          className="min-w-0 flex-1 overflow-y-auto overscroll-contain pb-8"
          onScroll={(event) => positions.current.set(location.key, event.currentTarget.scrollTop)}
        >
          <Outlet />
        </main>
      </div>
      <DownloadsStrip />
      <MobileBar context={context} onMore={() => setMoreOpen(true)} />
      <MoreSheet open={moreOpen} onOpenChange={setMoreOpen} context={context} />
      <DesktopAppPrompt />
      <OnboardingWizard />
      <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
      <ShortcutsDialog open={helpOpen} onOpenChange={setHelpOpen} />
    </div>
  );
}
