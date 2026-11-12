import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Activity,
  AlertTriangle,
  ArrowUpRight,
  Database,
  HardDrive,
  Radio,
  RefreshCw,
  ShieldCheck,
  Webhook,
} from "lucide-react";
import { api, getApiBase } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatDate, formatRelative } from "@/lib/format";
import { t } from "@/lib/i18n";
import {
  formatMs,
  formatRate,
  formatShare,
  seriesValues,
  sparkline,
  type SparklineGeometry,
} from "@/lib/observability";
import type { ObservabilitySnapshot, ObservabilityWindow } from "@/lib/types";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, IconButton, Segmented, Skeleton } from "@/components/ui/controls";
import { cn } from "@/lib/cn";

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

/** Label/value pair used across the KPI grid. */
function Metric({
  label,
  value,
  hint,
  tone,
}: {
  label: string;
  value: string;
  hint?: string;
  tone?: "danger" | "ok" | "warn";
}) {
  return (
    <div className="flex flex-col gap-1 border-t border-line py-4">
      <dt className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3">{label}</dt>
      <dd
        className={cn(
          "num text-[26px] leading-none tracking-[-0.02em]",
          tone === "danger" && "text-danger",
          tone === "ok" && "text-ok",
          tone === "warn" && "text-warn",
        )}
      >
        {value}
      </dd>
      {hint && <span className="text-[11.5px] leading-snug text-ink-3">{hint}</span>}
    </div>
  );
}

function Spark({
  geometry,
  label,
  tone,
}: {
  geometry: SparklineGeometry;
  label: string;
  tone?: "danger" | "ok";
}) {
  return (
    <svg
      viewBox="0 0 240 56"
      preserveAspectRatio="none"
      role="img"
      aria-label={label}
      className={cn(
        "h-14 w-full text-signal",
        tone === "danger" && "text-danger",
        tone === "ok" && "text-ok",
      )}
    >
      <title>{label}</title>
      {geometry.area !== "" && <path d={geometry.area} fill="currentColor" fillOpacity="0.12" />}
      {geometry.line !== "" && (
        <polyline
          points={geometry.line}
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinejoin="round"
          strokeLinecap="round"
          vectorEffect="non-scaling-stroke"
        />
      )}
    </svg>
  );
}

function RequestPanel({ data }: { data: ObservabilitySnapshot }) {
  const requests = sparkline(seriesValues(data.series));
  const errors = sparkline(seriesValues(data.series, "errors"));
  const latency = sparkline(seriesValues(data.series, "latency"));
  const errorLabel = t("Errors over the selected window: {count} of {total} requests", {
    count: data.requests.clientErrors + data.requests.serverErrors,
    total: data.requests.total,
  });
  const requestLabel = t("Requests over the selected window: {count} total", {
    count: data.requests.total,
  });
  const latencyLabel = t("Average latency over the selected window: p95 {p95}", {
    p95: formatMs(data.requests.p95Ms),
  });
  return (
    <section aria-labelledby="traffic-heading" className="flex flex-col gap-6">
      <SectionHeader
        id="traffic-heading"
        title={t("Traffic")}
        description={t("Last {window}", {
          window: data.window === "1h" ? t("1 hour") : t("24 hours"),
        })}
      />
      <dl className="grid gap-x-8 sm:grid-cols-2 xl:grid-cols-3">
        <Metric
          label={t("Requests")}
          value={String(data.requests.total)}
          hint={t("{rate} average", { rate: formatRate(data.requests.perSecond) })}
        />
        <Metric
          label={t("Error rate")}
          value={formatShare(data.requests.errorRate)}
          hint={t("{errors} client · {server} server", {
            errors: data.requests.clientErrors,
            server: data.requests.serverErrors,
          })}
          tone={
            data.requests.errorRate >= 0.05
              ? "danger"
              : data.requests.errorRate >= 0.01
                ? "warn"
                : "ok"
          }
        />
        <Metric
          label={t("p95 latency")}
          value={formatMs(data.requests.p95Ms)}
          hint={t("p50 {p50} · p99 {p99}", {
            p50: formatMs(data.requests.p50Ms),
            p99: formatMs(data.requests.p99Ms),
          })}
        />
      </dl>
      {data.series.length === 0 ? (
        <p className="text-[13px] text-ink-3">
          {t("No requests recorded in this window yet.")}
        </p>
      ) : (
        <div className="grid gap-6 md:grid-cols-3">
          <figure className="flex flex-col gap-2">
            <Spark geometry={requests} label={requestLabel} />
            <figcaption className="flex items-baseline justify-between gap-3 text-[11.5px] text-ink-3">
              <span>{t("Requests per bucket")}</span>
              <span className="num">
                {t("peak {peak}", { peak: requests.max })}
              </span>
            </figcaption>
          </figure>
          <figure className="flex flex-col gap-2">
            <Spark geometry={errors} label={errorLabel} tone="danger" />
            <figcaption className="flex items-baseline justify-between gap-3 text-[11.5px] text-ink-3">
              <span>{t("Errors per bucket")}</span>
              <span className="num">
                {t("last {last}", { last: errors.last })}
              </span>
            </figcaption>
          </figure>
          <figure className="flex flex-col gap-2">
            <Spark geometry={latency} label={latencyLabel} tone="ok" />
            <figcaption className="flex items-baseline justify-between gap-3 text-[11.5px] text-ink-3">
              <span>{t("Latency per bucket")}</span>
              <span className="num">
                {t("last {last}", { last: formatMs(latency.last) })}
              </span>
            </figcaption>
          </figure>
        </div>
      )}
    </section>
  );
}

function ActivePanel({ data }: { data: ObservabilitySnapshot }) {
  return (
    <section aria-labelledby="active-heading" className="flex flex-col gap-4">
      <SectionHeader id="active-heading" title={t("Active work")} />
      <dl className="grid grid-cols-2 gap-x-6">
        <Metric
          label={t("Scans")}
          value={String(data.active.scans)}
          hint={data.active.scans > 0 ? t("Running") : t("Idle")}
          tone={data.active.scans > 0 ? "ok" : undefined}
        />
        <Metric
          label={t("Downloads")}
          value={String(data.active.downloads)}
          hint={t("Streaming now")}
        />
        <Metric
          label={t("Ingest running")}
          value={String(data.active.ingestRunning)}
          hint={t("{queued} queued", { queued: data.active.ingestQueued })}
        />
        <Metric
          label={t("Integrity")}
          value={data.active.integrityRunning ? t("Running") : t("Idle")}
          hint={data.active.integrityRunning ? t("Checking archives") : t("No check running")}
          tone={data.active.integrityRunning ? "ok" : undefined}
        />
      </dl>
    </section>
  );
}

function DatabasePanel({ data }: { data: ObservabilitySnapshot }) {
  return (
    <section aria-labelledby="database-heading" className="flex flex-col gap-4">
      <SectionHeader id="database-heading" title={t("Database")} />
      <dl className="grid grid-cols-2 gap-x-6">
        <Metric
          label={t("Size")}
          value={formatBytes(data.database.sizeBytes)}
          hint={data.database.backend}
        />
        <Metric
          label={t("Probe")}
          value={formatMs(data.database.latencyMs)}
          hint={data.database.ok ? t("Reachable") : t("Degraded")}
          tone={data.database.ok ? "ok" : "danger"}
        />
        <Metric
          label={t("Pool")}
          value={`${data.database.connections}/${data.database.maxConnections}`}
          hint={t("{idle} idle", { idle: data.database.idleConnections })}
        />
        <Metric
          label={t("Uptime")}
          value={formatUptime(data.server.uptimeSeconds)}
          hint={t("v{version} · {commit}", {
            version: data.server.version,
            commit: data.server.commit,
          })}
        />
      </dl>
    </section>
  );
}

function StoragePanel({ data }: { data: ObservabilitySnapshot }) {
  const storage = data.storage;
  const rows: [string, string][] = [
    [t("Data volume free"), formatBytes(storage.disk.dataFreeBytes)],
    [t("Games volume free"), formatBytes(storage.disk.gamesFreeBytes)],
    [t("Media volume free"), formatBytes(storage.disk.mediaFreeBytes)],
    [
      t("Media cache"),
      t("{size} · {files} files", {
        size: formatBytes(storage.media.bytes),
        files: storage.media.files,
      }),
    ],
    [
      t("Cloud saves"),
      t("{size} · {count} snapshots", {
        size: formatBytes(storage.saves.bytes),
        count: storage.saves.snapshots,
      }),
    ],
    [
      t("Backups"),
      storage.backups.archives === 0
        ? t("None found")
        : t("{size} · {count} archives · latest {when}", {
            size: formatBytes(storage.backups.bytes),
            count: storage.backups.archives,
            when: formatRelative(storage.backups.latestAt),
          }),
    ],
  ];
  return (
    <section id="storage" aria-labelledby="storage-heading" className="flex flex-col gap-4">
      <SectionHeader
        id="storage-heading"
        title={t("Storage")}
        description={t("Backend: {backend}", { backend: storage.backend })}
      />
      <dl className="border-t border-line">
        {rows.map(([label, value]) => (
          <div
            key={label}
            className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 border-b border-line py-2.5"
          >
            <dt className="text-[12.5px] text-ink-3">{label}</dt>
            <dd className="num text-[12.5px] text-ink-2">{value}</dd>
          </div>
        ))}
      </dl>
      {storage.roots.length === 0 ? (
        <p className="text-[13px] text-ink-3">{t("No library roots configured.")}</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[420px] border-collapse text-left">
            <caption className="sr-only">{t("Library roots")}</caption>
            <thead>
              <tr className="border-b border-line font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3">
                <th scope="col" className="py-2 pr-3 font-normal">
                  {t("Root")}
                </th>
                <th scope="col" className="py-2 pr-3 font-normal">
                  {t("Versions")}
                </th>
                <th scope="col" className="py-2 font-normal">
                  {t("State")}
                </th>
              </tr>
            </thead>
            <tbody>
              {storage.roots.map((root) => (
                <tr key={root.label} className="border-b border-line text-[12.5px]">
                  <th scope="row" className="py-2 pr-3 font-normal text-ink-2">
                    {root.label}
                  </th>
                  <td className="num py-2 pr-3 text-ink-3">{root.versions}</td>
                  <td className="py-2">
                    {root.available ? (
                      <Badge tone="ok">{t("Available")}</Badge>
                    ) : (
                      <Badge tone="danger">{t("Unavailable")}</Badge>
                    )}
                    {!root.enabled && <span className="ml-2 text-[11.5px] text-ink-3">{t("Disabled")}</span>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function EventsPanel({ data }: { data: ObservabilitySnapshot }) {
  return (
    <section id="recent-events" aria-labelledby="events-heading" className="flex flex-col gap-4">
      <SectionHeader
        id="events-heading"
        title={t("Recent audit events")}
        description={t("Newest first, capped at 12.")}
      />
      {data.recentEvents.length === 0 ? (
        <p className="text-[13px] text-ink-3">{t("No audit events recorded yet.")}</p>
      ) : (
        <ul className="border-t border-line">
          {data.recentEvents.map((event) => (
            <li
              key={event.id}
              className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 border-b border-line py-2.5"
            >
              <span className="num text-[12.5px] text-ink-2">{event.action}</span>
              <span className="text-[11.5px] text-ink-3">
                {event.actorKind}
                {event.targetType ? ` · ${event.targetType}` : ""} ·{" "}
                <time dateTime={event.createdAt ?? undefined}>{formatRelative(event.createdAt)}</time>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function ErrorsPanel({ data }: { data: ObservabilitySnapshot }) {
  return (
    <section aria-labelledby="errors-heading" className="flex flex-col gap-4">
      <SectionHeader
        id="errors-heading"
        title={t("Recent errors")}
        description={t("Most frequent 4xx/5xx responses since the server started.")}
      />
      {data.topErrors.length === 0 ? (
        <p className="text-[13px] text-ink-3">
          {t("No errors recorded since the server started.")}
        </p>
      ) : (
        <ul className="border-t border-line">
          {data.topErrors.map((row) => (
            <li
              key={`${row.method}-${row.route}-${row.status}`}
              className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 border-b border-line py-2.5"
            >
              <span className="num min-w-0 break-all text-[12.5px] text-ink-2">
                {row.method} {row.route}
              </span>
              <span className="text-[11.5px] text-ink-3">
                <span className={row.status >= 500 ? "text-danger" : "text-warn"}>
                  {t("HTTP {status}", { status: row.status })}
                </span>{" "}
                · {t("{count} responses", { count: row.count })}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function WebhooksPanel({ data }: { data: ObservabilitySnapshot }) {
  return (
    <section aria-labelledby="webhooks-heading" className="flex flex-col gap-4">
      <SectionHeader
        id="webhooks-heading"
        title={t("Webhook deliveries")}
        actions={
          <span className="flex items-center gap-2">
            <Badge tone={data.webhooks.failed > 0 ? "danger" : "outline"}>
              {t("{count} failed", { count: data.webhooks.failed })}
            </Badge>
            <Badge tone="outline">{t("{count} pending", { count: data.webhooks.pending })}</Badge>
          </span>
        }
      />
      {data.webhooks.recentFailures.length === 0 ? (
        <p className="text-[13px] text-ink-3">{t("No undelivered webhook attempts.")}</p>
      ) : (
        <ul className="border-t border-line">
          {data.webhooks.recentFailures.map((failure) => (
            <li key={failure.id} className="flex flex-col gap-1 border-b border-line py-3">
              <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
                <span className="num text-[12.5px] text-ink-2">{failure.event}</span>
                <span className="text-[11.5px] text-ink-3">
                  {failure.statusCode !== null
                    ? t("HTTP {status}", { status: failure.statusCode })
                    : t("No response")}{" "}
                  · {t("{count} attempts", { count: failure.attempts })}
                </span>
              </div>
              {failure.lastError && (
                <p className="text-[12px] leading-snug text-danger">{failure.lastError}</p>
              )}
              <span className="text-[11.5px] text-ink-3">
                {failure.nextAttemptAt
                  ? t("Retrying {when}", { when: formatRelative(failure.nextAttemptAt) })
                  : t("No retry scheduled")}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function IntegrityPanel({ data }: { data: ObservabilitySnapshot }) {
  const last = data.integrity.last;
  return (
    <section id="integrity" aria-labelledby="integrity-heading" className="flex flex-col gap-4">
      <SectionHeader
        id="integrity-heading"
        title={t("Integrity")}
        actions={
          last ? (
            <Badge tone={last.status === "completed" ? (last.findings > 0 ? "danger" : "ok") : "outline"}>
              {last.status}
            </Badge>
          ) : undefined
        }
      />
      {!last ? (
        <p className="text-[13px] text-ink-3">{t("No integrity report has run yet.")}</p>
      ) : (
        <dl className="border-t border-line">
          <div className="flex flex-wrap items-baseline justify-between gap-x-4 border-b border-line py-2.5">
            <dt className="text-[12.5px] text-ink-3">{t("Findings")}</dt>
            <dd className={cn("num text-[12.5px]", last.findings > 0 ? "text-danger" : "text-ink-2")}>
              {last.findings}
            </dd>
          </div>
          <div className="flex flex-wrap items-baseline justify-between gap-x-4 border-b border-line py-2.5">
            <dt className="text-[12.5px] text-ink-3">{t("Finished")}</dt>
            <dd className="num text-[12.5px] text-ink-2">{formatDate(last.finishedAt)}</dd>
          </div>
        </dl>
      )}
      {data.integrity.recentFindings.length > 0 && (
        <ul className="flex flex-col gap-2">
          {data.integrity.recentFindings.map((finding, index) => (
            <li
              key={`${finding.kind}-${finding.targetId ?? index}`}
              className="flex flex-col gap-0.5 border-l-2 border-danger/60 pl-3"
            >
              <span className="text-[12px] text-ink-2">
                {finding.kind} · {finding.targetType}
                {finding.targetId ? ` · ${finding.targetId}` : ""}
              </span>
              {finding.detail && (
                <span className="text-[11.5px] leading-snug text-ink-3">{finding.detail}</span>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function LinksPanel({ data }: { data: ObservabilitySnapshot }) {
  const metricsUrl = `${getApiBase()}/metrics`;
  const linkClass =
    "inline-flex items-center gap-1.5 text-[12.5px] text-ink-2 underline decoration-line-2 underline-offset-4 transition-colors duration-[120ms] hover:text-ink";
  return (
    <section aria-labelledby="links-heading" className="flex flex-col gap-4">
      <SectionHeader
        id="links-heading"
        title={t("Diagnostics")}
        description={t("Prometheus, audit trail and integrity reports.")}
      />
      <div className="flex flex-col gap-3 border-t border-line pt-4 text-[12.5px]">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <a href={metricsUrl} target="_blank" rel="noreferrer" className={linkClass}>
            <Radio className="size-3.5" aria-hidden="true" />
            {t("Metrics endpoint")}
            <ArrowUpRight className="size-3" aria-hidden="true" />
          </a>
          <Badge tone={data.prometheus.tokenProtected ? "signal" : "ok"}>
            {data.prometheus.tokenProtected ? t("Token required") : t("Public scrape")}
          </Badge>
        </div>
        <a href="#recent-events" className={linkClass}>
          <Activity className="size-3.5" aria-hidden="true" />
          {t("Audit trail")}
        </a>
        <a href="#integrity" className={linkClass}>
          <ShieldCheck className="size-3.5" aria-hidden="true" />
          {t("Integrity reports")}
        </a>
        <a href="#storage" className={linkClass}>
          <HardDrive className="size-3.5" aria-hidden="true" />
          {t("Storage roots")}
        </a>
      </div>
    </section>
  );
}

export function ObservabilityScreen() {
  const [window, setWindow] = useState<ObservabilityWindow>("1h");
  const { data, isPending, isError, error, refetch, isFetching, dataUpdatedAt } = useQuery({
    queryKey: ["observability", window],
    queryFn: () => api.adminObservability(window),
    refetchInterval: 30_000,
    staleTime: 10_000,
  });

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Observability")}
        meta={
          <>
            {data && (
              <span className="flex items-center gap-2">
                <span
                  className={cn(
                    "size-1.5 rounded-full",
                    data.database.ok ? "bg-ok" : "bg-danger",
                  )}
                  aria-hidden="true"
                />
                {data.database.ok ? t("Server healthy") : t("Database degraded")}
              </span>
            )}
            {data && (
              <span className="num">
                {t("v{version} · up {uptime}", {
                  version: data.server.version,
                  uptime: formatUptime(data.server.uptimeSeconds),
                })}
              </span>
            )}
            {dataUpdatedAt > 0 && (
              <span>
                {t("Updated {when}", { when: formatRelative(new Date(dataUpdatedAt).toISOString()) })}
              </span>
            )}
          </>
        }
        description={t(
          "How this server is doing right now: traffic, background work, storage and the most recent failures. Refreshes every 30 seconds; the metrics endpoint stays the source of truth for Prometheus.",
        )}
        actions={
          <>
            <Segmented
              value={window}
              onChange={setWindow}
              label={t("Time window")}
              size="sm"
              options={[
                { value: "1h", label: t("1 hour") },
                { value: "24h", label: t("24 hours") },
              ]}
            />
            <IconButton
              label={t("Refresh now")}
              onClick={() => void refetch()}
              loading={isFetching}
              icon={<RefreshCw className="size-4" />}
            />
          </>
        }
      />

      {isError && <ErrorState message={errorMessage(error)} onRetry={() => void refetch()} />}

      {isPending && (
        <div className="mt-8 grid gap-6 md:grid-cols-2" aria-hidden="true">
          {Array.from({ length: 4 }).map((_, index) => (
            <div key={index} className="flex flex-col gap-3 border-t border-line pt-4">
              <Skeleton className="h-3 w-24" />
              <Skeleton className="h-8 w-32" />
              <Skeleton className="h-14 w-full" />
            </div>
          ))}
        </div>
      )}

      {data && (
        <div className="mt-8 flex flex-col gap-12">
          <div className="grid gap-10 lg:grid-cols-3">
            <div className="lg:col-span-2">
              <RequestPanel data={data} />
            </div>
            <ActivePanel data={data} />
          </div>

          <div className="grid gap-10 lg:grid-cols-2">
            <StoragePanel data={data} />
            <div className="flex flex-col gap-12">
              <DatabasePanel data={data} />
              <LinksPanel data={data} />
            </div>
          </div>

          <div className="grid gap-10 lg:grid-cols-2">
            <EventsPanel data={data} />
            <div className="flex flex-col gap-12">
              <WebhooksPanel data={data} />
              <ErrorsPanel data={data} />
              <IntegrityPanel data={data} />
            </div>
          </div>

          <footer className="flex flex-wrap items-center gap-2 border-t border-line pt-4 text-[11.5px] text-ink-3">
            <Database className="size-3.5" aria-hidden="true" />
            {t("Backend {backend} · {size} · probe {latency}", {
              backend: data.database.backend,
              size: formatBytes(data.database.sizeBytes),
              latency: formatMs(data.database.latencyMs),
            })}
            <Webhook className="ml-4 size-3.5" aria-hidden="true" />
            {t("{count} undelivered webhook attempts", { count: data.webhooks.pending })}
            <AlertTriangle className="ml-4 size-3.5" aria-hidden="true" />
            {t("{count} integrity findings", {
              count: data.integrity.last?.findings ?? 0,
            })}
          </footer>
        </div>
      )}
    </div>
  );
}
