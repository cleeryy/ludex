import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, ArrowUpRight, Lock } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate, formatMinutes } from "@/lib/format";
import { useAuth } from "@/state/auth";
import { useBookmark, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { isNative } from "@/lib/native";
import { GameCard } from "@/components/game/game-card";
import { CollectionCard } from "@/components/collections/collection-card";
import { AchievementShowcase } from "@/components/community/achievement-showcase";
import { ActivityList } from "@/components/community/activity-list";
import { EmptyState, ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Skeleton } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

export function ProfileScreen() {
  const { id } = useParams();
  const userId = Number(id);
  const navigate = useNavigate();
  const { user } = useAuth();
  const installed = useInstalledMap();
  const bookmark = useBookmark();
  const launch = useLaunchGame();
  const own = user?.id === userId;

  const profile = useQuery({
    queryKey: ["community-user", userId],
    queryFn: () => api.communityUser(userId),
    enabled: Number.isFinite(userId),
  });
  const activity = useInfiniteQuery({
    queryKey: ["community-user-activity", userId],
    queryFn: ({ pageParam }) => api.communityUserActivity(userId, 20, pageParam),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (last) => last.nextBefore ?? undefined,
    enabled: Number.isFinite(userId) && !profile.isError,
  });

  if (profile.isPending) {
    return (
      <div className="page page-in pt-8">
        <Skeleton className="h-3 w-20" />
        <Skeleton className="mt-6 h-10 w-1/2" />
        <Skeleton className="mt-3 h-3 w-2/3" />
        <Skeleton className="mt-8 h-20 w-full" />
      </div>
    );
  }

  if (profile.isError || !profile.data) {
    const notFound =
      profile.error instanceof Error && (profile.error.message.toLowerCase().includes("not found") || profile.error.message.includes("404"));
    return (
      <div className="page page-in pt-8">
        <ErrorState
          title={notFound ? t("Profile not found") : t("This profile didn’t load")}
          message={
            notFound
              ? t("This account either has no public profile or no longer exists. Private profiles are hidden from everyone else.")
              : errorMessage(profile.error)
          }
          onRetry={() => void profile.refetch()}
          action={
            <button
              type="button"
              onClick={() => navigate("/community")}
              className="h-11 border border-line px-4 text-sm text-ink-2 hover:text-ink"
            >
              {t("Back to community")}
            </button>
          }
        />
      </div>
    );
  }

  const data = profile.data;
  const display = data.user.displayName ?? data.user.username;
  const items = activity.data?.pages.flatMap((page) => page.items) ?? [];

  return (
    <div className="page page-in">
      <Link
        to="/community"
        className="inline-flex h-8 items-center gap-2 text-[12.5px] text-ink-2 transition-colors hover:text-ink"
      >
        <ArrowLeft className="rtl:-scale-x-100 size-3.5" aria-hidden="true" />
        {t("Community")}
      </Link>

      <PageHeader
        className="mt-5"
        title={
          <span className="flex flex-wrap items-center gap-4">
            <span className="grid size-12 shrink-0 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-[14px] text-ink">
              {display.slice(0, 2).toUpperCase()}
            </span>
            {display}
          </span>
        }
        meta={
          <>
            <span className="font-mono">@{data.user.username}</span>
            {data.user.publicProfile ? <Badge tone="signal">{t("Public")}</Badge> : <Badge tone="outline">{t("Private")}</Badge>}
            <span>{t("Member since")} {formatDate(data.user.createdAt)}</span>
            {own && (
              <Link to="/settings" className="inline-flex items-center gap-1 text-ink-3 underline-offset-4 hover:text-ink hover:underline">
                {t("Edit profile")}
                <ArrowUpRight className="size-3" aria-hidden="true" />
              </Link>
            )}
          </>
        }
        description={data.user.bio ?? undefined}
      />

      {!data.user.publicProfile && own && (
        <p className="mt-5 flex items-start gap-2 border-l-2 border-line-2 pl-3 text-[12px] leading-relaxed text-ink-3">
          <Lock className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          {t("Only you can see this page. Turn on the public profile in Settings to share your library and activity.")}
        </p>
      )}

      <section className="mt-8 grid grid-cols-2 gap-3 sm:grid-cols-4" aria-label={t("Profile statistics")}>
        {[
          { label: t("Games in library"), value: String(data.stats.games) },
          { label: t("Completed"), value: String(data.stats.completed) },
          { label: t("Playtime"), value: formatMinutes(data.stats.minutesPlayed) ?? "0m" },
          { label: t("Collections"), value: String(data.stats.collections) },
        ].map((stat) => (
          <div key={stat.label} className="border border-line bg-surface/40 p-3">
            <p className="num text-[22px] leading-none text-ink">{stat.value}</p>
            <p className="mt-2 text-[11.5px] text-ink-3">{stat.label}</p>
          </div>
        ))}
      </section>

      <AchievementShowcase userId={userId} />

      <section className="mt-12" aria-labelledby="profile-library">
        <SectionHeader
          title={t("Library")}
          count={data.library.length}
          description={t("Games with progress or a bookmark")}
        />
        {data.library.length === 0 ? (
          <EmptyState
            compact
            title={t("Nothing on the shelf yet")}
            description={own ? t("Start a game or bookmark one and it will appear here.") : `${display} has not recorded any progress yet.`}
          />
        ) : (
          <div id="profile-library" className="grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
            {data.library.slice(0, 10).map((game) => (
              <GameCard
                key={game.id}
                game={game}
                index={game.id}
                installed={installed.get(game.id)}
                onBookmark={() => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked })}
                onPlay={
                  own && isNative() && installed.has(game.id)
                    ? () => launch.mutate({ id: game.id, title: game.title, executable: installed.get(game.id)?.executable })
                    : undefined
                }
              />
            ))}
          </div>
        )}
      </section>

      <section className="mt-12" aria-labelledby="profile-collections">
        <SectionHeader title={t("Collections")} count={data.collections.length} description={t("Public shelves only")} />
        {data.collections.length === 0 ? (
          <EmptyState
            compact
            title={t("No public collections")}
            description={own ? t("Publish a collection from its page to show it here.") : `${display} has no public shelves.`}
          />
        ) : (
          <div id="profile-collections" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {data.collections.map((collection) => (
              <CollectionCard key={collection.id} collection={collection} />
            ))}
          </div>
        )}
      </section>

      <section className="mt-12" aria-labelledby="profile-activity">
        <SectionHeader title={t("Activity")} description={t("Newest first")} />
        {activity.isPending ? (
          <Skeleton className="h-24 w-full" />
        ) : (
          <ActivityList
            items={items}
            emptyTitle={t("No public activity")}
            emptyDescription={own ? t("Play a game or create a collection to start the feed.") : `${display} has not published any activity yet.`}
            hasMore={Boolean(activity.hasNextPage)}
            loadingMore={activity.isFetchingNextPage}
            onLoadMore={() => void activity.fetchNextPage()}
          />
        )}
      </section>
    </div>
  );
}
