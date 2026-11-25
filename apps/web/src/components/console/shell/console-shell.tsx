import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ComponentType } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { api, assetUrl } from "@/lib/api";
import { downloadGame } from "@/lib/download";
import { useDocumentTitle } from "@/lib/document-title";
import { errorMessage } from "@/lib/http";
import { isNative, native, type SleepCapability } from "@/lib/native";
import { listServers } from "@/lib/servers";
import type { GameSummary } from "@/lib/types";
import { useInstalledMap, useLaunchGame, useLibraryGames } from "@/lib/hooks";
import { focusGamepadTarget, gamepadDebugEnabled, type ActionSource, type GamepadAction } from "@/lib/gamepad";
import { GamepadDebugPanel } from "@/components/gamepad/gamepad-debug";
import { axisSamplesFor, pickActivePad, useConnectedPads, useControllerDevices } from "@/lib/use-controller-devices";
import { resolveProfileForPad } from "@/lib/controller-resolve";
import { sourceFromId, sourceId } from "@/lib/controller-devices";
import { presetProfile } from "@/lib/controller-profiles";
import { identityLabel, padFingerprint, shortFingerprint } from "@/lib/controller-identity";
import { uiSound } from "@/lib/ui-sound";
import type { CueId } from "@/lib/ui-sound-specs";
import { useAuth } from "@/state/auth";
import { useActiveCount, useJobPhases, useTitleFor } from "@/state/downloads";
import { useSettings } from "@/state/settings";
import { useInstallAction } from "@/lib/use-install-action";
import { Wordmark } from "@/components/brand";
import { installBridge } from "@/lib/install";
import { ConsoleGameSurface } from "@/components/console/surfaces/console-game-surface";
import { Button } from "@/components/ui/controls";
import { Glyph } from "@/components/big-picture/glyphs";
import { useConsoleNotice } from "@/components/console/surfaces/console-notice";
import { useInput, useInputScope, type InputScopeOptions } from "@/components/console/input/input-provider";
import { clearFocusMarks, setFocusMark } from "@/components/console/input/focus-store";
import { ConsoleOverlayInputScope } from "@/components/console/input/overlay-scope";
import { consolePathFor, type ConsoleSurface } from "@/components/console/routes/surface";
import { useConsoleSession } from "@/components/console/shell/use-console-session";
import { useConsoleSettings } from "@/components/console/shell/use-console-settings";
import { useConsoleAudio } from "@/components/console/shell/use-console-audio";
import { ConsoleVolumeOsd } from "@/components/console/surfaces/console-volume";
import { ENTRANCE_MS } from "@/components/big-picture/entrance";
import {
  AUTO_PROFILE_ID,
  cycleHapticsStrength,
  padFamily,
  padFamilyLabel,
  profileOptions,
  withBigPictureDefaults,
} from "@/components/big-picture/controller";
import { cueForCommand } from "@/components/big-picture/cues";
import { useBigPicturePrefs } from "@/components/big-picture/use-controller-prefs";
import {
  createTransitionHub,
  cueForMotion,
  exitOf,
  INSTANT_HOLD_MS,
  motionDebugEnabled,
  transitionOf,
  type MotionKind,
} from "@/components/big-picture/motion";
import { MotionProbe } from "@/components/big-picture/motion-probe";
import {
  LAUNCH_CEILING_MS,
  LAUNCH_FAIL_HOLD_MS,
  LAUNCH_MIN_HOLD_MS,
  plateCanDismiss,
  type LaunchPlateStatus,
  type PlateRect,
} from "@/components/big-picture/launch-plate";
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
import { ShelfLayout } from "@/components/big-picture/layout-shelf";
import { LayoutSkeleton } from "@/components/big-picture/layout-skeleton";
import {
  clampIndex,
  gridColumns,
  type BigPictureLayout,
  type MoveDirection,
} from "@/components/big-picture/layout";
import { consoleRoomLabel, layoutLabel } from "@/components/big-picture/labels";
import { ConsoleBackdrop } from "@/components/big-picture/console-backdrop";
import { RoomPreview } from "@/components/big-picture/room-preview";
import { useConsoleRoom } from "@/components/big-picture/use-console-room";
import {
  cabinetTokens,
  consoleRoomAttributes,
  motionProfile,
  soundEnabled,
  soundReduced,
} from "@/theme/console-room";
import { cssVarsFor } from "@/theme/registry";
import { extractArtworkAccent } from "@/theme/artwork-accent";
import { BUILTIN_THEMES } from "@/theme/registry";
import type { BigPictureLayoutProps } from "@/components/big-picture/model";
import { useBigPictureNavigation, type BigPictureShelf } from "@/components/big-picture/navigation";
import { BigPictureFooter } from "@/components/big-picture/parts";
import { COMMAND_ITEMS } from "@/components/big-picture/overlays";
import { ConsoleDialogs } from "@/components/console/surfaces/console-dialogs";
import { ConsoleOverlays } from "@/components/console/surfaces/console-overlays";
import { HomeBand, type BandItemDetail } from "@/components/big-picture/home-row";
import { PowerApp } from "@/components/big-picture/power-app";
import { SettingsApp } from "@/components/big-picture/settings-app";
import { SystemBar } from "@/components/big-picture/system-bar";
import {
  HOME_ITEM_IDS,
  SETTINGS_SECTIONS,
  closeSystemApp,
  isSettingsSection,
  openSystemApp,
  restoreZone,
  sameLevel,
  stepSettingsSection,
  wrapIndex,
  zoneExit,
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
import { usePointerIdle } from "@/components/big-picture/pointer-idle";
import { t, tn, useI18n } from "@/lib/i18n";
import { volumeKeyAction } from "@/lib/volume-osd";
import { qk } from "@/lib/query-keys";

/** Ask the server once per open; a quick round trip through the route is free. */
const REFRESH_ON_OPEN_MS = 10_000;
/** Regaining focus only refetches when the shelf has had time to age. */
const REFRESH_ON_FOCUS_MS = 30_000;
let lastRefreshAt = 0;

/**
 * Every layout is wired here, with no fallback: adding a seventh id is a
 * compile error until its view exists (#197). The screen's body never guesses.
 */
const LAYOUT_VIEWS: Record<BigPictureLayout, ComponentType<BigPictureLayoutProps>> = {
  shelf: ShelfLayout,
  grid: GridLayout,
  compact: CompactLayout,
};

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

/** The shell level a surface renders: library for home/library/downloads/search. */
function levelForSurface(surface: ConsoleSurface): ShellLevel {
  if (surface === "settings") return { kind: "app", id: "settings" };
  if (surface === "power") return { kind: "app", id: "power" };
  return { kind: "library" };
}

/** `/console/settings/:section`; an unknown slug keeps the last room. */
function settingsSectionFromSlug(slug: string | null): SettingsSectionId {
  return isSettingsSection(slug) ? slug : SETTINGS_SECTIONS[0];
}

/** Downloads and search are routes, not modal overlays (#267). */
function overlayForSurface(surface: ConsoleSurface): BigPictureOverlay | null {
  return surface === "downloads" || surface === "search" ? surface : null;
}

/**
 * Console 2.0 shell (#267): the same archive language at ten feet (#141) —
 * shelf, hero, cover grid and compact list — driven by one selection model,
 * one key map and one pad owner. The layout is a device preference
 * (`use-layout.ts`).
 *
 * The shell is mounted by the `/console/*` routes; its surface comes from the
 * URL, so a deep link, a browser Back and an in-shell push all land in the
 * same place. Desktop globals (toasts, update dialog, overlay host) never
 * mount here: errors render on the console's own notice surface.
 */
export interface ConsoleShellProps {
  surface: ConsoleSurface;
  gameId?: number | null;
  section?: string | null;
}

export function ConsoleShell({ surface, gameId = null, section = null }: ConsoleShellProps) {
  const navigate = useNavigate();
  const { setTv } = useSettings();
  useDocumentTitle(t("Console"));
  const debug = gamepadDebugEnabled();
  const { notify: push } = useConsoleNotice();
  const { user, signOut } = useAuth();
  const { deviceOverride } = useI18n();
  const { theme, resolved, setTheme, tokens: themeTokens } = useSettings();
  const queryClient = useQueryClient();
  const library = useLibraryGames();
  const games = library.items;
  const { isPending, isError, isFetching, refetch } = library;
  const installed = useInstalledMap();
  const jobPhases = useJobPhases();
  const activeCount = useActiveCount();
  const titleFor = useTitleFor();
  // The plate is the only messaging channel in Console (#268).
  const launch = useLaunchGame({ silent: true });
  const [layout, setLayout] = useBigPictureLayout();
  const [room, setRoom] = useConsoleRoom();
  const [installOpen, setInstallOpen] = useState(false);
  const [entranceDone, setEntranceDone] = useState(false);
  const [layoutTouched, setLayoutTouched] = useState(false);
  const [artworkAccent, setArtworkAccent] = useState<string | null>(null);
  // #196/#267: the shell level follows the route. Local state keeps the push
  // and pop animations honest while the URL stays the source of truth.
  const [level, setLevelState] = useState<ShellLevel>(() => levelForSurface(surface));
  const [zone, setZone] = useState<FocusZone>("library");
  const [homeIndex, setHomeIndex] = useState(0);
  const [appIndex, setAppIndex] = useState(0);
  const [settingsSection, setSettingsSection] = useState<SettingsSectionId>(
    () => settingsSectionFromSlug(section),
  );
  const [deviceOnly, setDeviceOnly] = useState(() => deviceOverride !== null);
  const [pendingPower, setPendingPower] = useState<{ action: PowerAction; index: number } | null>(null);
  const confirmWasOpen = useRef(false);
  // #229: the launch handoff plate. It opens on the Play press (cover FLIP,
  // dimmed library, hold frame), ends on the launch promise, and gives up at
  // 8 s with the honest line.
  const [launchPlate, setLaunchPlate] = useState<{
    game: GameSummary;
    rect: PlateRect | null;
    status: LaunchPlateStatus;
    detail: string | null;
  } | null>(null);
  // #267: the machine's own state (console mode, fullscreen, battery, the
  // running game, online) lives in one session hook.
  const {
    online,
    padBattery,
    consoleMode,
    consoleActive,
    runningGame,
    setRunningGame,
    exit,
  } = useConsoleSession();
  // The motion knob folds into the platform preference: Calm is the reduced
  // path (short fade, no spring), Off lands on the final frame.
  const prefersReducedMotion = useReducedMotion();
  const roomMotion = motionProfile(room, prefersReducedMotion);
  const reduceMotion = roomMotion.reduced;
  const columns = gridColumns(useViewportWidth());

  // #220: one motion hub for the whole screen. Every transition announces
  // itself here before the state change, so the next press can cut the old
  // tween and the dev probe can report what actually ran. The hub never gates
  // input: `accepts()` is always true and the animation is replaced on the
  // next frame.
  const transitions = useRef(createTransitionHub()).current;
  // A held key or a repeated pad direction lands on the final frame instead of
  // restarting the move. The ref is set by the input handler before the state
  // update that renders the transition; the effect clears it after the commit.
  const heldInput = useRef(false);
  const instantTimer = useRef<number | undefined>(undefined);
  // `?motionDebug=1`: the probe reads the top-level query and the hash query.
  const debugMotion = motionDebugEnabled();

  // #198: one emitter for every cue. The sound always plays; haptics only when
  // the press came from a pad, so a keyboard or mouse never rumbles. The
  // manager collapses a double emission inside the cue's repeat gap.
  const padGesture = useRef(false);
  const pulseRef = useRef<(cue: CueId) => void>(() => undefined);
  const emit = useCallback((cue: CueId | null) => {
    if (!cue) return;
    uiSound.play(cue);
    if (padGesture.current) pulseRef.current(cue);
  }, []);

  /**
   * Starts a transition and speaks its cue exactly once, at the motion start
   * the cue describes (#220). Surfaces own their cue now; the command router
   * only speaks for movement.
   */
  const beginMotion = useCallback(
    (kind: MotionKind, instant = heldInput.current) => {
      transitions.begin(kind, { instant });
      emit(cueForMotion(kind));
    },
    [emit, transitions],
  );

  // #138 controller-first state: the open overlay, its highlight, the couch
  // search query (debounced by the filter below) and the device-local profile
  // choice. None of these leave Big Picture when they change.
  const [overlay, setOverlay] = useState<BigPictureOverlay | null>(() => overlayForSurface(surface));
  const [overlayIndex, setOverlayIndex] = useState(0);
  const overlayReturnRef = useRef<"library" | "home" | "actions" | "app">("library");
  /**
   * The re-focus a closed sheet owes the surface it covered (#270). It is an
   * intent run from a commit effect, not a `setTimeout`: a timer raced the
   * scope pop, so the pop's snapshot restore could land *after* the ring was
   * already back on the card and take the mark away for a frame. A passive
   * effect always runs after the child scope's unmount cleanup, which fixes the
   * order — the restore goes back on the surface, then this intent, the fresher
   * of the two, is drawn last.
   */
  const overlayFocusRef = useRef<(() => void) | null>(null);
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [searchIndex, setSearchIndex] = useState(0);
  const [prefs, updatePrefs] = useBigPicturePrefs();
  const searchInputRef = useRef<HTMLInputElement>(null);

  // Big Picture is watched from a couch: the provider lifts text to 7:1 and
  // widens the focus ring while this screen is mounted (#140).
  useLayoutEffect(() => {
    setTv(true);
    return () => setTv(false);
  }, [setTv]);

  // #218: a mouse cursor left on the artwork reads as a bug at ten feet. The
  // root hides it after a pause and the first pointer movement brings it back;
  // it only ever applies inside this screen.
  const rootRef = useRef<HTMLDivElement>(null);
  usePointerIdle(rootRef);

  // #220: the held/repeat flag is read during the render the input caused, then
  // cleared once that commit is on screen. `data-bp-instant` lives until the
  // fallback timer removes it: it must outlive the commit's style resolution,
  // so a held direction lands at its final scale instead of starting a tween.
  const instantMotion = heldInput.current || roomMotion.instant;
  useEffect(() => {
    heldInput.current = false;
  });
  useEffect(
    () => () => {
      if (instantTimer.current !== undefined) window.clearTimeout(instantTimer.current);
    },
    [],
  );

  // A layout switch animates; a remount with the same layout (returning from an
  // app, a language change) must not. The witness is read during render and
  // advanced after commit.
  const layoutSeen = useRef(layout);
  const layoutSwitched = layoutSeen.current !== layout;
  useLayoutEffect(() => {
    layoutSeen.current = layout;
  }, [layout]);

  // The audio context is created suspended and resumed on the first gesture;
  // a browser that refuses it simply stays silent. Every cue is decoded in
  // parallel so the first press after mount already has its buffer.
  useEffect(() => {
    void uiSound.preload();
    const unlock = () => uiSound.resume();
    window.addEventListener("pointerdown", unlock, { passive: true });
    window.addEventListener("keydown", unlock);
    return () => {
      window.removeEventListener("pointerdown", unlock);
      window.removeEventListener("keydown", unlock);
      uiSound.stopAll();
    };
  }, []);

  // The sound knob owns enable/reduce; the volume row keeps the device volume
  // it always had. One effect, because the two settings answer one question.
  useEffect(() => {
    uiSound.setPrefs({ ...prefs.sound, enabled: soundEnabled(room) });
    uiSound.setReducedMotion(Boolean(reduceMotion) || soundReduced(room));
  }, [prefs.sound, room.sound, reduceMotion]);

  // No cue may play while a game is running, even if this window is visible.
  useEffect(() => {
    uiSound.setGameRunning(runningGame !== null);
  }, [runningGame]);

  // #267: the route owns the shell level and the route-backed overlays. A
  // browser Back, a language remount and a cold deep link all arrive here.
  const routeLevel = useMemo(() => levelForSurface(surface), [surface]);
  useEffect(() => {
    setLevelState((current) => (sameLevel(current, routeLevel) ? current : routeLevel));
    if (routeLevel.kind === "app" && routeLevel.id === "settings") {
      setSettingsSection(settingsSectionFromSlug(section));
    }
    if (surface === "downloads" || surface === "search") {
      setOverlay((current) => (current === surface ? current : surface));
    } else if (surface === "home" || surface === "library" || surface === "game") {
      setOverlay((current) =>
        current === "downloads" || current === "search" ? null : current,
      );
    }
  }, [routeLevel, section, surface]);

  // A job that reaches `done` while the Console is open gets the completion
  // cue; an install that fails gets the error cue. #267: the witness is the
  // phase string from the downloads selector, not the job list, so a progress
  // tick never re-renders the shell.
  const phaseWitness = useRef<Map<string, string> | null>(null);
  useEffect(() => {
    const list = jobPhases
      ? jobPhases.split("|").map((entry) => {
          const at = entry.lastIndexOf(":");
          return { jobId: entry.slice(0, at), phase: entry.slice(at + 1) };
        })
      : [];
    if (phaseWitness.current === null) {
      if (list.length === 0) return;
      phaseWitness.current = new Map(list.map((job) => [job.jobId, job.phase]));
      return;
    }
    const previous = phaseWitness.current;
    const next = new Map<string, string>();
    for (const job of list) {
      next.set(job.jobId, job.phase);
      const before = previous.get(job.jobId);
      if (job.phase === before) continue;
      if (job.phase === "done") emit("installComplete");
      else if (job.phase === "error" && before !== undefined) emit("error");
    }
    phaseWitness.current = next;
  }, [jobPhases, emit]);

  // #269: the OS sleep probe, so the Power row is disabled with the real reason
  // instead of failing on press. A desktop without `sleep_capability` answers
  // null and keeps the pre-#269 behaviour.
  const [sleep, setSleep] = useState<SleepCapability | null>(null);
  useEffect(() => {
    if (!isNative()) return;
    let cancelled = false;
    void native
      .sleepCapability()
      .then((capability) => {
        if (!cancelled) setSleep(capability);
      })
      .catch(() => {
        /* keep the row enabled and report the real error on press */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const shelves = useMemo<BigPictureShelf[]>(() => {
    const recent = games
      .filter((game) => game.lastPlayedAt)
      .sort((a, b) => Date.parse(b.lastPlayedAt ?? "") - Date.parse(a.lastPlayedAt ?? ""));
    const installedGames = games
      .filter((game) => installed.has(game.id))
      .sort((a, b) => Date.parse(b.lastPlayedAt ?? "") - Date.parse(a.lastPlayedAt ?? ""));
    const bookmarked = games.filter((game) => game.bookmarked);
    return [
      { id: "recent", label: t("Recently played"), games: recent },
      { id: "installed", label: t("Installed"), games: installedGames },
      { id: "bookmarks", label: t("Bookmarks"), games: bookmarked },
      { id: "all", label: t("All games"), games },
    ].filter((shelf) => shelf.games.length > 0);
  }, [games, installed]);

  const nav = useBigPictureNavigation({
    shelves,
    layout,
    columns,
    data: games,
    reduceMotion,
    instant: instantMotion,
  });
  const selected = nav.selected;

  const detail = useQuery({
    queryKey: qk.game.detail(selected?.id),
    queryFn: () => api.getGame(selected!.id),
    enabled: Boolean(selected),
    staleTime: 60_000,
  });
  const version = detail.data?.versions[0];
  const installedGame = selected ? installed.get(selected.id) : undefined;
  // #267: one game's job through a selector; the shell no longer holds the
  // whole active list, so another game's progress never repaints this screen.
  // #237: the primary button now comes from the shared action matrix; the
  // Console leaves pause/resume to the Downloads overlay (#196).
  const installAction = useInstallAction(selected?.id ?? -1, {
    installed: Boolean(installedGame),
    archiveAvailable: Boolean(version),
    offerResume: false,
    resumeInPlace: false,
  });
  const job = installAction.activeJob;
  const percent = installAction.percent;

  // #267: a 600-game library is three server pages. Ask for the next one when
  // the ring is near the end of what is loaded, so stepping right never runs
  // out of shelf at the 200-row cap. The ref stops two commits in the same tick
  // from requesting the same page twice.
  const pageFetching = useRef(false);
  useEffect(() => {
    if (!library.hasNextPage || library.isFetchingNextPage || pageFetching.current) return;
    const index = nav.index;
    const count = nav.games.length;
    if (count === 0 || index < count - 12) return;
    pageFetching.current = true;
    void library.fetchNextPage().finally(() => {
      pageFetching.current = false;
    });
  }, [library, nav.index, nav.games.length]);

  // #267: `/console/game/:id` is a real deep link. Select that game once its
  // page is loaded; a typo leaves the current selection alone rather than
  // pretending a game exists.
  const gameRouteApplied = useRef<number | null>(null);
  useEffect(() => {
    if (surface !== "game") {
      gameRouteApplied.current = null;
      return;
    }
    if (gameId === null || games.length === 0) return;
    if (gameRouteApplied.current === gameId) return;
    const target = findGameShelf(shelves, gameId);
    gameRouteApplied.current = gameId;
    if (target) nav.selectGame(target.shelf, target.index);
  }, [surface, gameId, games.length, shelves, nav]);

  // #268: Cabinet resolves the app's rendered tokens through the room, then
  // through the TV-safe pass, so every pair the console paints clears 7:1.
  // Daylight renders the app's own light map; no second palette is shipped.
  const lightTokens = useMemo(
    () => BUILTIN_THEMES.find((candidate) => candidate.id === "bauhaus")?.tokens ?? themeTokens,
    [themeTokens],
  );
  const darkTokens = useMemo(
    () => BUILTIN_THEMES.find((candidate) => candidate.id === "speranza")?.tokens ?? themeTokens,
    [themeTokens],
  );
  const cabinet = useMemo(
    () =>
      cabinetTokens({
        settings: room,
        tokens: themeTokens,
        lightTokens,
        darkTokens,
        artworkAccent,
        mode: resolved,
      }),
    [room, themeTokens, lightTokens, darkTokens, artworkAccent, resolved],
  );
  const backdropArt =
    detail.data?.backgroundUrl ??
    selected?.backgroundUrl ??
    detail.data?.screenshots?.[0] ??
    selected?.coverUrl ??
    null;

  // "From artwork" reads the selected cover once, off the focus path, and
  // caches by game id. A cover with no usable colour leaves the theme signal.
  useEffect(() => {
    if (room.accent !== "artwork" || !selected) {
      setArtworkAccent(null);
      return;
    }
    let cancelled = false;
    void extractArtworkAccent({
      url: assetUrl(selected.coverUrl) ?? assetUrl(selected.backgroundUrl),
      gameId: selected.id,
    }).then((color) => {
      if (!cancelled) setArtworkAccent(color);
    });
    return () => {
      cancelled = true;
    };
  }, [room.accent, selected?.id]);

  const serversQuery = useQuery({
    queryKey: qk.servers,
    queryFn: listServers,
    staleTime: 60_000,
    retry: false,
  });
  const serverLabel = serversQuery.data?.servers.find((server) => server.active)?.label ?? null;

  // Collections are a home-row surface (#196): the count is live in the tile
  // and the list opens in the same non-modal shell as the queue. The query
  // only runs when the row or its overlay can actually use it.
  const collectionsQuery = useQuery({
    queryKey: qk.collections.root,
    queryFn: () => api.listCollections(),
    enabled: online && (level.kind === "library" || overlay === "collections"),
    staleTime: 60_000,
    retry: 1,
  });
  const collections = collectionsQuery.data ?? [];

  const notificationsQuery = useQuery({
    queryKey: qk.notifications.bell(),
    queryFn: () => api.notifications(10),
    enabled: online && Boolean(user),
    staleTime: 60_000,
    retry: false,
  });
  const notifications = notificationsQuery.data?.items ?? [];
  const unreadNotifications = notificationsQuery.data?.unreadCount ?? 0;
  const visibleNotifications = notifications.slice(0, 3);

  // The controller profiles (#51) that drive navigation. The automatic choice
  // is what the library screen resolves (account, then per-game override); an
  // explicit choice is a device preference and applies to the next press.
  const profilesQuery = useQuery({
    queryKey: qk.controllerProfiles,
    queryFn: () => api.controllerProfiles(),
    staleTime: 5 * 60_000,
  });
  // A memoised fallback: `presetProfile` builds a new object, and a fresh
  // profile every render would rebuild the pad resolution and re-register the
  // input scope in a loop (#267).
  const fallbackProfile = useMemo(() => presetProfile("xbox"), []);
  const accountProfile = profilesQuery.data?.global?.profile ?? fallbackProfile;
  const gameProfile =
    profilesQuery.data?.games.find((entry) => entry.gameId === selected?.id)?.profile ?? null;
  const profileChoices = useMemo(
    () => profileOptions(profilesQuery.data, accountProfile),
    [profilesQuery.data, accountProfile],
  );
  // #230: the pad's own fingerprint picks the profile first (Account / Game /
  // Preset), then the automatic chain above. A pad that reconnects with the
  // same fingerprint gets the same profile back; a non-standard pad gets the
  // generic fallback instead of a dead end.
  const connectedPads = useConnectedPads();
  const [devices, deviceStore] = useControllerDevices();
  const activePad = useMemo(
    () => pickActivePad(connectedPads, devices.last),
    [connectedPads, devices.last],
  );
  const padSources = useMemo(
    () => ({
      accountProfile,
      gameProfile,
      gameId: selected?.id ?? null,
      presets: profilesQuery.data?.presets,
      prefsProfileId: prefs.profileId,
    }),
    [accountProfile, gameProfile, selected?.id, profilesQuery.data?.presets, prefs.profileId],
  );
  const padResolution = useMemo(
    () =>
      resolveProfileForPad(
        activePad?.id ?? null,
        padFamily(activePad?.id ?? null, accountProfile),
        devices,
        {
          ...padSources,
          pad: activePad,
          samples: activePad ? axisSamplesFor(activePad.identity.fingerprint) : null,
        },
      ),
    [activePad, devices, padSources, accountProfile],
  );
  const activeProfile = useMemo(
    () => withBigPictureDefaults(padResolution.profile),
    [padResolution],
  );
  // The profile overlay ticks the profile the pad will really use: its device
  // entry first, then the console-wide choice.
  const selectedProfileId =
    padResolution.source.kind === "auto"
      ? prefs.profileId ?? AUTO_PROFILE_ID
      : sourceId(padResolution.source);
  // The haptics knob is the console's own answer (#268): Off silences the pad,
  // Standard and Strong reuse the engine's existing scales.
  const haptics = room.haptics !== "off" && activeProfile.vibration !== false;

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
          ? activeCount + 1
          : overlay === "notifications"
            ? notificationsQuery.isError
              ? 1
              : visibleNotifications.length + (unreadNotifications > 0 && visibleNotifications.length > 0 ? 1 : 0)
            : overlay === "profiles"
              ? profileChoices.length + 2
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
      [qk.games.root, qk.installed, qk.gameUpdates].map((queryKey) =>
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

  // The OS window is not this screen's business any more: `useConsoleSession`
  // reads the Rust window policy (#269) and takes or hands back fullscreen on
  // mount and unmount, and it owns the one `exit`.

  /* ----------------------------- shell surfaces ---------------------------- */

  const syncLevelUrl = (next: ShellLevel, nextSection?: SettingsSectionId) => {
    setLevelState(next);
    if (next.kind === "app") {
      const sectionForPath = nextSection ?? settingsSection;
      navigate(
        consolePathFor(next.id === "settings" ? "settings" : "power", { section: sectionForPath }),
        { replace: true },
      );
      return;
    }
    navigate(consolePathFor("library"), { replace: true });
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
    // One ring at a time: the game card keeps its selected ring, not the
    // gamepad outline it wore before the zone step.
    setFocusMark(element);
  }, []);

  /* ------------------------------ zone focus ------------------------------- */

  // The Spotlight action row is built by the layout (the primary node comes
  // from the screen), so the zone addresses its buttons in DOM order instead of
  // holding refs. One wrapper per action keeps the order stable.
  const actionTargets = () =>
    Array.from(document.querySelectorAll<HTMLElement>("[data-bp-action='true'] button"));

  const actionIndexRef = useRef(0);
  const zoneRef = useRef<FocusZone>(zone);
  zoneRef.current = zone;

  const focusActions = useCallback((index: number) => {
    const targets = actionTargets();
    if (targets.length === 0) return;
    const next = clampIndex(index, targets.length);
    actionIndexRef.current = next;
    focusGamepadTarget(targets, next, false, { scroll: false });
  }, []);

  const activateAction = (index: number) => {
    emit("confirm");
    const targets = actionTargets();
    targets[clampIndex(index, targets.length)]?.click();
  };

  const openApp = (id: "settings" | "power") => {
    beginMotion("screenPush");
    setOverlay(null);
    setZone("library");
    setAppIndex(0);
    setPendingPower(null);
    syncLevelUrl(openSystemApp(id), id === "settings" ? settingsSection : undefined);
  };

  const closeApp = () => {
    beginMotion("screenPop");
    setPendingPower(null);
    setAppIndex(0);
    syncLevelUrl(closeSystemApp());
    window.setTimeout(() => nav.focusSelected(), 0);
  };

  const selectSettingsSection = (next: SettingsSectionId) => {
    beginMotion("settings");
    setSettingsSection(next);
    setAppIndex(0);
    syncLevelUrl({ kind: "app", id: "settings" }, next);
  };

  const enterHome = () => {
    setZone("home");
    setHomeIndex(0);
  };

  const returnToLibrary = (cue: CueId | null = "click") => {
    emit(cue);
    setZone("library");
    setOverlay(null);
    setPendingPower(null);
    window.setTimeout(() => nav.focusSelected(), 0);
  };

  /**
   * The game row handed a direction to the shell (#197). Home lands on the
   * system row; actions lands on the Spotlight quick actions; the library is
   * where B and the exit direction always return.
   */
  const enterZone = (target: FocusZone) => {
    if (target === "home") {
      emit("move");
      enterHome();
      return;
    }
    if (target === "actions") {
      emit("move");
      setZone("actions");
      actionIndexRef.current = 0;
      window.setTimeout(() => focusActions(0), 0);
      return;
    }
    // A direction that points back at the games is a movement, not a "back".
    returnToLibrary(null);
  };

  /**
   * One direction inside a secondary zone. Left/right wrap the system ring and
   * clamp the action row; the direction that points back at the games leaves
   * the zone, and the opposite direction is a dead end on purpose.
   */
  const moveZone = (direction: MoveDirection) => {
    if (zone === "home") {
      if (direction === "left" || direction === "right") {
        emit("move");
        setHomeIndex((current) => wrapIndex(current, direction === "left" ? -1 : 1, HOME_ITEM_IDS.length));
      } else if (direction === zoneExit(layout, "home")) {
        // Leaving a zone with a direction is a movement, not a "back".
        returnToLibrary(null);
      }
      return;
    }
    if (zone === "actions") {
      if (direction === "left" || direction === "right") {
        const before = actionIndexRef.current;
        const next = clampIndex(before + (direction === "left" ? -1 : 1), actionTargets().length);
        if (next !== before) emit("move");
        focusActions(next);
      } else if (direction === zoneExit(layout, "actions")) {
        returnToLibrary(null);
      }
    }
  };

  /**
   * The selected card's artwork, in viewport px (#229). Measured at the press,
   * before React re-renders, so the launch plate starts exactly on the cover the
   * player is looking at. `null` when the game is not the selected card (a
   * recent-game launch with no card on screen) — the plate then fades in.
   */
  const coverRectOfSelected = (gameId: number): PlateRect | null => {
    if (selected?.id !== gameId) return null;
    const card = document.querySelector<HTMLElement>('[data-bp-card="true"][aria-current="true"]');
    const cover = card?.querySelector<HTMLElement>(".cover") ?? card;
    if (!cover) return null;
    const box = cover.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) return null;
    return { left: box.left, top: box.top, width: box.width, height: box.height };
  };

  const startGame = async (game: GameSummary, executable?: string | null) => {
    const entry = installed.get(game.id);
    // #229: the plate flies from the card the player is looking at, and the
    // launch cue is this press — one voice, said once, at the start.
    const rect = coverRectOfSelected(game.id);
    const openedAt = performance.now();
    setLaunchPlate({ game, rect, status: "launching", detail: null });
    beginMotion("launch");
    try {
      const result = await launch.mutateAsync({
        id: game.id,
        title: game.title,
        executable: executable ?? entry?.executable ?? null,
      });
      setRunningGame({ sessionId: result.sessionId, gameId: game.id, title: game.title });
      const close = () =>
        setLaunchPlate((current) => (current && current.game.id === game.id ? null : current));
      // A launch that resolves in 80 ms must not blink the FLIP away.
      const wait = LAUNCH_MIN_HOLD_MS - (performance.now() - openedAt);
      if (wait > 0) window.setTimeout(close, wait);
      else close();
    } catch (cause) {
      /* `useLaunchGame` already shows the failure; the cue says it too */
      emit("error");
      setLaunchPlate((current) =>
        current && current.game.id === game.id
          ? { ...current, status: "failed", detail: errorMessage(cause) }
          : current,
      );
      window.setTimeout(() => {
        setLaunchPlate((current) =>
          current && current.game.id === game.id && current.status === "failed" ? null : current,
        );
      }, LAUNCH_FAIL_HOLD_MS);
    }
  };

  /** A refused action always says why: error cue plus a visible reason. */
  const refuse = (reason?: string) => {
    emit("error");
    push({ title: t("Not available"), description: reason, tone: "danger" });
  };

  // #229: the plate ends on the launch promise. Past the ceiling it stops
  // pretending and says the true thing; it never spins forever.
  useEffect(() => {
    if (!launchPlate || launchPlate.status !== "launching") return;
    const gameId = launchPlate.game.id;
    const timer = window.setTimeout(() => {
      setLaunchPlate((current) =>
        current && current.game.id === gameId && current.status === "launching"
          ? { ...current, status: "slow" }
          : current,
      );
    }, LAUNCH_CEILING_MS);
    return () => window.clearTimeout(timer);
  }, [launchPlate]);

  /** `B` on the plate. Only a launch that has given up is dismissable. */
  const dismissLaunchPlate = useCallback(() => {
    setLaunchPlate((current) => (current && plateCanDismiss(current.status) ? null : current));
  }, []);

  const power = useMemo(
    () =>
      powerActions({
        console: consoleActive,
        native: isNative(),
        gameTitle: runningGame?.title ?? null,
        // #267: the download state lives in one store; `activeCount` is the same
        // set the old screen's `activeJobs.length` counted (#269).
        activeInstalls: activeCount,
        sleep,
      }),
    [consoleActive, runningGame?.title, activeCount, sleep],
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
          // The confirmation modal already ran (it defaults to Cancel), so an
          // active install is an explicit choice by the time we get here.
          await native.systemSleep({ confirmActiveInstalls: activeCount > 0 });
          return;
        case "restart":
          await native.restartApp();
          return;
        case "quit":
          try {
            await native.quitApp();
          } catch (cause) {
            // An older desktop without `quit_app` still ends the session through
            // the console exit: the player asked to stop, not to stay trapped.
            if (consoleActive) {
              await native.exitConsole();
              return;
            }
            throw cause;
          }
          return;
        case "sign-out":
          await signOut();
          return;
        case "quit-game":
          if (!runningGame) return;
          await native.stopGame(runningGame.sessionId);
          setRunningGame(null);
          return;
      }
    } catch (cause) {
      emit("error");
      push({ title: t("Not available"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const choosePower = (index: number) => {
    const action = power[index];
    if (!action) return;
    if (!action.enabled) {
      refuse(action.reason);
      return;
    }
    if (!needsPowerConfirm(action.id)) {
      emit("power");
      void runPower(action);
      return;
    }
    beginMotion("powerConfirm");
    setPendingPower({ action, index: powerConfirmActive() });
  };

  const chooseConfirm = (index: number) => {
    const pending = pendingPower;
    if (!pending) return;
    setPendingPower(null);
    if (resolvePowerConfirm(index) === "confirm") {
      emit("power");
      void runPower(pending.action);
    } else {
      emit("click");
    }
  };

  const download = useCallback(async () => {
    if (!selected) return;
    try {
      await downloadGame(selected.id, version?.id);
    } catch (cause) {
      emit("error");
      push({ title: t("Download did not start"), description: errorMessage(cause), tone: "danger" });
    }
  }, [selected, version?.id, push, emit]);

  const confirm = () => {
    if (!selected) {
      refuse(t("No games"));
      return;
    }
    // #237: the shared matrix decides; the Console only maps the intent to its
    // own surfaces (launch plate, Downloads overlay, install dialog, download).
    switch (installAction.intent) {
      case "play":
      case "resume-session":
        // #229: `startGame` speaks the launch cue at the press; a confirm here
        // would say the same press twice.
        void startGame(selected, installedGame?.executable ?? detail.data?.launch.executable);
        break;
      case "open-downloads":
      case "resume-install":
        // The queue stays in the console now (#196): pausing and retrying live
        // on the full screen, but the progress panel does not leave Big Picture.
        openOverlay("downloads");
        break;
      case "install":
        if (!online) {
          refuse(t("This device is offline. Installed games still play."));
          break;
        }
        emit("confirm");
        setInstallOpen(true);
        break;
      case "download":
        if (!online) {
          refuse(t("This device is offline. Installed games still play."));
          break;
        }
        emit("confirm");
        void download();
        break;
    }
  };

  /* ------------------------------- overlays ------------------------------- */

  const openOverlay = (next: BigPictureOverlay) => {
    beginMotion("overlayOpen");
    overlayReturnRef.current =
      level.kind === "app" ? "app" : zone === "home" ? "home" : zone === "actions" ? "actions" : "library";
    setOverlay(next);
    // #267: the queue and the couch search are routes. The state changes first
    // so the surface opens in this commit; the URL follows with `replace` so
    // Back still leaves the Console instead of walking through its panels.
    if (next === "downloads" || next === "search") {
      navigate(consolePathFor(next), { replace: true });
    }
    if (next === "search") {
      setSearchIndex(0);
    } else if (next === "profiles") {
      const current = profileChoices.findIndex((option) => option.id === selectedProfileId);
      setOverlayIndex(Math.max(0, current));
    } else if (next === "exit") {
      // Default to "Stay": a reflex A press must never quit the app.
      setOverlayIndex(1);
    } else {
      setOverlayIndex(0);
    }
  };

  const closeOverlay = (refocus = true) => {
    // A transition to another surface (menu → search) is not a close: the new
    // surface plays its own open cue.
    if (refocus && overlay) beginMotion("overlayClose");
    if (overlay === "downloads" || overlay === "search") {
      navigate(consolePathFor("library"), { replace: true });
    }
    setOverlay(null);
    if (!refocus) return;
    const target = overlayReturnRef.current;
    // The card exists again once the sheet has unmounted, and the navigation
    // hook's ref already holds the new selection (a search pick moves it while
    // the sheet is up), so the ring lands on the right one.
    overlayFocusRef.current = () => {
      if (target === "home") focusSurface("home");
      else if (target === "actions") focusActions(actionIndexRef.current);
      else if (target === "app") focusSurface("app");
      else navRef.current.focusSelected();
    };
  };

  // The sheet is gone and the input scope has restored what it covered; draw
  // the surface's own target last (see `overlayFocusRef`).
  const sheetOpen = Boolean(overlay || pendingPower || launchPlate);
  useEffect(() => {
    if (sheetOpen) return;
    const intent = overlayFocusRef.current;
    if (!intent) return;
    overlayFocusRef.current = null;
    intent();
  }, [sheetOpen]);

  /** Would this direction change an overlay's highlight? Edge presses are silent. */
  const overlayWillMove = (direction: MoveDirection): boolean => {
    if (overlay === "search") return moveSearchTarget(rows, searchIndex, direction) !== searchIndex;
    if (!overlay) return false;
    const delta = direction === "up" || direction === "left" ? -1 : 1;
    return clampIndex(overlayIndex + delta, overlayCount) !== overlayIndex;
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
            emit("confirm");
            closeOverlay(false);
            return navigate(consolePathFor("game", { gameId: selected.id }));
          case "search":
            return openOverlay("search");
          case "downloads":
            return openOverlay("downloads");
          case "notifications":
            return openOverlay("notifications");
          case "profiles":
            return openOverlay("profiles");
          case "diagnostics": {
            if (!selected) return;
            emit("confirm");
            closeOverlay(false);
            void native
              .launchDiagnosticsBundle(selected.id)
              .then((bundle) => {
                push({
                  title: t("Diagnostics bundle saved"),
                  description: bundle.fileName,
                  tone: "success",
                  durationMs: 10_000,
                });
              })
              .catch((cause: Error) => {
                emit("error");
                push({
                  title: t("Could not build the diagnostics bundle"),
                  description: errorMessage(cause),
                  tone: "danger",
                });
              });
            return;
          }
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
        if (index === 0) {
          emit("power");
          exit();
        } else {
          closeOverlay();
        }
        return;
      case "collections": {
        if (collectionsQuery.isError) {
          emit("confirm");
          void collectionsQuery.refetch();
          return;
        }
        const collection = collections[index];
        if (!collection) return;
        emit("confirm");
        closeOverlay();
        void api
          .collectionGames(collection.id, 1, 1)
          .then((list) => {
            const first = list.items[0];
            if (first) {
              const target = findGameShelf(shelves, first.id);
              if (target) nav.selectGame(target.shelf, target.index);
            }
            push({
              title: collection.name,
              description: tn(collection.gameCount, "1 game", "{count} games"),
            });
          })
          .catch(() => {
            push({ title: collection.name });
          });
        return;
      }
      case "downloads":
        // Managed by ConsoleDownloadsOverlay's own input scope; stays inside Console.
        emit("click");
        return;
      case "notifications": {
        if (notificationsQuery.isError) {
          emit("confirm");
          void notificationsQuery.refetch();
          return;
        }
        if (index === visibleNotifications.length && unreadNotifications > 0) {
          emit("confirm");
          void api
            .readAllNotifications()
            .then(() => queryClient.invalidateQueries({ queryKey: qk.notifications.root }))
            .catch(() => undefined);
          return;
        }
        const item = visibleNotifications[index];
        if (!item) return;
        emit("confirm");
        if (item.readAt === null) {
          void api
            .readNotification(item.id)
            .then(() => queryClient.invalidateQueries({ queryKey: qk.notifications.root }))
            .catch(() => undefined);
        }
        if (item.game?.id) {
          closeOverlay(false);
          navigate(consolePathFor("game", { gameId: item.game.id }));
          return;
        }
        if (item.kind === "collection_share") {
          openOverlay("collections");
          return;
        }
        closeOverlay();
        return;
      }
      case "profiles": {
        const option = profileChoices[index];
        if (option) {
          emit("confirm");
          const id = option.id === "auto" ? null : option.id;
          updatePrefs({ profileId: id });
          // #230: with a pad attached the choice belongs to that pad too, or
          // the device entry would keep overriding it on the next press.
          if (activePad) deviceStore.assign(activePad.identity, { source: sourceFromId(option.id) });
          return;
        }
        if (index === profileChoices.length) {
          emit("confirm");
          updatePrefs({
            haptics: !prefs.haptics,
            hapticsStrength: prefs.hapticsStrength === "off" ? "standard" : prefs.hapticsStrength,
          });
          return;
        }
        if (index === profileChoices.length + 1) {
          const next = cycleHapticsStrength(prefs.hapticsStrength, 1);
          emit("confirm");
          updatePrefs({ hapticsStrength: next, haptics: next !== "off" });
        }
        return;
      }
      case "search": {
        const cell = cells[searchIndex];
        if (!cell) return;
        emit("confirm");
        if (cell.kind === "key") return typeSearchKey(cell.key);
        return pickResult(cell.game.id);
      }
      default:
        return;
    }
  };

  /**
   * One click path for every surface (#267): the menu, queue, profile sheet and
   * collection index activate their ring; couch search activates the cell that
   * was actually clicked.
   */
  const chooseOverlay = (index: number) => {
    if (overlay === "search") {
      setSearchIndex(index);
      const cell = cells[index];
      if (!cell) return;
      if (cell.kind === "key") {
        typeSearchKey(cell.key);
        return;
      }
      pickResult(cell.game.id);
      return;
    }
    setOverlayIndex(index);
    activateOverlay(index);
  };

  const runCommand = (command: BigPictureCommand) => {
    // #198: the command table decides the movement cue, before execution, from
    // the state the press was made in. An edge press is silent because `moved`
    // is false. #220: surfaces own their own cue now, so the router no longer
    // announces an overlay or an app; it only records the motion it starts.
    const moved =
      command.type === "move"
        ? nav.willMove(command.direction)
        : command.type === "highlight"
          ? overlayWillMove(command.direction)
          : false;
    emit(cueForCommand(command, { moved }));
    if (moved && (command.type === "move" || command.type === "highlight")) {
      transitions.begin("focus", { instant: heldInput.current });
    } else if (command.type === "shelf") {
      transitions.begin("shelf", { instant: heldInput.current });
    } else if (command.type === "page") {
      transitions.begin("page", { instant: heldInput.current });
    }
    switch (command.type) {
      case "move": {
        // The step model returns the zone the press belongs to when the game
        // row has run out (#197); the shell owns the zone state, not the hook.
        const target = nav.move(command.direction);
        if (target) enterZone(target);
        return;
      }
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
      case "game":
        if (selected) navigate(consolePathFor("game", { gameId: selected.id }));
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
    // The library card or action keeps its selected state, not the outline.
    setFocusMark(active, { focus: overlay !== "search" });
  }, [overlay, overlayIndex]);

  useEffect(() => {
    if (overlay !== "search") return;
    searchInputRef.current?.focus({ preventScroll: true });
  }, [overlay]);

  // Gamepad: the same commands as the keyboard, resolved from the active pad
  // action and the physical input that produced it (#138).
  const handleGamepad = (action: GamepadAction, source: ActionSource | null) => {
    // #229: the launch plate owns the pad while it is up. B closes it once the
    // launch has given up (the native call has no abort); everything else is
    // swallowed so a reflex A cannot start a second launch.
    if (launchPlate) {
      if (action === "back" || action === "menu") {
        if (plateCanDismiss(launchPlate.status)) {
          emit("click");
          dismissLaunchPlate();
        }
      }
      return;
    }

    if (pendingPower) {
      if (action === "back" || action === "menu") {
        emit("click");
        return setPendingPower(null);
      }
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

    if (zone !== "library") {
      switch (action) {
        case "left":
        case "right":
        case "up":
        case "down":
          return moveZone(action);
        case "confirm":
          return zone === "home" ? activateHome(homeIndex) : activateAction(actionIndexRef.current);
        case "back":
          return returnToLibrary();
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

  // A pad press owns the cue's haptics for the length of this synchronous
  // dispatch; every other path (keyboard, mouse, a deferred event) leaves the
  // flag false and therefore never rumbles.
  const onGamepad = (action: GamepadAction, source: ActionSource | null, repeated = false) => {
    padGesture.current = true;
    // #220: the pad's auto-repeat is a held direction, not a new press.
    heldInput.current = repeated;
    if (repeated) {
      rootRef.current?.setAttribute("data-bp-instant", "true");
      if (instantTimer.current !== undefined) window.clearTimeout(instantTimer.current);
      instantTimer.current = window.setTimeout(() => {
        heldInput.current = false;
        rootRef.current?.removeAttribute("data-bp-instant");
      }, INSTANT_HOLD_MS);
    } else {
      rootRef.current?.removeAttribute("data-bp-instant");
      if (instantTimer.current !== undefined) {
        window.clearTimeout(instantTimer.current);
        instantTimer.current = undefined;
      }
    }
    try {
      handleGamepad(action, source);
    } finally {
      padGesture.current = false;
    }
  };
  // #267: the polling loop lives in `InputProvider`; the shell registers a
  // scope and consumes the same commands as every other surface.
  const input = useInput();
  const gamepad = input;
  pulseRef.current = gamepad.pulse;
  // The hook may have switched pads since the render above (the pad that last
  // produced input wins), so name the pad it settled on, not the guess.
  const shownPad =
    connectedPads.find((pad) => pad.index === gamepad.index && pad.id === gamepad.id) ?? activePad;
  const padResolutionNow =
    shownPad === activePad
      ? padResolution
      : resolveProfileForPad(
          shownPad?.id ?? null,
          padFamily(shownPad?.id ?? null, accountProfile),
          devices,
          {
            ...padSources,
            pad: shownPad,
            samples: shownPad ? axisSamplesFor(shownPad.identity.fingerprint) : null,
          },
        );
  const padFamilyName = padResolutionNow.glyphFamily;
  const swapAB = padResolutionNow.swapAB;
  const padSummary = shownPad
    ? {
        label: identityLabel(shownPad.identity),
        fingerprint: shortFingerprint(shownPad.identity.fingerprint),
        count: connectedPads.filter(
          (pad) => pad.identity.fingerprint === shownPad.identity.fingerprint,
        ).length,
      }
    : null;

  // Remember which pad drove the console: the manager lists it as the last
  // used one and the overlay can name it after a reload. Written on change
  // only, never per frame.
  useEffect(() => {
    if (!gamepad.connected) return;
    deviceStore.touch(padResolutionNow.identity.fingerprint);
  }, [gamepad.connected, deviceStore, padResolutionNow.identity.fingerprint]);

  // After a layout switch the ring must come back to the same surface. A zone
  // the new layout does not draw falls back to the game row; a surviving zone
  // (the system row exists everywhere) is refocused where it now sits. The
  // guard keeps a mouse click on the switcher from stealing focus.
  const lastLayout = useRef(layout);
  useEffect(() => {
    if (lastLayout.current === layout) return;
    lastLayout.current = layout;
    const restored = restoreZone(layout, zoneRef.current);
    if (restored !== zoneRef.current) {
      zoneRef.current = restored;
      setZone(restored);
    }
    // An app level owns the ring; only the library has a surface to refocus.
    if (level.kind !== "library") return;
    const active = document.activeElement as HTMLElement | null;
    const ownsFocus =
      !active ||
      active === document.body ||
      active.hasAttribute("data-gamepad-focus") ||
      active.closest("[data-gamepad-target]") !== null;
    if (!ownsFocus) return;
    if (restored === "home") focusSurface("home");
    else if (restored === "actions") focusActions(actionIndexRef.current);
    else nav.focusSelected();
  }, [layout, level, nav, focusSurface, focusActions]);

  // #267: the settings app's model is a module now (shell/use-console-settings).
  const audio = useConsoleAudio();
  const settings = useConsoleSettings({
    section: settingsSection,
    layout,
    setLayout,
    markLayoutTouched: () => setLayoutTouched(true),
    room,
    setRoom,
    artworkAccent,
    theme,
    resolved,
    setTheme,
    prefs,
    updatePrefs,
    deviceOnly,
    setDeviceOnly,
    padConnected: gamepad.connected,
    padFamilyName,
    hapticsActive: haptics,
    activeProfile,
    emit,
    pulse: (cue) => pulseRef.current(cue),
    openProfiles: () => openOverlay("profiles"),
    openRemotePlay: () =>
      push({
        title: t("Remote play"),
        description: t("Manage Sunshine and Moonlight pairing from the desktop host settings."),
      }),
    audio,
  });
  const settingsRows = settings.rows;
  const activateSetting = settings.activate;
  const adjustSetting = settings.adjust;

  /* --------------------------------- the band ------------------------------ */

  const bandDetails: Record<HomeItemId, BandItemDetail> = {
    search: { detail: t("Your whole library, one query away.") },
    library: { detail: layoutLabel(layout) },
    downloads:
      activeCount > 0
        ? {
            detail: tn(activeCount, "1 downloading", "{count} downloading"),
            count: activeCount,
            dot: true,
          }
        : { detail: t("No downloads running.") },
    settings: { detail: consoleRoomLabel(room.room) },
    power: { detail: consoleActive ? t("End session") : t("Quit") },
  };

  const activateHome = (index: number) => {
    const id = HOME_ITEM_IDS[index];
    if (!id) return;
    switch (id) {
      case "search":
        openOverlay("search");
        return;
      case "library":
        returnToLibrary();
        return;
      case "downloads":
        openOverlay("downloads");
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
    if (level.kind !== "library" || zone !== "actions" || overlay || pendingPower) return;
    focusActions(actionIndexRef.current);
  }, [level, zone, overlay, pendingPower, focusActions]);

  /**
   * #218: the card ring is drawn by the card itself, never by the global
   * gamepad mark. A mark left behind by the surface the player came from would
   * paint a second outline, or keep a ghost on the card they just left, so
   * every layout, shelf, zone, app and overlay step drops it — and so does
   * unmounting the screen.
   */
  useEffect(() => {
    const clear = () => clearFocusMarks("[data-bp-card][data-gamepad-focus]");
    clear();
    return clear;
  }, [layout, nav.shelfIndex, zone, level, overlay]);

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
    setFocusMark(active);
  }, [pendingPower, level, focusSurface]);

  /**
   * #267: the key map is a scope handler, not a private window listener. The
   * InputProvider gives it to the top console scope, so an overlay and the
   * screen behind it can never both answer one press.
   */
  const onConsoleKey = (event: KeyboardEvent): void => {
    if (document.querySelector('[role="dialog"][aria-modal="true"]')) return;
      const key = event.key;

      // #289: media keys drive the app-owned OSD on the selected sink. They
      // are consumed only when the desktop can actually route audio; a web
      // build keeps the browser/OS behaviour.
      const volumeKey = volumeKeyAction(key);
      if (volumeKey && isNative() && audio.supported) {
        event.preventDefault();
        if (volumeKey === "toggleMute") audio.toggleMute();
        else audio.step(volumeKey === "up" ? 1 : -1);
        return;
      }

      // #220: a held key (auto-repeat) lands on the final frame; a fresh press
      // animates. The attribute covers the CSS focus move for this commit only.
      heldInput.current = event.repeat;
      if (event.repeat) {
        rootRef.current?.setAttribute("data-bp-instant", "true");
        if (instantTimer.current !== undefined) window.clearTimeout(instantTimer.current);
        instantTimer.current = window.setTimeout(() => {
          // A key the DOM owns (typing) never commits; clear the flag anyway so
          // the next click or selection is not treated as a hold.
          heldInput.current = false;
          rootRef.current?.removeAttribute("data-bp-instant");
        }, INSTANT_HOLD_MS);
      } else {
        rootRef.current?.removeAttribute("data-bp-instant");
        if (instantTimer.current !== undefined) {
          window.clearTimeout(instantTimer.current);
          instantTimer.current = undefined;
        }
      }

      // The console quit chord (#172) wins over every screen shortcut: it is
      // the only way back to the desktop when Sunshine owns the session.
      if (consoleActive && matchesExitShortcut(event, consoleMode?.exitShortcut)) {
        event.preventDefault();
        exit();
        return;
      }

      // #229: the launch plate is the top surface. B/Escape closes it once the
      // launch has given up; every other key is swallowed so a stray press
      // cannot start a second launch behind the first.
      if (launchPlate) {
        if (key === "Escape" || key === "Backspace") {
          event.preventDefault();
          if (plateCanDismiss(launchPlate.status)) {
            emit("click");
            dismissLaunchPlate();
          }
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
          emit("click");
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

      if (zone !== "library") {
        const direction =
          key === "ArrowLeft" ? "left" : key === "ArrowRight" ? "right" : key === "ArrowUp" ? "up" : key === "ArrowDown" ? "down" : null;
        if (direction) {
          event.preventDefault();
          moveZone(direction);
          return;
        }
        if (key === "Enter") {
          event.preventDefault();
          if (zone === "home") activateHome(homeIndex);
          else activateAction(actionIndexRef.current);
          return;
        }
        if (key === "Backspace") {
          event.preventDefault();
          returnToLibrary();
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

  // The Console scope. The top surface owns the pad and the keys; a menu or a
  // launch plate mounts a child scope (below), so one press runs one action.
  const consoleInputOptions = useMemo<InputScopeOptions>(
    () => ({
      profile: activeProfile,
      // #230: resolves the pad that is actually driving the UI, so a second pad
      // or a reconnect never inherits the first pad's profile.
      resolveProfile: (pad) =>
        withBigPictureDefaults(
          resolveProfileForPad(pad?.id ?? null, padFamily(pad?.id ?? null, accountProfile), devices, {
            ...padSources,
            pad,
            samples: pad ? axisSamplesFor(padFingerprint(pad.id, undefined, { mapping: pad.mapping }).fingerprint) : null,
          }).profile,
        ),
      haptics,
      hapticsStrength: room.haptics,
      // The cue decides the pattern; the screen calls `pulse` from `emit`.
      cueHaptics: true,
    }),
    [activeProfile, accountProfile, devices, padSources, haptics, room.haptics],
  );
  useInputScope({
    id: "console",
    onAction: onGamepad,
    onKey: onConsoleKey,
    options: consoleInputOptions,
  });

  const offlineBlockedPrimary =
    !online && (installAction.intent === "install" || installAction.intent === "download");
  const primary = !selected ? null : (
    <Button
      variant={installAction.intent === "open-downloads" || installAction.intent === "resume-install" ? "default" : "primary"}
      size="lg"
      className="bp-button"
      disabled={installAction.disabled || offlineBlockedPrimary}
      title={offlineBlockedPrimary ? t("This device is offline. Installed games still play.") : undefined}
      icon={
        <Glyph
          name={installAction.kind === "play" || installAction.kind === "resume" ? "play" : "install"}
          className="size-[calc(var(--bp-u)*22)]"
        />
      }
      onClick={confirm}
    >
      {installAction.label}
    </Button>
  );

  const viewProps: BigPictureLayoutProps = {
    nav,
    layout,
    columns,
    reduceMotion,
    instant: instantMotion,
    settled: entranceDone || layoutTouched,
    detail: detail.data,
    installed,
    job,
    percent,
    primary,
    onOpenGame: (gameId) => navigate(consolePathFor("game", { gameId })),
    onOpenMenu: () => openOverlay("menu"),
    onOpenCollections: () => openOverlay("collections"),
  };

  const body = isPending ? (
    <LayoutSkeleton layout={layout} columns={columns} />
  ) : isError && games.length === 0 ? (
    <div className="flex min-h-0 flex-1 flex-col items-start justify-center gap-4 bp-pad-x">
      <Wordmark height={48} cut="console" className="text-ink" />
      <p className="max-w-[56ch] text-bp-body leading-relaxed text-ink-2">
        {t("The library could not be loaded.")}
      </p>
      <div className="flex flex-wrap items-center gap-3">
        <Button variant="primary" className="bp-button" onClick={() => void refetch()}>
          {t("Try again")}
        </Button>
        <Button variant="default" className="bp-button" onClick={exit}>
          {t("Exit Console mode")}
        </Button>
      </div>
    </div>
  ) : !selected ? (
    <div className="flex min-h-0 flex-1 flex-col items-start justify-center gap-4 bp-pad-x">
      <Wordmark height={48} cut="console" className="text-ink" />
      <p className="max-w-[56ch] text-bp-body leading-relaxed text-ink-2">
        {t("The library is empty. Add a game first.")}
      </p>
      <Button variant="primary" className="bp-button" onClick={exit}>
        {t("Exit Console mode")}
      </Button>
    </div>
  ) : (
    (() => {
      const View = LAYOUT_VIEWS[layout];
      return <View {...viewProps} />;
    })()
  );

  const footerContext = pendingPower
    ? "exit"
    : overlay
      ? hintContextFor(overlay)
      : level.kind === "app"
        ? level.id
        : zone === "home"
          ? "home"
          : zone === "actions"
            ? "actions"
            : "library";

  // #268: the band is always the bottom zone of the home axis — Search ·
  // Library · Downloads · Settings · Power. State lives in the bar, the band
  // carries destinations, and the layout no longer moves the chrome around.
  const band = (
    <HomeBand
      active={homeIndex}
      focused={zone === "home"}
      details={bandDetails}
      onHighlight={setHomeIndex}
      onChoose={activateHome}
      settled={entranceDone || layoutTouched}
    />
  );
  const gameSurfaceTarget =
    surface === "game"
      ? (games.find((entry) => entry.id === gameId) ?? selected ?? null)
      : null;
  const bandVisible = level.kind === "library" && !gameSurfaceTarget;
  // One hint strip at a time: a sheet owns the strip while it is open, and the
  // launch plate owns the screen.
  const footerVisible = !overlay && !pendingPower && !launchPlate;

  return (
    <div
      ref={rootRef}
      className="bp-root relative flex h-dvh flex-col overflow-hidden text-ink"
      style={cssVarsFor(cabinet)}
      {...consoleRoomAttributes(room)}
    >
      {((overlay && overlay !== "downloads") || pendingPower || launchPlate) && (
        <ConsoleOverlayInputScope
          onAction={onGamepad}
          onKey={onConsoleKey}
          options={consoleInputOptions}
        />
      )}
      <div className="arc-stripe relative z-20" aria-hidden="true" />
      <div className="grain" aria-hidden="true" />
      <ConsoleBackdrop
        wall={room.wall}
        room={room.room}
        title={selected?.title ?? ""}
        artUrl={backdropArt}
      />

      <SystemBar
        user={user}
        serverLabel={serverLabel}
        online={online}
        padConnected={gamepad.connected}
        padFamily={padFamilyLabel(padFamilyName)}
        padBattery={padBattery?.level ?? null}
        padCharging={padBattery?.charging ?? false}
        refreshing={!isPending && isFetching}
        consoleActive={consoleActive}
        unreadNotifications={unreadNotifications}
        onOpenNotifications={() => openOverlay("notifications")}
      />

      {!online && (
        <div
          role="status"
          data-bp-offline="true"
          className="relative z-20 flex shrink-0 items-center gap-2 border-b border-line bg-surface py-1.5 text-bp-meta text-warn bp-pad-x"
        >
          <span className="bp-dot bg-warn" aria-hidden="true" />
          <span className="truncate">
            {installed.size > 0
              ? t("Offline · {summary}", {
                  summary: tn(
                    installed.size,
                    "1 installed game still plays",
                    "{count} installed games still play",
                  ),
                })
              : t("Offline · Connect to the server to download games.")}
          </span>
        </div>
      )}

      <main className="relative z-10 flex min-h-0 flex-1 flex-col">
        {/*
          #220: two stacking layers, not a remount. The library stays mounted
          under a system app, so a push slides the opaque app screen over it and
          a pop reveals the same selection without rebuilding the covers. The
          library is `inert` while an app owns the ring; the exiting app has
          `pointer-events: none`, so the next press lands on the library at once.
        */}
        <div className="relative flex min-h-0 flex-1 flex-col">
          <div
            className="absolute inset-0 flex min-h-0 flex-col"
            data-bp-screen="library"
            aria-hidden={level.kind === "app" || Boolean(gameSurfaceTarget) ? true : undefined}
            inert={level.kind === "app" || Boolean(gameSurfaceTarget) ? true : undefined}
          >
            <motion.div
              key={layout}
              className="flex min-h-0 flex-1 flex-col"
              data-bp-layout={layout}
              {...transitionOf("layout", { reduce: reduceMotion, instant: instantMotion })}
              initial={
                layoutSwitched
                  ? transitionOf("layout", { reduce: reduceMotion, instant: instantMotion }).initial
                  : false
              }
            >
              {body}
            </motion.div>
          </div>
          <AnimatePresence initial={false}>
            {gameSurfaceTarget && level.kind === "library" && (
              <motion.div
                key={`game-${gameSurfaceTarget.id}`}
                className="bp-app-screen absolute inset-0 z-10 flex min-h-0 flex-col"
                data-bp-screen="game"
                {...transitionOf("gameDetail", { reduce: reduceMotion, instant: instantMotion })}
                {...exitOf("screenPop", { reduce: reduceMotion, instant: instantMotion })}
              >
                <ConsoleGameSurface
                  game={gameSurfaceTarget}
                  detail={detail.data}
                  installed={installed.get(gameSurfaceTarget.id)}
                  primaryLabel={
                    installed.has(gameSurfaceTarget.id)
                      ? t("Play")
                      : installAction.intent === "open-downloads" || installAction.intent === "resume-install"
                        ? t("Downloads")
                        : isNative()
                          ? t("Install")
                          : t("Download")
                  }
                  primaryGlyph={
                    installed.has(gameSurfaceTarget.id)
                      ? "play"
                      : installAction.intent === "open-downloads" || installAction.intent === "resume-install"
                        ? "downloads"
                        : "install"
                  }
                  onPrimary={confirm}
                  onOpenProfiles={() => openOverlay("profiles")}
                  onOpenFolder={
                    installed.get(gameSurfaceTarget.id)?.installPath
                      ? () => {
                          const path = installed.get(gameSurfaceTarget.id)?.installPath;
                          if (path) void installBridge.openPath(path);
                        }
                      : undefined
                  }
                  onBack={() => {
                    emit("closeGroup");
                    navigate(consolePathFor("library"), { replace: true });
                    window.setTimeout(() => navRef.current.focusSelected(), 0);
                  }}
                  profile={activeProfile}
                  padFamilyName={padFamilyLabel(padFamilyName)}
                  online={online}
                />
              </motion.div>
            )}
          </AnimatePresence>
          <AnimatePresence initial={false}>
            {level.kind === "app" && (
              <motion.div
                key={level.id}
                className="bp-app-screen absolute inset-0 z-10 flex min-h-0 flex-col"
                data-bp-screen={level.id}
                {...transitionOf("screenPush", { reduce: reduceMotion, instant: instantMotion })}
                {...exitOf("screenPop", { reduce: reduceMotion, instant: instantMotion })}
              >
                {level.id === "settings" ? (
                  <SettingsApp
                    section={settingsSection}
                    rows={settingsRows}
                    active={appIndex}
                    accountName={user?.username ?? null}
                    serverLabel={serverLabel}
                    version={__APP_VERSION__}
                    padLabel={
                      gamepad.connected
                        ? `${padFamilyLabel(padFamilyName)} · ${activeProfile.name}`
                        : t("No controller detected.")
                    }
                    preview={
                      settingsSection === "appearance" ? (
                        <RoomPreview settings={room} layout={layout} artUrl={backdropArt} />
                      ) : undefined
                    }
                    reduceMotion={reduceMotion}
                    instant={instantMotion}
                    onSelectSection={selectSettingsSection}
                    onHighlight={setAppIndex}
                    onActivate={activateSetting}
                  />
                ) : (
                  <PowerApp
                    actions={power}
                    active={appIndex}
                    onHighlight={setAppIndex}
                    onChoose={choosePower}
                  />
                )}
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </main>

      {bandVisible && band}

      {footerVisible && (
        <BigPictureFooter
          layout={layout}
          connected={gamepad.connected}
          family={padFamilyName}
          swapAB={swapAB}
          profileName={padResolutionNow.profile.name}
          context={footerContext}
          haptics={haptics}
        />
      )}

      <ConsoleVolumeOsd osd={audio.osd} reduceMotion={Boolean(reduceMotion)} />

      <ConsoleOverlays
        overlay={overlay}
        active={overlayIndex}
        onHighlight={setOverlayIndex}
        onChoose={chooseOverlay}
        onClose={() => closeOverlay()}
        layout={layout}
        connected={gamepad.connected}
        family={padFamilyName}
        swapAB={Boolean(swapAB)}
        reduceMotion={Boolean(reduceMotion)}
        instant={instantMotion}
        downloads={activeCount}
        titleFor={titleFor}
        emit={emit}
        online={online}
        notifications={notifications}
        unreadNotifications={unreadNotifications}
        notificationsLoading={notificationsQuery.isPending}
        notificationsError={notificationsQuery.isError ? notificationsQuery.error : null}
        onNotificationsRetry={() => void notificationsQuery.refetch()}
        profileOptions={profileChoices}
        selectedProfileId={selectedProfileId}
        profile={padResolutionNow.profile}
        pad={padSummary}
        haptics={haptics}
        strength={room.haptics}
        searchInputRef={searchInputRef}
        query={query}
        onQuery={setQuery}
        rows={rows}
        searchActive={searchIndex}
        onSearchHighlight={setSearchIndex}
        collections={collections}
        collectionsLoading={collectionsQuery.isPending}
        collectionsError={collectionsQuery.isError ? collectionsQuery.error : null}
        onCollectionsRetry={() => void collectionsQuery.refetch()}
      />
      <ConsoleDialogs
        layout={layout}
        connected={gamepad.connected}
        family={padFamilyName}
        swapAB={Boolean(swapAB)}
        reduceMotion={Boolean(reduceMotion)}
        instant={instantMotion}
        launchPlate={launchPlate}
        onDismissPlate={dismissLaunchPlate}
        pendingPower={pendingPower}
        onConfirmPower={chooseConfirm}
        onHighlightPower={(index) => setPendingPower((current) => (current ? { ...current, index } : current))}
        onClosePower={() => setPendingPower(null)}
        installModal={
          installOpen && version && detail.data
            ? { game: detail.data, version }
            : null
        }
        onInstalled={() => {
          setInstallOpen(false);
          openOverlay("downloads");
        }}
        onCloseInstall={() => setInstallOpen(false)}
        emit={emit}
      />

      {debug && <GamepadDebugPanel info={gamepad.debug} />}
      {debugMotion && <MotionProbe transitions={transitions} />}
    </div>
  );
}
