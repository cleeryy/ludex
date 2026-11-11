import { useQuery } from "@tanstack/react-query";
import { CircleHelp } from "lucide-react";
import { isNative, native } from "@/lib/native";
import type { ProtonDbRating } from "@/lib/types";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";

/**
 * ProtonDB rating for a game (#91).
 *
 * The rating comes from the desktop cache (seven days). When ProtonDB cannot
 * be reached the last known tier is shown as cached; with no cache the badge
 * degrades to a muted "offline" note and the page still loads.
 */
export function ProtonDbBadge({ gameId }: { gameId: number }) {
  const query = useQuery({
    queryKey: ["protondb", gameId],
    queryFn: () => native.protondbRating(gameId),
    enabled: isNative() && Number.isFinite(gameId) && gameId > 0,
    staleTime: 6 * 60 * 60 * 1000,
    retry: false,
  });

  if (!isNative() || !Number.isFinite(gameId) || gameId <= 0) return null;
  const rating = query.data;
  if (!rating || rating.status === "noAppId") return null;

  const tier = rating.tier;
  if (!tier) {
    const offline = rating.status === "unavailable";
    return (
      <span
        className="flex items-center gap-1.5 text-[11.5px] text-ink-3"
        title={rating.message ?? undefined}
      >
        <CircleHelp className="size-3.5" aria-hidden="true" />
        {offline ? t("ProtonDB offline") : t("No ProtonDB reports")}
      </span>
    );
  }

  return <TierBadge rating={rating} tier={tier} />;
}

const TIER_STYLES: Record<string, string> = {
  platinum: "border-ink-2/45 text-ink",
  gold: "border-signal/50 text-signal-text",
  silver: "border-line-2 text-ink-2",
  bronze: "border-warn/45 text-warn",
  borked: "border-danger/45 text-danger",
  pending: "border-line-2 text-ink-3",
};

function TierBadge({ rating, tier }: { rating: ProtonDbRating; tier: string }) {
  const label = tier.charAt(0).toUpperCase() + tier.slice(1);
  const cached = rating.status === "cached" || rating.status === "stale";
  const hint = [
    rating.total > 0 ? `${rating.total} reports` : null,
    rating.confidence ? `${rating.confidence} confidence` : null,
    cached ? "cached rating" : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <a
      href={rating.url ?? `https://www.protondb.com/app/${rating.appId ?? ""}`}
      target="_blank"
      rel="noreferrer noopener"
      title={hint || t("ProtonDB compatibility")}
      className={cn(
        "inline-flex h-5 items-center gap-1.5 rounded-[2px] border px-1.5 text-[11px] leading-none transition-opacity hover:opacity-80",
        TIER_STYLES[tier] ?? TIER_STYLES.pending,
      )}
    >
      <span className="size-1.5 rounded-full bg-current" aria-hidden="true" />
      <span>{t("ProtonDB")} {label}</span>
      {cached && <span className="font-mono text-[10px] opacity-70">{t("cached")}</span>}
    </a>
  );
}
