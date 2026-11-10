import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Lock, RefreshCw, Trophy } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate, formatPercent } from "@/lib/format";
import type { Achievement } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { EmptyState, SectionHeader } from "@/components/page";
import { Badge, Button, Progress, Skeleton } from "@/components/ui/controls";
import { t , tn } from "@/lib/i18n";

/**
 * Achievement section of the game page: progress, definitions and the local
 * scan trigger for editors. Hidden achievements stay masked until unlocked.
 */
export function AchievementsPanel({
  gameId,
  isEditor,
  installPath,
}: {
  gameId: number;
  isEditor: boolean;
  installPath?: string | null;
}) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const [scanning, setScanning] = useState(false);

  const { data, isPending, isError, error } = useQuery({
    queryKey: ["game-achievements", gameId],
    queryFn: () => api.gameAchievements(gameId),
  });

  const scan = async () => {
    setScanning(true);
    try {
      const report = await api.scanAchievements(gameId, {
        installPath: installPath ?? null,
      });
      const failed = report.providers.filter((provider) => provider.status === "error");
      push({
        title: t("Achievement scan finished"),
        description:
          report.found === 0
            ? "No achievement files were found for this game. Check the install path in Settings."
            : `${report.found} definitions read, ${report.unlocked} new unlock${report.unlocked === 1 ? "" : "s"}.`,
        tone: failed.length > 0 ? "danger" : "success",
      });
      await queryClient.invalidateQueries({ queryKey: ["game-achievements", gameId] });
      await queryClient.invalidateQueries({ queryKey: ["me-achievements"] });
    } catch (cause) {
      push({ title: t("Scan failed"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setScanning(false);
    }
  };

  return (
    <section className="mt-14" aria-labelledby="achievements-heading">
      <SectionHeader
        title={t("Achievements")}
        count={data?.total}
        description={data && data.total > 0 ? `${data.unlocked} of ${data.total} · ${data.unlockedPoints}/${data.points} pts` : undefined}
        actions={
          isEditor ? (
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw className="size-3.5" />}
              loading={scanning}
              onClick={() => void scan()}
            >
              {t("Scan local files")}
            </Button>
          ) : undefined
        }
      />

      <div id="achievements-heading" className="border-t border-line pt-4">
        {isPending ? (
          <div className="flex flex-col gap-2">
            <Skeleton className="h-3 w-full" />
            <Skeleton className="h-12 w-full" />
            <Skeleton className="h-12 w-full" />
          </div>
        ) : isError ? (
          <p className="text-[12.5px] text-ink-3">{t("Achievements could not be loaded:")} {errorMessage(error)}</p>
        ) : !data || data.total === 0 ? (
          <EmptyState
            compact
            title={t("No achievements yet")}
            description={t("Achievements arrive from a local scan (Steam emulation files or a Galaxy database) or from an administrator adding them by hand.")}
          />
        ) : (
          <div className="flex flex-col gap-5">
            <div>
              <div className="flex items-baseline justify-between gap-4 pb-1.5">
                <span className="font-mono text-[10.5px] uppercase tracking-[0.08em] text-ink-3">{t("Completion")}</span>
                <span className="num text-[12px] text-ink-2">{formatPercent(data.completion, 1)}</span>
              </div>
              <Progress
                value={data.completion}
                label={`Achievements: ${data.unlocked} of ${data.total} unlocked`}
              />
              <p className="mt-2 flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-ink-3">
                <span>{tn(data.total, "1 achievement", "{count} achievements")}</span>
                <span>{tn(data.points, "1 point", "{count} points")}</span>
                {data.lastScan && <span>{t("Scanned")} {formatDate(data.lastScan.scannedAt)}</span>}
              </p>
            </div>

            <ul className="border-t border-line">
              {data.achievements.map((achievement) => (
                <AchievementRow key={achievement.id} achievement={achievement} />
              ))}
            </ul>
          </div>
        )}
      </div>
    </section>
  );
}

function AchievementRow({ achievement }: { achievement: Achievement }) {
  const secret = achievement.hidden && !achievement.unlocked;
  return (
    <li
      className={
        achievement.unlocked
          ? "flex items-start gap-3 border-b border-line py-3"
          : "flex items-start gap-3 border-b border-line py-3 opacity-70"
      }
    >
      <AchievementIcon achievement={achievement} />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-2.5 gap-y-1">
          <p className={achievement.unlocked ? "text-[13.5px] text-ink" : "text-[13.5px] text-ink-2"}>
            {achievement.name}
          </p>
          {achievement.unlocked ? (
            <Badge tone="signal">
              <Trophy className="mr-1 size-3" aria-hidden="true" />
              {t("{count} pts", { count: achievement.points })}
            </Badge>
          ) : (
            <span className="inline-flex items-center gap-1 font-mono text-[10.5px] text-ink-3">
              <Lock className="size-3" aria-hidden="true" />
              {t("{count} pts", { count: achievement.points })}
            </span>
          )}
          {achievement.hidden && <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3">{t("Hidden")}</span>}
        </div>
        {achievement.description && (
          <p className="mt-0.5 text-[12px] leading-relaxed text-ink-3">{achievement.description}</p>
        )}
        <p className="mt-1 flex flex-wrap items-baseline gap-x-3 font-mono text-[10px] text-ink-3">
          <span>{t("{percent} of players", { percent: formatPercent(achievement.unlockPercentage, 1) })}</span>
          {achievement.unlockedAt && <span>{t("Unlocked")} {formatDate(achievement.unlockedAt)}</span>}
          {achievement.provider !== "manual" && <span>{providerLabel(achievement.provider)}</span>}
          {secret && <span>{t("Details hidden")}</span>}
        </p>
      </div>
    </li>
  );
}

function AchievementIcon({ achievement }: { achievement: Achievement }) {
  if (achievement.iconUrl) {
    return (
      <img
        src={achievement.iconUrl}
        alt=""
        loading="lazy"
        className={achievement.unlocked ? "size-10 shrink-0 rounded-[2px] border border-line object-cover" : "size-10 shrink-0 rounded-[2px] border border-line object-cover grayscale"}
      />
    );
  }
  const letter = achievement.name.trim().charAt(0).toUpperCase() || "?";
  return (
    <span
      aria-hidden="true"
      className={
        achievement.unlocked
          ? "grid size-10 shrink-0 place-items-center rounded-[2px] border border-signal/40 bg-signal/10 font-mono text-[13px] text-signal-text"
          : "grid size-10 shrink-0 place-items-center rounded-[2px] border border-line bg-surface-2 font-mono text-[13px] text-ink-3"
      }
    >
      {achievement.hidden && !achievement.unlocked ? <Lock className="size-3.5" /> : letter}
    </span>
  );
}

function providerLabel(provider: string): string {
  switch (provider) {
    case "game_files":
      return "Steam files";
    case "gog_galaxy_api":
      return "Galaxy API";
    case "gog_galaxy_sqlite":
      return "Galaxy database";
    default:
      return provider;
  }
}
