import { formatRelative } from "@/lib/format";
import { t } from "@/lib/i18n";
import type { AppNotification } from "@/lib/types";
import { Glyph } from "@/components/big-picture/glyphs";
import { OverlayRow, OverlayShell } from "@/components/big-picture/overlays";

const MAX_VISIBLE = 3;

export function notificationCopy(item: AppNotification): { title: string; detail: string } {
  const actor = item.actor?.username ?? t("Someone");
  switch (item.kind) {
    case "friend_request":
      return {
        title: t("{name} sent you a friend request", { name: actor }),
        detail: t("Friend request"),
      };
    case "friend_accepted":
      return {
        title: t("{name} accepted your friend request", { name: actor }),
        detail: t("Friends"),
      };
    case "friend_achievement":
      return {
        title: item.achievement?.name
          ? t("{name} unlocked {achievement}", { name: actor, achievement: item.achievement.name })
          : t("{name} unlocked an achievement", { name: actor }),
        detail: item.game?.title ?? t("Achievement"),
      };
    case "collection_share":
      return {
        title: t("{name} shared a collection with you", { name: actor }),
        detail: t("Open collections"),
      };
  }
}

/**
 * Console-native Notifications sheet.
 *
 * Accessible from the SystemBar avatar or Quick Actions. Shows at most 3
 * recent updates at ten feet; `A` marks read and jumps to the related Console
 * surface (never a desktop route), `B` closes and restores exact focus.
 */
export function ConsoleNotificationsSheet({
  active,
  onChoose,
  onHighlight,
  online,
  notifications,
  unreadCount,
  loading,
  error,
  onRetry,
  ...shell
}: {
  active: number;
  onChoose: (index: number) => void;
  onHighlight: (index: number) => void;
  online: boolean;
  notifications: readonly AppNotification[];
  unreadCount: number;
  loading: boolean;
  error: string | null;
  onRetry: () => void;
} & Omit<Parameters<typeof OverlayShell>[0], "children" | "title" | "subtitle">) {
  const visible = notifications.slice(0, MAX_VISIBLE);
  const subtitle = !online
    ? t("Offline · Connect to check notifications.")
    : notifications.length > MAX_VISIBLE
      ? t("Showing {shown} of {total} recent updates.", {
          shown: MAX_VISIBLE,
          total: notifications.length,
        })
      : t("Recent activity from your library and friends.");

  return (
    <OverlayShell title={t("Notifications")} subtitle={subtitle} {...shell}>
      {!online && visible.length === 0 ? (
        <p className="px-3 py-6 text-center text-bp-body text-ink-3">
          {t("Notifications need a connection to the server.")}
        </p>
      ) : loading ? (
        <div className="flex flex-col gap-1" aria-busy="true" aria-label={t("Loading…")}>
          {[0, 1, 2].map((index) => (
            <span key={index} className="h-[52px] w-full animate-pulse bg-surface-2" aria-hidden="true" />
          ))}
        </div>
      ) : error ? (
        <div className="flex flex-col gap-2 px-1 py-2">
          <p className="text-bp-meta leading-relaxed text-ink-3">{error}</p>
          <OverlayRow
            active={active === 0}
            onChoose={onRetry}
            onHighlight={() => onHighlight(0)}
            icon={<Glyph name="back" className="size-[calc(var(--bp-u)*26)]" />}
            label={t("Try again")}
            detail={t("Unavailable")}
          />
        </div>
      ) : visible.length === 0 ? (
        <p className="px-3 py-6 text-center text-bp-body text-ink-3">{t("No notifications yet.")}</p>
      ) : (
        <>
          <ul className="flex flex-col gap-0.5">
            {visible.map((item, index) => {
              const copy = notificationCopy(item);
              const unread = item.readAt === null;
              return (
                <li key={item.id}>
                  <OverlayRow
                    active={index === active}
                    onChoose={() => onChoose(index)}
                    onHighlight={() => onHighlight(index)}
                    icon={
                      unread ? (
                        <span className="bp-dot" aria-hidden="true" />
                      ) : (
                        <Glyph name="bell" className="size-[calc(var(--bp-u)*24)]" />
                      )
                    }
                    label={copy.title}
                    detail={copy.detail}
                    trailing={<span className="num">{formatRelative(item.createdAt)}</span>}
                  />
                </li>
              );
            })}
          </ul>
          {unreadCount > 0 && (
            <div className="mt-2 border-t border-line pt-2">
              <OverlayRow
                active={active === visible.length}
                onChoose={() => onChoose(visible.length)}
                onHighlight={() => onHighlight(visible.length)}
                icon={<Glyph name="check" className="size-[calc(var(--bp-u)*24)]" />}
                label={t("Mark all as read")}
                detail={t("{count} unread", { count: unreadCount })}
              />
            </div>
          )}
        </>
      )}
    </OverlayShell>
  );
}
