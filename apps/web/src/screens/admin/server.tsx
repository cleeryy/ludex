import { useQuery } from "@tanstack/react-query";
import { Check, Copy, Lock, Minus } from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import { useToast } from "@/state/toasts";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Skeleton } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

export function ServerScreen() {
  const { push } = useToast();
  const config = useQuery({ queryKey: ["config"], queryFn: () => api.config() });
  const health = useQuery({ queryKey: ["health"], queryFn: () => api.health(), refetchInterval: 30_000 });
  const folders = useQuery({ queryKey: ["folders"], queryFn: () => api.folders() });

  const copy = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value);
      push({ title: t("{label} copied", { label }) });
    } catch {
      push({ title: t("Copy failed"), description: t("Your browser blocked clipboard access."), tone: "danger" });
    }
  };

  const features = config.data
    ? [
        [t("IGDB metadata"), config.data.features.igdb],
        [t("Steam metadata"), config.data.features.steam],
        [t("Ingest"), config.data.features.ingest],
        [t("Local mode"), config.data.features.localMode],
        [t("OIDC sign-in"), config.data.features.oidc],
      ] as const
    : [];

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Server")}
        meta={
          <>
            <span className="flex items-center gap-2">
              <span className="size-1.5 rounded-full bg-ok" aria-hidden="true" />
              {t("Healthy")}
            </span>
            {health.data && <span className="num">{t("Up")} {formatUptime(health.data.uptimeSeconds)}</span>}
            {USING_MOCKS && <Badge tone="outline">{t("Demo data")}</Badge>}
          </>
        }
        description={t("Read-only view of what this server is running and where it keeps files. Change these through environment variables and restart the container.")}
      />

      {config.isError && <ErrorState message={errorMessage(config.error)} onRetry={() => void config.refetch()} />}

      <div className="mt-8 grid gap-10 lg:grid-cols-2">
        <section>
          <SectionHeader title={t("Identity")} />
          <dl className="border-t border-line">
            {[
              [t("Server name"), config.data?.serverName ?? "—"],
              [t("Version"), config.data?.version ?? "—"],
              [t("Max upload"), config.data ? formatBytes(config.data.maxUploadBytes) : "—"],
              [t("Uptime"), health.data ? formatUptime(health.data.uptimeSeconds) : "—"],
            ].map(([label, value]) => (
              <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
                <dt className="text-[12.5px] text-ink-3">{label}</dt>
                <dd className="num text-[12.5px] text-ink-2">{value}</dd>
              </div>
            ))}
          </dl>
          {config.isPending && <Skeleton className="mt-2 h-3 w-32" />}
        </section>

        <section>
          <SectionHeader title={t("Features")} description={t("Set by environment variables")} />
          <div className="border-t border-line">
            {features.map(([label, enabled]) => (
              <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
                <span className="text-[12.5px] text-ink-2">{label}</span>
                <span className="flex items-center gap-1.5 text-[12px] text-ink-3">
                  {enabled ? <Check className="size-3.5 text-ok" aria-hidden="true" /> : <Minus className="size-3.5" aria-hidden="true" />}
                  {enabled ? t("Enabled") : t("Off")}
                </span>
              </div>
            ))}
            {config.isPending &&
              Array.from({ length: 4 }).map((_, index) => <Skeleton key={index} className="my-3 h-3 w-full" />)}
          </div>
          <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
            {t("Metadata providers are tried in order: IGDB when credentials are set, then the Steam store, then the filename alone.")}
          </p>
        </section>

        <section className="lg:col-span-2">
          <SectionHeader title={t("Folders")} description={t("Mounted into the container")} />
          <div className="border-t border-line">
            {folders.isError && <ErrorState message={errorMessage(folders.error)} onRetry={() => void folders.refetch()} />}
            {folders.isPending &&
              Array.from({ length: 3 }).map((_, index) => <Skeleton key={index} className="my-3 h-3 w-2/3" />)}
            {folders.data &&
              (
                [
                  [t("Games"), folders.data.gamesDir, "Archives and portable executables"],
                  [t("Media"), folders.data.mediaDir, "Cached covers, backgrounds, screenshots"],
                  [t("Data"), folders.data.dataDir, "SQLite database and JWT secret"],
                ] as const
              ).map(([label, path, hint]) => (
                <div key={label} className="flex flex-wrap items-baseline justify-between gap-3 border-b border-line py-3">
                  <div>
                    <p className="text-[13px] text-ink">{label}</p>
                    <p className="mt-0.5 text-[11.5px] text-ink-3">{hint}</p>
                  </div>
                  <div className="flex min-w-0 items-baseline gap-2">
                    <code className="truncate font-mono text-[11px] text-ink-2">{path}</code>
                    <button
                      type="button"
                      onClick={() => void copy(path, `${label} folder`)}
                      aria-label={t("Copy {label} folder path", { label })}
                      className="text-ink-3 hover:text-ink"
                    >
                      <Copy className="size-3.5" />
                    </button>
                  </div>
                </div>
              ))}
          </div>
        </section>
      </div>

      <p className="mt-8 flex items-center gap-2 text-[11.5px] text-ink-3">
        <Lock className="size-3.5" aria-hidden="true" />
        {t("Configuration is read-only here by design; the server owns its environment.")}
      </p>
    </div>
  );
}
