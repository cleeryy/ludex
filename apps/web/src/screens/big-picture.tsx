import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useReducedMotion } from "motion/react";
import { Download, Gamepad2, Play, RefreshCw, X } from "lucide-react";
import { api } from "@/lib/api";
import { downloadGame } from "@/lib/download";
import { errorMessage } from "@/lib/http";
import { formatPercent } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import type { ConsoleStatus } from "@/lib/types";
import { useAllGames, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { useGamepad, gamepadDebugEnabled, type ActionSource, type GamepadAction } from "@/lib/gamepad";
import { GamepadDebugPanel } from "@/components/gamepad/gamepad-debug";
import { useControllerProfile } from "@/lib/use-controller-profile";
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
import { LayoutPicker, LayoutTrigger } from "@/components/big-picture/layout-switcher";
import {
  BIG_PICTURE_LAYOUTS,
  clampIndex,
  gridColumns,
  type BigPictureLayout,
} from "@/components/big-picture/layout";
import { jobPercent, type BigPictureLayoutProps } from "@/components/big-picture/model";
import { useBigPictureNavigation, type BigPictureShelf } from "@/components/big-picture/navigation";
import { BigPictureFooter } from "@/components/big-picture/parts";
import { CommandMenu, COMMAND_ITEMS, DownloadsOverlay, ExitOverlay } from "@/components/big-picture/overlays";
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
import { t, tn } from "@/lib/i18n";

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
  const debug = gamepadDebugEnabled(location.search);
  const { push } = useToast();
  const queryClient = useQueryClient();
  const { data, isPending, isError, error, isFetching, refetch } = useAllGames();
  const installed = useInstalledMap();
  const downloads = useDownloads();
  const launch = useLaunchGame();
  const [layout, setLayout] = useBigPictureLayout();
  const [installOpen, setInstallOpen] = useState(false);
  const [clock, setClock] = useState(() => new Date());
  const [entranceDone, setEntranceDone] = useState(false);
  const [layoutTouched, setLayoutTouched] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerIndex, setPickerIndex] = useState(0);
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
            : overlay === "search"
              ? cells.length
              : 0;

  // A download completing or a profile list changing can shorten an overlay
  // under the highlight; never leave the ring on a row that no longer exists.
  useEffect(() => {
    if (!overlay || overlay === "search") return;
    setOverlayIndex((current) => clampIndex(current, Math.max(1, overlayCount)));
  }, [overlay, overlayCount]);

  useEffect(() => {
    const timer = setInterval(() => setClock(new Date()), 30_000);
    return () => clearInterval(timer);
  }, []);

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

  const download = useCallback(async () => {
    if (!selected) return;
    try {
      await downloadGame(selected.id, version?.id);
    } catch (cause) {
      push({ title: t("Download did not start"), description: errorMessage(cause), tone: "danger" });
    }
  }, [selected, version?.id, push]);

  const confirm = () => {
    if (!selected) return;
    if (installedGame) {
      launch.mutate({
        id: selected.id,
        title: selected.title,
        executable: installedGame.executable ?? detail.data?.launch.executable,
      });
    } else if (job) {
      navigate("/downloads");
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
  const togglePicker = () => (pickerOpen ? closePicker() : openPicker());

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
    if (refocus) {
      // The card exists again after the overlay unmounts; the navigation hook's
      // ref already holds the new selection, so the ring lands on the right one.
      window.setTimeout(() => nav.focusSelected(), 0);
    }
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
            return navigate("/settings");
          case "exit":
            return openOverlay("exit");
        }
        return;
      }
      case "exit":
        if (index === 0) exit();
        else closeOverlay();
        return;
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

      const command = routeKey(overlay, key);
      // `null` means the DOM owns the key: printable characters and the native
      // backspace in the search input stay native.
      if (!command) return;
      event.preventDefault();
      runRef.current(command);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [overlay, pickerOpen, consoleActive, consoleMode?.exitShortcut]);

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
    runRef.current(routeGamepadAction(overlay, action, source));
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

  const time = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit" }).format(clock);

  const primary = !selected ? null : installedGame ? (
    <Button
      variant="primary"
      size="lg"
      icon={<Play className="size-4" />}
      onClick={() =>
        launch.mutate({
          id: selected.id,
          title: selected.title,
          executable: installedGame.executable ?? detail.data?.launch.executable,
        })
      }
    >
      {t("Play")}
    </Button>
  ) : job ? (
    <Button variant="default" size="lg" icon={<Download className="size-4" />} onClick={() => navigate("/downloads")}>
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

  return (
    <div className="flex h-dvh flex-col overflow-hidden bg-bg-deep text-ink">
      <div className="grain" aria-hidden="true" />

      <header className="relative z-20 flex h-16 shrink-0 items-center gap-4 border-b border-line px-5 lg:px-7">
        <Wordmark height={17} />
        <span className="hidden font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3 sm:inline">{t("Big Picture")}</span>
        {!isPending && isFetching && (
          <span
            role="status"
            className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.14em] text-ink-3"
          >
            <RefreshCw className="size-3 animate-spin [animation-duration:1.8s] motion-reduce:animate-none" aria-hidden="true" />
            <span className="sr-only sm:not-sr-only">{t("Refreshing")}</span>
          </span>
        )}
        <div className="ml-auto flex items-center gap-3">
          {gamepad.connected && (
            <span
              className="hidden items-center gap-1.5 border border-line px-2 py-1 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3 md:flex"
              title={selectedProfile.label}
            >
              <Gamepad2 className="size-3.5 text-signal-text" aria-hidden="true" />
              <span className="text-ink-2">{padFamilyLabel(padFamilyName)}</span>
              <span aria-hidden="true">·</span>
              <span className="max-w-[16ch] truncate normal-case tracking-normal text-ink-3">
                {selectedProfile.profile.name}
              </span>
            </span>
          )}
          {downloads.activeCount > 0 && (
            <button
              type="button"
              onClick={() => navigate("/downloads")}
              className="flex h-8 items-center gap-2 border border-line px-2.5 text-[11.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
            >
              <Download className="size-3.5 text-signal-text" aria-hidden="true" />
              <span>{tn(downloads.activeCount, "1 downloading", "{count} downloading")}</span>
              {downloads.aggregate?.percent !== null && downloads.aggregate && (
                <span className="num text-ink-3">{formatPercent(downloads.aggregate.percent)}</span>
              )}
            </button>
          )}
          <LayoutTrigger layout={layout} open={pickerOpen} onToggle={togglePicker} />
          <span className="num hidden text-[12px] text-ink-2 sm:inline">{time}</span>
          <Button variant="ghost" size="sm" icon={<X className="size-3.5" />} onClick={exit}>
            {consoleActive ? t("Quit") : t("Exit")}
          </Button>
        </div>
      </header>

      <main className="relative z-10 flex min-h-0 flex-1 flex-col">{body}</main>

      <BigPictureFooter
        layout={layout}
        connected={gamepad.connected}
        family={padFamilyName}
        profileName={selectedProfile.profile.name}
        context={hintContextFor(overlay)}
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
      {overlay === "exit" && (
        <ExitOverlay
          {...overlayShell}
          context="exit"
          active={overlayIndex}
          onHighlight={setOverlayIndex}
          onChoose={(index) => {
            setOverlayIndex(index);
            activateOverlay(index);
          }}
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
