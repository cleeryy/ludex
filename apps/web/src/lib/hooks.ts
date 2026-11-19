import { useEffect, useMemo } from "react";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { isNative, native } from "@/lib/native";
import { api } from "@/lib/api";
import { USING_MOCKS } from "@/lib/flags";
import { mockInstalls } from "@/lib/mock-install";
import type { GameDetail, GameList, GameSummary, GameUpdate, InstalledGame } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { t, tn } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/** Installed games, merged into the server library when running in Tauri. */
export function useInstalledGames() {
  return useQuery({
    queryKey: qk.installed,
    queryFn: () => (USING_MOCKS ? mockInstalls.installed() : native.listInstalled()),
    enabled: isNative() || USING_MOCKS,
    staleTime: 10_000,
    refetchOnWindowFocus: true,
  });
}

/** The whole library, used by Home, Downloads and the install manager. */
export function useAllGames() {
  return useQuery({
    queryKey: qk.games.all(),
    queryFn: () => api.listGames({ limit: 200 }),
    staleTime: 60_000,
  });
}

/** Server page size for the paginated library (#267). The API caps at 200. */
export const LIBRARY_PAGE_SIZE = 200;

export interface LibraryPages {
  items: GameSummary[];
  total: number;
}

/**
 * The next page to request, or `undefined` when the last answer covered the
 * library. Pure so the pagination rule is tested without a query client: a
 * server that underfills a page (an age filter, a vanished archive) still ends
 * the walk instead of looping on an empty response.
 */
export function nextLibraryPage(lastPage: GameList | undefined, pageCount: number): number | undefined {
  if (!lastPage) return 1;
  if (lastPage.items.length === 0) return undefined;
  if (lastPage.total > 0 && lastPage.page * lastPage.limit >= lastPage.total) return undefined;
  return pageCount + 1;
}

/**
 * Flatten the fetched pages, keeping server order and dropping duplicates: a
 * game inserted between two fetches can arrive twice, and React keys (and the
 * console's selection index) must stay one per game.
 */
export function flattenLibraryPages(pages: readonly GameList[]): LibraryPages {
  const seen = new Set<number>();
  const items: GameSummary[] = [];
  for (const page of pages) {
    for (const game of page.items) {
      if (seen.has(game.id)) continue;
      seen.add(game.id);
      items.push(game);
    }
  }
  const total = pages.length > 0 ? Math.max(pages[0].total, items.length) : 0;
  return { items, total };
}

/**
 * Paginated library (#267). The 200-row cap used to be the whole library; the
 * Console asks for the next page when the ring approaches the end of what is
 * loaded, so a 600-game archive renders every game without one 3-page fetch.
 */
export function useLibraryGames() {
  const query = useInfiniteQuery({
    queryKey: qk.games.pages(),
    queryFn: ({ pageParam }) => api.listGames({ limit: LIBRARY_PAGE_SIZE, page: pageParam }),
    initialPageParam: 1,
    getNextPageParam: (lastPage, pages) => nextLibraryPage(lastPage, pages.length),
    staleTime: 60_000,
  });
  const flattened = useMemo(() => flattenLibraryPages(query.data?.pages ?? []), [query.data]);
  return {
    items: flattened.items,
    total: flattened.total,
    isPending: query.isPending,
    isError: query.isError,
    error: query.error,
    isFetching: query.isFetching,
    refetch: query.refetch,
    hasNextPage: query.hasNextPage,
    isFetchingNextPage: query.isFetchingNextPage,
    fetchNextPage: query.fetchNextPage,
  };
}

/** Recently launched games on this device, newest first (Home + tray). */
export function useRecentGames(limit = 6) {
  return useQuery({
    queryKey: qk.recentGames.byLimit(limit),
    queryFn: () => (USING_MOCKS ? mockInstalls.installed() : native.listRecentGames(limit)),
    enabled: isNative() || USING_MOCKS,
    staleTime: 15_000,
  });
}

/**
 * Installed games with a newer version on the server. Checked on demand and
 * whenever the install list changes; empty in the browser build.
 */
export function useGameUpdates() {
  return useQuery({
    queryKey: qk.gameUpdates,
    queryFn: () => (USING_MOCKS ? Promise.resolve([]) : native.checkGameUpdates()),
    enabled: isNative() && !USING_MOCKS,
    staleTime: 5 * 60_000,
    retry: 0,
  });
}

export function useUpdateMap(): Map<number, GameUpdate> {
  const { data } = useGameUpdates();
  return new Map((data ?? []).map((update) => [update.gameId, update]));
}

/** Desktop playtime: totals, per-day buckets and recent sessions. */
export function useGamePlaytime(gameId: number, enabled = true) {
  return useQuery({
    queryKey: qk.playtime.forGame(gameId),
    queryFn: () => native.getGamePlaytime(gameId),
    enabled: isNative() && enabled && Number.isFinite(gameId),
    staleTime: 30_000,
  });
}

/** Install the newest version of a game (reuses the install pipeline). */
export function useUpdateGame() {
  const queryClient = useQueryClient();
  const { push } = useToast();

  return useMutation({
    mutationFn: (game: { id: number; title: string }) => native.updateGame(game.id),
    onSuccess: (_job, game) => {
      push({
        title: t("Updating {title}", { title: game.title }),
        description: t("The previous install stays playable until the new files are in place."),
      });
      void queryClient.invalidateQueries({ queryKey: qk.gameUpdates });
      void queryClient.invalidateQueries({ queryKey: qk.games.root });
    },
    onError: (error: Error) => {
      push({ title: t("Could not start the update"), description: error.message, tone: "danger" });
    },
  });
}

export function useInstalledMap(): Map<number, InstalledGame> {
  const { data } = useInstalledGames();
  return new Map((data ?? []).map((game) => [game.gameId, game]));
}

export function useGenres() {
  const { data } = useQuery({
    queryKey: qk.facets.genres,
    queryFn: () => api.listGames({ limit: 200 }),
    staleTime: 5 * 60_000,
  });
  const genres = new Set<string>();
  for (const game of data?.items ?? []) for (const genre of game.genres) genres.add(genre);
  return [...genres].sort((a, b) => a.localeCompare(b));
}

export function useBookmark() {
  const queryClient = useQueryClient();
  const { push } = useToast();

  return useMutation({
    mutationFn: ({ id, bookmarked }: { id: number; bookmarked: boolean }) => api.setBookmark(id, bookmarked),
    onMutate: async ({ id, bookmarked }) => {
      await queryClient.cancelQueries({ queryKey: qk.games.root });
      queryClient.setQueriesData<GameList>({ queryKey: qk.games.root }, (current) =>
        current
          ? { ...current, items: current.items.map((game) => (game.id === id ? { ...game, bookmarked } : game)) }
          : current,
      );
      queryClient.setQueryData<GameDetail>(qk.game.detail(id), (current) => (current ? { ...current, bookmarked } : current));
      queryClient.setQueryData<GameList>(qk.paletteGames, (current) =>
        current
          ? { ...current, items: current.items.map((game) => (game.id === id ? { ...game, bookmarked } : game)) }
          : current,
      );
    },
    onError: (error: Error) => {
      push({ title: t("Bookmark not saved"), description: error.message, tone: "danger" });
      void queryClient.invalidateQueries({ queryKey: qk.games.root });
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: qk.games.root });
      void queryClient.invalidateQueries({ queryKey: qk.game.root });
    },
  });
}

/**
 * Launches a game and refreshes the library around it.
 *
 * `silent` is for Console: the launch plate is the only messaging channel there,
 * and a desktop toast would paint over a TV interface (#268).
 */
export function useLaunchGame({ silent = false }: { silent?: boolean } = {}) {
  const queryClient = useQueryClient();
  const { push } = useToast();

  return useMutation({
    mutationFn: (game: { id: number; title: string; executable?: string | null }) =>
      native.launchGame({ gameId: game.id, executable: game.executable ?? null }),
    onSuccess: (_result, game) => {
      if (!silent) {
        push({ title: t("{title} is starting", { title: game.title }), description: t("Playtime is recorded when the game exits.") });
      }
      void queryClient.invalidateQueries({ queryKey: qk.games.root });
      void queryClient.invalidateQueries({ queryKey: qk.recentGames.root });
      // #191: the game page flips to Resume without waiting for the next poll.
      void queryClient.invalidateQueries({ queryKey: qk.gameSession.forGame(game.id) });
    },
    onError: (error: Error) => {
      if (silent) return;
      push({ title: t("Could not launch the game"), description: error.message, tone: "danger" });
    },
  });
}

/**
 * The live session of one game (#191).
 *
 * Polled while the page is open, because the process can start or end without
 * a command from this window (tray launch, crash). `game://exited` invalidates
 * it immediately, so a crashed game never leaves a stale "running" hero.
 */
export function useGameSession(gameId: number, enabled = true) {
  return useQuery({
    queryKey: qk.gameSession.forGame(gameId),
    queryFn: () => native.gameSession(gameId),
    enabled: isNative() && enabled && Number.isFinite(gameId),
    refetchInterval: 2_500,
    refetchOnWindowFocus: true,
    staleTime: 0,
    retry: 0,
  });
}

/** Bring the running game's window back; the UI explains the Alt+Tab fallback. */
export function useResumeGame() {
  const { push } = useToast();

  return useMutation({
    mutationFn: (gameId: number) => native.resumeGame(gameId),
    onSuccess: (result) => {
      if (result.focused) return;
      push({
        title: t("Game window not found"),
        description: t("Ludex was hidden. Use Alt+Tab to switch back to the game."),
      });
    },
    onError: (error: Error) => {
      push({ title: t("Could not resume the game"), description: error.message, tone: "danger" });
    },
  });
}

/** Force close a running game and refresh what the page shows (#191). */
export function useForceCloseGame(gameId: number) {
  const queryClient = useQueryClient();
  const { push } = useToast();

  return useMutation({
    mutationFn: (sessionId: string) => native.forceCloseGame(sessionId),
    onSuccess: () => {
      push({
        title: t("Game closed"),
        description: t("The game stopped and its session was saved."),
        tone: "success",
      });
    },
    onError: (error: Error) => {
      push({ title: t("Could not close the game"), description: error.message, tone: "danger" });
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: qk.gameSession.forGame(gameId) });
      void queryClient.invalidateQueries({ queryKey: qk.playtime.forGame(gameId) });
      void queryClient.invalidateQueries({ queryKey: qk.installed });
      void queryClient.invalidateQueries({ queryKey: qk.recentGames.root });
      void queryClient.invalidateQueries({ queryKey: qk.game.detail(gameId) });
    },
  });
}

/** Global Tauri event wiring: playtime sync and installed-state refresh. */
export function useNativeEvents() {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const navigate = useNavigate();

  useEffect(() => {
    if (!isNative()) return;
    const unlisteners: Array<() => void> = [];
    let cancelled = false;

    void (async () => {
      const offExited = await native.onGameExited(({ gameId, minutesPlayed, exitCode, immediateExit }) => {
        void queryClient.invalidateQueries({ queryKey: qk.installed });
        void queryClient.invalidateQueries({ queryKey: qk.recentGames.root });
        void queryClient.invalidateQueries({ queryKey: qk.playtime.forGame(gameId) });
        void queryClient.invalidateQueries({ queryKey: qk.gameSession.forGame(gameId) });
        void queryClient.invalidateQueries({ queryKey: qk.game.detail(gameId) });
        void queryClient.invalidateQueries({ queryKey: qk.games.root });
        // #187: a game that dies within seconds is a failed launch, not a
        // session; show the exit code (NTSTATUS values in hex) instead of
        // silently doing nothing.
        if (immediateExit) {
          const code = exitCode !== null && exitCode !== undefined
            ? `0x${exitCode.toString(16).toUpperCase()}`
            : null;
          push({
            title: code
              ? t("The game exited immediately (code {code})", { code })
              : t("The game exited immediately"),
            tone: "danger",
          });
        } else if (minutesPlayed > 0) {
          push({
            title: t("Session saved"),
            description: tn(minutesPlayed, "1 minute added to your playtime.", "{count} minutes added to your playtime."),
            tone: "success",
          });
        }
      });
      if (cancelled) offExited();
      else unlisteners.push(offExited);

      const offStatus = await native.onLocalServerStatus(() => {
        void queryClient.invalidateQueries();
      });
      if (cancelled) offStatus();
      else unlisteners.push(offStatus);

      // #219: the game is running but its window could not be brought forward.
      // The launch already succeeded, so this is a warning, not an error: the
      // game page's Resume and Force close actions are the recovery.
      const offFocus = await native.onGameFocus(({ focused }) => {
        if (focused) return;
        push({
          title: t("Game window not found"),
          description: t(
            "The game is running, but Ludex could not bring its window forward. Use Resume to try again, or Force close on the game page.",
          ),
          tone: "danger",
        });
      });
      if (cancelled) offFocus();
      else unlisteners.push(offFocus);

      // Tray menu (#15): navigate to a route or launch a recent game.
      const offNavigate = await native.onNavigate((path) => navigate(path));
      if (cancelled) offNavigate();
      else unlisteners.push(offNavigate);

      const offLaunch = await native.onLaunchGame((gameId) => {
        void native
          .launchGame({ gameId })
          .then(() => {
            void queryClient.invalidateQueries({ queryKey: qk.recentGames.root });
            void queryClient.invalidateQueries({ queryKey: qk.games.root });
            // #191: a tray launch while the game page is open flips it to Resume.
            void queryClient.invalidateQueries({ queryKey: qk.gameSession.root });
          })
          .catch((cause: Error) => push({ title: t("Could not launch the game"), description: cause.message, tone: "danger" }));
      });
      if (cancelled) offLaunch();
      else unlisteners.push(offLaunch);

      // Suspend inhibitor (#288): the desktop pushes a snapshot on every
      // reason change, so the banner and the diagnostics row flip immediately
      // instead of waiting for the next poll.
      const offInhibit = await native.onInhibitStatus((status) => {
        queryClient.setQueryData(qk.inhibitStatus, status);
      });
      if (cancelled) offInhibit();
      else unlisteners.push(offInhibit);

      // App updates (#16, #163): the update center's notifier owns the launch
      // prompt, the badge and the skip decision, so no duplicate toast here.
    })();

    // Mobile companion (#167): claim installs queued from a phone and mirror
    // their progress. The first pass waits for the window to settle; later
    // passes are cheap no-ops when nothing is queued.
    const syncRemote = () => {
      if (cancelled) return;
      void native.remoteInstallsSync().then((result) => {
        if (cancelled || result.claimed.length === 0) return;
        void queryClient.invalidateQueries({ queryKey: qk.remoteInstalls });
        void queryClient.invalidateQueries({ queryKey: qk.installJobs });
        push({
          title: t("Remote install started"),
          description: tn(
            result.claimed.length,
            "1 install queued from your phone is now running on this PC.",
            "{count} installs queued from your phone are now running on this PC.",
          ),
          tone: "success",
        });
      });
    };
    const remoteDelay = window.setTimeout(syncRemote, 8_000);
    const remoteTimer = window.setInterval(syncRemote, 30_000);

    return () => {
      cancelled = true;
      window.clearTimeout(remoteDelay);
      window.clearInterval(remoteTimer);
      for (const off of unlisteners) off();
    };
  }, [queryClient, push, navigate]);
}
