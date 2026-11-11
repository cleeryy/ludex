import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { useReducedMotion } from "motion/react";
import { Gamepad2, LayoutGrid, List, Plus, SlidersHorizontal, X } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { formatBytes, formatRating, formatYear, sortLabel, stateLabel, formatPercent } from "@/lib/format";
import { isNative } from "@/lib/native";
import type { GameListQuery, GameSummary } from "@/lib/types";
import { useBookmark, useGenres, useInstalledMap, useLaunchGame, useUpdateGame, useUpdateMap } from "@/lib/hooks";
import { useDownloads } from "@/state/downloads";
import { installsAvailable } from "@/lib/install";
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
import { focusGamepadTarget, gamepadDebugEnabled, useGamepad, type GamepadAction } from "@/lib/gamepad";
import { moveTargetIndex, type TargetRect } from "@/lib/gamepad-core";
import { GamepadDebugPanel } from "@/components/gamepad/gamepad-debug";
import { useControllerProfile } from "@/lib/use-controller-profile";
import { OPEN_FILTERS_EVENT } from "@/lib/ui-events";
import { EmptyState, ErrorState, PageHeader } from "@/components/page";
import { GameCard } from "@/components/game/game-card";
import { GameCover } from "@/components/art/cover";
import { STATE_TONE } from "@/components/game/state";
import { FilterPanel } from "@/components/library/filter-panel";
import { LibrarySearch } from "@/components/library/search-field";
import { Badge, Button, Segmented, Skeleton } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { t, tn } from "@/lib/i18n";

export type LibraryMode = "library" | "recent" | "bookmarks" | "index";

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

export function LibraryScreen({ mode }: { mode: LibraryMode }) {
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const location = useLocation();
  const debug = gamepadDebugEnabled(location.search);
  const isEditor = useIsEditor();
  const { user } = useAuth();
  const { view, setView } = useSettings();
  const genres = useGenres();
  const platforms = useQuery({ queryKey: ["platforms"], queryFn: () => api.listPlatforms(), staleTime: 60_000 });
  const shelves = (platforms.data ?? []).filter((platform) => platform.gameCount > 0).slice(0, 8);
  const installed = useInstalledMap();
  const updates = useUpdateMap();
  const updateGame = useUpdateGame();
  const bookmark = useBookmark();
  const launch = useLaunchGame();
  const downloads = useDownloads();
  const reduceMotion = useReducedMotion();

  const filters = readFilters(params, mode);
  const chips = activeFilterChips(filters).filter((chip) => chip.key !== "installed" || installsAvailable());
  const installedOnly = filters.installed && installsAvailable();
  const hasFilters = chips.length > 0 || installedOnly;
  const effectiveView = mode === "index" ? "list" : view;

  const [filterOpen, setFilterOpen] = useState(false);
  const [padIndex, setPadIndex] = useState(0);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
    queryKey: ["games", mode, query],
    queryFn: () => api.listGames(query),
  });

  const all = data?.items ?? [];
  const items = useMemo(() => {
    let list = mode === "recent" ? all.filter((game) => game.lastPlayedAt) : all;
    if (installedOnly) list = list.filter((game) => installed.has(game.id));
    return list;
  }, [all, mode, installedOnly, installed]);

  const totalBytes = items.reduce((sum, game) => sum + game.totalSize, 0);

  const update = (key: FilterKey, value: string | boolean | null) => {
    setParams((current) => setFilterParam(current, key, value), { replace: true });
  };

  const clearFilters = () => {
    setParams((current) => clearFilterParams(current), { replace: true });
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

  const installingPercent = (gameId: number): number | null => {
    const job = downloads.jobForGame(gameId);
    if (!job) return null;
    return job.bytesTotal ? (job.bytesDone / job.bytesTotal) * 100 : 0;
  };

  const groups = useMemo(() => {
    if (mode !== "index") return [];
    const map = new Map<string, GameSummary[]>();
    for (const game of items) {
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
  const gamepad = useGamepad(onGamepad, { profile: controllerProfile, debug });

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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, search, effectiveView, items.length, gamepad.connected]);

  const headerActions = (
    <>
      {isEditor && mode === "library" && (
        <Button variant="ghost" size="sm" icon={<Plus className="size-3.5" />} onClick={() => navigate("/admin/ingest")}>
          {t("Add games")}
        </Button>
      )}
      {mode !== "index" && (
        <Segmented
          label={t("Layout")}
          value={view}
          onChange={setView}
          options={[
            { value: "grid", label: t("Grid"), icon: <LayoutGrid className="size-3.5" /> },
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
            <span className="num">{tn(items.length, "1 game", "{count} games")}</span>
            {totalBytes > 0 && <span className="num">{formatBytes(totalBytes)}</span>}
            {mode === "recent" ? (
              <span>{t("Most recent first")}</span>
            ) : items.length > 0 && mode !== "index" ? (
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

      {mode === "library" && !(items.length === 0 && !hasFilters) && (
        <div className="sticky top-0 z-20 -mx-[var(--page-x)] mt-4 border-b border-line bg-bg px-[var(--page-x)] py-2.5">
          <div className="flex flex-wrap items-center gap-2">
            <LibrarySearch value={filters.q} onCommit={(value) => update("q", value)} className="min-w-0 flex-1 sm:max-w-[340px]" />
            <div className="hidden w-[150px] sm:block">
              <Select
                label={t("Filter by progress state")}
                value={filters.state || "any"}
                onChange={(value) => update("state", value === "any" ? null : value)}
                options={stateOptions()}
                className="h-9 text-[12.5px]"
              />
            </div>
            <Button
              variant="default"
              size="sm"
              icon={<SlidersHorizontal className="size-3.5" />}
              onClick={() => setFilterOpen(true)}
              aria-haspopup="dialog"
            >
              {t("Filters")}
              {chips.length > 0 && (
                <span className="ml-1 inline-flex h-4 min-w-4 items-center justify-center rounded-[2px] bg-signal px-1 font-mono text-[9.5px] text-on-signal">
                  {chips.length}
                </span>
              )}
            </Button>
            <div className="w-[150px] md:hidden">
              <Select
                label={t("Sort games")}
                value={filters.sort}
                onChange={(value) => update("sort", value)}
                options={sortOptions()}
                className="h-9 text-[12.5px]"
              />
            </div>
            <div className="ml-auto flex items-center gap-3">
              {hasFilters && (
                <button
                  type="button"
                  onClick={clearFilters}
                  className="flex h-9 items-center gap-1 text-[12.5px] text-ink-3 transition-colors hover:text-ink"
                >
                  <X className="size-3.5" aria-hidden="true" />
                  {t("Clear all")}
                </button>
              )}
            </div>
          </div>

          {shelves.length > 0 && (
            <div className="mt-3 flex flex-wrap items-center gap-1.5" aria-label={t("Platform shelves")}>
              <span className="mr-1 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{t("Platforms")}</span>
              {shelves.map((platform) => (
                <button
                  key={platform.id}
                  type="button"
                  onClick={() => update("platform", filters.platform === platform.id ? null : platform.id)}
                  aria-pressed={filters.platform === platform.id}
                  className={cn(
                    "inline-flex h-6 items-center gap-1.5 rounded-[2px] border px-1.5 font-mono text-[10.5px] uppercase tracking-[0.06em] transition-colors",
                    filters.platform === platform.id
                      ? "border-signal/60 bg-signal/12 text-signal-text"
                      : "border-line-2 text-ink-3 hover:border-ink-3 hover:text-ink-2",
                  )}
                >
                  {platform.id}
                  <span className="num text-[9.5px] opacity-70">{platform.gameCount}</span>
                </button>
              ))}
            </div>
          )}

          <div className="mt-2 flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-center sm:gap-x-6">
            <div className="nav-scroll -mb-0.5 flex min-w-0 flex-1 items-center gap-4 overflow-x-auto">
              <button
                type="button"
                onClick={() => update("genre", null)}
                className={cn(
                  "relative h-8 shrink-0 text-[13px] transition-colors",
                  !filters.genre ? "text-ink" : "text-ink-3 hover:text-ink-2",
                )}
              >
                {t("All")}
                {!filters.genre && <span className="absolute inset-x-0 bottom-0 h-[2px] bg-signal" aria-hidden="true" />}
              </button>
              {genres.map((item) => (
                <button
                  key={item}
                  type="button"
                  onClick={() => update("genre", item === filters.genre ? null : item)}
                  aria-pressed={filters.genre === item}
                  className={cn(
                    "relative h-8 shrink-0 text-[13px] transition-colors",
                    filters.genre === item ? "text-ink" : "text-ink-3 hover:text-ink-2",
                  )}
                >
                  {item}
                  {filters.genre === item && <span className="absolute inset-x-0 bottom-0 h-[2px] bg-signal" aria-hidden="true" />}
                </button>
              ))}
            </div>
          </div>

          {chips.length > 0 && (
            <div className="mt-2.5 flex flex-wrap items-center gap-1.5" aria-label={t("Active filters")}>
              {chips.map((chip) => (
                <button
                  key={`${chip.key}-${chip.label}`}
                  type="button"
                  onClick={() => removeChip(chip.key)}
                  aria-label={t("Remove filter: {label}", { label: chip.label })}
                  className="inline-flex h-6 items-center gap-1.5 rounded-[2px] border border-line-2 bg-surface px-1.5 text-[11.5px] text-ink-2 transition-colors hover:border-ink-3 hover:text-ink"
                >
                  {chip.label}
                  <X className="size-3 text-ink-3" aria-hidden="true" />
                </button>
              ))}
              <button
                type="button"
                onClick={clearFilters}
                className="ml-1 h-6 text-[11.5px] text-ink-3 transition-colors hover:text-ink"
              >
                {t("Clear all")}
              </button>
            </div>
          )}

          {gamepad.connected && (
            <p className="mt-2 hidden items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3 sm:flex">
              <Gamepad2 className="size-3" aria-hidden="true" />
              {t("D-pad move · A open · B back · X filters · Y downloads · LB/RB tabs")}
            </p>
          )}
        </div>
      )}

      <div ref={gridRef} className="mt-7">
        {isPending && <LibrarySkeleton view={effectiveView} />}

        {isError && (
          <ErrorState
            title={t("The library didn’t load")}
            message={error instanceof Error ? error.message : t("The server did not answer. Check that it is running, then try again.")}
            onRetry={() => void refetch()}
          />
        )}

        {!isPending && !isError && items.length === 0 && (
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
          <div className="stagger-in grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6">
            {items.map((game) => (
              <GameCard
                key={game.id}
                game={game}
                index={game.id}
                installed={installed.get(game.id)}
                installing={installingPercent(game.id)}
                update={updates.get(game.id)}
                onBookmark={() => toggleBookmark(game)}
                onPlay={
                  isNative() && installed.has(game.id)
                    ? () => launch.mutate({ id: game.id, title: game.title, executable: installed.get(game.id)?.executable })
                    : undefined
                }
                onUpdate={
                  isNative() && updates.has(game.id)
                    ? () => updateGame.mutate({ id: game.id, title: game.title })
                    : undefined
                }
              />
            ))}
          </div>
        )}

        {!isPending && !isError && items.length > 0 && effectiveView === "list" && mode !== "index" && (
          <div className="border-t border-line">
            <div className="grid grid-cols-[2rem_1fr_auto] items-center gap-4 py-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3 sm:grid-cols-[2rem_1fr_4rem_5rem_3rem] md:grid-cols-[2rem_1fr_4rem_5rem_3rem_6rem]">
              <span>{t("No.")}</span>
              <span>{t("Title")}</span>
              <span className="hidden text-right sm:block">{t("Year")}</span>
              <span className="text-right">{t("Size")}</span>
              <span className="hidden text-right md:block">{t("Score")}</span>
              <span className="hidden text-right sm:block">{t("State")}</span>
            </div>
            {items.map((game) => (
              <Link
                key={game.id}
                to={`/game/${game.id}`}
                data-gamepad-target="true"
                className="row-hover group grid grid-cols-[2rem_1fr_auto] items-center gap-4 border-t border-line py-2.5 sm:grid-cols-[2rem_1fr_4rem_5rem_3rem] md:grid-cols-[2rem_1fr_4rem_5rem_3rem_6rem]"
              >
                <span className="num text-[10.5px] text-ink-3">{String(game.id).padStart(3, "0")}</span>
                <div className="flex min-w-0 items-center gap-3">
                  <div className="w-9 shrink-0 overflow-hidden rounded-[1px]">
                    <GameCardCoverThumb game={game} />
                  </div>
                  <div className="min-w-0">
                    <p className="truncate text-[14px] text-ink">{game.title}</p>
                    <p className="truncate text-[11.5px] text-ink-3">{game.genres.join(", ")}</p>
                  </div>
                  {installingPercent(game.id) !== null ? (
                    <Badge tone="signal" className="hidden shrink-0 sm:inline-flex">
                      {t("Installing {percent}", { percent: formatPercent(installingPercent(game.id)) })}
                    </Badge>
                  ) : (
                    installed.has(game.id) && <Badge tone="outline" className="hidden shrink-0 sm:inline-flex">{t("Installed")}</Badge>
                  )}
                </div>
                <span className="num hidden text-right text-[11.5px] text-ink-3 sm:block">{formatYear(game.releaseDate)}</span>
                <span className="num text-right text-[11.5px] text-ink-3">{formatBytes(game.totalSize)}</span>
                <span className="num hidden text-right text-[11.5px] text-ink-2 md:block">{formatRating(game.rating) ?? "—"}</span>
                <span className="hidden justify-end text-[11.5px] sm:flex">
                  <span className={cn("inline-flex items-center gap-1.5", STATE_TONE[game.state].text)}>
                    <span className={cn("size-1.5 rounded-full", STATE_TONE[game.state].dot)} aria-hidden="true" />
                    {stateLabel(game.state)}
                  </span>
                </span>
              </Link>
            ))}
          </div>
        )}

        {!isPending && !isError && items.length > 0 && mode === "index" && (
          <div>
            <div className="grid grid-cols-[1fr_auto] items-baseline gap-4 border-t border-line py-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3">
              <span className="pl-12">{t("Title")}</span>
              <div className="flex items-baseline gap-5">
                <span className="hidden w-12 sm:inline">{t("Year")}</span>
                <span className="w-16 text-right">{t("Size")}</span>
                <span className="hidden w-20 text-right md:inline">{t("State")}</span>
              </div>
            </div>
            {groups.map(([letter, list]) => (
              <section key={letter} className="grid grid-cols-[2rem_1fr] gap-3 border-t border-line py-4 first:border-t-0 sm:grid-cols-[4rem_1fr] sm:gap-6">
                <h2 className="sticky top-0 self-start font-display text-[26px] font-semibold leading-none text-ink-3">{letter}</h2>
                <div>
                  {list.map((game) => (
                    <Link
                      key={game.id}
                      to={`/game/${game.id}`}
                      data-gamepad-target="true"
                      className="row-hover grid grid-cols-[1fr_auto] items-baseline gap-4 border-b border-line py-2.5 last:border-0"
                    >
                      <div className="flex min-w-0 items-baseline gap-3">
                        <span className="num shrink-0 text-[10.5px] text-ink-3">{String(game.id).padStart(3, "0")}</span>
                        <span className="truncate text-[15px] text-ink">{game.title}</span>
                        <span className="hidden truncate text-[11.5px] text-ink-3 lg:inline">{game.genres.join(", ")}</span>
                        {installed.has(game.id) && <Badge tone="outline" className="hidden sm:inline-flex">{t("Installed")}</Badge>}
                      </div>
                      <div className="flex shrink-0 items-baseline gap-5 num text-[11px] text-ink-3">
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

      {debug && <GamepadDebugPanel info={gamepad.debug} />}
    </div>
  );
}

function GameCardCoverThumb({ game }: { game: GameSummary }) {
  return <GameCover title={game.title} coverUrl={game.coverUrl} showType={false} />;
}

function LibrarySkeleton({ view }: { view: "grid" | "list" }) {
  if (view === "list") {
    return (
      <div className="border-t border-line">
        {Array.from({ length: 9 }).map((_, i) => (
          <div key={i} className="flex items-center gap-4 border-b border-line py-3">
            <Skeleton className="h-3 w-6" />
            <Skeleton className="h-12 w-9" />
            <div className="flex-1 space-y-2">
              <Skeleton className="h-3.5 w-1/3" />
              <Skeleton className="h-2.5 w-1/5" />
            </div>
            <Skeleton className="hidden h-3 w-16 sm:block" />
          </div>
        ))}
      </div>
    );
  }
  return (
    <div className="grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6">
      {Array.from({ length: 10 }).map((_, i) => (
        <div key={i}>
          <Skeleton className="aspect-[3/4] w-full" />
          <div className="mt-3 space-y-2">
            <Skeleton className="h-3.5 w-4/5" />
            <Skeleton className="h-2.5 w-2/5" />
          </div>
        </div>
      ))}
    </div>
  );
}

function LibraryEmpty({
  mode,
  hasFilters,
  installedOnly,
  isEditor,
  signedIn,
  onClear,
}: {
  mode: LibraryMode;
  hasFilters: boolean;
  installedOnly: boolean;
  isEditor: boolean;
  signedIn: boolean;
  onClear: () => void;
}) {
  const navigate = useNavigate();
  if (mode === "library" && hasFilters && installedOnly) {
    return (
      <EmptyState
        title={t("No installed games")}
        description={t("Nothing from this library is installed on this machine yet. Install a game from its page, or show the full library.")}
      >
        <Button variant="default" onClick={onClear}>
          {t("Show all games")}
        </Button>
      </EmptyState>
    );
  }
  if (hasFilters) {
    return (
      <EmptyState
        title={t("No games match")}
        description={t("Nothing in the library fits those filters. Widen the search, or clear them and start again.")}
      >
        <Button variant="default" onClick={onClear}>
          {t("Clear filters")}
        </Button>
      </EmptyState>
    );
  }

  if (mode === "bookmarks") {
    return (
      <EmptyState title={t("No bookmarks yet")} description={t("Bookmark a game from the library and it will wait for you here.")}>
        <Button variant="default" onClick={() => navigate("/library")}>
          {t("Browse the library")}
        </Button>
      </EmptyState>
    );
  }

  if (mode === "recent") {
    return (
      <EmptyState
        title={t("Nothing played yet")}
        description={t("Launch a game from the desktop app and it will show up here with its playtime.")}
      >
        <Button variant="default" onClick={() => navigate("/library")}>
          {t("Browse the library")}
        </Button>
      </EmptyState>
    );
  }

  return (
    <EmptyState
      className="max-w-[760px]"
      title={t("Nothing on the shelf yet")}
      description={
        isEditor
          ? t("The games folder is empty. Paste a download link and Ludex will fetch it, verify the archive and index it for you.")
          : t("This library is empty. An administrator can add games from the server, or drop archives into the games folder.")
      }
    >
      {isEditor ? (
        <Button variant="primary" onClick={() => navigate("/admin/ingest")} icon={<Plus className="size-4" />}>
          {t("Add games")}
        </Button>
      ) : (
        signedIn && <span className="text-[13px] text-ink-3">{t("Ask an administrator to add the first game.")}</span>
      )}
    </EmptyState>
  );
}
