import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { useReducedMotion } from "motion/react";
import { Gamepad2, LayoutGrid, List, Plus, SlidersHorizontal, X } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { formatBytes, formatRating, formatYear, stateLabel } from "@/lib/format";
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
  SORT_LABELS,
  type FilterKey,
} from "@/lib/library-filters";
import { focusGamepadTarget, useGamepad, type GamepadAction } from "@/lib/gamepad";
import { OPEN_FILTERS_EVENT } from "@/lib/ui-events";
import { EmptyState, ErrorState, PageHeader } from "@/components/page";
import { GameCard } from "@/components/game/game-card";
import { GameCover } from "@/components/art/cover";
import { STATE_TONE } from "@/components/game/state";
import { FilterPanel } from "@/components/library/filter-panel";
import { LibrarySearch } from "@/components/library/search-field";
import { Badge, Button, Segmented, Skeleton } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";

export type LibraryMode = "library" | "recent" | "bookmarks" | "index";

const MODE_TITLE: Record<LibraryMode, string> = {
  library: "Library",
  recent: "Recently played",
  bookmarks: "Bookmarks",
  index: "Index",
};

const MODE_DESCRIPTION: Record<LibraryMode, string> = {
  library: "",
  recent: "Everything you have launched, most recent first.",
  bookmarks: "Titles you set aside for later.",
  index: "Every title in the library, A to Z.",
};

const MODE_ROUTES: Record<LibraryMode, string> = {
  library: "/library",
  recent: "/recent",
  bookmarks: "/bookmarks",
  index: "/all",
};

const SORT_OPTIONS = Object.entries(SORT_LABELS).map(([value, label]) => ({ value, label }));

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

export function LibraryScreen({ mode }: { mode: LibraryMode }) {
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const isEditor = useIsEditor();
  const { user } = useAuth();
  const { view, setView } = useSettings();
  const genres = useGenres();
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
    if (saved) setParams(saved, { replace: true });
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

  const targets = () =>
    Array.from(gridRef.current?.querySelectorAll<HTMLElement>("[data-gamepad-target]") ?? []).filter(
      (element) => element.offsetParent !== null,
    );

  const columns = (elements: HTMLElement[]): number => {
    if (elements.length < 2) return 1;
    const top = elements[0].offsetTop;
    let count = 1;
    for (let index = 1; index < elements.length; index += 1) {
      if (elements[index].offsetTop !== top) break;
      count += 1;
    }
    return Math.max(1, count);
  };

  const currentIndex = (elements: HTMLElement[]): number => {
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
      elements[index]?.click();
      return;
    }

    const cols = columns(elements);
    const next =
      action === "left"
        ? index - 1
        : action === "right"
          ? index + 1
          : action === "up"
            ? index - cols
            : index + cols;
    const bounded = clamp(next, 0, elements.length - 1);
    setPadIndex(bounded);
    focusGamepadTarget(elements, bounded, !reduceMotion);
  };

  const gamepad = useGamepad(onGamepad, { enabled: !filterOpen });

  useEffect(() => {
    setPadIndex(0);
  }, [mode, search]);

  useEffect(() => {
    if (!gamepad.connected) return;
    const elements = targets();
    if (elements.length === 0) return;
    setPadIndex((current) => clamp(current, 0, elements.length - 1));
    // Only clamp when the result set changes size.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items.length, gamepad.connected]);

  const headerActions = (
    <>
      {isEditor && mode === "library" && (
        <Button variant="ghost" size="sm" icon={<Plus className="size-3.5" />} onClick={() => navigate("/admin/ingest")}>
          Add games
        </Button>
      )}
      {mode !== "index" && (
        <Segmented
          label="Layout"
          value={view}
          onChange={setView}
          options={[
            { value: "grid", label: "Grid", icon: <LayoutGrid className="size-3.5" /> },
            { value: "list", label: "List", icon: <List className="size-3.5" /> },
          ]}
        />
      )}
    </>
  );

  return (
    <div className="page page-in">
      <PageHeader
        title={MODE_TITLE[mode]}
        meta={
          <>
            <span className="num">{items.length} games</span>
            {totalBytes > 0 && <span className="num">{formatBytes(totalBytes)}</span>}
            {mode === "recent" ? (
              <span>Most recent first</span>
            ) : items.length > 0 && mode !== "index" ? (
              <span>
                {mode === "library" && filters.q.trim()
                  ? "Ranked by relevance"
                  : `Sorted by ${SORT_LABELS[filters.sort] ?? filters.sort}`}
              </span>
            ) : null}
            {gamepad.connected && (
              <span className="inline-flex items-center gap-1.5 text-ok">
                <Gamepad2 className="size-3.5" aria-hidden="true" />
                Gamepad
              </span>
            )}
          </>
        }
        description={MODE_DESCRIPTION[mode] || undefined}
        actions={headerActions}
      />

      {mode === "library" && !(items.length === 0 && !hasFilters) && (
        <div className="sticky top-0 z-20 -mx-[var(--page-x)] mt-4 border-b border-line bg-bg px-[var(--page-x)] py-2.5">
          <div className="flex flex-wrap items-center gap-2">
            <LibrarySearch value={filters.q} onCommit={(value) => update("q", value)} className="min-w-0 flex-1 sm:max-w-[340px]" />
            <div className="hidden w-[150px] sm:block">
              <Select
                label="Filter by progress state"
                value={filters.state || "any"}
                onChange={(value) => update("state", value === "any" ? null : value)}
                options={[
                  { value: "any", label: "Any state" },
                  { value: "unplayed", label: "Unplayed" },
                  { value: "playing", label: "Playing" },
                  { value: "completed", label: "Completed" },
                  { value: "onHold", label: "On hold" },
                  { value: "abandoned", label: "Abandoned" },
                  { value: "planToPlay", label: "Planned" },
                ]}
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
              Filters
              {chips.length > 0 && (
                <span className="ml-1 inline-flex h-4 min-w-4 items-center justify-center rounded-[2px] bg-signal px-1 font-mono text-[9.5px] text-on-signal">
                  {chips.length}
                </span>
              )}
            </Button>
            <div className="w-[150px] md:hidden">
              <Select
                label="Sort games"
                value={filters.sort}
                onChange={(value) => update("sort", value)}
                options={SORT_OPTIONS}
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
                  Clear all
                </button>
              )}
            </div>
          </div>

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
                All
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
            <div className="mt-2.5 flex flex-wrap items-center gap-1.5" aria-label="Active filters">
              {chips.map((chip) => (
                <button
                  key={`${chip.key}-${chip.label}`}
                  type="button"
                  onClick={() => removeChip(chip.key)}
                  aria-label={`Remove filter: ${chip.label}`}
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
                Clear all
              </button>
            </div>
          )}

          {gamepad.connected && (
            <p className="mt-2 hidden items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3 sm:flex">
              <Gamepad2 className="size-3" aria-hidden="true" />
              D-pad move · A open · B back · X filters · Y downloads · LB/RB tabs
            </p>
          )}
        </div>
      )}

      <div ref={gridRef} className="mt-7">
        {isPending && <LibrarySkeleton view={effectiveView} />}

        {isError && (
          <ErrorState
            title="The library didn’t load"
            message={error instanceof Error ? error.message : "The server did not answer. Check that it is running, then try again."}
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
              <span>No.</span>
              <span>Title</span>
              <span className="hidden text-right sm:block">Year</span>
              <span className="text-right">Size</span>
              <span className="hidden text-right md:block">Score</span>
              <span className="hidden text-right sm:block">State</span>
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
                      Installing {installingPercent(game.id)!.toFixed(0)}%
                    </Badge>
                  ) : (
                    installed.has(game.id) && <Badge tone="outline" className="hidden shrink-0 sm:inline-flex">Installed</Badge>
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
              <span className="pl-12">Title</span>
              <div className="flex items-baseline gap-5">
                <span className="hidden w-12 sm:inline">Year</span>
                <span className="w-16 text-right">Size</span>
                <span className="hidden w-20 text-right md:inline">State</span>
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
                        {installed.has(game.id) && <Badge tone="outline" className="hidden sm:inline-flex">Installed</Badge>}
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
        title="No installed games"
        description="Nothing from this library is installed on this machine yet. Install a game from its page, or show the full library."
      >
        <Button variant="default" onClick={onClear}>
          Show all games
        </Button>
      </EmptyState>
    );
  }
  if (hasFilters) {
    return (
      <EmptyState
        title="No games match"
        description="Nothing in the library fits those filters. Widen the search, or clear them and start again."
      >
        <Button variant="default" onClick={onClear}>
          Clear filters
        </Button>
      </EmptyState>
    );
  }

  if (mode === "bookmarks") {
    return (
      <EmptyState title="No bookmarks yet" description="Bookmark a game from the library and it will wait for you here.">
        <Button variant="default" onClick={() => navigate("/library")}>
          Browse the library
        </Button>
      </EmptyState>
    );
  }

  if (mode === "recent") {
    return (
      <EmptyState
        title="Nothing played yet"
        description="Launch a game from the desktop app and it will show up here with its playtime."
      >
        <Button variant="default" onClick={() => navigate("/library")}>
          Browse the library
        </Button>
      </EmptyState>
    );
  }

  return (
    <EmptyState
      className="max-w-[760px]"
      title="Nothing on the shelf yet"
      description={
        isEditor
          ? "The games folder is empty. Paste a download link and Ludex will fetch it, verify the archive and index it for you."
          : "This library is empty. An administrator can add games from the server, or drop archives into the games folder."
      }
    >
      {isEditor ? (
        <Button variant="primary" onClick={() => navigate("/admin/ingest")} icon={<Plus className="size-4" />}>
          Add games
        </Button>
      ) : (
        signedIn && <span className="text-[13px] text-ink-3">Ask an administrator to add the first game.</span>
      )}
    </EmptyState>
  );
}
