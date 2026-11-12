import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "react-router-dom";
import { Bell, Check, ListPlus, Trophy, UserPlus } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { formatRelative } from "@/lib/format";
import type { AppNotification } from "@/lib/types";
import { t } from "@/lib/i18n";
import { IconButton } from "@/components/ui/controls";
import { Popover } from "@/components/ui/popover";
import { useDropdown } from "@/components/ui/select";

function notificationText(item: AppNotification): string {
  const name = item.actor?.displayName ?? item.actor?.username ?? t("Someone");
  switch (item.kind) {
    case "friend_request":
      return t("{name} sent you a friend request", { name });
    case "friend_accepted":
      return t("{name} accepted your friend request", { name });
    case "friend_achievement":
      return t("{name} unlocked {achievement}", {
        name,
        achievement: item.achievement?.name ?? (item.payload.achievementName as string | undefined) ?? t("an achievement"),
      });
    case "collection_share":
      return t("{name} published {collection}", {
        name,
        collection: (item.payload.collectionName as string | undefined) ?? t("a collection"),
      });
    default:
      return name;
  }
}

function notificationTarget(item: AppNotification): string {
  switch (item.kind) {
    case "friend_request":
    case "friend_accepted":
      return "/community/friends";
    case "friend_achievement":
      return item.game ? `/game/${item.game.id}` : "/community/friends";
    case "collection_share": {
      const collectionId = item.payload.collectionId;
      return typeof collectionId === "number" ? `/collections/${collectionId}` : "/community/friends";
    }
    default:
      return "/community/friends";
  }
}

function KindIcon({ kind }: { kind: AppNotification["kind"] }) {
  const className = "size-3.5 shrink-0";
  if (kind === "friend_request") return <UserPlus className={className} aria-hidden="true" />;
  if (kind === "friend_accepted") return <Check className={className} aria-hidden="true" />;
  if (kind === "friend_achievement") return <Trophy className={className} aria-hidden="true" />;
  return <ListPlus className={className} aria-hidden="true" />;
}

/** Notification bell for the top bar: unread badge plus a bounded panel. */
export function NotificationBell() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const menu = useDropdown();
  const query = useQuery({
    queryKey: ["notifications", "bell"],
    queryFn: () => api.notifications(10),
    refetchInterval: 60_000,
    staleTime: 30_000,
  });

  const invalidate = async () => {
    await queryClient.invalidateQueries({ queryKey: ["notifications"] });
  };
  const read = useMutation({ mutationFn: (id: number) => api.readNotification(id), onSuccess: invalidate });
  const readAll = useMutation({ mutationFn: () => api.readAllNotifications(), onSuccess: invalidate });

  const page = query.data;
  const unread = page?.unreadCount ?? 0;
  const items = page?.items ?? [];

  const open = (item: AppNotification) => {
    menu.setOpen(false);
    if (item.readAt === null) read.mutate(item.id);
    navigate(notificationTarget(item));
  };

  return (
    <>
      <IconButton
        {...menu.triggerProps}
        label={unread > 0 ? t("Notifications, {count} unread", { count: unread }) : t("Notifications")}
        variant="ghost"
      >
        <span className="relative inline-flex">
          <Bell className="size-4" aria-hidden="true" />
          {unread > 0 && (
            <span className="num absolute -right-1.5 -top-1.5 grid min-w-4 place-items-center rounded-full bg-signal px-1 text-[9.5px] leading-4 text-bg">
              {unread > 9 ? "9+" : unread}
            </span>
          )}
        </span>
      </IconButton>
      <Popover
        {...menu.menuProps}
        role="dialog"
        label={t("Notifications")}
        initialFocus="container"
        maxHeight={420}
        className="w-[min(340px,calc(100vw-24px))] p-0"
      >
        <div className="flex items-center justify-between gap-3 border-b border-line px-3 py-2.5">
          <p className="text-[12.5px] font-medium text-ink">{t("Notifications")}</p>
          <button
            type="button"
            onClick={() => readAll.mutate()}
            disabled={unread === 0 || readAll.isPending}
            className="text-[11.5px] text-ink-3 underline-offset-4 transition-colors hover:text-ink hover:underline disabled:opacity-40"
          >
            {t("Mark all read")}
          </button>
        </div>
        {query.isPending ? (
          <p className="px-3 py-4 text-[12px] text-ink-3">{t("Loading notifications…")}</p>
        ) : items.length === 0 ? (
          <p className="px-3 py-6 text-center text-[12px] leading-relaxed text-ink-3">
            {t("Nothing new. Friend requests, accepted invites and friend unlocks land here.")}
          </p>
        ) : (
          <ul className="max-h-[320px] overflow-y-auto">
            {items.map((item) => (
              <li key={item.id} className="border-b border-line last:border-b-0">
                <button
                  type="button"
                  onClick={() => open(item)}
                  className={cn(
                    "flex w-full items-start gap-2.5 px-3 py-2.5 text-left transition-colors hover:bg-surface",
                    item.readAt === null ? "text-ink" : "text-ink-2",
                  )}
                >
                  <span
                    className={cn(
                      "mt-0.5 grid size-6 shrink-0 place-items-center rounded-[2px] border",
                      item.kind === "friend_achievement"
                        ? "border-signal/50 bg-signal/12 text-signal-text"
                        : "border-line-2 bg-surface-2 text-ink-3",
                    )}
                  >
                    <KindIcon kind={item.kind} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-[12.5px] leading-snug">{notificationText(item)}</span>
                    <span className="mt-0.5 block font-mono text-[10px] text-ink-3">{formatRelative(item.createdAt)}</span>
                  </span>
                  {item.readAt === null && <span className="mt-2 size-1.5 shrink-0 rounded-full bg-signal" aria-hidden="true" />}
                </button>
              </li>
            ))}
          </ul>
        )}
        <div className="border-t border-line px-3 py-2">
          <Link
            to="/community/friends"
            onClick={() => menu.setOpen(false)}
            className="text-[11.5px] text-ink-3 underline-offset-4 transition-colors hover:text-ink hover:underline"
          >
            {t("Open friends and requests")}
          </Link>
        </div>
      </Popover>
    </>
  );
}
