import { useState } from "react";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, ArrowUpRight, Lock, UserPlus } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate, formatMinutes } from "@/lib/format";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { useBookmark, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { isNative } from "@/lib/native";
import { GameCard } from "@/components/game/game-card";
import { CollectionCard } from "@/components/collections/collection-card";
import { AchievementShowcase } from "@/components/community/achievement-showcase";
import { PresenceBadge } from "@/components/community/friend-presence";
import { ActivityList } from "@/components/community/activity-list";
import { UserAvatar } from "@/components/community/user-avatar";
import { EmptyState, ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

export function ProfileScreen() {
  const { id } = useParams();
  const userId = Number(id);
  const navigate = useNavigate();
  const { user } = useAuth();
  const { push } = useToast();
  const [removeOpen, setRemoveOpen] = useState(false);
  const installed = useInstalledMap();
  const bookmark = useBookmark();
  const launch = useLaunchGame();
  const own = user?.id === userId;
  const queryClient = useQueryClient();

  const profile = useQuery({
    queryKey: qk.community.user(userId),
    queryFn: () => api.communityUser(userId),
    enabled: Number.isFinite(userId),
  });
  const activity = useInfiniteQuery({
    queryKey: qk.community.userActivity(userId),
    queryFn: ({ pageParam }) => api.communityUserActivity(userId, 20, pageParam),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (last) => last.nextBefore ?? undefined,
    enabled: Number.isFinite(userId) && !profile.isError,
  });
  const friends = useQuery({ queryKey: qk.friends.root, queryFn: () => api.friends(), enabled: !own && Number.isFinite(userId) });
  const invalidateFriends = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: qk.friends.root }),
      queryClient.invalidateQueries({ queryKey: qk.friends.presence }),
      queryClient.invalidateQueries({ queryKey: qk.notifications.root }),
    ]);
  };
  const addFriend = useMutation({ mutationFn: () => api.sendFriendRequest({ userId }), onSuccess: invalidateFriends });
  const unfriend = useMutation({ mutationFn: () => api.removeFriend(userId), onSuccess: invalidateFriends });
  const friend = friends.data?.friends.find((entry) => entry.user.id === userId);
  const outgoing = friends.data?.outgoing.some((entry) => entry.user.id === userId) ?? false;
  const incoming = friends.data?.incoming.find((entry) => entry.user.id === userId);

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
            <Button variant="outline" onClick={() => navigate("/community")}>
              {t("Back to community")}
            </Button>
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
        className="inline-flex h-8 items-center gap-2 text-label text-ink-2 transition-colors hover:text-ink"
      >
        <ArrowLeft className="rtl:-scale-x-100 size-3.5" aria-hidden="true" />
        {t("Community")}
      </Link>

      <PageHeader
        className="mt-5"
        title={
          <span className="flex flex-wrap items-center gap-4">
            <UserAvatar
              name={display}
              src={data.user.publicProfile ? data.user.avatarUrl : null}
              size={48}
            />
            {display}
          </span>
        }
        meta={
          <>
            <span className="font-mono">@{data.user.username}</span>
            {data.user.publicProfile ? <Badge tone="signal">{t("Public")}</Badge> : <Badge tone="outline">{t("Private")}</Badge>}
            <span>{t("Member since")} {formatDate(data.user.createdAt)}</span>
            {own && (
              <Link to="/settings?section=account" className="inline-flex items-center gap-1 text-ink-3 underline-offset-4 hover:text-ink hover:underline">
                {t("Edit profile")}
                <ArrowUpRight className="size-3" aria-hidden="true" />
              </Link>
            )}
          </>
        }
        description={data.user.bio ?? undefined}
      />

      {!data.user.publicProfile && own && (
        <p className="mt-5 flex items-start gap-2 border-l-2 border-line-2 pl-3 text-label leading-relaxed text-ink-3">
          <Lock className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          {t("Only you can see this page. Turn on the public profile in Settings to share your library and activity.")}
        </p>
      )}

      {!own && friends.isSuccess && (
        <div className="mt-5 flex flex-wrap items-center gap-3" aria-label={t("Friendship")}>
          {friend ? (
            <>
              <Badge tone="signal">{t("Friend")}</Badge>
              <PresenceBadge presence={friend.presence} />
              <Button variant="ghost" size="sm" loading={unfriend.isPending} onClick={() => setRemoveOpen(true)}>
                {t("Remove friend")}
              </Button>
            </>
          ) : outgoing ? (
            <Badge tone="outline">{t("Request sent")}</Badge>
          ) : incoming ? (
            <>
              <Badge tone="signal">{t("Wants to be friends")}</Badge>
              <Link to="/community/friends" className="text-label text-ink-3 underline-offset-4 hover:text-ink hover:underline">
                {t("Answer the request")}
              </Link>
            </>
          ) : (
            <Button
              variant="default"
              size="sm"
              loading={addFriend.isPending}
              onClick={() =>
                addFriend.mutate(undefined, {
                  onSuccess: () => push({ title: t("Friend request sent"), tone: "success" }),
                  onError: (cause) => push({ title: t("Could not send request"), description: errorMessage(cause), tone: "danger" }),
                })
              }
            >
              <UserPlus className="size-3.5" aria-hidden="true" />
              {t("Add friend")}
            </Button>
          )}
        </div>
      )}

      <section className="mt-8 grid grid-cols-2 gap-3 sm:grid-cols-4" aria-label={t("Profile statistics")}>
        {[
          { label: t("Games in library"), value: String(data.stats.games) },
          { label: t("Completed"), value: String(data.stats.completed) },
          { label: t("Playtime"), value: formatMinutes(data.stats.minutesPlayed) ?? "0m" },
          { label: t("Collections"), value: String(data.stats.collections) },
        ].map((stat) => (
          <div key={stat.label} className="rounded-[var(--r-md)] border border-line bg-surface p-4">
            <p className="num text-title leading-none text-ink">{stat.value}</p>
            <p className="mt-2 text-meta text-ink-3">{stat.label}</p>
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
            description={
              own
                ? t("Start a game or bookmark one and it will appear here.")
                : t("{name} has not recorded any progress yet.", { name: display })
            }
          />
        ) : (
          <div id="profile-library" className="grid grid-cols-2 gap-x-6 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
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
            description={
              own
                ? t("Publish a collection from its page to show it here.")
                : t("{name} has no public shelves.", { name: display })
            }
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
            emptyDescription={
              own
                ? t("Play a game or create a collection to start the feed.")
                : t("{name} has not published any activity yet.", { name: display })
            }
            hasMore={Boolean(activity.hasNextPage)}
            loadingMore={activity.isFetchingNextPage}
            onLoadMore={() => void activity.fetchNextPage()}
          />
        )}
      </section>

      <ConfirmDialog
        open={removeOpen}
        onOpenChange={setRemoveOpen}
        title={t("Remove {name} from friends?", { name: display })}
        description={t("They will no longer see your presence or friend-only activity. You can send a new request later.")}
        confirmLabel={t("Remove friend")}
        loading={unfriend.isPending}
        onConfirm={() =>
          unfriend.mutate(undefined, {
            onSuccess: () => {
              setRemoveOpen(false);
              push({ title: t("Removed {name} from friends", { name: display }), tone: "success" });
            },
            onError: (cause) => {
              push({ title: t("Could not remove friend"), description: errorMessage(cause), tone: "danger" });
            },
          })
        }
      />
    </div>
  );
}
