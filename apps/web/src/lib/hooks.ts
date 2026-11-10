import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { isNative, native } from "@/lib/native";
import { api } from "@/lib/api";
import { USING_MOCKS } from "@/lib/flags";
import { mockInstalls } from "@/lib/mock-install";
import type { GameDetail, GameList, InstalledGame } from "@/lib/types";
import { useToast } from "@/state/toasts";

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
      push({ title: "Bookmark not saved", description: error.message, tone: "danger" });
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
      push({ title: `${game.title} is starting`, description: "Playtime is recorded when the game exits." });
      void queryClient.invalidateQueries({ queryKey: ["games"] });
    },
    onError: (error: Error) => {
      push({ title: "Could not launch the game", description: error.message, tone: "danger" });
    },
  });
}

/** Global Tauri event wiring: playtime sync and installed-state refresh. */
export function useNativeEvents() {
  const queryClient = useQueryClient();
  const { push } = useToast();

  useEffect(() => {
    if (!isNative()) return;
    const unlisteners: Array<() => void> = [];
    let cancelled = false;

    void (async () => {
      const offExited = await native.onGameExited(({ gameId, minutesPlayed }) => {
        void queryClient.invalidateQueries({ queryKey: ["installed"] });
        void queryClient.invalidateQueries({ queryKey: ["game", gameId] });
        void queryClient.invalidateQueries({ queryKey: ["games"] });
        if (minutesPlayed > 0) {
          push({
            title: "Session saved",
            description: `${minutesPlayed} minutes added to your playtime.`,
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
    })();

    return () => {
      cancelled = true;
      for (const off of unlisteners) off();
    };
  }, [queryClient, push]);
}
