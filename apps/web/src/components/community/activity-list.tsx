import { Link } from "react-router-dom";
import { CheckCircle2, Download, ListPlus, Play, Plus, Trophy } from "lucide-react";
import type { Activity, ActivityKind } from "@/lib/types";
import { formatRelative } from "@/lib/format";
import { cn } from "@/lib/cn";
import { EmptyState } from "@/components/page";
import { t, tn, tr } from "@/lib/i18n";

const ICONS: Record<ActivityKind, React.ReactNode> = {
  game_added: <Plus className="size-3.5" aria-hidden="true" />,
  game_installed: <Download className="size-3.5" aria-hidden="true" />,
  game_played: <Play className="size-3.5" aria-hidden="true" />,
  game_completed: <CheckCircle2 className="size-3.5" aria-hidden="true" />,
  collection_created: <ListPlus className="size-3.5" aria-hidden="true" />,
  achievement_unlocked: <Trophy className="size-3.5" aria-hidden="true" />,
};

const TONES: Record<ActivityKind, string> = {
  game_added: "border-line-2 text-ink-3",
  game_installed: "border-ok/40 text-ok",
  game_played: "border-signal/50 text-signal-text",
  game_completed: "border-ok/40 text-ok",
  collection_created: "border-line-2 text-ink-2",
  achievement_unlocked: "border-signal/50 text-signal-text",
};

function sentence(activity: Activity): React.ReactNode {
  const who = activity.user.displayName ?? activity.user.username;
  const game = activity.game ? (
    <Link to={`/game/${activity.game.id}`} className="text-ink underline-offset-4 hover:underline">
      {activity.game.title}
    </Link>
  ) : (
    t("a game")
  );
  const collection = activity.collection ? (
    <Link to={`/collections/${activity.collection.id}`} className="text-ink underline-offset-4 hover:underline">
      {activity.collection.name}
    </Link>
  ) : (
    t("a shelf")
  );
  const minutes = typeof activity.metadata.minutes === "number" ? activity.metadata.minutes : null;

  switch (activity.kind) {
    case "game_added":
      return tr("{who} added {game} to the library", { who, game });
    case "game_installed":
      return tr("{who} installed {game}", { who, game });
    case "game_played":
      return minutes
        ? tr("{who} played {game} for {minutes}", { who, game, minutes: tn(minutes, "1 minute", "{count} minutes") })
        : tr("{who} played {game}", { who, game });
    case "game_completed":
      return tr("{who} finished {game}", { who, game });
    case "collection_created":
      return tr("{who} created the collection {collection}", { who, collection });
    case "achievement_unlocked": {
      const achievement =
        typeof activity.metadata.achievementName === "string" ? activity.metadata.achievementName : t("an achievement");
      return tr("{who} unlocked {achievement} in {game}", { who, achievement, game });
    }
    default:
      return tr("{who} did something", { who });
  }
}

export function ActivityRow({ activity, showActor = true }: { activity: Activity; showActor?: boolean }) {
  return (
    <div className="flex gap-3 border-b border-line py-3 last:border-0">
      <span
        className={cn("mt-0.5 grid size-8 shrink-0 place-items-center rounded-[2px] border", TONES[activity.kind])}
        aria-hidden="true"
      >
        {ICONS[activity.kind]}
      </span>
      <div className="min-w-0 flex-1">
        <p className="text-[13px] leading-relaxed text-ink-2">
          {showActor ? (
            sentence(activity)
          ) : (
            sentence({ ...activity, user: { ...activity.user, displayName: null, username: t("You") } })
          )}
        </p>
        <p className="mt-0.5 flex flex-wrap items-baseline gap-x-2 font-mono text-[10px] text-ink-3">
          <span>{formatRelative(activity.createdAt)}</span>
          {activity.game?.releaseYear ? <span>{activity.game.releaseYear}</span> : null}
          {showActor && (
            <Link to={`/community/u/${activity.user.id}`} className="underline-offset-4 hover:text-ink-2 hover:underline">
              {activity.user.displayName ?? activity.user.username}
            </Link>
          )}
        </p>
      </div>
    </div>
  );
}

export function ActivityList({
  items,
  emptyTitle,
  emptyDescription,
  onLoadMore,
  loadingMore,
  hasMore,
}: {
  items: Activity[];
  emptyTitle: string;
  emptyDescription: string;
  onLoadMore?: () => void;
  loadingMore?: boolean;
  hasMore?: boolean;
}) {
  if (items.length === 0) {
    return <EmptyState compact title={emptyTitle} description={emptyDescription} />;
  }
  return (
    <div>
      <div className="border-t border-line">
        {items.map((activity) => (
          <ActivityRow key={activity.id} activity={activity} />
        ))}
      </div>
      {hasMore && onLoadMore && (
        <div className="mt-4">
          <button
            type="button"
            onClick={onLoadMore}
            disabled={loadingMore}
            className="h-9 border border-line px-3 text-[12.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink disabled:opacity-40"
          >
            {loadingMore ? t("Loading…") : t("Load more")}
          </button>
        </div>
      )}
    </div>
  );
}
