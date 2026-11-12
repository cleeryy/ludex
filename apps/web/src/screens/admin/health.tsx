import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { AlertTriangle, RefreshCw, ShieldCheck, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatRelative } from "@/lib/format";
import type {
  HealthAction,
  HealthCategory,
  HealthSeverity,
  LibraryHealthActionInput,
  LibraryHealthFinding,
  LibraryHealthMediaGc,
} from "@/lib/types";
import { useIsAdmin } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { EmptyState, ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Progress, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

const DESTRUCTIVE: HealthAction[] = ["remove_version", "remove_orphan_game", "media_gc_apply"];

const SEVERITY_TONE: Record<HealthSeverity, "danger" | "signal" | "outline"> = {
  error: "danger",
  warning: "signal",
  info: "outline",
};

function severityLabel(severity: HealthSeverity): string {
  switch (severity) {
    case "error":
      return t("Error");
    case "warning":
      return t("Warning");
    case "info":
      return t("Info");
  }
}

function categoryLabel(category: HealthCategory): string {
  switch (category) {
    case "missing":
      return t("Missing files");
    case "unavailable":
      return t("Unavailable");
    case "orphan":
      return t("Orphaned");
    case "duplicate":
      return t("Duplicates");
    case "metadata":
      return t("Metadata");
    case "integrity":
      return t("Integrity");
    case "counts":
      return t("Index counts");
  }
}

function actionLabel(action: HealthAction): string {
  switch (action) {
    case "rescan_root":
      return t("Rescan");
    case "refresh_metadata":
      return t("Refresh metadata");
    case "mark_missing":
      return t("Mark missing");
    case "remove_version":
      return t("Remove entry");
    case "remove_orphan_game":
      return t("Remove game");
    case "media_gc_dry_run":
      return t("Preview cleanup");
    case "media_gc_apply":
      return t("Clean up media");
    case "review":
      return t("Review");
  }
}

function actionDescription(action: HealthAction): string {
  switch (action) {
    case "rescan_root":
      return t("Queue a scan of this root. Existing progress and saves are kept.");
    case "refresh_metadata":
      return t("Queue metadata enrichment for this game.");
    case "mark_missing":
      return t("Re-check the file, then mark the version missing. The file is never touched.");
    case "remove_version":
      return t("Delete the database row for this missing version. No file is deleted.");
    case "remove_orphan_game":
      return t("Delete a game that has no versions and no progress, bookmarks or saves.");
    case "media_gc_dry_run":
      return t("Compute what the media cleanup would remove. Nothing is deleted.");
    case "media_gc_apply":
      return t("Delete unreferenced artwork past the grace period. Archive files are never deleted.");
    case "review":
      return t("Open the related game to decide what to do.");
  }
}

function FindingRow({
  finding,
  busy,
  isAdmin,
  onAct,
  onReview,
}: {
  finding: LibraryHealthFinding;
  busy: boolean;
  isAdmin: boolean;
  onAct: (action: HealthAction, finding: LibraryHealthFinding) => void;
  onReview: (finding: LibraryHealthFinding) => void;
}) {
  const actions = finding.actions.filter((action) => action !== "media_gc_apply" || isAdmin);
  return (
    <tr className="align-top">
      <td className="py-3 pe-3">
        <Badge tone={SEVERITY_TONE[finding.severity]}>{severityLabel(finding.severity)}</Badge>
      </td>
      <td className="min-w-0 py-3 pe-4">
        <p className="text-[13.5px] font-medium leading-snug text-ink">{finding.title}</p>
        <p className="measure mt-1 text-[12.5px] leading-relaxed text-ink-2">{finding.detail}</p>
        {finding.path && (
          <code className="mt-1.5 block truncate font-mono text-[10.5px] text-ink-3" title={finding.path}>
            {finding.path}
          </code>
        )}
      </td>
      <td className="hidden py-3 pe-3 md:table-cell">
        <span className="text-[11.5px] text-ink-3">{categoryLabel(finding.category)}</span>
      </td>
      <td className="py-3">
        <div className="flex flex-wrap justify-end gap-1.5">
          {actions.length === 0 && <span className="text-[11.5px] text-ink-3">{t("No action")}</span>}
          {actions.map((action) => {
            const destructive = DESTRUCTIVE.includes(action);
            const disabled = busy || (destructive && !isAdmin);
            return (
              <Button
                key={action}
                size="sm"
                variant={destructive ? "danger" : action === "review" ? "ghost" : "default"}
                disabled={disabled}
                title={destructive && !isAdmin ? t("Administrator only") : actionDescription(action)}
                onClick={() => (action === "review" ? onReview(finding) : onAct(action, finding))}
              >
                {actionLabel(action)}
              </Button>
            );
          })}
        </div>
      </td>
    </tr>
  );
}

export function LibraryHealthScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const isAdmin = useIsAdmin();

  const [severity, setSeverity] = useState<HealthSeverity | "all">("all");
  const [category, setCategory] = useState<HealthCategory | "all">("all");
  const [limit, setLimit] = useState(200);
  const [confirm, setConfirm] = useState<{ action: HealthAction; findingIds: number[] } | null>(null);
  const [gcPreview, setGcPreview] = useState<LibraryHealthMediaGc | null>(null);

  const health = useQuery({
    queryKey: ["library-health", severity, category, limit],
    queryFn: () =>
      api.libraryHealth({
        severity: severity === "all" ? undefined : severity,
        category: category === "all" ? undefined : category,
        limit,
      }),
    refetchInterval: (query) => (query.state.data?.report?.status === "running" ? 1500 : 30_000),
  });

  const report = health.data?.report ?? null;
  const findings = health.data?.findings ?? [];
  const total = health.data?.findingsTotal ?? 0;

  const categories = useMemo(() => {
    const counts = new Map<HealthCategory, number>();
    for (const entry of report?.byCategory ?? []) counts.set(entry.category, entry.count);
    return [...counts.entries()].sort((left, right) => right[1] - left[1]);
  }, [report]);

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["library-health"] });
  };

  const start = useMutation({
    mutationFn: () => api.startLibraryHealth(),
    onSuccess: () => {
      push({ title: t("Health check started"), description: t("The report updates while it runs.") });
      invalidate();
    },
    onError: (error: Error) =>
      push({ title: t("The health check did not start"), description: error.message, tone: "danger" }),
  });

  const act = useMutation({
    mutationFn: (input: LibraryHealthActionInput) => api.libraryHealthAction(input),
    onSuccess: (result) => {
      if (result.mediaGc) setGcPreview(result.mediaGc);
      const description =
        result.applied > 0
          ? t("{count} finding(s) repaired.", { count: result.applied })
          : result.errors[0] ?? t("Nothing needed to change.");
      push({
        title: t("Repair applied"),
        description,
        tone: result.applied > 0 ? "success" : "default",
      });
      setConfirm(null);
      invalidate();
    },
    onError: (error: Error) =>
      push({ title: t("Repair did not apply"), description: error.message, tone: "danger" }),
  });

  const run = (action: HealthAction, finding?: LibraryHealthFinding) => {
    const input: LibraryHealthActionInput = {
      action,
      findingIds: finding ? [finding.id] : undefined,
    };
    if (DESTRUCTIVE.includes(action)) {
      setConfirm({ action, findingIds: input.findingIds ?? [] });
      return;
    }
    act.mutate(input);
  };

  const review = (finding: LibraryHealthFinding) => {
    if (finding.gameId) navigate(`/game/${finding.gameId}`);
    else push({ title: finding.title, description: finding.detail });
  };

  const busy = act.isPending || start.isPending;
  const running = report?.status === "running";
  const healthy = report?.status === "completed" && report.findingsCount === 0;

  const summary = report?.counts;

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Library health")}
        meta={
          <>
            <span className="flex items-center gap-2">
              <span
                className={cn(
                  "size-1.5 rounded-full",
                  running
                    ? "animate-pulse bg-signal"
                    : report?.status === "failed"
                      ? "bg-danger"
                      : healthy
                        ? "bg-ok"
                        : "bg-ink-3",
                )}
                aria-hidden="true"
              />
              {running
                ? t("Checking")
                : report?.status === "failed"
                  ? t("Last check failed")
                  : report?.finishedAt
                    ? t("Last checked {time}", { time: formatRelative(report.finishedAt) })
                    : t("Never checked")}
            </span>
            {report && <span className="num">{t("{count} findings", { count: report.findingsCount })}</span>}
            {report?.trigger === "scheduled" && <Badge tone="outline">{t("Nightly")}</Badge>}
          </>
        }
        description={t(
          "One report for missing files, orphaned artwork, duplicates and metadata gaps. Repairs are explicit, audited, and never delete an archive file.",
        )}
        actions={
          <>
            <Button
              variant="default"
              icon={<Trash2 className="size-4" />}
              disabled={busy}
              onClick={() => run("media_gc_dry_run")}
            >
              {t("Preview media cleanup")}
            </Button>
            <Button
              variant="primary"
              icon={<RefreshCw className={running ? "size-4 animate-spin" : "size-4"} />}
              loading={start.isPending}
              disabled={running || busy}
              onClick={() => start.mutate()}
            >
              {running ? t("Checking") : t("Run health check")}
            </Button>
          </>
        }
      />

      {health.isError && (
        <ErrorState
          message={errorMessage(health.error)}
          onRetry={() => void health.refetch()}
          title={t("The health report did not load")}
        />
      )}

      {running && report && (
        <section className="mt-6 border border-line bg-surface/40 p-4" aria-live="polite">
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <p className="text-[13px] text-ink">{t("Scanning the library")}</p>
            <span className="num text-[12px] text-ink-3">{report.progress}%</span>
          </div>
          <Progress
            className="mt-2"
            value={report.progress}
            label={t("Health check progress")}
          />
        </section>
      )}

      {report?.status === "failed" && report.error && (
        <div className="mt-6 flex items-start gap-3 border border-danger/40 bg-danger/5 p-4" role="alert">
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden="true" />
          <div>
            <p className="text-[13px] font-medium text-ink">{t("The last health check failed")}</p>
            <p className="mt-1 text-[12.5px] text-ink-2">{report.error}</p>
          </div>
        </div>
      )}

      {report && summary && (
        <section className="mt-8" aria-label={t("Findings by severity")}>
          <div className="grid grid-cols-2 gap-px border border-line bg-line sm:grid-cols-4">
            {(
              [
                [t("Errors"), summary.error, "text-danger"],
                [t("Warnings"), summary.warning, "text-signal-text"],
                [t("Info"), summary.info, "text-ink"],
                [t("Total"), summary.total, "text-ink"],
              ] as const
            ).map(([label, value, tone]) => (
              <div key={label} className="bg-bg px-4 py-5">
                <p className={cn("num text-[30px] leading-none", tone)}>{value}</p>
                <p className="mt-2 text-[11.5px] text-ink-3">{label}</p>
              </div>
            ))}
          </div>
        </section>
      )}

      {gcPreview && (
        <section className="mt-6 border border-line bg-surface/40 p-4" aria-live="polite">
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <p className="text-[13px] text-ink">
              {gcPreview.dryRun ? t("Media cleanup preview") : t("Media cleanup applied")}
            </p>
            <Badge tone={gcPreview.dryRun ? "outline" : "ok"}>
              {gcPreview.dryRun ? t("Dry run") : t("Applied")}
            </Badge>
          </div>
          <p className="mt-1 text-[12.5px] text-ink-2">
            {gcPreview.dryRun
              ? t(
                  "{files} unreferenced file(s), {bytes}, would be removed after the grace period.",
                  { files: gcPreview.orphanFiles, bytes: formatBytes(gcPreview.orphanBytes) },
                )
              : t("{files} file(s), {bytes}, removed.", {
                  files: gcPreview.deletedFiles,
                  bytes: formatBytes(gcPreview.deletedBytes),
                })}
          </p>
          {gcPreview.candidates.length > 0 && (
            <p className="mt-2 truncate font-mono text-[10.5px] text-ink-3" title={gcPreview.candidates.join(", ")}>
              {gcPreview.candidates.join(", ")}
            </p>
          )}
          <div className="mt-3 flex flex-wrap gap-2">
            {gcPreview.dryRun && isAdmin && gcPreview.orphanFiles > 0 && (
              <Button
                variant="danger"
                disabled={busy}
                onClick={() => setConfirm({ action: "media_gc_apply", findingIds: [] })}
              >
                {t("Clean up media")}
              </Button>
            )}
            <Button variant="ghost" onClick={() => setGcPreview(null)}>
              {t("Dismiss")}
            </Button>
          </div>
        </section>
      )}

      {report && (
        <section className="mt-8" aria-labelledby="health-findings">
          <SectionHeader
            id="health-findings"
            title={t("Findings")}
            count={report.findingsCount}
            description={t("Filter by severity or category, then apply one safe repair per row.")}
          />

          <div className="flex flex-col gap-3 border-y border-line py-3">
            <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label={t("Filter by severity")}>
              {(["all", "error", "warning", "info"] as const).map((value) => (
                <Button
                  key={value}
                  size="sm"
                  variant={severity === value ? "primary" : "ghost"}
                  aria-pressed={severity === value}
                  onClick={() => setSeverity(value)}
                >
                  {value === "all" ? t("All severities") : severityLabel(value)}
                </Button>
              ))}
            </div>
            <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label={t("Filter by category")}>
              <Button
                size="sm"
                variant={category === "all" ? "primary" : "ghost"}
                aria-pressed={category === "all"}
                onClick={() => setCategory("all")}
              >
                {t("All categories")}
              </Button>
              {categories.map(([id, count]) => (
                <Button
                  key={id}
                  size="sm"
                  variant={category === id ? "primary" : "ghost"}
                  aria-pressed={category === id}
                  onClick={() => setCategory(id)}
                >
                  {categoryLabel(id)} <span className="num ms-1 text-ink-3">{count}</span>
                </Button>
              ))}
            </div>
          </div>

          {health.isPending && (
            <div className="mt-4 space-y-3">
              {Array.from({ length: 5 }).map((_, index) => (
                <Skeleton key={index} className="h-16 w-full" />
              ))}
            </div>
          )}

          {!health.isPending && findings.length > 0 && (
            <div className="mt-4 overflow-x-auto">
              <table className="w-full border-collapse text-start">
                <caption className="sr-only">{t("Library health findings")}</caption>
                <thead>
                  <tr className="border-b border-line text-start font-mono text-[10.5px] uppercase tracking-[0.08em] text-ink-3">
                    <th scope="col" className="pb-2 pe-3 text-start font-normal">{t("Severity")}</th>
                    <th scope="col" className="pb-2 pe-4 text-start font-normal">{t("Finding")}</th>
                    <th scope="col" className="hidden pb-2 pe-3 text-start font-normal md:table-cell">{t("Category")}</th>
                    <th scope="col" className="pb-2 text-end font-normal">{t("Actions")}</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-line">
                  {findings.map((finding) => (
                    <FindingRow
                      key={finding.id}
                      finding={finding}
                      busy={busy}
                      isAdmin={isAdmin}
                      onAct={(action, row) => run(action, row)}
                      onReview={review}
                    />
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {!health.isPending && findings.length > 0 && total > findings.length && (
            <div className="mt-4 flex justify-center">
              <Button variant="default" onClick={() => setLimit(500)}>
                {t("Show more ({shown} of {total})", { shown: findings.length, total })}
              </Button>
            </div>
          )}

          {!health.isPending && findings.length === 0 && report.status === "completed" && report.findingsCount === 0 && (
            <EmptyState
              className="mt-4"
              title={t("Everything looks healthy")}
              description={t(
                "No missing archives, orphaned files, duplicates or metadata gaps were found. Run the check again after changing the library.",
              )}
            >
              <Button
                variant="primary"
                icon={<ShieldCheck className="size-4" />}
                onClick={() => start.mutate()}
              >
                {t("Run health check")}
              </Button>
            </EmptyState>
          )}

          {!health.isPending && findings.length === 0 && !healthy && (
            <p className="mt-4 text-[13px] text-ink-3">{t("No finding matches these filters.")}</p>
          )}
        </section>
      )}

      {!report && !health.isPending && (
        <EmptyState
          className="mt-8"
          title={t("No health report yet")}
          description={t(
            "Run a check to see missing archives, unavailable roots, orphaned artwork, duplicates and metadata gaps in one place.",
          )}
        >
          <Button
            variant="primary"
            icon={<RefreshCw className="size-4" />}
            loading={start.isPending}
            onClick={() => start.mutate()}
          >
            {t("Run health check")}
          </Button>
        </EmptyState>
      )}

      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(open) => !open && setConfirm(null)}
        title={confirm ? actionLabel(confirm.action) : t("Confirm repair")}
        description={confirm ? actionDescription(confirm.action) : ""}
        confirmLabel={t("Apply repair")}
        loading={act.isPending}
        onConfirm={() => {
          if (!confirm) return;
          act.mutate({
            action: confirm.action,
            findingIds: confirm.findingIds,
            confirm: true,
          });
        }}
      />
    </div>
  );
}
