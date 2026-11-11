import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { isNative, native } from "@/lib/native";
import { api } from "@/lib/api";
import { USING_MOCKS } from "@/lib/flags";
import { mockInstalls } from "@/lib/mock-install";
import type { GameDetail, GameList, GameUpdate, InstalledGame } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { t, tn } from "@/lib/i18n";

/** Installed games, merged into the server library when running in Tauri. */
export function useInstalledGames() {
  return useQuery({
    queryKey: ["installed"],
    queryFn: () => (USING_MOCKS ? mockInstalls.installed() : native.listInstalled()),
    enabled: isNative() || USING_MOCKS,
    staleTime: 10_000,
    refetchOnWindowFocus: true,
  });
}

/** The whole library, used by Home, Downloads and the install manager. */
export function useAllGames() {
  return useQuery({
    queryKey: ["games", "all"],
    queryFn: () => api.listGames({ limit: 200 }),
    staleTime: 60_000,
  });
}

/** Recently launched games on this device, newest first (Home + tray). */
export function useRecentGames(limit = 6) {
  return useQuery({
    queryKey: ["recent-games", limit],
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
    queryKey: ["game-updates"],
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
    queryKey: ["playtime", gameId],
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
      void queryClient.invalidateQueries({ queryKey: ["game-updates"] });
      void queryClient.invalidateQueries({ queryKey: ["games"] });
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
    queryKey: ["facets", "genres"],
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
      await queryClient.cancelQueries({ queryKey: ["games"] });
      queryClient.setQueriesData<GameList>({ queryKey: ["games"] }, (current) =>
        current
          ? { ...current, items: current.items.map((game) => (game.id === id ? { ...game, bookmarked } : game)) }
          : current,
      );
      queryClient.setQueryData<GameDetail>(["game", id], (current) => (current ? { ...current, bookmarked } : current));
      queryClient.setQueryData<GameList>(["palette-games"], (current) =>
        current
          ? { ...current, items: current.items.map((game) => (game.id === id ? { ...game, bookmarked } : game)) }
          : current,
      );
    },
    onError: (error: Error) => {
      push({ title: t("Bookmark not saved"), description: error.message, tone: "danger" });
      void queryClient.invalidateQueries({ queryKey: ["games"] });
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["games"] });
      void queryClient.invalidateQueries({ queryKey: ["game"] });
    },
  });
}

export function useLaunchGame() {
  const queryClient = useQueryClient();
  const { push } = useToast();

  return useMutation({
    mutationFn: (game: { id: number; title: string; executable?: string | null }) =>
      native.launchGame({ gameId: game.id, executable: game.executable ?? null }),
    onSuccess: (_result, game) => {
      push({ title: t("{title} is starting", { title: game.title }), description: t("Playtime is recorded when the game exits.") });
      void queryClient.invalidateQueries({ queryKey: ["games"] });
      void queryClient.invalidateQueries({ queryKey: ["recent-games"] });
    },
    onError: (error: Error) => {
      push({ title: t("Could not launch the game"), description: error.message, tone: "danger" });
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
      const offExited = await native.onGameExited(({ gameId, minutesPlayed }) => {
        void queryClient.invalidateQueries({ queryKey: ["installed"] });
        void queryClient.invalidateQueries({ queryKey: ["recent-games"] });
        void queryClient.invalidateQueries({ queryKey: ["playtime", gameId] });
        void queryClient.invalidateQueries({ queryKey: ["game", gameId] });
        void queryClient.invalidateQueries({ queryKey: ["games"] });
        if (minutesPlayed > 0) {
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

      // Tray menu (#15): navigate to a route or launch a recent game.
      const offNavigate = await native.onNavigate((path) => navigate(path));
      if (cancelled) offNavigate();
      else unlisteners.push(offNavigate);

      const offLaunch = await native.onLaunchGame((gameId) => {
        void native
          .launchGame({ gameId })
          .then(() => {
            void queryClient.invalidateQueries({ queryKey: ["recent-games"] });
            void queryClient.invalidateQueries({ queryKey: ["games"] });
          })
          .catch((cause: Error) => push({ title: t("Could not launch the game"), description: cause.message, tone: "danger" }));
      });
      if (cancelled) offLaunch();
      else unlisteners.push(offLaunch);

      // App updates (#16, #163): the update center's notifier owns the launch
      // prompt, the badge and the skip decision, so no duplicate toast here.
    })();

    return () => {
      cancelled = true;
      for (const off of unlisteners) off();
    };
  }, [queryClient, push, navigate]);
}
