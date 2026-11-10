import { useQuery } from "@tanstack/react-query";
import { Trophy } from "lucide-react";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import type { Achievement } from "@/lib/types";
import { SectionHeader } from "@/components/page";
import {  t , tn } from "@/lib/i18n";

/**
 * Achievement showcase on the community profile. Visibility follows the
 * profile's opt-in flag: the API returns 404 for private profiles, so a
 * failed query simply renders nothing.
 */
export function AchievementShowcase({ userId }: { userId: number }) {
  const { data, isError } = useQuery({
    queryKey: ["user-achievements", userId],
    queryFn: () => api.userAchievements(userId),
    retry: false,
  });

  if (isError || !data || data.unlocked === 0) return null;

  return (
    <section className="mt-12" aria-labelledby="profile-achievements">
      <SectionHeader
        title={t("Achievements")}
        count={data.unlocked}
        description={t("{points} points across {games}", { points: data.unlockedPoints, games: tn(data.games, "1 game", "{count} games") })}
      />
      <div id="profile-achievements" className="grid gap-2 border-t border-line pt-4 sm:grid-cols-2 xl:grid-cols-3">
        {data.achievements.slice(0, 6).map((achievement) => (
          <ShowcaseTile key={achievement.id} achievement={achievement} />
        ))}
      </div>
    </section>
  );
}

function ShowcaseTile({ achievement }: { achievement: Achievement }) {
  const letter = achievement.name.trim().charAt(0).toUpperCase() || "?";
  return (
    <div className="flex items-start gap-3 border border-line bg-surface/40 p-3">
      {achievement.iconUrl ? (
        <img
          src={achievement.iconUrl}
          alt=""
          loading="lazy"
          className="size-9 shrink-0 rounded-[2px] border border-line object-cover"
        />
      ) : (
        <span
          aria-hidden="true"
          className="grid size-9 shrink-0 place-items-center rounded-[2px] border border-signal/40 bg-signal/10 font-mono text-[12px] text-signal-text"
        >
          {letter}
        </span>
      )}
      <div className="min-w-0 flex-1">
        <p className="truncate text-[12.5px] text-ink">{achievement.name}</p>
        <p className="mt-0.5 truncate text-[11px] text-ink-3">{achievement.gameTitle ?? t("Library")}</p>
        <p className="mt-1 flex items-center gap-1 font-mono text-[10px] text-ink-3">
          <Trophy className="size-3" aria-hidden="true" />
          {t("{count} pts", { count: achievement.points })}
          {achievement.unlockedAt && <span className="text-ink-3">· {formatDate(achievement.unlockedAt)}</span>}
        </p>
      </div>
    </div>
  );
}
