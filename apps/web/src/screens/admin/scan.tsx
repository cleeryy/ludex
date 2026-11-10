import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { api } from "@/lib/api";
import { formatRelative } from "@/lib/format";
import { useToast } from "@/state/toasts";
import { PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Skeleton } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

export function ScanScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();

  const { data, isPending } = useQuery({
    queryKey: ["scan"],
    queryFn: () => api.scanStatus(),
    refetchInterval: (query) => (query.state.data?.running ? 1500 : 10_000),
  });

  const start = useMutation({
    mutationFn: () => api.startScan(),
    onSuccess: () => {
      push({ title: t("Scan started"), description: t("New files appear as they are indexed."), tone: "success" });
      void queryClient.invalidateQueries({ queryKey: ["scan"] });
    },
    onError: (error: Error) => push({ title: t("Scan did not start"), description: error.message, tone: "danger" }),
  });

  const counts = [
    [t("Scanned"), data?.scanned ?? 0],
    [t("Added"), data?.added ?? 0],
    [t("Updated"), data?.updated ?? 0],
    [t("Removed"), data?.removed ?? 0],
    [t("Metadata pending"), data?.metadataPending ?? 0],
  ] as const;

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Library scan")}
        meta={
          <>
            <span className="flex items-center gap-2">
              <span className={data?.running ? "size-1.5 rounded-full bg-signal" : "size-1.5 rounded-full bg-ink-3"} aria-hidden="true" />
              {data?.running ? t("Running") : t("Idle")}
            </span>
            {data?.finishedAt && <span>{t("Last finished")} {formatRelative(data.finishedAt)}</span>}
          </>
        }
        description={t("The scanner walks the games folder, indexes new archives and marks removed files as missing. It never deletes metadata or progress.")}
        actions={
          <Button
            variant="primary"
            icon={<RefreshCw className={data?.running ? "size-4 animate-spin" : "size-4"} />}
            loading={start.isPending}
            disabled={data?.running}
            onClick={() => start.mutate()}
          >
            {data?.running ? t("Scanning") : t("Scan now")}
          </Button>
        }
      />

      <section className="mt-8">
        <SectionHeader title={t("This scan")} />
        <div className="grid grid-cols-2 gap-px border border-line bg-line sm:grid-cols-3 lg:grid-cols-5">
          {counts.map(([label, value]) => (
            <div key={label} className="bg-bg px-4 py-5">
              {isPending ? (
                <Skeleton className="h-7 w-10" />
              ) : (
                <p className="num text-[30px] leading-none text-ink">{value}</p>
              )}
              <p className="mt-2 text-[11.5px] text-ink-3">{label}</p>
            </div>
          ))}
        </div>
        <div className="mt-3 flex flex-wrap items-baseline gap-x-6 gap-y-1 font-mono text-[10.5px] text-ink-3">
          <span>{t("Started")} {data?.startedAt ? formatRelative(data.startedAt) : "—"}</span>
          <span>{t("Finished")} {data?.finishedAt ? formatRelative(data.finishedAt) : "—"}</span>
        </div>
      </section>

      {data && data.errors.length > 0 && (
        <section className="mt-10">
          <SectionHeader title={t("Errors")} count={data.errors.length} />
          <div className="border-t border-line">
            {data.errors.map((message, index) => (
              <p key={index} className="flex items-start gap-2 border-b border-line py-2.5 text-[12.5px] text-danger">
                <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
                {message}
              </p>
            ))}
          </div>
        </section>
      )}

      <section className="mt-10 grid gap-8 lg:grid-cols-2">
        <div>
          <SectionHeader title={t("What it does")} />
          <ul className="flex flex-col gap-3 border-t border-line pt-4 text-[13px] leading-relaxed text-ink-2">
            <li className="flex gap-3">
              <span className="mt-[7px] size-1.5 shrink-0 bg-signal" aria-hidden="true" />
              <span>{t("Walks the games folder three levels deep and indexes archives and portable executables.")}</span>
            </li>
            <li className="flex gap-3">
              <span className="mt-[7px] size-1.5 shrink-0 bg-signal" aria-hidden="true" />
              <span>{t("Skips hidden entries, extracted game trees, Redist, Support, Tools and uninstallers.")}</span>
            </li>
            <li className="flex gap-3">
              <span className="mt-[7px] size-1.5 shrink-0 bg-signal" aria-hidden="true" />
              <span>{t("Compares file size and modification time before touching the database, so repeat scans are cheap.")}</span>
            </li>
            <li className="flex gap-3">
              <span className="mt-[7px] size-1.5 shrink-0 bg-signal" aria-hidden="true" />
              <span>{t("Marks missing files instead of deleting them. Progress, bookmarks and metadata survive a move.")}</span>
            </li>
          </ul>
        </div>
        <div>
          <SectionHeader title={t("Schedule")} />
          <div className="border-t border-line pt-4">
            <div className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
              <span className="text-[12.5px] text-ink-3">{t("Automatic scan")}</span>
              <Badge tone="ok">{t("Every 15 minutes")}</Badge>
            </div>
            <div className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
              <span className="text-[12.5px] text-ink-3">{t("After an ingest")}</span>
              <Badge tone="ok">{t("On completion")}</Badge>
            </div>
            <div className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
              <span className="text-[12.5px] text-ink-3">{t("Manual")}</span>
              <span className="text-[12.5px] text-ink-2">{t("This page, or the command palette")}</span>
            </div>
          </div>
          <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
            {t("A scan only stats files. Metadata enrichment runs separately and is rate limited by the provider.")}
          </p>
        </div>
      </section>
    </div>
  );
}
