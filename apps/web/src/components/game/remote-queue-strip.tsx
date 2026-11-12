import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { formatBytes, formatPercent } from "@/lib/format";
import { t } from "@/lib/i18n";
import { isRemoteActive, remotePercent, remoteStatusLabel, remoteStatusTone } from "@/lib/mobile";
import { useAuth } from "@/state/auth";
import { Badge, Progress } from "@/components/ui/controls";
import { SectionHeader } from "@/components/page";

/**
 * Compact remote queue shown on the browser Downloads page (#167), so an
 * install queued from the phone is visible where people look for downloads.
 * The full queue (cancel, history, transport notes) lives on `/remote`.
 */
export function RemoteQueueStrip() {
  const { user } = useAuth();
  const { data } = useQuery({
    queryKey: ["remote-installs"],
    queryFn: () => api.listRemoteInstalls(),
    enabled: Boolean(user),
    refetchInterval: (query) =>
      query.state.data?.some((install) => isRemoteActive(install.status)) ? 5_000 : false,
  });

  if (!user) return null;
  const installs = data ?? [];
  if (installs.length === 0) return null;

  return (
    <section className="mt-8" aria-labelledby="downloads-remote">
      <SectionHeader
        id="downloads-remote"
        title={t("Remote installs")}
        count={installs.length}
        actions={
          <Link to="/remote" className="inline-flex items-center gap-1 text-[12.5px] text-ink-2 hover:text-ink">
            {t("Open the queue")}
            <ArrowRight className="size-3.5 rtl:-scale-x-100" aria-hidden="true" />
          </Link>
        }
      />
      <ul className="flex flex-col gap-2">
        {installs.slice(0, 4).map((install) => {
          const percent = remotePercent(install);
          return (
            <li key={install.id} className="flex items-center gap-3 border border-line bg-surface/40 px-3 py-2.5">
              <div className="min-w-0 flex-1">
                <Link to={`/games/${install.gameId}`} className="block truncate text-[13.5px] text-ink hover:text-signal-text">
                  {install.gameTitle}
                </Link>
                <p className="mt-0.5 flex flex-wrap items-center gap-x-2 text-[11px] text-ink-3">
                  <Badge tone={remoteStatusTone(install.status)}>{remoteStatusLabel(install.status)}</Badge>
                  {install.sizeBytes !== null && <span className="num">{formatBytes(install.sizeBytes)}</span>}
                  {percent !== null && <span className="num">{formatPercent(percent)}</span>}
                </p>
                {percent !== null && isRemoteActive(install.status) && (
                  <Progress value={percent} label={t("Install progress")} className={cn("mt-1.5 h-1")} />
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
