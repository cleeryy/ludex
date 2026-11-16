import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, tokenStore } from "@/lib/api";
import { presetProfile } from "@/lib/controller-profiles";
import type { ControllerProfile } from "@/lib/types";
import { qk } from "@/lib/query-keys";

/**
 * Loads the effective controller profile for the current account and, when a
 * game is selected, its per-game override (#51). The profile is cached with
 * the rest of the library data, so navigating between screens does not
 * re-fetch it on every gamepad frame.
 */
export function useControllerProfile(gameId?: number | null): ControllerProfile {
  const profiles = useQuery({
    queryKey: qk.controllerProfiles,
    queryFn: () => api.controllerProfiles(),
    // #267: the input provider mounts outside the auth gate (the floating
    // overlay tree has no AuthProvider), so gate on the stored token instead of
    // fetching while signed out — a 401 on /setup logged a browser error.
    enabled: Boolean(tokenStore.get()),
    staleTime: 5 * 60_000,
  });
  const global = profiles.data?.global?.profile;
  const game =
    gameId && profiles.data ? profiles.data.games.find((entry) => entry.gameId === gameId)?.profile : undefined;
  // #267: the fallback preset is built once. A new profile object per render
  // made every `useInputScope` re-read its options and re-render the provider,
  // which React reports as a maximum update depth.
  const fallback = useMemo(() => presetProfile("xbox"), []);
  return useMemo(() => game ?? global ?? fallback, [global, game, fallback]);
}
