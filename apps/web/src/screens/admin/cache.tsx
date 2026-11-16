import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, RefreshCw, Settings2, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatDate, formatRelative } from "@/lib/format";
import type { CacheEntryView, CacheOverview } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { EmptyState, ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, IconButton, Progress, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { t, tn, tr, formatNumber } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/** Twelve leading hex characters are plenty to tell two archives apart. */
function shortSha(sha256: string): string {
  return sha256.length > 12 ? `${sha256.slice(0, 12)}…` : sha256;
}

function usedPercent(overview: CacheOverview): number {
  if (overview.budgetBytes <= 0) return 0;
  return Math.min(100, Math.round((overview.usedBytes / overview.budgetBytes) * 100));
}

const linkClass =
  "inline-flex items-center gap-1.5 text-[12.5px] text-ink-2 underline decoration-line-2 underline-offset-4 transition-colors duration-[120ms] hover:text-ink";

/** One number of the capacity strip. */
function Metric({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="flex flex-col gap-1 py-4 sm:px-6 sm:first:ps-0 sm:last:pe-0">
      <span className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{label}</span>
      <span className="num text-[clamp(1.35rem,2vw,1.85rem)] font-semibold leading-none text-ink">{value}</span>
      {hint && <span className="num text-[11.5px] text-ink-3">{hint}</span>}
    </div>
  );
}

/**
 * Administration → Extracted cache (#231).
 *
 * The server extracts an archive once and serves the individual files to an
 * install, so a client never stores or unpacks it. This screen is the honest
 * ledger of that cache: whether the feature is on, how much of the budget is
 * used, and which entries can be dropped to give the space back.
 */
export function ExtractedCacheScreen() {
  const { push } = useToast();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [target, setTarget] = useState<CacheEntryView | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const overview = useQuery({
    queryKey: qk.admin.extractedCache,
    queryFn: () => api.extractedCache(),
    refetchInterval: 30_000,
  });

  // Only needed to word the "feature off" panel: local mode points at the
  // device setting, a self-hosted server at its environment variable.
  const config = useQuery({
    queryKey: qk.config,
    queryFn: () => api.config(),
    enabled: overview.data?.enabled === false,
  });

  const invalidate = () => void queryClient.invalidateQueries({ queryKey: qk.admin.extractedCache });

  const evict = useMutation({
    mutationFn: (sha256: string) => api.evictExtractedCache(sha256),
    // The row leaves the moment the administrator confirms; a refused eviction
    // puts the previous overview back, so the table never lies about the disk.
    onMutate: async (sha256) => {
      await queryClient.cancelQueries({ queryKey: qk.admin.extractedCache });
      const previous = queryClient.getQueryData<CacheOverview>(qk.admin.extractedCache);
      if (previous) {
        const entry = previous.entries.find((item) => item.sha256 === sha256);
        queryClient.setQueryData<CacheOverview>(qk.admin.extractedCache, {
          ...previous,
          usedBytes: Math.max(0, previous.usedBytes - (entry?.bytes ?? 0)),
          entries: previous.entries.filter((item) => item.sha256 !== sha256),
        });
      }
      return { previous };
    },
    onSuccess: () => {
      push({ title: t("Cache entry removed"), tone: "success" });
      setTarget(null);
    },
    onError: (error, _sha256, context) => {
      if (context?.previous) queryClient.setQueryData(qk.admin.extractedCache, context.previous);
      push({
        title: t("The cache entry could not be removed"),
        description: errorMessage(error),
        tone: "danger",
      });
      setTarget(null);
    },
    onSettled: () => invalidate(),
  });

  const copy = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value);
      push({ title: t("{label} copied", { label }) });
    } catch {
      push({
        title: t("Copy failed"),
        description: t("Your browser blocked clipboard access."),
        tone: "danger",
      });
    }
  };

  const refresh = async () => {
    setRefreshing(true);
    try {
      await overview.refetch();
    } finally {
      setRefreshing(false);
    }
  };

  const data = overview.data;
  const percent = data ? usedPercent(data) : 0;
  const localMode = config.data?.features.localMode;

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Extracted cache")}
        meta={
          data && (
            <>
              <Badge tone={data.enabled ? "ok" : "outline"}>
                {data.enabled ? t("On") : t("Off")}
              </Badge>
              {data.enabled && (
                <span className="num">
                  {tn(data.entries.length, "1 entry", "{count} entries")}
                </span>
              )}
              {data.enabled && data.usedBytes > 0 && (
                <span className="num">
                  {t("{used} of {budget}", {
                    used: formatBytes(data.usedBytes),
                    budget: formatBytes(data.budgetBytes),
                  })}
                </span>
              )}
            </>
          )
        }
        description={t("Archives the server has extracted once, so an install downloads files instead of a 40 GB archive. Removing an entry only costs the next install its preparation; it falls back on the archive by itself.")}
        actions={
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            loading={refreshing}
            onClick={() => void refresh()}
          >
            {t("Refresh")}
          </Button>
        }
      />

      {overview.isError ? (
        <div className="mt-8">
          <ErrorState
            message={errorMessage(overview.error)}
            onRetry={() => void overview.refetch()}
          />
        </div>
      ) : !data ? (
        <div className="mt-8 flex flex-col gap-3">
          <Skeleton className="h-24" />
          <Skeleton className="h-10" />
          <Skeleton className="h-10" />
        </div>
      ) : !data.enabled ? (
        <section aria-labelledby="cache-off-heading" className="mt-8 border border-line bg-surface/40 p-6 sm:p-8">
          <Badge tone="outline">{t("Off")}</Badge>
          <h2
            id="cache-off-heading"
            className="mt-4 text-title font-semibold tracking-[-0.015em]"
          >
            {t("The extracted cache is off")}
          </h2>
          <p className="measure mt-2 text-body leading-relaxed text-ink-2">
            {t("Installs download the archive and extract it on the client. When the cache is on, the server extracts an archive once and serves its files instead, so a client never stores or unpacks it.")}
          </p>
          {localMode === undefined ? (
            <Skeleton className="mt-5 h-4 w-64" />
          ) : localMode ? (
            <Link to="/settings?section=downloads" className={`mt-5 ${linkClass}`}>
              <Settings2 className="size-3.5" aria-hidden="true" />
              {t("Open download settings")}
            </Link>
          ) : (
            <p className="mt-5 text-[12.5px] leading-relaxed text-ink-3">
              {tr("Set {variable} on the server and restart it to turn the cache on.", {
                variable: (
                  <code className="font-mono text-[11.5px] text-ink-2">LUDEX_EXTRACTED_CACHE=1</code>
                ),
              })}
            </p>
          )}
        </section>
      ) : (
        <>
          <section aria-labelledby="cache-capacity-heading" className="mt-8">
            <SectionHeader
              id="cache-capacity-heading"
              title={t("Capacity")}
              description={t("A bounded cache: the least recently used entry is evicted when the budget is reached.")}
            />
            <div className="grid grid-cols-1 border-y border-line sm:grid-cols-3 sm:divide-x sm:divide-line">
              <Metric
                label={t("Used")}
                value={formatBytes(data.usedBytes)}
                hint={t("{percent}% of the budget", { percent })}
              />
              <Metric label={t("Budget")} value={formatBytes(data.budgetBytes)} />
              <Metric
                label={t("Entries")}
                value={String(data.entries.length)}
                hint={tn(data.entries.length, "1 extracted archive", "{count} extracted archives")}
              />
            </div>
            <Progress
              value={data.budgetBytes > 0 ? (data.usedBytes / data.budgetBytes) * 100 : 0}
              tone={percent >= 90 ? "danger" : "signal"}
              label={t("Cache usage")}
              className="mt-4"
            />
            <div className="mt-3 flex flex-wrap items-baseline justify-between gap-3 border-b border-line pb-3">
              <span className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
                {t("Cache folder")}
              </span>
              <code className="break-all font-mono text-[11.5px] text-ink-2">{data.root || "—"}</code>
            </div>
          </section>

          <section aria-labelledby="cache-entries-heading" className="mt-10">
            <SectionHeader
              id="cache-entries-heading"
              title={t("Entries")}
              count={data.entries.length}
              description={t("One row per archive digest. An entry serves every version of that archive, so prepare an archive once.")}
            />

            {data.entries.length === 0 ? (
              <EmptyState
                className="mt-2"
                compact
                level={3}
                title={t("No extracted archives")}
                description={t("Prepare a version from Admin → Games to extract it once. Installs then download its files instead of the archive.")}
              >
                <Button variant="default" size="sm" onClick={() => navigate("/admin/games")}>
                  {t("Open Admin → Games")}
                </Button>
              </EmptyState>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full min-w-[860px] border-collapse text-left text-[12.5px]">
                  <thead>
                    <tr className="border-b border-line-2 font-mono text-[10px] uppercase tracking-[0.1em] text-ink-3">
                      <th className="py-2 pe-3">{t("Archive")}</th>
                      <th className="py-2 pe-3 text-end">{t("Version")}</th>
                      <th className="py-2 pe-3 text-end">{t("Extracted")}</th>
                      <th className="py-2 pe-3 text-end">{t("Files")}</th>
                      <th className="py-2 pe-3">{t("Created")}</th>
                      <th className="py-2 pe-3">{t("Last used")}</th>
                      <th className="w-20 py-2 text-end">{t("Actions")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.entries.map((entry) => (
                      <tr key={entry.sha256} className="border-b border-line align-middle">
                        <td className="py-2 pe-3">
                          <span className="flex items-center gap-1">
                            <code
                              title={entry.sha256}
                              className="font-mono text-[11.5px] text-ink-2"
                            >
                              {shortSha(entry.sha256)}
                            </code>
                            <IconButton
                              label={t("Copy archive digest")}
                              size="sm"
                              variant="ghost"
                              className="text-ink-3 hover:text-ink"
                              onClick={() => void copy(entry.sha256, t("Archive digest"))}
                            >
                              <Copy className="size-3.5" />
                            </IconButton>
                          </span>
                        </td>
                        <td className="num py-2 pe-3 text-end text-ink-2">#{entry.versionId}</td>
                        <td className="num py-2 pe-3 text-end text-ink">{formatBytes(entry.bytes)}</td>
                        <td className="num py-2 pe-3 text-end text-ink-2">{formatNumber(entry.files)}</td>
                        <td className="num py-2 pe-3 text-ink-3" title={formatDate(entry.createdAt)}>
                          {formatRelative(entry.createdAt)}
                        </td>
                        <td className="num py-2 pe-3 text-ink-3" title={formatDate(entry.lastUsedAt)}>
                          {formatRelative(entry.lastUsedAt)}
                        </td>
                        <td className="py-2 text-end">
                          <IconButton
                            label={t("Remove entry")}
                            size="sm"
                            variant="ghost"
                            className="text-danger hover:bg-danger/10"
                            disabled={evict.isPending}
                            onClick={() => setTarget(entry)}
                          >
                            <Trash2 className="size-4" />
                          </IconButton>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>
        </>
      )}

      <ConfirmDialog
        open={target !== null}
        onOpenChange={(open) => {
          if (!open) setTarget(null);
        }}
        title={t("Remove this cache entry?")}
        description={t("Installs fall back to downloading the archive until the entry is prepared again.")}
        confirmLabel={t("Remove entry")}
        loading={evict.isPending}
        onConfirm={() => target && evict.mutate(target.sha256)}
      />
    </div>
  );
}
