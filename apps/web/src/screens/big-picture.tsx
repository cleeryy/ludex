import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useReducedMotion } from "motion/react";
import { Download, Play, RefreshCw, X } from "lucide-react";
import { api } from "@/lib/api";
import { downloadGame } from "@/lib/download";
import { errorMessage } from "@/lib/http";
import { formatPercent } from "@/lib/format";
import { isNative } from "@/lib/native";
import { useAllGames, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { useGamepad, gamepadDebugEnabled, type GamepadAction } from "@/lib/gamepad";
import { GamepadDebugPanel } from "@/components/gamepad/gamepad-debug";
import { useControllerProfile } from "@/lib/use-controller-profile";
import { useDownloads } from "@/state/downloads";
import { useSettings } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { Wordmark } from "@/components/brand";
import { InstallDialog } from "@/components/game/install-dialog";
import { Button } from "@/components/ui/controls";
import { ENTRANCE_MS } from "@/components/big-picture/entrance";
import { CompactLayout } from "@/components/big-picture/layout-compact";
import { GridLayout } from "@/components/big-picture/layout-grid";
import { HeroLayout } from "@/components/big-picture/layout-hero";
import { ShelfLayout } from "@/components/big-picture/layout-shelf";
import { LayoutSkeleton } from "@/components/big-picture/layout-skeleton";
import { LayoutPicker, LayoutTrigger } from "@/components/big-picture/layout-switcher";
import {
  BIG_PICTURE_LAYOUTS,
  gridColumns,
  type BigPictureLayout,
} from "@/components/big-picture/layout";
import { jobPercent, type BigPictureLayoutProps } from "@/components/big-picture/model";
import { useBigPictureNavigation, type BigPictureShelf } from "@/components/big-picture/navigation";
import { BigPictureFooter } from "@/components/big-picture/parts";
import { useBigPictureLayout, useViewportWidth } from "@/components/big-picture/use-layout";
import { t, tn } from "@/lib/i18n";

/** Ask the server once per open; a quick round trip through the route is free. */
const REFRESH_ON_OPEN_MS = 10_000;
/** Regaining focus only refetches when the shelf has had time to age. */
const REFRESH_ON_FOCUS_MS = 30_000;
let lastRefreshAt = 0;

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
  // Mirror of the highlight that is always in step with the last key press:
  // a fast A after two d-pad clicks must apply the card that is on screen.
  const pickerIndexRef = useRef(0);
  const reduceMotion = useReducedMotion();
  const columns = gridColumns(useViewportWidth());

  // Big Picture is watched from a couch: the provider lifts text to 7:1 and
  // widens the focus ring while this screen is mounted (#140).
  useLayoutEffect(() => {
    setTv(true);
    return () => setTv(false);
  }, [setTv]);

  const games = data?.items ?? [];

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
  useEffect(() => {
    if (!isNative()) return;
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
  }, [push]);

  const exit = () => {
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

  // One action map for the keyboard and the pad. Movement and shelf changes go
  // through the navigation hook, so a layout switch never re-wires them.
  const controls = useRef({
    left: () => {},
    right: () => {},
    up: () => {},
    down: () => {},
    prevShelf: () => {},
    nextShelf: () => {},
    confirm: () => {},
    back: () => {},
    downloads: () => {},
    menu: () => {},
    applyPicker: () => {},
    movePicker: (_delta: number) => {},
    closePicker: () => {},
  });
  controls.current = {
    left: nav.actions.left,
    right: nav.actions.right,
    up: nav.actions.up,
    down: nav.actions.down,
    prevShelf: nav.actions.prevShelf,
    nextShelf: nav.actions.nextShelf,
    confirm,
    back: exit,
    downloads: () => navigate("/downloads"),
    menu: openPicker,
    applyPicker: () => applyLayout(BIG_PICTURE_LAYOUTS[pickerIndexRef.current] ?? layout),
    movePicker,
    closePicker,
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (document.querySelector('[role="dialog"][aria-modal="true"]')) return;
      const key = event.key;

      // The quick switcher owns the arrows while it is open, so a stray Enter
      // never launches a game behind it.
      if (pickerOpen) {
        if (key === "ArrowLeft") {
          event.preventDefault();
          controls.current.movePicker(-1);
        } else if (key === "ArrowRight") {
          event.preventDefault();
          controls.current.movePicker(1);
        } else if (key === "Enter") {
          event.preventDefault();
          controls.current.applyPicker();
        } else if (key === "Escape" || key.toLowerCase() === "l") {
          event.preventDefault();
          controls.current.closePicker();
        }
        return;
      }

      if (key === "ArrowLeft") {
        event.preventDefault();
        controls.current.left();
      } else if (key === "ArrowRight") {
        event.preventDefault();
        controls.current.right();
      } else if (key === "ArrowUp") {
        event.preventDefault();
        controls.current.up();
      } else if (key === "ArrowDown") {
        event.preventDefault();
        controls.current.down();
      } else if (key === "Enter") {
        event.preventDefault();
        controls.current.confirm();
      } else if (key === "Escape" || key === "Backspace") {
        event.preventDefault();
        controls.current.back();
      } else if (key === "[") {
        event.preventDefault();
        controls.current.prevShelf();
      } else if (key === "]") {
        event.preventDefault();
        controls.current.nextShelf();
      } else if (key.toLowerCase() === "d") {
        controls.current.downloads();
      } else if (key.toLowerCase() === "l") {
        event.preventDefault();
        controls.current.menu();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pickerOpen]);

  // Gamepad: d-pad/stick moves, A confirms, B exits, X opens the layout
  // switcher, Y opens downloads and the shoulders change shelf (the same
  // actions as the arrows). Back, X and downloads stay live on an empty shelf.
  const onGamepad = (action: GamepadAction) => {
    if (pickerOpen) {
      if (action === "back" || action === "menu") return closePicker();
      if (action === "confirm") return applyLayout(BIG_PICTURE_LAYOUTS[pickerIndexRef.current] ?? layout);
      if (action === "left") return movePicker(-1);
      if (action === "right") return movePicker(1);
      return;
    }

    if (action === "back") return exit();
    if (action === "downloads") return navigate("/downloads");
    if (action === "menu") return openPicker();
    if (action === "prevTab") return nav.actions.prevShelf();
    if (action === "nextTab") return nav.actions.nextShelf();
    if (!selected) return;
    if (action === "left") nav.actions.left();
    else if (action === "right") nav.actions.right();
    else if (action === "up") nav.actions.up();
    else if (action === "down") nav.actions.down();
    else if (action === "confirm") confirm();
  };
  const controllerProfile = useControllerProfile(selected?.id ?? null);
  const gamepad = useGamepad(onGamepad, { profile: controllerProfile, debug });

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
            {t("Exit")}
          </Button>
        </div>
      </header>

      <main className="relative z-10 flex min-h-0 flex-1 flex-col">{body}</main>

      <BigPictureFooter layout={layout} connected={gamepad.connected} />

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
