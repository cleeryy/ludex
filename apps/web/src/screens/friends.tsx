import { useEffect, useState } from "react";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { ArrowUpRight, Check, ShieldOff, UserPlus, Users, X } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import type { Friend, FriendPresence } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { ActivityList } from "@/components/community/activity-list";
import { PresenceBadge, isOnline } from "@/components/community/friend-presence";
import { EmptyState, ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Field, Input, Skeleton } from "@/components/ui/controls";
import { t, tn } from "@/lib/i18n";

function displayName(user: Friend["user"]): string {
  return user.displayName ?? user.username;
}

function initials(user: Friend["user"]): string {
  return displayName(user).slice(0, 2).toUpperCase();
}

export function FriendsScreen() {
  const { user } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [username, setUsername] = useState("");
  const [feedback, setFeedback] = useState<string | null>(null);

  const friendsQuery = useQuery({
    queryKey: ["friends"],
    queryFn: () => api.friends(),
    refetchInterval: 60_000,
  });
  const presenceQuery = useQuery({
    queryKey: ["friends-presence"],
    queryFn: () => api.friendsPresence(),
    refetchInterval: 30_000,
  });
  const activity = useInfiniteQuery({
    queryKey: ["friends-activity"],
    queryFn: ({ pageParam }) => api.friendsActivity(20, pageParam),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (last) => last.nextBefore ?? undefined,
  });
  const profile = useQuery({ queryKey: ["my-profile"], queryFn: () => api.myProfile() });

  const invalidate = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["friends"] }),
      queryClient.invalidateQueries({ queryKey: ["friends-presence"] }),
      queryClient.invalidateQueries({ queryKey: ["friends-activity"] }),
      queryClient.invalidateQueries({ queryKey: ["notifications"] }),
      queryClient.invalidateQueries({ queryKey: ["community-feed"] }),
    ]);
  };

  const send = useMutation({ mutationFn: () => api.sendFriendRequest({ username: username.trim() }), onSuccess: invalidate });
  const accept = useMutation({ mutationFn: (id: number) => api.acceptFriendRequest(id), onSuccess: invalidate });
  const decline = useMutation({ mutationFn: (id: number) => api.declineFriendRequest(id), onSuccess: invalidate });
  const cancel = useMutation({ mutationFn: (id: number) => api.cancelFriendRequest(id), onSuccess: invalidate });
  const remove = useMutation({ mutationFn: (id: number) => api.removeFriend(id), onSuccess: invalidate });
  const unblock = useMutation({ mutationFn: (id: number) => api.unblockUser(id), onSuccess: invalidate });

  // A lightweight heartbeat keeps the account online for friends while this
  // page is open. The server ignores it unless presence is enabled.
  useEffect(() => {
    if (!profile.data?.presenceEnabled) return;
    const beat = () => {
      if (document.visibilityState !== "visible") return;
      void api.sendPresence({ state: "online" }).catch(() => undefined);
    };
    beat();
    const timer = window.setInterval(beat, 60_000);
    return () => window.clearInterval(timer);
  }, [profile.data?.presenceEnabled]);

  const data = friendsQuery.data;
  const presence = new Map((presenceQuery.data?.friends ?? []).map((entry) => [entry.userId, entry]));
  const presenceFor = (friend: Friend): FriendPresence | null => {
    const entry = presence.get(friend.user.id);
    return entry ? { state: entry.state, game: entry.game, lastSeen: entry.lastSeen } : friend.presence;
  };
  const onlineCount = (data?.friends ?? []).filter((friend) => isOnline(presenceFor(friend))).length;
  const items = activity.data?.pages.flatMap((page) => page.items) ?? [];

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setFeedback(null);
    send.mutate(undefined, {
      onSuccess: () => {
        setUsername("");
        push({ title: t("Friend request sent"), tone: "success" });
      },
      onError: (error) => setFeedback(errorMessage(error)),
    });
  };

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Friends")}
        meta={
          <span>
            {tn(data?.friends.length ?? 0, "1 friend", "{count} friends")} ·{" "}
            <span className="text-signal-text">{t("{count} online", { count: onlineCount })}</span>
          </span>
        }
        description={t(
          "Requests go to an exact username; there is no member search. Presence and activity are visible to friends only, and only when you turn them on.",
        )}
        actions={
          <Link
            to="/community"
            className="inline-flex h-9 items-center gap-2 border border-line px-3 text-[12.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
          >
            {t("Community")}
            <ArrowUpRight className="size-3.5" aria-hidden="true" />
          </Link>
        }
      />

      <section className="mt-8 border border-line bg-surface/40 p-4" aria-labelledby="add-friend-heading">
        <h2 id="add-friend-heading" className="text-[13px] font-medium text-ink">
          {t("Add a friend")}
        </h2>
        <p className="mt-1 max-w-[60ch] text-[11.5px] leading-relaxed text-ink-3">
          {t("Type their exact login name. The account is never revealed by a partial search, and no email is ever shared.")}
        </p>
        <form onSubmit={submit} className="mt-3 flex flex-wrap items-end gap-3">
          <Field label={t("Username")} htmlFor="friend-username" className="max-w-[280px] flex-1">
            <Input
              id="friend-username"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              autoComplete="off"
              spellCheck={false}
              placeholder={user?.username === "mara" ? "jonas" : "mara"}
            />
          </Field>
          <Button type="submit" variant="default" loading={send.isPending} disabled={!username.trim()}>
            <UserPlus className="size-4" aria-hidden="true" />
            {t("Send request")}
          </Button>
        </form>
        {feedback && (
          <p role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12px] text-danger">
            {feedback}
          </p>
        )}
      </section>

      {data && data.incoming.length > 0 && (
        <section className="mt-12" aria-labelledby="requests-heading">
          <SectionHeader title={t("Requests")} count={data.incoming.length} description={t("Waiting for you")} />
          <ul className="border-t border-line">
            {data.incoming.map((request) => (
              <li key={request.id} className="flex flex-wrap items-center gap-3 border-b border-line py-3">
                <Link to={`/community/u/${request.user.id}`} className="flex min-w-0 flex-1 items-center gap-3 group">
                  <span className="grid size-9 shrink-0 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-[11px] text-ink">
                    {initials(request.user)}
                  </span>
                  <span className="min-w-0">
                    <span className="block truncate text-[13px] text-ink group-hover:underline">{displayName(request.user)}</span>
                    <span className="block truncate font-mono text-[10.5px] text-ink-3">@{request.user.username}</span>
                  </span>
                </Link>
                <div className="flex items-center gap-2">
                  <Button
                    variant="default"
                    size="sm"
                    loading={accept.isPending && accept.variables === request.id}
                    onClick={() => accept.mutate(request.id)}
                  >
                    <Check className="size-3.5" aria-hidden="true" />
                    {t("Accept")}
                  </Button>
                  <Button variant="ghost" size="sm" onClick={() => decline.mutate(request.id)}>
                    <X className="size-3.5" aria-hidden="true" />
                    {t("Decline")}
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        </section>
      )}

      <section className="mt-12" aria-labelledby="friends-heading">
        <SectionHeader
          title={t("Your friends")}
          count={data?.friends.length ?? 0}
          description={t("Presence is visible to friends only")}
          actions={
            <Link to="/settings" className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline">
              {t("Presence settings")}
            </Link>
          }
        />
        {friendsQuery.isPending && <Skeleton className="h-16 w-full" />}
        {friendsQuery.isError && (
          <ErrorState
            title={t("Friends didn’t load")}
            message={errorMessage(friendsQuery.error)}
            onRetry={() => void friendsQuery.refetch()}
          />
        )}
        {!friendsQuery.isPending && !friendsQuery.isError && (data?.friends.length ?? 0) === 0 && (
          <EmptyState
            compact
            title={t("No friends yet")}
            description={t("Send a request with an exact username above. Accepted friends can see your presence and activity if you opt in.")}
          >
            <Link
              to="/community"
              className="inline-flex h-11 items-center gap-2 rounded-[2px] border border-line-2 bg-surface px-4 text-sm text-ink transition-colors hover:border-ink-3 hover:bg-surface-2"
            >
              <Users className="size-4" aria-hidden="true" />
              {t("Browse the community")}
            </Link>
          </EmptyState>
        )}
        {(data?.friends.length ?? 0) > 0 && (
          <ul id="friends-heading" className="border-t border-line">
            {data?.friends.map((friend) => {
              const current = presenceFor(friend);
              return (
                <li key={friend.user.id} className="flex flex-wrap items-center gap-3 border-b border-line py-3">
                  <Link to={`/community/u/${friend.user.id}`} className="flex min-w-0 flex-1 items-center gap-3 group">
                    <span className="relative grid size-9 shrink-0 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-[11px] text-ink">
                      {initials(friend.user)}
                      {isOnline(current) && (
                        <span className="absolute -right-0.5 -top-0.5 size-2 rounded-full border border-bg bg-signal" aria-hidden="true" />
                      )}
                    </span>
                    <span className="min-w-0">
                      <span className="block truncate text-[13px] text-ink group-hover:underline">{displayName(friend.user)}</span>
                      <span className="block truncate font-mono text-[10.5px] text-ink-3">@{friend.user.username}</span>
                    </span>
                  </Link>
                  <PresenceBadge presence={current} />
                  <Button variant="ghost" size="sm" onClick={() => remove.mutate(friend.user.id)}>
                    {t("Remove")}
                  </Button>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {data && data.outgoing.length > 0 && (
        <section className="mt-12" aria-labelledby="outgoing-heading">
          <SectionHeader title={t("Sent requests")} count={data.outgoing.length} description={t("Waiting for them")} />
          <ul className="border-t border-line">
            {data.outgoing.map((request) => (
              <li key={request.id} className="flex items-center gap-3 border-b border-line py-3">
                <span className="grid size-9 shrink-0 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-[11px] text-ink-3">
                  {initials(request.user)}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] text-ink">{displayName(request.user)}</span>
                  <span className="block truncate font-mono text-[10.5px] text-ink-3">@{request.user.username}</span>
                </span>
                <Button variant="ghost" size="sm" onClick={() => cancel.mutate(request.id)}>
                  {t("Cancel")}
                </Button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <section className="mt-12" aria-labelledby="friends-activity-heading">
        <SectionHeader title={t("Friend activity")} description={t("Only friends who opted into sharing")} />
        {activity.isPending && <Skeleton className="h-20 w-full" />}
        {activity.isError && (
          <ErrorState
            title={t("Friend activity didn’t load")}
            message={errorMessage(activity.error)}
            onRetry={() => void activity.refetch()}
          />
        )}
        {!activity.isPending && !activity.isError && (
          <ActivityList
            items={items}
            emptyTitle={t("Quiet on this side")}
            emptyDescription={t("Nothing shared yet. Friends appear here when they opt into sharing activity and play, finish or unlock something.")}
            hasMore={Boolean(activity.hasNextPage)}
            loadingMore={activity.isFetchingNextPage}
            onLoadMore={() => void activity.fetchNextPage()}
          />
        )}
      </section>

      {data && data.blocked.length > 0 && (
        <section className="mt-12" aria-labelledby="blocked-heading">
          <SectionHeader title={t("Blocked accounts")} count={data.blocked.length} description={t("No presence, activity or requests")} />
          <ul className="border-t border-line">
            {data.blocked.map((blocked) => (
              <li key={blocked.id} className="flex items-center gap-3 border-b border-line py-3">
                <ShieldOff className="size-3.5 shrink-0 text-ink-3" aria-hidden="true" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] text-ink">{displayName(blocked)}</span>
                  <span className="block truncate font-mono text-[10.5px] text-ink-3">@{blocked.username}</span>
                </span>
                <Button variant="ghost" size="sm" loading={unblock.isPending && unblock.variables === blocked.id} onClick={() => unblock.mutate(blocked.id)}>
                  {t("Unblock")}
                </Button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <p className="mt-10 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
        {t(
          "Presence goes stale after two minutes of silence. Blocking an account removes the friendship and its pending requests, and hides its notifications.",
        )}
        {profile.data?.presenceEnabled === false
          ? ` ${t("Your presence is off. Turn it on in Settings to let friends see when you are online.")}`
          : ""}
      </p>

      {profile.data && (
        <p className="mt-2 flex items-center gap-2 text-[11.5px] text-ink-3">
          <Badge tone={profile.data.friendsActivity ? "signal" : "outline"}>
            {profile.data.friendsActivity ? t("Activity shared with friends") : t("Activity private")}
          </Badge>
          <Link to="/settings" className="underline-offset-4 hover:text-ink hover:underline">
            {t("Change")}
          </Link>
        </p>
      )}
    </div>
  );
}
