import { Badge } from "@/components/ui/controls";
import { formatRelative } from "@/lib/format";
import type { FriendPresence } from "@/lib/types";
import { t } from "@/lib/i18n";

/** Presence badge for a friend. Never rendered for non-friends: the API only
 * returns presence to accepted friends of opted-in accounts. */
export function PresenceBadge({ presence }: { presence: FriendPresence | null | undefined }) {
  if (!presence) return null;
  if (presence.state === "in_game" && presence.game) {
    return <Badge tone="signal">{t("In game · {game}", { game: presence.game.title })}</Badge>;
  }
  if (presence.state === "online") return <Badge tone="signal">{t("Online")}</Badge>;
  if (!presence.lastSeen) return <Badge tone="outline">{t("Offline")}</Badge>;
  return (
    <Badge tone="outline">
      {t("Last seen {when}", { when: formatRelative(presence.lastSeen) })}
    </Badge>
  );
}

/** Whether a presence entry represents an online account. */
export function isOnline(presence: FriendPresence | null | undefined): boolean {
  return presence?.state === "online" || presence?.state === "in_game";
}
