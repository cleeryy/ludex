import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useReducedMotion } from "motion/react";
import {
  AlertTriangle,
  Gamepad2,
  Grid2x2,
  LayoutGrid,
  List,
  Plus,
  RectangleHorizontal,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatRating, formatYear, sortLabel, stateLabel } from "@/lib/format";
import { native } from "@/lib/native";
import type { GameListQuery, GameSummary, InstalledGame } from "@/lib/types";
import { useBookmark, useGenres, useInstalledGames, useLaunchGame, useUpdateGame, useUpdateMap } from "@/lib/hooks";
import { useToast } from "@/state/toasts";
import { installBridge, installsAvailable } from "@/lib/install";
import {
  buildLibraryModel,
  launcherCounts,
  launcherLabel,
  launcherSourceOptions,
  type ImportLibraryEntry,
  type LauncherSourceId,
} from "@/lib/library-entries";
import { useAuth, useIsEditor } from "@/state/auth";
import { useSettings } from "@/state/settings";
import {
  activeFilterChips,
  clearFilterParams,
  filterQuery,
  hasFilterParams,
  loadFilters,
  readFilters,
  removeFilterChip,
  saveFilters,
  setFilterParam,
  sortOptions,
  stateOptions,
  type FilterKey,
} from "@/lib/library-filters";
import { focusGamepadTarget, gamepadDebugEnabled, type GamepadAction } from "@/lib/gamepad";
import { useInput, useInputScope } from "@/components/console/input/input-provider";
import { moveTargetIndex, type TargetRect } from "@/lib/gamepad-core";
import { GamepadDebugPanel } from "@/components/gamepad/gamepad-debug";
import { useControllerProfile } from "@/lib/use-controller-profile";
import { OPEN_FILTERS_EVENT } from "@/lib/ui-events";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { GameCard } from "@/components/game/game-card";
import { STATE_TONE } from "@/components/game/state";
import { FilterPanel } from "@/components/library/filter-panel";
import {
  GameCardCoverThumb,
  InstallStateBadges,
  LibraryEmpty,
  LibrarySkeleton,
  type LibraryMode,
} from "@/components/library/library-views";
import { PresetsBar, SurpriseMeButton } from "@/components/library/presets-bar";
import { applyPresetParams } from "@/lib/library-presets";
import {
  ExternalGameCard,
  ExternalGameRow,
  ExternalImportDialog,
} from "@/components/library/external-games";
import { LibrarySearch } from "@/components/library/search-field";
import { Badge, Button, Segmented } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { t, tn } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

export type { LibraryMode };

function modeTitle(mode: LibraryMode): string {
  switch (mode) {
    case "recent":
      return t("Recently played");
    case "bookmarks":
      return t("Bookmarks");
    case "index":
      return t("Index");
    case "library":
    default:
      return t("Library");
  }
}

function modeDescription(mode: LibraryMode): string {
  switch (mode) {
    case "recent":
      return t("Everything you have launched, most recent first.");
    case "bookmarks":
      return t("Titles you set aside for later.");
    case "index":
      return t("Every title in the library, A to Z.");
    case "library":
    default:
      return "";
  }
}

const MODE_ROUTES: Record<LibraryMode, string> = {
  library: "/library",
  recent: "/recent",
  bookmarks: "/bookmarks",
  index: "/all",
};

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

const NO_INSTALLS: InstalledGame[] = [];

type CardStyle = "poster" | "wide";
type GridDensity = "compact" | "standard" | "large";

const CARD_STYLE_KEY = "ludex.library.cardStyle";
const GRID_DENSITY_KEY = "ludex.library.density";

function readCardStyle(): CardStyle {
  try {
    return window.localStorage.getItem(CARD_STYLE_KEY) === "wide" ? "wide" : "poster";
  } catch {
    return "poster";
  }
}

function readGridDensity(): GridDensity {
  try {
    const raw = window.localStorage.getItem(GRID_DENSITY_KEY);
    return raw === "compact" || raw === "large" ? raw : "standard";
  } catch {
    return "standard";
  }
}

function gridClassFor(style: CardStyle, density: GridDensity): string {
  if (style === "wide") {
    if (density === "compact") return "grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5";
    if (density === "large") return "grid-cols-1 gap-6 sm:grid-cols-2 xl:grid-cols-3";
    return "grid-cols-1 gap-5 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4";
  }
  if (density === "compact") {
    return "grid-cols-3 gap-x-4 gap-y-6 sm:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 2xl:grid-cols-7";
  }
  if (density === "large") {
    return "grid-cols-2 gap-x-6 gap-y-8 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4";
  }
  return "grid-cols-2 gap-x-5 gap-y-7 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6";
}

export function LibraryScreen({ mode }: { mode: LibraryMode }) {
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const location = useLocation();
  const debug = gamepadDebugEnabled(location.search);
  const isEditor = useIsEditor();
  const { user } = useAuth();
  const { view, setView } = useSettings();
  const genres = useGenres();
  const platforms = useQuery({ queryKey: qk.platforms, queryFn: () => api.listPlatforms(), staleTime: 60_000 });
  const shelves = (platforms.data ?? []).filter((platform) => platform.gameCount > 0).slice(0, 8);
  const installedGames = useInstalledGames();
  const installs = installedGames.data ?? NO_INSTALLS;
  const installed = useMemo(() => new Map(installs.map((game) => [game.gameId, game])), [installs]);
  const updates = useUpdateMap();
  const updateGame = useUpdateGame();
  const bookmark = useBookmark();
  const launch = useLaunchGame();
  const reduceMotion = useReducedMotion();
  const queryClient = useQueryClient();
  const { push } = useToast();

  const filters = readFilters(params, mode);
  const chips = activeFilterChips(filters).filter((chip) => chip.key !== "installed" || installsAvailable());
  const installedOnly = filters.installed && installsAvailable();
  const hasFilters = chips.length > 0 || installedOnly;
  const effectiveView = mode === "index" ? "list" : view;

  const [filterOpen, setFilterOpen] = useState(false);
  const [pickingRandom, setPickingRandom] = useState(false);
  const [cardStyle, setCardStyleState] = useState<CardStyle>(() => readCardStyle());
  const [density, setDensityState] = useState<GridDensity>(() => readGridDensity());
  const [padIndex, setPadIndex] = useState(0);

  const setCardStyle = (next: CardStyle) => {
    setCardStyleState(next);
    try {
      window.localStorage.setItem(CARD_STYLE_KEY, next);
    } catch {
      /* ignore storage errors */
    }
  };

  const cycleDensity = () => {
    const order: GridDensity[] = ["compact", "standard", "large"];
    const next = order[(order.indexOf(density) + 1) % order.length];
    setDensityState(next);
    try {
      window.localStorage.setItem(GRID_DENSITY_KEY, next);
    } catch {
      /* ignore storage errors */
    }
  };
  const [importEntry, setImportEntry] = useState<ImportLibraryEntry | null>(null);
  const [removingImport, setRemovingImport] = useState(false);
  const gridRef = useRef<HTMLDivElement>(null);
  const restored = useRef(false);
  const search = params.toString();

  // Restore the last filter set when the library is opened without params.
  useEffect(() => {
    if (mode !== "library" || restored.current) return;
    restored.current = true;
    if (hasFilterParams(params) || params.has("sort") || params.has("order")) return;
    const saved = loadFilters();
    if (saved) {
      // Diagnostic flags are not filters; restoring the saved filter set must
      // not drop `?gamepadDebug=1`.
      const diagnostic = params.get("gamepadDebug");
      if (diagnostic) saved.set("gamepadDebug", diagnostic);
      setParams(saved, { replace: true });
    }
    // Run once per mount; params deliberately read at mount time.
  }, [mode]);

  // Persist the current filter set (best effort, localStorage may be disabled).
  useEffect(() => {
    if (mode !== "library") return;
    saveFilters(new URLSearchParams(search));
  }, [search, mode]);

  useEffect(() => {
    const onOpenFilters = () => setFilterOpen(true);
    window.addEventListener(OPEN_FILTERS_EVENT, onOpenFilters);
    return () => window.removeEventListener(OPEN_FILTERS_EVENT, onOpenFilters);
  }, []);

  const query: GameListQuery =
    mode === "recent"
      ? { sort: "lastPlayed", order: "desc", limit: 200 }
      : mode === "bookmarks"
        ? { bookmarked: true, sort: filters.sort, order: filters.order, limit: 200 }
        : mode === "index"
          ? { sort: "title", order: "asc", limit: 200 }
          : { ...filterQuery(filters), limit: 200 };

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: qk.games.list(mode, query),
    queryFn: () => api.listGames(query),
  });

  // Count of metadata rows hidden by the default availability filter (#210).
  // The toggle is the only way to bring them back; a deep link to a game page
  // still resolves without it.
  const missingCount = useQuery({
    queryKey: qk.games.missingCount,
    queryFn: () => api.listGames({ status: "missing", limit: 1 }),
    staleTime: 60_000,
  });
  const hiddenMissing = missingCount.data?.total ?? 0;

  const all = data?.items ?? [];
  const model = useMemo(() => {
    const games = mode === "recent" ? all.filter((game) => game.lastPlayedAt) : all;
    return buildLibraryModel(games, installs, {
      installedOnly,
      source: (filters.launcher as LauncherSourceId | "") || "",
      query: mode === "library" ? filters.q : "",
      sort: filters.sort,
      order: filters.order,
      // The Library keeps its launcher imports group visible; the Installed
      // filter and the source chips narrow it like any other entry (#154).
      includeUnmatched: mode === "library" && installsAvailable(),
    });
  }, [all, installs, mode, installedOnly, filters.launcher, filters.q, filters.sort, filters.order]);
  const items = model.server;
  const imports = model.imports;
  const totalBytes =
    items.reduce((sum, entry) => sum + entry.sizeBytes, 0) +
    imports.reduce((sum, entry) => sum + entry.sizeBytes, 0);

  const importCounts = useMemo(() => launcherCounts(installs), [installs]);
  const importChips = launcherSourceOptions().filter(
    (option) => (importCounts.get(option.value) ?? 0) > 0 || filters.launcher === option.value,
  );

  const update = (key: FilterKey, value: string | boolean | null) => {
    setParams((current) => setFilterParam(current, key, value), { replace: true });
  };

  const clearFilters = () => {
    setParams((current) => clearFilterParams(current), { replace: true });
  };

  const applyPreset = (preset: Readonly<Record<string, string>>) => {
    setParams((current) => applyPresetParams(current, preset), { replace: true });
  };

  const surpriseMe = async () => {
    if (pickingRandom) return;
    if (installedOnly || filters.launcher) {
      if (items.length === 0) {
        push({ title: t("No game matches these filters"), tone: "default" });
        return;
      }
      const pick = items[Math.floor(Math.random() * items.length)].game;
      push({ title: t("Picked {title}", { title: pick.title }), tone: "default" });
      navigate(`/game/${pick.id}`);
      return;
    }
    setPickingRandom(true);
    try {
      const picked = await api.randomGame(filterQuery(filters));
      push({ title: t("Picked {title}", { title: picked.title }), tone: "default" });
      navigate(`/game/${picked.id}`);
    } catch {
      if (items.length > 0) {
        const fallback = items[Math.floor(Math.random() * items.length)].game;
        push({ title: t("Picked {title}", { title: fallback.title }), tone: "default" });
        navigate(`/game/${fallback.id}`);
      } else {
        push({ title: t("No game matches these filters"), tone: "default" });
      }
    } finally {
      setPickingRandom(false);
    }
  };

  const removeChip = (key: FilterKey) => {
    const patch = removeFilterChip(key);
    setParams(
      (current) => {
        const next = new URLSearchParams(current);
        for (const [patchKey, patchValue] of Object.entries(patch)) {
          setFilterParam(next, patchKey as FilterKey, patchValue as string | boolean);
        }
        return next;
      },
      { replace: true },
    );
  };

  const toggleBookmark = (game: GameSummary) => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked });

  const launchImport = (entry: ImportLibraryEntry) => {
    launch.mutate({ id: entry.gameId, title: entry.title, executable: entry.install.executable });
  };

  const openImportFolder = (entry: ImportLibraryEntry) => {
    const path = entry.install.installPath;
    if (!path) return;
    void installBridge.openPath(path).catch((cause: Error) => {
      push({ title: t("Could not open the folder"), description: cause.message, tone: "danger" });
    });
  };

  const removeImport = async (entry: ImportLibraryEntry) => {
    setRemovingImport(true);
    try {
      await native.unlinkInstallation(entry.gameId);
      push({
        title: t("Removed from Ludex"),
        description: t("{title} stays installed in {source}.", {
          title: entry.title,
          source: entry.launcher ? launcherLabel(entry.launcher) : t("the launcher"),
        }),
        tone: "success",
      });
      setImportEntry(null);
      await queryClient.invalidateQueries({ queryKey: qk.installed });
      await queryClient.invalidateQueries({ queryKey: qk.recentGames.root });
      await queryClient.invalidateQueries({ queryKey: qk.games.root });
    } catch (cause) {
      push({ title: t("Could not remove the import"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setRemovingImport(false);
    }
  };

  const groups = useMemo(() => {
    if (mode !== "index") return [];
    const map = new Map<string, GameSummary[]>();
    for (const entry of items) {
      const game = entry.game;
      const first = game.sortTitle.trim().charAt(0).toUpperCase();
      const letter = /[A-Z]/.test(first) ? first : "#";
      const list = map.get(letter) ?? [];
      list.push(game);
      map.set(letter, list);
    }
    return [...map.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [items, mode]);

  /* ------------------------------- gamepad -------------------------------- */

  // Only real, laid-out targets count: hidden filter results, skeletons and
  // elements collapsed by the responsive grid are skipped. Geometry comes from
  // the DOM rectangles, so the movement follows the rendered grid (#137).
  const targets = () => {
    const root = gridRef.current;
    if (!root) return [];
    return Array.from(root.querySelectorAll<HTMLElement>("[data-gamepad-target]")).filter((element) => {
      if (element.offsetParent === null) return false;
      const rect = element.getBoundingClientRect();
      return rect.width > 1 && rect.height > 1;
    });
  };

  const rects = (elements: HTMLElement[]): TargetRect[] =>
    elements.map((element) => {
      const rect = element.getBoundingClientRect();
      return { top: rect.top, left: rect.left, width: rect.width, height: rect.height };
    });

  const currentIndex = (elements: HTMLElement[]): number => {
    const marked = elements.findIndex((element) => element.hasAttribute("data-gamepad-focus"));
    if (marked >= 0) return marked;
    const focused = elements.findIndex(
      (element) => element === document.activeElement || element.contains(document.activeElement),
    );
    return focused >= 0 ? focused : clamp(padIndex, 0, Math.max(0, elements.length - 1));
  };

  const cycleMode = (delta: number) => {
    const order: LibraryMode[] = ["library", "recent", "bookmarks", "index"];
    const current = order.indexOf(mode);
    const next = order[(current + delta + order.length) % order.length];
    navigate(MODE_ROUTES[next]);
  };

  const onGamepad = (action: GamepadAction) => {
    if (action === "prevTab") return cycleMode(-1);
    if (action === "nextTab") return cycleMode(1);
    if (action === "downloads") return navigate("/downloads");
    if (action === "menu") {
      if (mode === "library") setFilterOpen(true);
      return;
    }
    if (action === "back") {
      if (window.history.length > 1) navigate(-1);
      else navigate("/library");
      return;
    }

    const elements = targets();
    if (elements.length === 0) return;
    const index = currentIndex(elements);

    if (action === "confirm") {
      const target = elements[index];
      target?.click();
      return;
    }

    if (action !== "left" && action !== "right" && action !== "up" && action !== "down") return;
    const next = moveTargetIndex(rects(elements), index, action);
    setPadIndex(next);
    focusGamepadTarget(elements, next, !reduceMotion);
  };

  const controllerProfile = useControllerProfile();
  // #267: the library registers a scope on the one input owner instead of
  // running its own polling loop.
  const gamepad = useInput();
  useInputScope({ id: "library", onAction: onGamepad, options: { profile: controllerProfile } });

  // Keep the numeric cursor and the ring in sync when the result set, the
  // layout or the route changes (filters, sort, grid/list, new data). The ring
  // is re-applied only for a connected pad, when the gamepad owns focus, and
  // never from a form field or an open dialog.
  useEffect(() => {
    const elements = targets();
    if (elements.length === 0) return;
    const marked = elements.findIndex((element) => element.hasAttribute("data-gamepad-focus"));
    const next = marked >= 0 ? marked : clamp(padIndex, 0, elements.length - 1);
    setPadIndex((current) => (current === next ? current : next));
    if (!gamepad.connected) return;
    if (document.querySelector('[role="dialog"][aria-modal="true"]')) return;
    const active = document.activeElement as HTMLElement | null;
    const ownsFocus =
      !active ||
      active === document.body ||
      active.hasAttribute("data-gamepad-focus") ||
      active.closest("[data-gamepad-target]") !== null;
    if (!ownsFocus) return;
    focusGamepadTarget(elements, next, false, { scroll: marked >= 0 });
    // The DOM is the source of truth here; mode/search/view changes rebuild it.
  }, [mode, search, effectiveView, model.total, gamepad.connected]);

  const headerViewValue = effectiveView === "list" ? "list" : cardStyle;
  const setHeaderView = (next: "poster" | "wide" | "list") => {
    if (next === "list") {
      setView("list");
    } else {
      setView("grid");
      setCardStyle(next);
    }
  };

  const densityLabel = density === "compact" ? t("Compact") : density === "large" ? t("Large") : t("Standard");

  const headerActions = (
    <>
      {isEditor && mode === "library" && (
        <Button variant="ghost" size="sm" icon={<Plus className="size-3.5" />} onClick={() => navigate("/admin/ingest")}>
          {t("Add games")}
        </Button>
      )}
      {mode !== "index" && effectiveView === "grid" && (
        <Button
          variant="ghost"
          size="sm"
          icon={<Grid2x2 className="size-3.5" />}
          onClick={cycleDensity}
          title={t("Card size")}
          aria-label={`${t("Card size")}: ${densityLabel}`}
        >
          <span className="hidden md:inline">{densityLabel}</span>
        </Button>
      )}
      {mode !== "index" && (
        <Segmented
          label={t("Layout")}
          value={headerViewValue}
          onChange={setHeaderView}
          options={[
            { value: "poster", label: t("Poster"), icon: <LayoutGrid className="size-3.5" /> },
            { value: "wide", label: t("Wide"), icon: <RectangleHorizontal className="size-3.5" /> },
            { value: "list", label: t("List"), icon: <List className="size-3.5" /> },
          ]}
        />
      )}
    </>
  );

  return (
    <div className="page page-in">
      <PageHeader
        title={modeTitle(mode)}
        meta={
          <>
            <span className="num">{tn(model.total, "1 game", "{count} games")}</span>
            {installedOnly && (
              <span className="num">{tn(model.installedCount, "1 installed", "{count} installed")}</span>
            )}
            {totalBytes > 0 && <span className="num">{formatBytes(totalBytes)}</span>}
            {mode === "recent" ? (
              <span>{t("Most recent first")}</span>
            ) : model.total > 0 && mode !== "index" ? (
              <span>
                {mode === "library" && filters.q.trim()
                  ? t("Ranked by relevance")
                  : t("Sorted by {sort}", { sort: sortLabel(filters.sort) })}
              </span>
            ) : null}
            {gamepad.connected && (
              <span className="inline-flex items-center gap-1.5 text-ok">
                <Gamepad2 className="size-3.5" aria-hidden="true" />
                {t("Gamepad")}
              </span>
            )}
          </>
        }
        description={modeDescription(mode) || undefined}
        actions={headerActions}
      />

      {mode === "library" && !(model.total === 0 && !hasFilters) && (
        <div className="sticky top-0 z-20 -mx-[var(--page-x)] mt-4 border-b border-line bg-bg/95 px-[var(--page-x)] py-2.5 backdrop-blur-sm">
          <div className="flex flex-wrap items-center gap-2">
            <LibrarySearch value={filters.q} onCommit={(value) => update("q", value)} className="min-w-0 flex-1 sm:max-w-[300px]" />
            <div className="hidden w-[144px] sm:block">
              <Select
                label={t("Filter by progress state")}
                value={filters.state || "any"}
                onChange={(value) => update("state", value === "any" ? null : value)}
                options={stateOptions()}
                className="h-9 text-label"
              />
            </div>
            {genres.length > 0 && (
              <div className="hidden w-[150px] md:block">
                <Select
                  label={t("Genre")}
                  value={filters.genre || "all"}
                  onChange={(value) => update("genre", value === "all" ? null : value)}
                  options={[
                    { value: "all", label: t("All genres") },
                    ...genres.map((item) => ({ value: item, label: item })),
                  ]}
                  className="h-9 text-label"
                />
              </div>
            )}
            {(hiddenMissing > 0 || filters.missing) && (
              <Button
                variant={filters.missing ? "default" : "ghost"}
                size="sm"
                icon={<AlertTriangle className="size-3.5" />}
                aria-pressed={filters.missing}
                onClick={() => update("missing", filters.missing ? null : true)}
                title={t("Games whose archive was deleted outside Ludex stay in the database. They are hidden unless this filter is on.")}
              >
                {t("Show missing")}
                <span className="num ml-1 text-ink-3">{hiddenMissing}</span>
              </Button>
            )}
            <Button
              variant="default"
              size="sm"
              icon={<SlidersHorizontal className="size-3.5" />}
              onClick={() => setFilterOpen(true)}
              aria-haspopup="dialog"
            >
              {t("Filters")}
              {chips.length > 0 && (
                <span className="ml-1 inline-flex h-4 min-w-4 items-center justify-center rounded-[var(--r-sm)] bg-signal px-1 font-mono text-micro text-on-signal">
                  {chips.length}
                </span>
              )}
            </Button>
            <SurpriseMeButton
              loading={pickingRandom}
              disabled={model.total === 0}
              onClick={() => void surpriseMe()}
            />
            <div className="w-[144px]">
              <Select
                label={t("Sort games")}
                value={filters.sort}
                onChange={(value) => update("sort", value)}
                options={sortOptions()}
                className="h-9 text-label"
              />
            </div>
            {hasFilters && (
              <button
                type="button"
                onClick={clearFilters}
                className="ml-auto flex h-9 items-center gap-1 text-label text-ink-3 transition-colors hover:text-ink"
              >
                <X className="size-3.5" aria-hidden="true" />
                {t("Clear all")}
              </button>
            )}
          </div>

          {/* Single compact horizontal strip: saved views + platform/launcher quick filters */}
          <div className="nav-scroll mt-2.5 flex items-center gap-2 overflow-x-auto pb-0.5">
            <PresetsBar
              inline
              params={params}
              includeInstalled={installsAvailable()}
              hasFilters={hasFilters}
              onApply={applyPreset}
              onClear={clearFilters}
            />

            {shelves.length > 0 && (
              <>
                <span className="h-4 w-px shrink-0 bg-line" aria-hidden="true" />
                <div className="flex shrink-0 items-center gap-1.5" aria-label={t("Platform shelves")}>
                  {shelves.map((platform) => (
                    <button
                      key={platform.id}
                      type="button"
                      onClick={() => update("platform", filters.platform === platform.id ? null : platform.id)}
                      aria-pressed={filters.platform === platform.id}
                      className={cn(
                        "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-[var(--r-sm)] border px-2 text-meta transition-colors",
                        filters.platform === platform.id
                          ? "border-signal/60 bg-signal/12 text-signal-text"
                          : "border-line-2 text-ink-3 hover:border-ink-3 hover:text-ink-2",
                      )}
                    >
                      <span className="uppercase">{platform.id}</span>
                      <span className="num text-micro opacity-70">{platform.gameCount}</span>
                    </button>
                  ))}
                </div>
              </>
            )}

            {importChips.length > 0 && (
              <>
                <span className="h-4 w-px shrink-0 bg-line" aria-hidden="true" />
                <div className="flex shrink-0 items-center gap-1.5" aria-label={t("Launcher imports")}>
                  {importChips.map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      onClick={() => update("launcher", filters.launcher === option.value ? null : option.value)}
                      aria-pressed={filters.launcher === option.value}
                      className={cn(
                        "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-[var(--r-sm)] border px-2 text-meta transition-colors",
                        filters.launcher === option.value
                          ? "border-signal/60 bg-signal/12 text-signal-text"
                          : "border-line-2 text-ink-3 hover:border-ink-3 hover:text-ink-2",
                      )}
                    >
                      <span>{option.label}</span>
                      <span className="num text-micro opacity-70">{importCounts.get(option.value) ?? 0}</span>
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>

          {chips.length > 0 && (
            <div className="mt-2 flex flex-wrap items-center gap-1.5" aria-label={t("Active filters")}>
              {chips.map((chip) => (
                <button
                  key={`${chip.key}-${chip.label}`}
                  type="button"
                  onClick={() => removeChip(chip.key)}
                  aria-label={t("Remove filter: {label}", { label: chip.label })}
                  className="inline-flex h-6 items-center gap-1.5 rounded-[var(--r-sm)] border border-line-2 bg-surface px-1.5 text-meta text-ink-2 transition-colors hover:border-ink-3 hover:text-ink"
                >
                  {chip.label}
                  <X className="size-3 text-ink-3" aria-hidden="true" />
                </button>
              ))}
            </div>
          )}

          {gamepad.connected && (
            <p className="mt-2 hidden items-center gap-2 text-micro uppercase tracking-[0.08em] text-ink-3 sm:flex">
              <Gamepad2 className="size-3" aria-hidden="true" />
              {t("D-pad move · A open · B back · X filters · Y downloads · LB/RB tabs")}
            </p>
          )}
        </div>
      )}

      <div ref={gridRef} className="mt-6">
        {isPending && <LibrarySkeleton view={effectiveView} cardStyle={cardStyle} />}

        {isError && (
          <ErrorState
            title={t("The library didn’t load")}
            message={error instanceof Error ? error.message : t("The server did not answer. Check that it is running, then try again.")}
            onRetry={() => void refetch()}
          />
        )}

        {!isPending && !isError && model.total === 0 && (
          <LibraryEmpty
            mode={mode}
            hasFilters={hasFilters}
            installedOnly={installedOnly}
            isEditor={isEditor}
            signedIn={Boolean(user)}
            onClear={clearFilters}
          />
        )}

        {!isPending && !isError && items.length > 0 && effectiveView === "grid" && (
          <div className={cn("stagger-in grid", gridClassFor(cardStyle, density))}>
            {items.map((entry) => (
              <GameCard
                key={entry.gameId}
                game={entry.game}
                index={entry.gameId}
                variant={cardStyle}
                installed={entry.install ?? undefined}
                source={entry.launcher ? launcherLabel(entry.launcher) : null}
                update={updates.get(entry.gameId)}
                onBookmark={() => toggleBookmark(entry.game)}
                onPlay={
                  entry.install
                    ? () => launch.mutate({ id: entry.gameId, title: entry.title, executable: entry.install?.executable })
                    : undefined
                }
                onUpdate={() => updateGame.mutate({ id: entry.gameId, title: entry.title })}
              />
            ))}
          </div>
        )}

        {!isPending && !isError && imports.length > 0 && effectiveView === "grid" && (
          <section className="mt-10" aria-labelledby="imports-heading">
            <div id="imports-heading">
              <SectionHeader
                title={t("Launcher imports")}
                count={imports.length}
                description={t("Indexed here, launched through their client.")}
              />
            </div>
            <div className="stagger-in grid grid-cols-2 gap-x-6 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6">
              {imports.map((entry) => (
                <ExternalGameCard
                  key={entry.gameId}
                  entry={entry}
                  onOpen={() => setImportEntry(entry)}
                  onLaunch={() => launchImport(entry)}
                  onOpenFolder={() => openImportFolder(entry)}
                />
              ))}
            </div>
          </section>
        )}

        {!isPending && !isError && items.length > 0 && effectiveView === "list" && mode !== "index" && (
          <div className="border-t border-line">
            <div className="grid grid-cols-[2rem_1fr_auto] items-center gap-4 py-2 text-micro uppercase tracking-[0.08em] text-ink-3 sm:grid-cols-[2rem_1fr_4rem_5rem_3rem] md:grid-cols-[2rem_1fr_4rem_5rem_3rem_6rem]">
              <span>{t("No.")}</span>
              <span>{t("Title")}</span>
              <span className="hidden text-right sm:block">{t("Year")}</span>
              <span className="text-right">{t("Size")}</span>
              <span className="hidden text-right md:block">{t("Score")}</span>
              <span className="hidden text-right sm:block">{t("State")}</span>
            </div>
            {items.map((entry) => {
              const game = entry.game;
              return (
                <Link
                  key={game.id}
                  to={`/game/${game.id}`}
                  data-gamepad-target="true"
                  className="row-hover group grid grid-cols-[2rem_1fr_auto] items-center gap-4 border-t border-line py-2.5 sm:grid-cols-[2rem_1fr_4rem_5rem_3rem] md:grid-cols-[2rem_1fr_4rem_5rem_3rem_6rem]"
                >
                  <span className="num text-micro text-ink-3">{String(game.id).padStart(3, "0")}</span>
                  <div className="flex min-w-0 items-center gap-3">
                    <div className="w-9 shrink-0 overflow-hidden rounded-[var(--r-xs)]">
                      <GameCardCoverThumb game={game} />
                    </div>
                    <div className="min-w-0">
                      <p className="truncate text-body text-ink">{game.title}</p>
                      <p className="truncate text-meta text-ink-3">{game.genres.join(", ")}</p>
                    </div>
                    <InstallStateBadges
                      gameId={game.id}
                      installed={Boolean(entry.install)}
                      launcher={entry.launcher}
                    />
                  </div>
                  <span className="num hidden text-right text-meta text-ink-3 sm:block">{formatYear(game.releaseDate)}</span>
                  <span className="num text-right text-meta text-ink-3">{formatBytes(game.totalSize)}</span>
                  <span className="num hidden text-right text-meta text-ink-2 md:block">{formatRating(game.rating) ?? "—"}</span>
                  <span className="hidden justify-end text-meta sm:flex">
                    <span className={cn("inline-flex items-center gap-1.5", STATE_TONE[game.state].text)}>
                      <span className={cn("size-1.5 rounded-full", STATE_TONE[game.state].dot)} aria-hidden="true" />
                      {stateLabel(game.state)}
                    </span>
                  </span>
                </Link>
              );
            })}
          </div>
        )}

        {!isPending && !isError && imports.length > 0 && effectiveView === "list" && (
          <section className="mt-8" aria-labelledby="imports-list-heading">
            <div id="imports-list-heading">
              <SectionHeader
                title={t("Launcher imports")}
                count={imports.length}
                description={t("Indexed here, launched through their client.")}
              />
            </div>
            <div className="border-t border-line">
              {imports.map((entry) => (
                <ExternalGameRow
                  key={entry.gameId}
                  entry={entry}
                  onOpen={() => setImportEntry(entry)}
                  onLaunch={() => launchImport(entry)}
                  onOpenFolder={() => openImportFolder(entry)}
                />
              ))}
            </div>
          </section>
        )}

        {!isPending && !isError && items.length > 0 && mode === "index" && (
          <div>
            <div className="grid grid-cols-[1fr_auto] items-baseline gap-4 border-t border-line py-2 text-micro uppercase tracking-[0.08em] text-ink-3">
              <span className="pl-12">{t("Title")}</span>
              <div className="flex items-baseline gap-5">
                <span className="hidden w-12 sm:inline">{t("Year")}</span>
                <span className="w-16 text-right">{t("Size")}</span>
                <span className="hidden w-20 text-right md:inline">{t("State")}</span>
              </div>
            </div>
            {groups.map(([letter, list]) => (
              <section key={letter} className="grid grid-cols-[2rem_1fr] gap-3 border-t border-line py-4 first:border-t-0 sm:grid-cols-[4rem_1fr] sm:gap-6">
                <h2 className="sticky top-0 self-start font-display text-title font-semibold leading-none text-ink-3">{letter}</h2>
                <div>
                  {list.map((game) => (
                    <Link
                      key={game.id}
                      to={`/game/${game.id}`}
                      data-gamepad-target="true"
                      className="row-hover grid grid-cols-[1fr_auto] items-baseline gap-4 border-b border-line py-2.5 last:border-0"
                    >
                      <div className="flex min-w-0 items-baseline gap-3">
                        <span className="num shrink-0 text-micro text-ink-3">{String(game.id).padStart(3, "0")}</span>
                        <span className="truncate text-lead text-ink">{game.title}</span>
                        <span className="hidden truncate text-meta text-ink-3 lg:inline">{game.genres.join(", ")}</span>
                        {installed.has(game.id) && <Badge tone="outline" className="hidden sm:inline-flex">{t("Installed")}</Badge>}
                      </div>
                      <div className="flex shrink-0 items-baseline gap-5 num text-meta text-ink-3">
                        <span className="hidden w-12 sm:inline">{formatYear(game.releaseDate)}</span>
                        <span className="w-16 text-right">{formatBytes(game.totalSize)}</span>
                        <span className="hidden w-20 text-right md:inline">{stateLabel(game.state)}</span>
                      </div>
                    </Link>
                  ))}
                </div>
              </section>
            ))}
          </div>
        )}
      </div>

      <FilterPanel
        open={filterOpen}
        onOpenChange={setFilterOpen}
        filters={filters}
        activeCount={chips.length}
        onChange={update}
        onClearAll={clearFilters}
      />

      <ExternalImportDialog
        entry={importEntry}
        open={importEntry !== null}
        onOpenChange={(open) => {
          if (!open) setImportEntry(null);
        }}
        onLaunch={launchImport}
        onOpenFolder={openImportFolder}
        onRemove={(entry) => void removeImport(entry)}
        removing={removingImport}
      />

      {debug && <GamepadDebugPanel info={gamepad.debug} />}
    </div>
  );
}
