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
      // #191: the game page flips to Resume without waiting for the next poll.
      void queryClient.invalidateQueries({ queryKey: ["game-session", game.id] });
    },
    onError: (error: Error) => {
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
    queryKey: ["game-session", gameId],
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
      void queryClient.invalidateQueries({ queryKey: ["game-session", gameId] });
      void queryClient.invalidateQueries({ queryKey: ["playtime", gameId] });
      void queryClient.invalidateQueries({ queryKey: ["installed"] });
      void queryClient.invalidateQueries({ queryKey: ["recent-games"] });
      void queryClient.invalidateQueries({ queryKey: ["game", gameId] });
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
        void queryClient.invalidateQueries({ queryKey: ["installed"] });
        void queryClient.invalidateQueries({ queryKey: ["recent-games"] });
        void queryClient.invalidateQueries({ queryKey: ["playtime", gameId] });
        void queryClient.invalidateQueries({ queryKey: ["game-session", gameId] });
        void queryClient.invalidateQueries({ queryKey: ["game", gameId] });
        void queryClient.invalidateQueries({ queryKey: ["games"] });
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
            void queryClient.invalidateQueries({ queryKey: ["recent-games"] });
            void queryClient.invalidateQueries({ queryKey: ["games"] });
            // #191: a tray launch while the game page is open flips it to Resume.
            void queryClient.invalidateQueries({ queryKey: ["game-session"] });
          })
          .catch((cause: Error) => push({ title: t("Could not launch the game"), description: cause.message, tone: "danger" }));
      });
      if (cancelled) offLaunch();
      else unlisteners.push(offLaunch);

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
        void queryClient.invalidateQueries({ queryKey: ["remote-installs"] });
        void queryClient.invalidateQueries({ queryKey: ["install-jobs"] });
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
