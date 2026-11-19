import { useQuery } from "@tanstack/react-query";
import { t, tn } from "@/lib/i18n";
import { isNative, native } from "@/lib/native";
import { qk } from "@/lib/query-keys";
import type { InhibitStatus } from "@/lib/types";

/**
 * The suspend inhibitor's live state (#288), shared by the banner, the console
 * notice and the diagnostics row.
 *
 * The desktop pushes every transition on `inhibit://status` (wired in
 * `useNativeEvents`); the poll is only a safety net for a missed event, and it
 * stops while nothing is running.
 */
export function useInhibitStatus() {
  return useQuery({
    queryKey: qk.inhibitStatus,
    queryFn: () => native.inhibitStatus(),
    enabled: isNative(),
    staleTime: 10_000,
    refetchInterval: (query) => (query.state.data?.active ? 15_000 : false),
    retry: false,
  });
}

/**
 * The sentence the banner shows, in the user's language. `null` when Ludex is
 * not doing any work (the grace window alone does not deserve a banner).
 */
export function keepAwakeSentence(status: InhibitStatus | null | undefined): string | null {
  if (!status?.active) return null;
  if (status.jobs > 0) {
    return tn(
      status.jobs,
      "Keeping the machine awake while downloading (1 job)",
      "Keeping the machine awake while downloading ({count} jobs)",
    );
  }
  if (status.reasons.some((reason) => reason.kind === "game")) {
    return t("Keeping the machine awake while a game is running");
  }
  if (status.reasons.some((reason) => reason.kind === "stream")) {
    return t("Keeping the machine awake during a stream");
  }
  return t("Keeping the machine awake while checking the install");
}

/** The reasons, as "Hades II · Celeste", without a trailing separator. */
export function keepAwakeLabels(status: InhibitStatus | null | undefined): string {
  return (status?.reasons ?? []).map((reason) => reason.label).join(" · ");
}
