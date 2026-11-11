import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { resolveControllerProfile } from "@/lib/controller-profiles";
import type { ControllerProfile } from "@/lib/types";

/**
 * Loads the effective controller profile for the current account and, when a
 * game is selected, its per-game override (#51). The profile is cached with
 * the rest of the library data, so navigating between screens does not
 * re-fetch it on every gamepad frame.
 */
export function useControllerProfile(gameId?: number | null): ControllerProfile {
  const profiles = useQuery({
    queryKey: ["controller-profiles"],
    queryFn: () => api.controllerProfiles(),
    staleTime: 5 * 60_000,
  });
  const global = profiles.data?.global?.profile;
  const game =
    gameId && profiles.data ? profiles.data.games.find((entry) => entry.gameId === gameId)?.profile : undefined;
  return resolveControllerProfile(global, game);
}
