import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useReducedMotion } from "motion/react";
import { Download, Play } from "lucide-react";
import { api } from "@/lib/api";
import { downloadGame } from "@/lib/download";
import { errorMessage } from "@/lib/http";
import { formatPercent } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import { listServers } from "@/lib/servers";
import type { ConsoleStatus, GameSummary } from "@/lib/types";
import { useAllGames, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { useGamepad, gamepadDebugEnabled, type ActionSource, type GamepadAction } from "@/lib/gamepad";
import { GamepadDebugPanel } from "@/components/gamepad/gamepad-debug";
import { useControllerProfile } from "@/lib/use-controller-profile";
import { useAuth } from "@/state/auth";
import { useDownloads } from "@/state/downloads";
import { useSettings } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { Wordmark } from "@/components/brand";
import { InstallDialog } from "@/components/game/install-dialog";
import { Button } from "@/components/ui/controls";
import { ENTRANCE_MS } from "@/components/big-picture/entrance";
import {
  padFamily,
  padFamilyLabel,
  profileOptions,
  readControllerPrefs,
  resolveProfileChoice,
  withBigPictureDefaults,
  writeControllerPrefs,
  type BigPictureControllerPrefs,
} from "@/components/big-picture/controller";
import {
  hintContextFor,
  routeGamepadAction,
  routeKey,
  type BigPictureCommand,
  type BigPictureOverlay,
  type SearchStep,
} from "@/components/big-picture/input";
import { CompactLayout } from "@/components/big-picture/layout-compact";
import { GridLayout } from "@/components/big-picture/layout-grid";
import { HeroLayout } from "@/components/big-picture/layout-hero";
import { ShelfLayout } from "@/components/big-picture/layout-shelf";
import { LayoutSkeleton } from "@/components/big-picture/layout-skeleton";
import { LayoutPicker } from "@/components/big-picture/layout-switcher";
import {
  BIG_PICTURE_LAYOUTS,
  clampIndex,
  gridColumns,
  type BigPictureLayout,
} from "@/components/big-picture/layout";
import { layoutLabel } from "@/components/big-picture/labels";
import { jobPercent, type BigPictureLayoutProps } from "@/components/big-picture/model";
import { useBigPictureNavigation, type BigPictureShelf } from "@/components/big-picture/navigation";
import { BigPictureFooter } from "@/components/big-picture/parts";
import {
  CollectionsOverlay,
  CommandMenu,
  COMMAND_ITEMS,
  ConfirmPanel,
  DownloadsOverlay,
} from "@/components/big-picture/overlays";
import { HomeRow } from "@/components/big-picture/home-row";
import { PowerApp } from "@/components/big-picture/power-app";
import { SettingsApp, type SettingsRow } from "@/components/big-picture/settings-app";
import { SystemBar } from "@/components/big-picture/system-bar";
import {
  HOME_ITEM_IDS,
  atLibraryTop,
  closeSystemApp,
  levelFromSearch,
  openSystemApp,
  sameLevel,
  settingsSectionFromSearch,
  stepSettingsSection,
  wrapIndex,
  type FocusZone,
  type HomeItemId,
  type SettingsSectionId,
  type ShellLevel,
} from "@/components/big-picture/system-zones";
import {
  needsPowerConfirm,
  powerActions,
  powerConfirmActive,
  powerGridStep,
  resolvePowerConfirm,
  type PowerAction,
} from "@/components/big-picture/power";
import { ProfileOverlay } from "@/components/big-picture/profile-overlay";
import { SearchOverlay } from "@/components/big-picture/search-overlay";
import {
  SEARCH_DEBOUNCE_MS,
  applySearchKey,
  clampSearchIndex,
  filterGames,
  findGameShelf,
  flattenSearchRows,
  moveSearchTarget,
  searchRows,
} from "@/components/big-picture/search";
import { useBigPictureLayout, useViewportWidth } from "@/components/big-picture/use-layout";
import { scopedStorageKey } from "@/lib/server-storage";
import { localeMeta, selectableLocales, t, tn, useI18n } from "@/lib/i18n";

/** Ask the server once per open; a quick round trip through the route is free. */
const REFRESH_ON_OPEN_MS = 10_000;
/** Regaining focus only refetches when the shelf has had time to age. */
const REFRESH_ON_FOCUS_MS = 30_000;
let lastRefreshAt = 0;

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** The profile preference is per device and per server, like the layout. */
function prefsKey(): string {
  try {
    return scopedStorageKey("ludex.bigPicture.controller");
  } catch {
    return "ludex.bigPicture.controller";
  }
}

/**
 * Does a key event match the console quit chord (#172)? The setting is a
 * `+`-joined combination such as `f12` or `ctrl+shift+q`.
 */
function matchesExitShortcut(event: KeyboardEvent, shortcut: string | undefined): boolean {
  const parts = (shortcut ?? "f12")
    .toLowerCase()
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean);
  if (parts.length === 0) return false;
  const modifiers = new Set(parts.slice(0, -1));
  const key = parts[parts.length - 1];
  if (modifiers.has("ctrl") !== event.ctrlKey) return false;
  if (modifiers.has("alt") !== event.altKey) return false;
  if (modifiers.has("shift") !== event.shiftKey) return false;
  if (modifiers.has("meta") !== event.metaKey) return false;
  const pressed = event.key.toLowerCase();
  return pressed === key || event.code.toLowerCase() === key;
}

/**
 * Big Picture: the same archive language at ten feet (#141). One screen, four
 * layouts — shelf, hero, cover grid and compact list — driven by the same
 * selection model, the same keys and the same gamepad buttons. The layout is a
 * device preference: it lives in `localStorage` (see `use-layout.ts`) and
 * applies from here, from the quick switcher or from Settings → Appearance.
 */
export function BigPictureScreen() {
  const navigate = useNavigate();
  const { setTv } = useSettings();
  const location = useLocation();
  const [, setSearchParams] = useSearchParams();
  const debug = gamepadDebugEnabled(location.search);
  const { push } = useToast();
  const { user, updateLocale } = useAuth();
  const { locale, deviceOverride, setLocale, clearDeviceOverride } = useI18n();
  const { theme, resolved, setTheme } = useSettings();
  const queryClient = useQueryClient();
  const { data, isPending, isError, error, isFetching, refetch } = useAllGames();
  const installed = useInstalledMap();
  const downloads = useDownloads();
  const launch = useLaunchGame();
  const [layout, setLayout] = useBigPictureLayout();
  const [installOpen, setInstallOpen] = useState(false);
  const [entranceDone, setEntranceDone] = useState(false);
  const [layoutTouched, setLayoutTouched] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerIndex, setPickerIndex] = useState(0);
  // #196: the console shell level. It lives in the URL so an in-shell language
  // change (which remounts the provider tree) comes back to the same app.
  const [level, setLevelState] = useState<ShellLevel>(() => levelFromSearch(location.search));
  const [zone, setZone] = useState<FocusZone>("library");
  const [homeIndex, setHomeIndex] = useState(0);
  const [appIndex, setAppIndex] = useState(0);
  const [settingsSection, setSettingsSection] = useState<SettingsSectionId>(() =>
    settingsSectionFromSearch(location.search),
  );
  const [deviceOnly, setDeviceOnly] = useState(() => deviceOverride !== null);
  const [pendingPower, setPendingPower] = useState<{ action: PowerAction; index: number } | null>(null);
  const confirmWasOpen = useRef(false);
  const [runningGame, setRunningGame] = useState<{ sessionId: string; gameId: number; title: string } | null>(null);
  const [online, setOnline] = useState(() => (typeof navigator === "undefined" ? true : navigator.onLine));
  const [padBattery, setPadBattery] = useState<{ level: number; charging: boolean } | null>(null);
  // #172: when Sunshine started `ludex --console`, this screen is the console
  // shell: fullscreen per the settings, no route back into the desktop shell,
  // and quitting closes the app so Moonlight returns to the host desktop.
  const [consoleMode, setConsoleMode] = useState<ConsoleStatus | null>(null);
  const consoleActive = consoleMode?.active === true;
  // Mirror of the highlight that is always in step with the last key press:
  // a fast A after two d-pad clicks must apply the card that is on screen.
  const pickerIndexRef = useRef(0);
  const reduceMotion = useReducedMotion();
  const columns = gridColumns(useViewportWidth());

  // #138 controller-first state: the open overlay, its highlight, the couch
  // search query (debounced by the filter below) and the device-local profile
  // choice. None of these leave Big Picture when they change.
  const [overlay, setOverlay] = useState<BigPictureOverlay | null>(null);
  const [overlayIndex, setOverlayIndex] = useState(0);
  const overlayReturnRef = useRef<"library" | "home" | "app">("library");
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [searchIndex, setSearchIndex] = useState(0);
  const [prefs, setPrefs] = useState<BigPictureControllerPrefs>(() =>
    readControllerPrefs(storage(), prefsKey()),
  );
  const searchInputRef = useRef<HTMLInputElement>(null);

  // Big Picture is watched from a couch: the provider lifts text to 7:1 and
  // widens the focus ring while this screen is mounted (#140).
  useLayoutEffect(() => {
    setTv(true);
    return () => setTv(false);
  }, [setTv]);

  useEffect(() => {
    writeControllerPrefs(storage(), prefs, prefsKey());
  }, [prefs]);

  const games = data?.items ?? [];

  // The shell level is URL state. A browser Back, a language remount or a
  // deep link all arrive through `location.search`; keep the local level in
  // step without pushing a new history entry.
  useEffect(() => {
    const next = levelFromSearch(location.search);
    setLevelState((current) => (sameLevel(current, next) ? current : next));
    if (next.kind === "app" && next.id === "settings") {
      setSettingsSection(settingsSectionFromSearch(location.search));
    }
  }, [location.search]);

  useEffect(() => {
    const update = () => setOnline(navigator.onLine);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);

  // Pad battery only exists through the native listener; the Gamepad API does
  // not report it. Absent (or a pad with no battery) hides the slot instead of
  // showing a fabricated value.
  useEffect(() => {
    if (!isNative()) return;
    let cancelled = false;
    let timer: number | undefined;
    const load = () => {
      void native
        .overlayStatus()
        .then((status) => {
          if (cancelled) return;
          const controllers = status?.listener.controllers ?? [];
          const active = controllers.find((controller) => controller.active) ?? controllers[0];
          setPadBattery(
            active && active.battery !== null
              ? { level: Math.round(active.battery), charging: active.charging }
              : null,
          );
        })
        .catch(() => {
          /* the listener is optional; the bar simply omits the battery */
        });
    };
    load();
    timer = window.setInterval(load, 60_000);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, []);

  // A game launched from Big Picture is the only one this shell can stop, and
  // the exit event clears it even when the player leaves through the game.
  useEffect(() => {
    if (!isNative()) return;
    let cancelled = false;
    let off: (() => void) | undefined;
    void native
      .onGameExited(({ gameId }) => {
        setRunningGame((current) => (current && current.gameId === gameId ? null : current));
      })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else off = unlisten;
      })
      .catch(() => {
        /* no session tracking without the desktop event bridge */
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  useEffect(() => {
    if (!isNative()) return;
    let cancelled = false;
    void native
      .consoleMode()
      .then((status) => {
        if (!cancelled) setConsoleMode(status);
      })
      .catch(() => {
        /* the desktop build always answers; a failure stays in normal mode */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const shelves = useMemo<BigPictureShelf[]>(() => {
    const recent = games
      .filter((game) => game.lastPlayedAt)
      .sort((a, b) => Date.parse(b.lastPlayedAt ?? "") - Date.parse(a.lastPlayedAt ?? ""));
    return [
      { id: "recent", label: t("Recently played"), games: recent },
      { id: "all", label: t("All games"), games },
    ].filter((shelf) => shelf.games.length > 0);
  }, [games]);

  const nav = useBigPictureNavigation({
    shelves,
    layout,
    columns,
    data: data?.items,
    reduceMotion,
  });
  const selected = nav.selected;

  const detail = useQuery({
    queryKey: ["game", selected?.id],
    queryFn: () => api.getGame(selected!.id),
    enabled: Boolean(selected),
    staleTime: 60_000,
  });
  const version = detail.data?.versions[0];
  const installedGame = selected ? installed.get(selected.id) : undefined;
  const job = selected ? downloads.jobForGame(selected.id) : undefined;
  const percent = jobPercent(job);

  const serversQuery = useQuery({
    queryKey: ["servers"],
    queryFn: listServers,
    staleTime: 60_000,
    retry: false,
  });
  const serverLabel = serversQuery.data?.servers.find((server) => server.active)?.label ?? null;

  // Collections are a home-row surface (#196): the count is live in the tile
  // and the list opens in the same non-modal shell as the queue. The query
  // only runs when the row or its overlay can actually use it.
  const collectionsQuery = useQuery({
    queryKey: ["collections"],
    queryFn: () => api.listCollections(),
    enabled: level.kind === "library" || overlay === "collections",
    staleTime: 60_000,
    retry: 1,
  });
  const collections = collectionsQuery.data ?? [];

  // The controller profiles (#51) that drive navigation. The automatic choice
  // is what the library screen resolves (account, then per-game override); an
  // explicit choice is a device preference and applies to the next press.
  const accountProfile = useControllerProfile(selected?.id ?? null);
  const profilesQuery = useQuery({
    queryKey: ["controller-profiles"],
    queryFn: () => api.controllerProfiles(),
    staleTime: 5 * 60_000,
  });
  const profileChoices = useMemo(
    () => profileOptions(profilesQuery.data, accountProfile),
    [profilesQuery.data, accountProfile],
  );
  const selectedProfile = useMemo(
    () => resolveProfileChoice(profileChoices, prefs.profileId, accountProfile),
    [profileChoices, prefs.profileId, accountProfile],
  );
  const activeProfile = useMemo(
    () => withBigPictureDefaults(selectedProfile.profile),
    [selectedProfile],
  );
  const haptics = prefs.haptics && activeProfile.vibration;

  // Couch search (#138): the query is debounced so a fast typist does not
  // rebuild the result grid on every letter, and the focus grid includes the
  // on-screen keyboard rows plus one row per result.
  useEffect(() => {
    if (overlay !== "search") return;
    if (debouncedQuery === query) return;
    const timer = window.setTimeout(() => setDebouncedQuery(query), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [query, debouncedQuery, overlay]);

  const searchResults = useMemo(
    () => filterGames(games, debouncedQuery),
    [games, debouncedQuery],
  );
  const rows = useMemo(() => searchRows(searchResults), [searchResults]);
  const cells = useMemo(() => flattenSearchRows(rows), [rows]);

  useEffect(() => {
    setSearchIndex((current) => clampSearchIndex(current, cells.length));
  }, [cells.length]);

  const overlayCount =
    overlay === "menu"
      ? COMMAND_ITEMS.length
      : overlay === "exit"
        ? 2
        : overlay === "downloads"
          ? downloads.active.length + 1
          : overlay === "profiles"
            ? profileChoices.length + 1
            : overlay === "collections"
              ? collectionsQuery.isError
                ? 1
                : collections.length
              : overlay === "search"
                ? cells.length
                : 0;

  // A download completing or a profile list changing can shorten an overlay
  // under the highlight; never leave the ring on a row that no longer exists.
  useEffect(() => {
    if (!overlay || overlay === "search") return;
    setOverlayIndex((current) => clampIndex(current, Math.max(1, overlayCount)));
  }, [overlay, overlayCount]);

  // #142: opening Big Picture asks the server for the current library, installs
  // and update state. The queries keep their cached data, so this happens in the
  // background: no skeleton, no empty grid, and the shelf keeps focus.
  const refreshingRef = useRef(false);
  const refreshLibrary = useCallback(async () => {
    if (refreshingRef.current) return;
    refreshingRef.current = true;
    lastRefreshAt = Date.now();
    const results = await Promise.allSettled(
      [["games"], ["installed"], ["game-updates"]].map((queryKey) =>
        // cancelRefetch: false joins an in-flight mount fetch instead of
        // starting a second request for the same data.
        queryClient.refetchQueries({ queryKey, type: "active" }, { throwOnError: true, cancelRefetch: false }),
      ),
    );
    refreshingRef.current = false;
    const failure = results.find((result) => result.status === "rejected");
    if (failure?.status === "rejected") {
      // A failed refresh never replaces the cached shelf: report it and keep
      // playing. The next open or focus retries.
      push({
        title: t("Could not refresh the library"),
        description: errorMessage(failure.reason),
        tone: "danger",
      });
    }
  }, [push, queryClient]);

  const refreshRef = useRef(refreshLibrary);
  refreshRef.current = refreshLibrary;

  useEffect(() => {
    // One refresh per open, then on focus only when the data has aged and no
    // dialog, hidden tab or offline session is in the way.
    if (Date.now() - lastRefreshAt >= REFRESH_ON_OPEN_MS) void refreshRef.current();

    const onFocus = () => {
      if (document.visibilityState !== "visible") return;
      if (!navigator.onLine) return;
      if (document.querySelector('[role="dialog"][aria-modal="true"]')) return;
      if (Date.now() - lastRefreshAt < REFRESH_ON_FOCUS_MS) return;
      void refreshRef.current();
    };

    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    return () => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
    };
  }, []);

  // The cascade is for the first paint that actually shows a game: a cold
  // skeleton never eats the entrance. Cards that arrive later (a shelf switch,
  // a refresh adding an entry or a layout switch) mount settled instead of
  // replaying it.
  const ready = Boolean(selected);
  useEffect(() => {
    if (!ready) return;
    const timer = window.setTimeout(() => setEntranceDone(true), ENTRANCE_MS);
    return () => window.clearTimeout(timer);
  }, [ready]);

  // Big Picture owns the OS window in the desktop app: enter real fullscreen on
  // mount and hand the window back on the way out. The browser build only gets
  // the full-viewport layout, and a refused fullscreen never traps the user.
  // In console mode (#172) the native window is already fullscreen — or was
  // deliberately started windowed — so this only fills in for a manual visit.
  useEffect(() => {
    if (!isNative()) return;
    // Wait for the native console status so a windowed console session is not
    // forced fullscreen for a frame first.
    if (consoleMode === null) return;
    if (consoleActive && !consoleMode.fullscreen) return;
    let unmounted = false;
    let entered = false;

    const enter = async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        if ((await win.isFullscreen()) || unmounted) return;
        await win.setFullscreen(true);
        entered = true;
      } catch (cause) {
        console.warn("Big Picture could not enter fullscreen", cause);
        if (!unmounted) {
          push({
            title: "Fullscreen unavailable",
            description: "Staying in the in-app Big Picture view.",
            tone: "default",
          });
        }
      }
    };

    const start = enter();

    // Every exit (Esc, Backspace, gamepad B, the Exit button, a route change or
    // launching a game) unmounts this screen. Cleanup waits for a still-running
    // enter so it can restore only what Ludex itself took over.
    return () => {
      unmounted = true;
      void start.then(async () => {
        if (!entered) return;
        try {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          await getCurrentWindow().setFullscreen(false);
        } catch (cause) {
          console.warn("Big Picture could not leave fullscreen", cause);
        }
      });
    };
  }, [push, consoleMode, consoleActive]);

  const exit = () => {
    // A console session (#172) has one exit: quit back to the host desktop.
    if (consoleActive) {
      void native.exitConsole().catch((cause) => {
        console.warn("Console mode could not quit", cause);
        if (window.history.length > 1) navigate(-1);
        else navigate("/");
      });
      return;
    }
    if (window.history.length > 1) navigate(-1);
    else navigate("/");
  };

  /* ----------------------------- shell surfaces ---------------------------- */

  const syncLevelUrl = (next: ShellLevel, section?: SettingsSectionId) => {
    const params = new URLSearchParams(location.search);
    if (next.kind === "app") {
      params.set("app", next.id);
      if (next.id === "settings") params.set("section", section ?? settingsSection);
      else params.delete("section");
    } else {
      params.delete("app");
      params.delete("section");
    }
    setSearchParams(params, { replace: true });
    setLevelState(next);
  };

  const navRef = useRef(nav);
  navRef.current = nav;

  const focusSurface = useCallback((target: "home" | "app" | "library") => {
    if (target === "library") {
      navRef.current.focusSelected();
      return;
    }
    const selector = target === "home" ? "[data-home-active='true']" : "[data-system-active='true']";
    const element = document.querySelector<HTMLElement>(selector);
    if (!element) return;
    element.setAttribute("data-gamepad-focus", "true");
    element.focus({ preventScroll: true });
  }, []);

  const openApp = (id: "settings" | "power") => {
    setOverlay(null);
    setZone("library");
    setAppIndex(0);
    setPendingPower(null);
    syncLevelUrl(openSystemApp(id), id === "settings" ? settingsSection : undefined);
  };

  const closeApp = () => {
    setPendingPower(null);
    setAppIndex(0);
    syncLevelUrl(closeSystemApp());
    window.setTimeout(() => nav.focusSelected(), 0);
  };

  const selectSettingsSection = (next: SettingsSectionId) => {
    setSettingsSection(next);
    setAppIndex(0);
    syncLevelUrl({ kind: "app", id: "settings" }, next);
  };

  const enterHome = () => {
    setZone("home");
    setHomeIndex(0);
  };

  const returnToLibrary = () => {
    setZone("library");
    setOverlay(null);
    setPendingPower(null);
    window.setTimeout(() => nav.focusSelected(), 0);
  };

  const startGame = async (game: GameSummary, executable?: string | null) => {
    const entry = installed.get(game.id);
    try {
      const result = await launch.mutateAsync({
        id: game.id,
        title: game.title,
        executable: executable ?? entry?.executable ?? null,
      });
      setRunningGame({ sessionId: result.sessionId, gameId: game.id, title: game.title });
    } catch {
      /* `useLaunchGame` already shows the failure; keep the console mounted */
    }
  };

  const localeChoices = useMemo(() => selectableLocales(), []);
  const cycleLocale = async (delta: -1 | 1) => {
    const current = Math.max(0, localeChoices.findIndex((entry) => entry.code === locale));
    const next = localeChoices[wrapIndex(current, delta, localeChoices.length)];
    if (!next) return;
    try {
      if (!user || deviceOnly) await setLocale(next.code);
      else await updateLocale(next.code);
    } catch (cause) {
      push({ title: t("Could not save the language"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const useAccountDefault = async () => {
    try {
      await clearDeviceOverride();
      setDeviceOnly(false);
    } catch (cause) {
      push({ title: t("Could not save the language"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const themeChoices = ["system", "dark", "light"] as const;
  const themeChoice = theme === "system" ? "system" : resolved === "dark" ? "dark" : "light";
  const cycleTheme = (delta: -1 | 1) => {
    const next = themeChoices[wrapIndex(themeChoices.indexOf(themeChoice), delta, themeChoices.length)];
    setTheme(next === "system" ? "system" : next === "dark" ? "midnight" : "light");
  };

  const cycleLayout = (delta: -1 | 1) => {
    const next = BIG_PICTURE_LAYOUTS[wrapIndex(BIG_PICTURE_LAYOUTS.indexOf(layout), delta, BIG_PICTURE_LAYOUTS.length)];
    if (!next || next === layout) return;
    setLayout(next);
    setLayoutTouched(true);
  };

  const power = useMemo(
    () =>
      powerActions({
        console: consoleActive,
        native: isNative(),
        gameTitle: runningGame?.title ?? null,
      }),
    [consoleActive, runningGame?.title],
  );

  const runPower = async (action: PowerAction) => {
    try {
      switch (action.id) {
        case "exit":
          exit();
          return;
        case "end-session":
          await native.exitConsole();
          return;
        case "sleep":
          await native.systemSleep();
          return;
        case "restart":
          await native.restartApp();
          return;
        case "quit":
          try {
            await native.quitApp();
          } catch (cause) {
            // `quit_app` is still pending in the desktop lane. In a console
            // session the existing exit command is the honest fallback: the
            // player asked to end the app, not to stay trapped.
            if (consoleActive) {
              await native.exitConsole();
              return;
            }
            throw cause;
          }
          return;
        case "quit-game":
          if (!runningGame) return;
          await native.stopGame(runningGame.sessionId);
          setRunningGame(null);
          return;
      }
    } catch (cause) {
      push({ title: t("Not available"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const choosePower = (index: number) => {
    const action = power[index];
    if (!action) return;
    if (!action.enabled) {
      push({ title: t("Not available"), description: action.reason, tone: "danger" });
      return;
    }
    if (!needsPowerConfirm(action.id)) {
      void runPower(action);
      return;
    }
    setPendingPower({ action, index: powerConfirmActive() });
  };

  const chooseConfirm = (index: number) => {
    const pending = pendingPower;
    if (!pending) return;
    setPendingPower(null);
    if (resolvePowerConfirm(index) === "confirm") void runPower(pending.action);
  };

  const download = useCallback(async () => {
    if (!selected) return;
    try {
      await downloadGame(selected.id, version?.id);
    } catch (cause) {
      push({ title: t("Download did not start"), description: errorMessage(cause), tone: "danger" });
    }
  }, [selected, version?.id, push]);

  const confirm = () => {
    if (!selected) {
      push({ title: t("Unavailable"), description: t("No games"), tone: "danger" });
      return;
    }
    if (installedGame) {
      void startGame(selected, installedGame.executable ?? detail.data?.launch.executable);
    } else if (job) {
      // The queue stays in the console now (#196): pausing and retrying live
      // on the full screen, but the progress panel does not leave Big Picture.
      openOverlay("downloads");
    } else if (isNative()) {
      setInstallOpen(true);
    } else {
      void download();
    }
  };

  /* ------------------------------ the layout ------------------------------ */

  const highlightPicker = useCallback((index: number) => {
    pickerIndexRef.current = index;
    setPickerIndex(index);
  }, []);
  const openPicker = () => {
    highlightPicker(Math.max(0, BIG_PICTURE_LAYOUTS.indexOf(layout)));
    setPickerOpen(true);
  };
  const closePicker = () => setPickerOpen(false);

  const applyLayout = (next: BigPictureLayout) => {
    setPickerOpen(false);
    if (next === layout) return;
    setLayout(next);
    // A manual switch must not replay the #142 cascade: the new view mounts
    // settled, keeping the entrance to one per open.
    setLayoutTouched(true);
  };
  const movePicker = (delta: number) => {
    const count = BIG_PICTURE_LAYOUTS.length;
    highlightPicker((((pickerIndexRef.current + delta) % count) + count) % count);
  };

  /* ------------------------------- overlays ------------------------------- */

  const openOverlay = (next: BigPictureOverlay) => {
    overlayReturnRef.current = level.kind === "app" ? "app" : zone === "home" ? "home" : "library";
    setOverlay(next);
    if (next === "search") {
      setSearchIndex(0);
    } else if (next === "profiles") {
      const current = profileChoices.findIndex((option) => option.id === selectedProfile.id);
      setOverlayIndex(Math.max(0, current));
    } else if (next === "exit") {
      // Default to "Stay": a reflex A press must never quit the app.
      setOverlayIndex(1);
    } else {
      setOverlayIndex(0);
    }
  };

  const closeOverlay = (refocus = true) => {
    setOverlay(null);
    if (!refocus) return;
    const target = overlayReturnRef.current;
    // The card exists again after the overlay unmounts; the navigation hook's
    // ref already holds the new selection, so the ring lands on the right one.
    window.setTimeout(() => {
      if (target === "home") focusSurface("home");
      else if (target === "app") focusSurface("app");
      else nav.focusSelected();
    }, 0);
  };

  const moveOverlay = (direction: "up" | "down" | "left" | "right") => {
    if (overlay === "search") {
      setSearchIndex((current) => moveSearchTarget(rows, current, direction));
      return;
    }
    const delta = direction === "up" || direction === "left" ? -1 : 1;
    setOverlayIndex((current) => clampIndex(current + delta, overlayCount));
  };

  const pickResult = (gameId: number) => {
    const target = findGameShelf(shelves, gameId);
    if (target) nav.selectGame(target.shelf, target.index);
    closeOverlay();
  };

  const typeSearchKey = (key: string) => {
    setQuery((current) => applySearchKey(current, key));
    searchInputRef.current?.focus({ preventScroll: true });
  };

  const applySearchStep = (step: SearchStep) => {
    if (step === "submit") {
      // Enter/RB opens the result under the ring, or the best match when the
      // ring is still on the keyboard.
      const current = cells[searchIndex];
      if (current?.kind === "result") return pickResult(current.game.id);
      const first = cells.find((cell) => cell.kind === "result");
      if (first && first.kind === "result") pickResult(first.game.id);
      return;
    }
    typeSearchKey(step);
  };

  const activateOverlay = (index = overlayIndex) => {
    switch (overlay) {
      case "menu": {
        const item = COMMAND_ITEMS[index];
        if (!item) return;
        switch (item.id) {
          case "play":
            closeOverlay(false);
            return confirm();
          case "game":
            if (!selected) return;
            closeOverlay(false);
            return navigate(`/game/${selected.id}`);
          case "search":
            return openOverlay("search");
          case "downloads":
            return openOverlay("downloads");
          case "profiles":
            return openOverlay("profiles");
          case "layout":
            closeOverlay(false);
            return openPicker();
          case "settings":
            closeOverlay(false);
            return openApp("settings");
          case "exit":
            closeOverlay(false);
            return openApp("power");
        }
        return;
      }
      case "exit":
        if (index === 0) exit();
        else closeOverlay();
        return;
      case "collections": {
        if (collectionsQuery.isError) {
          void collectionsQuery.refetch();
          return;
        }
        const collection = collections[index];
        if (!collection) return;
        closeOverlay(false);
        navigate(`/collections/${collection.id}`);
        return;
      }
      case "downloads":
        // Leaving the route is deliberate: pausing and retrying live there, and
        // the queue keeps running either way.
        navigate("/downloads");
        return;
      case "profiles": {
        const option = profileChoices[index];
        if (option) {
          setPrefs((current) => ({ ...current, profileId: option.id === "auto" ? null : option.id }));
          return;
        }
        if (index === profileChoices.length) {
          setPrefs((current) => ({ ...current, haptics: !current.haptics }));
        }
        return;
      }
      case "search": {
        const cell = cells[searchIndex];
        if (!cell) return;
        if (cell.kind === "key") return typeSearchKey(cell.key);
        return pickResult(cell.game.id);
      }
      default:
        return;
    }
  };

  const runCommand = (command: BigPictureCommand) => {
    switch (command.type) {
      case "move":
        if (
          zone === "library" &&
          command.direction === "up" &&
          atLibraryTop({ layout, index: nav.index, shelfIndex: nav.shelfIndex, columns })
        ) {
          enterHome();
          return;
        }
        nav.actions[command.direction]();
        return;
      case "shelf":
        if (command.delta < 0) nav.actions.prevShelf();
        else nav.actions.nextShelf();
        return;
      case "page":
        nav.actions.page(command.delta);
        return;
      case "activate":
        confirm();
        return;
      case "open":
        openOverlay(command.overlay);
        return;
      case "close":
        closeOverlay();
        return;
      case "exit":
        exit();
        return;
      case "highlight":
        moveOverlay(command.direction);
        return;
      case "select":
        activateOverlay();
        return;
      case "search":
        applySearchStep(command.step);
        return;
      case "layout":
        openPicker();
        return;
      case "game":
        if (selected) navigate(`/game/${selected.id}`);
        return;
      case "power":
        openApp("power");
        return;
      case "none":
        return;
    }
  };
  const runRef = useRef(runCommand);
  runRef.current = runCommand;

  // Focus follows the highlight in every list overlay. The search overlay is
  // the exception: its real input keeps focus so physical typing works, and the
  // ring is the highlighted cell.
  useEffect(() => {
    if (!overlay) return;
    const active = document.querySelector<HTMLElement>("[data-overlay-active='true']");
    if (!active) return;
    // `pointerdown` clears every mark, and React will not rewrite an unchanged
    // attribute; re-marking here keeps the ring visible after a mouse click.
    active.setAttribute("data-gamepad-focus", "true");
    if (overlay !== "search") active.focus({ preventScroll: true });
  }, [overlay, overlayIndex]);

  useEffect(() => {
    if (overlay !== "search") return;
    searchInputRef.current?.focus({ preventScroll: true });
  }, [overlay]);

  // Picker (the quick layout switcher) keeps its own arrows: the overlays below
  // never see input while it is open.
  const movePickerRef = useRef(movePicker);
  movePickerRef.current = movePicker;
  const applyPickerRef = useRef<() => void>(() => {});
  applyPickerRef.current = () =>
    applyLayout(BIG_PICTURE_LAYOUTS[pickerIndexRef.current] ?? layout);

  // Gamepad: the same commands as the keyboard, resolved from the active pad
  // action and the physical input that produced it (#138).
  const onGamepad = (action: GamepadAction, source: ActionSource | null) => {
    if (pickerOpen) {
      if (action === "back" || action === "menu") return closePicker();
      if (action === "confirm") return applyLayout(BIG_PICTURE_LAYOUTS[pickerIndexRef.current] ?? layout);
      if (action === "left") return movePicker(-1);
      if (action === "right") return movePicker(1);
      return;
    }

    if (pendingPower) {
      if (action === "back" || action === "menu") return setPendingPower(null);
      if (action === "confirm") return chooseConfirm(pendingPower.index);
      if (action === "left" || action === "up") return setPendingPower({ ...pendingPower, index: 0 });
      if (action === "right" || action === "down") return setPendingPower({ ...pendingPower, index: 1 });
      return;
    }

    if (overlay) {
      runRef.current(routeGamepadAction(overlay, action, source));
      return;
    }

    if (level.kind === "app") {
      if (action === "back") return closeApp();
      if (level.id === "power") {
        if (action === "confirm") return choosePower(appIndex);
        if (action === "left" || action === "right" || action === "up" || action === "down") {
          setAppIndex((current) => powerGridStep(current, action, power.length));
        }
        return;
      }
      if (action === "prevTab") return selectSettingsSection(stepSettingsSection(settingsSection, -1));
      if (action === "nextTab") return selectSettingsSection(stepSettingsSection(settingsSection, 1));
      if (action === "confirm") return activateSetting(appIndex);
      if (action === "left") return adjustSetting(appIndex, -1);
      if (action === "right") return adjustSetting(appIndex, 1);
      if (action === "up") return setAppIndex((current) => clampIndex(current - 1, settingsRows.length));
      if (action === "down") return setAppIndex((current) => clampIndex(current + 1, settingsRows.length));
      return;
    }

    if (zone === "home") {
      switch (action) {
        case "left":
          return setHomeIndex((current) => wrapIndex(current, -1, HOME_ITEM_IDS.length));
        case "right":
          return setHomeIndex((current) => wrapIndex(current, 1, HOME_ITEM_IDS.length));
        case "up":
          return;
        case "down":
        case "back":
          return returnToLibrary();
        case "confirm":
          return activateHome(homeIndex);
        case "menu":
          return openOverlay("menu");
        case "downloads":
          return openOverlay("downloads");
        default:
          return;
      }
    }

    runRef.current(routeGamepadAction(null, action, source));
  };
  const gamepad = useGamepad(onGamepad, {
    profile: activeProfile,
    debug,
    haptics: prefs.haptics,
  });
  const padFamilyName = padFamily(gamepad.id, activeProfile);

  // After a layout switch the ring must come back to the same game. Only when
  // the pad owns focus: a mouse click on the switcher keeps its own focus.
  const lastLayout = useRef(layout);
  useEffect(() => {
    if (lastLayout.current === layout) return;
    lastLayout.current = layout;
    if (!gamepad.connected) return;
    const active = document.activeElement as HTMLElement | null;
    const ownsFocus =
      !active ||
      active === document.body ||
      active.hasAttribute("data-gamepad-focus") ||
      active.closest("[data-gamepad-target]") !== null;
    if (ownsFocus) nav.focusSelected();
  }, [layout, gamepad.connected, nav]);

  const recentGame = shelves.find((shelf) => shelf.id === "recent")?.games[0] ?? null;
  const themeValue =
    themeChoice === "system" ? t("Follow the system") : themeChoice === "dark" ? t("Dark") : t("Light");

  /* ------------------------------ settings app ----------------------------- */

  const settingsRows: SettingsRow[] = (() => {
    if (settingsSection === "language") {
      const rows: SettingsRow[] = [
        {
          id: "language",
          label: t("Language"),
          detail: t("Applies to your account on every device. A device override stays on this device."),
          value: localeMeta(locale).label,
          kind: "choice",
        },
        {
          id: "device-only",
          label: t("Only this device"),
          detail: t("Changes apply immediately."),
          value: deviceOnly ? t("On") : t("Off"),
          kind: "toggle",
        },
      ];
      if (deviceOverride) {
        rows.push({
          id: "account-default",
          label: t("Use account default"),
          detail: t("Applies to your account on every device. A device override stays on this device."),
          value: t("Select"),
          kind: "action",
        });
      }
      return rows;
    }
    if (settingsSection === "appearance") {
      return [
        {
          id: "layout",
          label: t("Layout"),
          detail: t("Change how the library is presented."),
          value: layoutLabel(layout),
          kind: "choice",
        },
        {
          id: "theme",
          label: t("Theme"),
          detail: t("Theme applies to this browser; language is saved to your account."),
          value: themeValue,
          kind: "choice",
        },
      ];
    }
    if (settingsSection === "controllers") {
      return [
        {
          id: "profile",
          label: t("Controller profile"),
          detail: gamepad.connected ? `${padFamilyLabel(padFamilyName)} · ${activeProfile.name}` : t("No controller detected."),
          value: activeProfile.name,
          kind: "action",
        },
        {
          id: "vibration",
          label: t("Vibration"),
          detail: t("Switch the pad setup and vibration."),
          value: prefs.haptics ? t("On") : t("Off"),
          kind: "toggle",
        },
      ];
    }
    return [
      {
        id: "remote-play",
        label: t("Remote play"),
        detail: t("Publish the Ludex library as Sunshine apps and start streams from Moonlight. Ludex never bundles either application."),
        value: t("Set up remote play"),
        kind: "action",
      },
    ];
  })();

  const activateSetting = (index: number) => {
    const row = settingsRows[index];
    if (!row || row.disabled) return;
    switch (row.id) {
      case "language":
        void cycleLocale(1);
        return;
      case "device-only":
        setDeviceOnly((current) => !current);
        return;
      case "account-default":
        void useAccountDefault();
        return;
      case "layout":
        cycleLayout(1);
        return;
      case "theme":
        cycleTheme(1);
        return;
      case "profile":
        openOverlay("profiles");
        return;
      case "vibration":
        setPrefs((current) => ({ ...current, haptics: !current.haptics }));
        return;
      case "remote-play":
        navigate("/remote");
        return;
    }
  };

  const adjustSetting = (index: number, delta: -1 | 1) => {
    const row = settingsRows[index];
    if (!row || row.disabled) return;
    switch (row.id) {
      case "language":
        void cycleLocale(delta);
        return;
      case "layout":
        cycleLayout(delta);
        return;
      case "theme":
        cycleTheme(delta);
        return;
      default:
        return;
    }
  };

  /* -------------------------------- home row ------------------------------- */

  const totalCollectionGames = collections.reduce((sum, collection) => sum + collection.gameCount, 0);
  const homeDetails: Record<HomeItemId, { detail: string; disabled?: boolean }> = {
    continue: recentGame
      ? {
          detail: installed.has(recentGame.id)
            ? recentGame.title
            : `${recentGame.title} · ${t("Install")}`,
        }
      : { detail: t("Nothing played yet"), disabled: true },
    library: { detail: t("All games") },
    downloads:
      downloads.activeCount > 0
        ? {
            detail: `${tn(downloads.activeCount, "1 downloading", "{count} downloading")}${
              downloads.aggregate?.percent !== null && downloads.aggregate
                ? ` · ${formatPercent(downloads.aggregate.percent)}`
                : ""
            }`,
          }
        : { detail: t("No downloads running.") },
    collections: collectionsQuery.isPending
      ? { detail: t("Loading…") }
      : collectionsQuery.isError
        ? { detail: t("Unavailable") }
        : collections.length === 0
          ? { detail: t("No collections yet.") }
          : { detail: t("{count} games", { count: totalCollectionGames }) },
    settings: { detail: `${layoutLabel(layout)} · ${themeValue}` },
    power: { detail: consoleActive ? t("End session") : t("Quit") },
  };

  const activateHome = (index: number) => {
    const id = HOME_ITEM_IDS[index];
    if (!id) return;
    switch (id) {
      case "continue": {
        if (!recentGame) {
          returnToLibrary();
          return;
        }
        const target = findGameShelf(shelves, recentGame.id);
        if (target) nav.selectGame(target.shelf, target.index);
        setZone("library");
        if (installed.has(recentGame.id)) void startGame(recentGame);
        else window.setTimeout(() => nav.focusSelected(), 0);
        return;
      }
      case "library":
        returnToLibrary();
        return;
      case "downloads":
        openOverlay("downloads");
        return;
      case "collections":
        openOverlay("collections");
        return;
      case "settings":
        openApp("settings");
        return;
      case "power":
        openApp("power");
        return;
    }
  };

  /* ------------------------------- focus sync ------------------------------ */

  useEffect(() => {
    if (level.kind !== "library" || zone !== "home" || overlay || pendingPower) return;
    focusSurface("home");
  }, [level, zone, homeIndex, overlay, pendingPower, focusSurface]);

  useEffect(() => {
    if (level.kind !== "app" || overlay || pendingPower) return;
    focusSurface("app");
  }, [level, appIndex, settingsSection, overlay, pendingPower, focusSurface]);

  useEffect(() => {
    setAppIndex((current) => {
      if (level.kind !== "app") return 0;
      const count = level.id === "power" ? power.length : settingsRows.length;
      return clampIndex(current, Math.max(1, count));
    });
  }, [level, power.length, settingsRows.length]);

  useEffect(() => {
    if (!pendingPower) {
      if (!confirmWasOpen.current) return;
      confirmWasOpen.current = false;
      // The confirm rows unmount before this runs; put the ring back on the
      // Power tile instead of leaving DOM focus on the body.
      window.setTimeout(() => focusSurface(level.kind === "app" ? "app" : "library"), 0);
      return;
    }
    confirmWasOpen.current = true;
    const active = document.querySelector<HTMLElement>("[data-overlay-active='true']");
    if (!active) return;
    active.setAttribute("data-gamepad-focus", "true");
    active.focus({ preventScroll: true });
  }, [pendingPower, level, focusSurface]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (document.querySelector('[role="dialog"][aria-modal="true"]')) return;
      const key = event.key;

      // The console quit chord (#172) wins over every screen shortcut: it is
      // the only way back to the desktop when Sunshine owns the session.
      if (consoleActive && matchesExitShortcut(event, consoleMode?.exitShortcut)) {
        event.preventDefault();
        exit();
        return;
      }

      // The quick switcher owns the arrows while it is open, so a stray Enter
      // never launches a game behind it.
      if (pickerOpen) {
        if (key === "ArrowLeft") {
          event.preventDefault();
          movePickerRef.current(-1);
        } else if (key === "ArrowRight") {
          event.preventDefault();
          movePickerRef.current(1);
        } else if (key === "Enter") {
          event.preventDefault();
          applyPickerRef.current();
        } else if (key === "Escape" || key.toLowerCase() === "l") {
          event.preventDefault();
          closePicker();
        }
        return;
      }

      // Overlays keep their router; the shell only sees keys when none is open.
      if (overlay) {
        const command = routeKey(overlay, key);
        if (!command) return;
        event.preventDefault();
        runRef.current(command);
        return;
      }

      if (pendingPower) {
        if (key === "Enter") {
          event.preventDefault();
          chooseConfirm(pendingPower.index);
        } else if (key === "Escape" || key === "Backspace") {
          event.preventDefault();
          setPendingPower(null);
        } else if (key === "ArrowLeft" || key === "ArrowUp") {
          event.preventDefault();
          setPendingPower({ ...pendingPower, index: 0 });
        } else if (key === "ArrowRight" || key === "ArrowDown") {
          event.preventDefault();
          setPendingPower({ ...pendingPower, index: 1 });
        }
        return;
      }

      if (level.kind === "app") {
        if (key === "Escape" || key === "Backspace") {
          event.preventDefault();
          closeApp();
          return;
        }
        if (level.id === "power") {
          if (key === "Enter") {
            event.preventDefault();
            choosePower(appIndex);
          } else if (key === "ArrowLeft" || key === "ArrowRight" || key === "ArrowUp" || key === "ArrowDown") {
            event.preventDefault();
            setAppIndex((current) =>
              powerGridStep(current, key.slice(5).toLowerCase() as "left" | "right" | "up" | "down", power.length),
            );
          }
          return;
        }
        if (key === "Enter") {
          event.preventDefault();
          activateSetting(appIndex);
        } else if (key === "ArrowLeft") {
          event.preventDefault();
          adjustSetting(appIndex, -1);
        } else if (key === "ArrowRight") {
          event.preventDefault();
          adjustSetting(appIndex, 1);
        } else if (key === "ArrowUp") {
          event.preventDefault();
          setAppIndex((current) => clampIndex(current - 1, settingsRows.length));
        } else if (key === "ArrowDown") {
          event.preventDefault();
          setAppIndex((current) => clampIndex(current + 1, settingsRows.length));
        } else if (key === "[" || key === "]") {
          event.preventDefault();
          selectSettingsSection(stepSettingsSection(settingsSection, key === "[" ? -1 : 1));
        }
        return;
      }

      if (zone === "home") {
        if (key === "ArrowLeft") {
          event.preventDefault();
          setHomeIndex((current) => wrapIndex(current, -1, HOME_ITEM_IDS.length));
          return;
        }
        if (key === "ArrowRight") {
          event.preventDefault();
          setHomeIndex((current) => wrapIndex(current, 1, HOME_ITEM_IDS.length));
          return;
        }
        if (key === "ArrowUp") {
          event.preventDefault();
          return;
        }
        if (key === "ArrowDown" || key === "Backspace") {
          event.preventDefault();
          returnToLibrary();
          return;
        }
        if (key === "Enter") {
          event.preventDefault();
          activateHome(homeIndex);
          return;
        }
        if (key === "Escape") {
          event.preventDefault();
          if (consoleActive) openApp("power");
          else exit();
          return;
        }
      }

      // In console mode Escape is a back/confirm gesture, never a silent
      // process exit: the player gets the Power surface and a Cancel default.
      if (consoleActive && key === "Escape") {
        event.preventDefault();
        openApp("power");
        return;
      }

      const command = routeKey(null, key);
      // `null` means the DOM owns the key: printable characters and the native
      // backspace in the search input stay native.
      if (!command) return;
      event.preventDefault();
      runRef.current(command);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    overlay,
    pickerOpen,
    consoleActive,
    consoleMode?.exitShortcut,
    pendingPower,
    level,
    zone,
    homeIndex,
    appIndex,
    settingsSection,
    settingsRows,
    power,
  ]);

  const primary = !selected ? null : installedGame ? (
    <Button
      variant="primary"
      size="lg"
      icon={<Play className="size-4" />}
      onClick={() => void startGame(selected, installedGame?.executable ?? detail.data?.launch.executable)}
    >
      {t("Play")}
    </Button>
  ) : job ? (
    <Button variant="default" size="lg" icon={<Download className="size-4" />} onClick={() => openOverlay("downloads")}>
      {t("Installing {percent}", { percent: percent !== null ? formatPercent(percent) : "" })}
    </Button>
  ) : isNative() ? (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} disabled={!version} onClick={() => setInstallOpen(true)}>
      {t("Install")}
    </Button>
  ) : (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => void download()}>
      {t("Download")}
    </Button>
  );

  const viewProps: BigPictureLayoutProps = {
    nav,
    layout,
    columns,
    reduceMotion,
    settled: entranceDone || layoutTouched,
    detail: detail.data,
    installed,
    job,
    percent,
    primary,
    onOpenGame: (gameId) => navigate(`/game/${gameId}`),
    jobFor: downloads.jobForGame,
  };

  const body = isPending ? (
    <LayoutSkeleton layout={layout} columns={columns} />
  ) : isError && games.length === 0 ? (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-4 px-6 text-center">
      <Wordmark height={22} />
      <p className="text-[15px] text-ink-2">{t("The library could not be loaded.")}</p>
      <p className="max-w-[56ch] font-mono text-[11px] leading-relaxed text-ink-3">{errorMessage(error)}</p>
      <div className="flex flex-wrap items-center justify-center gap-2">
        <Button variant="primary" onClick={() => void refetch()}>
          {t("Try again")}
        </Button>
        <Button variant="default" onClick={exit}>
          {t("Exit Big Picture")}
        </Button>
      </div>
    </div>
  ) : !selected ? (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-5 px-6 text-center">
      <Wordmark height={22} />
      <p className="text-[15px] text-ink-2">{t("The library is empty. Add a game first.")}</p>
      <Button variant="primary" onClick={exit}>
        {t("Exit Big Picture")}
      </Button>
    </div>
  ) : layout === "hero" ? (
    <HeroLayout {...viewProps} />
  ) : layout === "grid" ? (
    <GridLayout {...viewProps} />
  ) : layout === "compact" ? (
    <CompactLayout {...viewProps} />
  ) : (
    <ShelfLayout {...viewProps} />
  );

  const overlayShell = {
    layout,
    connected: gamepad.connected,
    family: padFamilyName,
    reduceMotion,
    onClose: () => closeOverlay(),
  };

  const footerContext = pendingPower
    ? "exit"
    : overlay
      ? hintContextFor(overlay)
      : level.kind === "app"
        ? level.id
        : zone === "home"
          ? "home"
          : "library";

  return (
    <div className="flex h-dvh flex-col overflow-hidden bg-bg-deep text-ink">
      <div className="grain" aria-hidden="true" />

      <SystemBar
        user={user}
        serverLabel={serverLabel}
        online={online}
        padConnected={gamepad.connected}
        padFamily={padFamilyLabel(padFamilyName)}
        padBattery={padBattery?.level ?? null}
        padCharging={padBattery?.charging ?? false}
        downloads={downloads.activeCount}
        downloadsPercent={downloads.aggregate?.percent ?? null}
        refreshing={!isPending && isFetching}
        consoleActive={consoleActive}
      />

      {level.kind === "library" && (
        <HomeRow
          active={homeIndex}
          focused={zone === "home"}
          details={homeDetails}
          onHighlight={setHomeIndex}
          onChoose={activateHome}
        />
      )}

      <main className="relative z-10 flex min-h-0 flex-1 flex-col">
        {level.kind === "app" && level.id === "settings" ? (
          <SettingsApp
            section={settingsSection}
            rows={settingsRows}
            active={appIndex}
            accountName={user?.username ?? null}
            accountAvatar={user?.avatarUrl ?? null}
            serverLabel={serverLabel}
            version={__APP_VERSION__}
            padLabel={
              gamepad.connected
                ? `${padFamilyLabel(padFamilyName)} · ${activeProfile.name}`
                : t("No controller detected.")
            }
            onSelectSection={selectSettingsSection}
            onHighlight={setAppIndex}
            onActivate={activateSetting}
          />
        ) : level.kind === "app" && level.id === "power" ? (
          <PowerApp
            actions={power}
            active={appIndex}
            onHighlight={setAppIndex}
            onChoose={choosePower}
          />
        ) : (
          body
        )}
      </main>

      <BigPictureFooter
        layout={layout}
        connected={gamepad.connected}
        family={padFamilyName}
        profileName={selectedProfile.profile.name}
        context={footerContext}
        haptics={haptics}
      />

      {overlay === "menu" && (
        <CommandMenu
          {...overlayShell}
          context="menu"
          active={overlayIndex}
          onHighlight={setOverlayIndex}
          onChoose={(index) => {
            setOverlayIndex(index);
            activateOverlay(index);
          }}
          downloads={downloads.activeCount}
        />
      )}
      {overlay === "downloads" && (
        <DownloadsOverlay
          {...overlayShell}
          context="downloads"
          active={overlayIndex}
          onHighlight={setOverlayIndex}
          onChoose={(index) => {
            setOverlayIndex(index);
            activateOverlay(index);
          }}
          jobs={downloads.active}
          titleFor={downloads.titleFor}
        />
      )}
      {overlay === "profiles" && (
        <ProfileOverlay
          {...overlayShell}
          context="profiles"
          active={overlayIndex}
          onHighlight={setOverlayIndex}
          onChoose={(index) => {
            setOverlayIndex(index);
            activateOverlay(index);
          }}
          options={profileChoices}
          selectedId={selectedProfile.id}
          profile={activeProfile}
          haptics={prefs.haptics}
        />
      )}
      {overlay === "search" && (
        <SearchOverlay
          {...overlayShell}
          context="search"
          inputRef={searchInputRef}
          query={query}
          onQuery={setQuery}
          rows={rows}
          active={searchIndex}
          onHighlight={setSearchIndex}
          onChoose={(index) => {
            setSearchIndex(index);
            // The highlight may still be the previous index inside this handler;
            // choose the cell that was actually clicked.
            const cell = cells[index];
            if (!cell) return;
            if (cell.kind === "key") {
              typeSearchKey(cell.key);
              return;
            }
            pickResult(cell.game.id);
          }}
        />
      )}

      {overlay === "collections" && (
        <CollectionsOverlay
          {...overlayShell}
          context="collections"
          active={overlayIndex}
          onHighlight={setOverlayIndex}
          onChoose={(index) => {
            setOverlayIndex(index);
            activateOverlay(index);
          }}
          collections={collections}
          loading={collectionsQuery.isPending}
          error={collectionsQuery.isError ? errorMessage(collectionsQuery.error) : null}
          onRetry={() => void collectionsQuery.refetch()}
        />
      )}
      {pendingPower && (
        <ConfirmPanel
          title={pendingPower.action.label}
          detail={pendingPower.action.detail}
          confirmLabel={pendingPower.action.label}
          tone={pendingPower.action.tone === "danger" ? "danger" : undefined}
          active={pendingPower.index}
          onHighlight={(index) => setPendingPower((current) => (current ? { ...current, index } : current))}
          onChoose={chooseConfirm}
          context="exit"
          layout={layout}
          connected={gamepad.connected}
          family={padFamilyName}
          reduceMotion={reduceMotion}
          onClose={() => setPendingPower(null)}
        />
      )}

      {version && detail.data && (
        <InstallDialog open={installOpen} onOpenChange={setInstallOpen} game={detail.data} version={version} />
      )}
      {debug && <GamepadDebugPanel info={gamepad.debug} />}
      {pickerOpen && (
        <LayoutPicker
          layout={layout}
          highlight={pickerIndex}
          reduceMotion={reduceMotion}
          onHighlight={highlightPicker}
          onSelect={applyLayout}
          onClose={closePicker}
        />
      )}
    </div>
  );
}
