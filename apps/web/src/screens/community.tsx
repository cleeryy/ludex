import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { ArrowUpRight, Radio, Settings2, Users } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import type { PublicProfile } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { ActivityList } from "@/components/community/activity-list";
import { UserAvatar } from "@/components/community/user-avatar";
import { EmptyState, ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Skeleton } from "@/components/ui/controls";
import { t , tn } from "@/lib/i18n";

function MemberChip({ profile }: { profile: PublicProfile }) {
  return (
    <Link
      to={`/community/u/${profile.id}`}
      className="group flex items-center gap-3 border border-line bg-surface/40 p-3 transition-colors hover:border-line-2 hover:bg-surface"
    >
      <UserAvatar name={profile.displayName ?? profile.username} src={profile.avatarUrl} size={36} />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] text-ink">{profile.displayName ?? profile.username}</span>
        <span className="mt-0.5 block truncate font-mono text-[10px] text-ink-3">@{profile.username}</span>
      </span>
      <ArrowUpRight className="size-3.5 shrink-0 text-ink-3 transition-colors group-hover:text-ink" aria-hidden="true" />
    </Link>
  );
}

export function CommunityScreen() {
  const { user } = useAuth();
  const profile = useQuery({ queryKey: ["my-profile"], queryFn: () => api.myProfile() });
  const members = useQuery({ queryKey: ["community-users"], queryFn: () => api.communityUsers() });
  const feed = useInfiniteQuery({
    queryKey: ["community-feed"],
    queryFn: ({ pageParam }) => api.communityFeed(20, pageParam),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (last) => last.nextBefore ?? undefined,
  });

  const items = feed.data?.pages.flatMap((page) => page.items) ?? [];
  const publicMembers = (members.data ?? []).filter((member) => member.id !== user?.id);
  const isPublic = profile.data?.publicProfile ?? false;

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Community")}
        meta={
          <span>
            {tn(members.data?.length ?? 0, "1 public profile", "{count} public profiles")} ·{" "}
            {items.length} {items.length === 1 ? "event" : "events"}
          </span>
        }
        description={t("What people are playing, finishing and collecting. Only accounts that opt in appear here.")}
        actions={
          <>
            <Link
              to="/community/friends"
              className="inline-flex h-9 items-center gap-2 border border-line px-3 text-[12.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
            >
              <Users className="size-3.5" aria-hidden="true" />
              {t("Friends")}
            </Link>
            <Link
              to={user ? `/community/u/${user.id}` : "/settings"}
              className="inline-flex h-9 items-center gap-2 border border-line px-3 text-[12.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
            >
              {isPublic ? t("Your public page") : t("Your profile")}
              <ArrowUpRight className="size-3.5" aria-hidden="true" />
            </Link>
          </>
        }
      />

      {profile.isPending ? (
        <Skeleton className="mt-8 h-[90px] w-full" />
      ) : (
        <section className="mt-8 flex flex-wrap items-center justify-between gap-4 border border-line bg-surface/40 p-4">
          <div className="flex items-start gap-3">
            <span className="grid size-10 shrink-0 place-items-center border border-line-2 bg-surface-2 text-ink-3">
              {isPublic ? <Radio className="size-4" aria-hidden="true" /> : <Settings2 className="size-4" aria-hidden="true" />}
            </span>
            <div>
              <p className="text-[13.5px] font-medium text-ink">
                {isPublic ? t("Your profile is public") : t("Your profile is private")}
              </p>
              <p className="mt-1 max-w-[62ch] text-[12px] leading-relaxed text-ink-3">
                {isPublic
                  ? t("Your activity appears in the feed, and other accounts can open your page. You can turn this off at any time.")
                  : t("Nothing about your account appears in the community. Turn on a public profile in Settings to join the feed.")}
              </p>
            </div>
          </div>
          <Link
            to="/settings"
            className="inline-flex h-9 items-center gap-2 border border-line px-3 text-[12.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
          >
            {isPublic ? t("Privacy settings") : t("Make it public")}
            <ArrowUpRight className="size-3.5" aria-hidden="true" />
          </Link>
        </section>
      )}

      <section className="mt-12" aria-labelledby="members-heading">
        <SectionHeader
          title={t("People")}
          count={publicMembers.length}
          description={t("Accounts with a public profile")}
          actions={
            <Link to="/settings" className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline">
              {t("Manage yours")}
            </Link>
          }
        />
        {members.isPending && <Skeleton className="h-16 w-full" />}
        {!members.isPending && publicMembers.length === 0 && (
          <EmptyState
            compact
            title={t("Nobody has opted in yet")}
            description={t("Public profiles are off by default. Turn yours on in Settings and your activity will be the first entry here.")}
          >
            <Link
              to="/settings"
              className="inline-flex h-11 items-center gap-2 rounded-[2px] border border-line-2 bg-surface px-4 text-sm text-ink transition-colors hover:border-ink-3 hover:bg-surface-2"
            >
              <Users className="size-4" aria-hidden="true" />
              {t("Open settings")}
            </Link>
          </EmptyState>
        )}
        {publicMembers.length > 0 && (
          <div id="members-heading" className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
            {publicMembers.map((member) => (
              <MemberChip key={member.id} profile={member} />
            ))}
          </div>
        )}
      </section>

      <section className="mt-12" aria-labelledby="feed-heading">
        <SectionHeader title={t("Activity")} description={t("Newest first")} />
        {feed.isPending && (
          <div className="flex flex-col gap-3 border-t border-line pt-3">
            {Array.from({ length: 4 }).map((_, index) => (
              <Skeleton key={index} className="h-12 w-full" />
            ))}
          </div>
        )}
        {feed.isError && (
          <ErrorState title={t("The feed didn’t load")} message={errorMessage(feed.error)} onRetry={() => void feed.refetch()} />
        )}
        {!feed.isPending && !feed.isError && (
          <ActivityList
            items={items}
            emptyTitle={t("No public activity yet")}
            emptyDescription={t("Play a game, finish one, or create a collection with a public profile on. Entries appear here as they happen.")}
            hasMore={Boolean(feed.hasNextPage)}
            loadingMore={feed.isFetchingNextPage}
            onLoadMore={() => void feed.fetchNextPage()}
          />
        )}
      </section>

      <p className="mt-10 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
        {t("The feed records library additions, installs reported by the desktop app, play sessions, completions and new collections. Playtime totals stay on your account; the feed only shows the session length.")}
        {profile.data?.publicProfile === false
          ? ` ${t("Your own private activity is never shown to anyone else.")}`
          : ""}
      </p>
    </div>
  );
}
