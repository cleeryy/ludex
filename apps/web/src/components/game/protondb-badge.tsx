import { useQuery } from "@tanstack/react-query";
import { CircleHelp } from "lucide-react";
import { isNative, native } from "@/lib/native";
import type { GameDetail, ProtonDbRating } from "@/lib/types";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

const STEAM_APP_URL_RE = /store\.steampowered\.com\/app\/(\d+)/i;

/** Extracts a Steam App ID from a game's metadata provider reference (#360). */
export function steamAppIdFromProvider(provider: GameDetail["provider"] | undefined): number | null {
  if (!provider) return null;
  if (provider.slug === "steam" && /^\d+$/.test(provider.id.trim())) {
    const parsed = Number(provider.id.trim());
    return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null;
  }
  const match = provider.url?.match(STEAM_APP_URL_RE);
  if (match) {
    const parsed = Number(match[1]);
    return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null;
  }
  return null;
}

/**
 * ProtonDB rating for a game (#91, #360).
 *
 * Resolves the Steam App ID from the game's metadata provider (or falls back
 * to the desktop's saved launch options / Steam import / title lookup), then
 * reads the cached 7-day ProtonDB summary.
 */
export function ProtonDbBadge({
  gameId,
  provider,
  title,
  kind,
}: {
  gameId: number;
  provider?: GameDetail["provider"];
  title?: string | null;
  kind?: string;
}) {
  const steamAppId = steamAppIdFromProvider(provider);
  const enabled = isNative() && kind !== "rom" && Number.isFinite(gameId) && gameId > 0;
  const query = useQuery({
    queryKey: [...qk.protondb(gameId), steamAppId ?? 0, title ?? ""],
    queryFn: () => native.protondbRating(gameId, steamAppId, false, title),
    enabled,
    staleTime: 6 * 60 * 60 * 1000,
    retry: false,
  });

  if (!enabled) return null;
  const rating = query.data;
  if (!rating || rating.status === "noAppId") return null;

  const tier = rating.tier;
  if (!tier) {
    const offline = rating.status === "unavailable";
    return (
      <span
        className="flex items-center gap-1.5 text-meta text-ink-3"
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
  native: "border-ok/50 text-ok",
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
        "inline-flex h-5 items-center gap-1.5 rounded-[var(--r-sm)] border px-1.5 text-meta leading-none transition-opacity hover:opacity-80",
        TIER_STYLES[tier] ?? TIER_STYLES.pending,
      )}
    >
      <span className="size-1.5 rounded-full bg-current" aria-hidden="true" />
      <span>{t("ProtonDB")} {label}</span>
      {cached && <span className="font-mono text-micro opacity-70">{t("cached")}</span>}
    </a>
  );
}
