import { useEffect, useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  FolderX,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatRelative, formatYear } from "@/lib/format";
import {
  ADMIN_PAGE_SIZE,
  ADMIN_STATUS_VALUES,
  ADMIN_SORT_VALUES,
  confirmationMatches,
  readAdminGameFilters,
  rowSelectionLabel,
  sortLabelFor,
  statusLabel,
  statusTone,
  toAdminGamesQuery,
  withFilter,
  writeAdminGameFilters,
  type AdminGameFilters,
} from "@/lib/admin-games";
import type {
  AdminBulkAction,
  AdminBulkPreview,
  AdminGameRow,
  AdminGameStatus,
} from "@/lib/types";
import { useToast } from "@/state/toasts";
import { EmptyState, ErrorState, PageHeader } from "@/components/page";
import { Badge, Button, Checkbox, Field, Input, Skeleton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";
import { t, tn } from "@/lib/i18n";

const DESTRUCTIVE_ACTIONS: AdminBulkAction[] = [
  "remove_from_library",
  "delete_archives",
  "delete_missing_versions",
];

function actionLabel(action: AdminBulkAction): string {
  switch (action) {
    case "remove_from_library":
      return t("Remove from library");
    case "delete_archives":
      return t("Delete game and archives");
    case "delete_missing_versions":
      return t("Delete missing versions");
    case "refresh_metadata":
      return t("Refresh metadata");
  }
}

function actionWarning(action: AdminBulkAction): string {
  switch (action) {
    case "remove_from_library":
      return t("Deletes the database rows only. Archive files stay on disk and a scan can re-add the game. Progress, bookmarks, achievements and cloud saves for these games are deleted.");
    case "delete_archives":
      return t("Deletes the archive files below the library root, then the database rows. Progress, bookmarks, achievements and cloud saves for these games are deleted.");
    case "delete_missing_versions":
      return t("Deletes only the version rows whose archive is already missing. No file is touched, and the game stays in the library.");
    case "refresh_metadata":
      return t("Queues a metadata refresh for every selected game. Nothing is deleted.");
  }
}

function statusBadge(status: AdminGameStatus) {
  return <Badge tone={statusTone(status)}>{statusLabel(status)}</Badge>;
}

function SortButton({
  filters,
  onChange,
}: {
  filters: AdminGameFilters;
  onChange: (patch: Partial<AdminGameFilters>) => void;
}) {
  return (
    <Select
      label={t("Sort games")}
      value={filters.sort}
      onChange={(value) => onChange({ sort: value })}
      options={ADMIN_SORT_VALUES.map((value) => ({ value, label: sortLabelFor(value) }))}
      className="h-9 text-[12.5px]"
    />
  );
}

function Drawer({
  row,
  onClose,
  onAction,
}: {
  row: AdminGameRow;
  onClose: () => void;
  onAction: (action: AdminBulkAction, ids: number[]) => void;
}) {
  const navigate = useNavigate();
  const detail = useQuery({
    queryKey: ["game", row.id],
    queryFn: () => api.getGame(row.id),
    enabled: true,
  });

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={row.title}
      description={[formatYear(row.releaseDate), row.kind === "rom" ? row.platform?.toUpperCase() : null, row.providerSlug]
        .filter(Boolean)
        .join(" · ")}
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t("Close")}
          </Button>
          <Button
            variant="default"
            icon={<ExternalLink className="size-3.5" />}
            onClick={() => navigate(`/game/${row.id}`)}
          >
            {t("Open game page")}
          </Button>
          <Button
            variant="danger"
            icon={<Trash2 className="size-3.5" />}
            onClick={() => onAction("remove_from_library", [row.id])}
          >
            {t("Remove from library")}
          </Button>
          <Button
            variant="danger"
            icon={<FolderX className="size-3.5" />}
            onClick={() => onAction("delete_archives", [row.id])}
          >
            {t("Delete archives")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        <dl className="grid gap-x-6 gap-y-2 text-[12.5px] sm:grid-cols-2">
          <div className="flex items-baseline justify-between gap-3 border-b border-line pb-2">
            <dt className="text-ink-3">{t("Status")}</dt>
            <dd>{statusBadge(row.status)}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-3 border-b border-line pb-2">
            <dt className="text-ink-3">{t("Available versions")}</dt>
            <dd className="num">{row.availableVersionCount}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-3 border-b border-line pb-2">
            <dt className="text-ink-3">{t("Missing versions")}</dt>
            <dd className="num">{row.missingVersionCount}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-3 border-b border-line pb-2">
            <dt className="text-ink-3">{t("Total size")}</dt>
            <dd className="num">{formatBytes(row.totalSize)}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-3 border-b border-line pb-2">
            <dt className="text-ink-3">{t("Default version")}</dt>
            <dd className="num">{row.defaultVersionId ?? "—"}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-3 border-b border-line pb-2">
            <dt className="text-ink-3">{t("Installed reports")}</dt>
            <dd className="num">
              {row.installedReports}
              {row.lastInstalledAt ? ` · ${formatRelative(row.lastInstalledAt)}` : ""}
            </dd>
          </div>
          <div className="flex items-baseline justify-between gap-3 border-b border-line pb-2">
            <dt className="text-ink-3">{t("Added")}</dt>
            <dd>{formatRelative(row.createdAt)}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-3 border-b border-line pb-2">
            <dt className="text-ink-3">{t("Updated")}</dt>
            <dd>{formatRelative(row.updatedAt)}</dd>
          </div>
        </dl>

        <div>
          <p className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{t("Archive files")}</p>
          {detail.isPending ? (
            <Skeleton className="mt-2 h-16" />
          ) : detail.isError ? (
            <p className="mt-2 text-[12.5px] text-danger">{errorMessage(detail.error)}</p>
          ) : (
            <div className="mt-2 flex flex-col divide-y divide-line border-y border-line">
              {[...(detail.data?.versions ?? []), ...(detail.data?.missingVersions ?? [])].map((version) => (
                <div key={version.id} className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 py-2 text-[12px]">
                  <span className="min-w-0 break-all font-mono text-[11.5px]">{version.fileName}</span>
                  <span className="flex items-center gap-2">
                    {version.missing ? (
                      <Badge tone="danger">{t("Missing")}</Badge>
                    ) : version.rootUnavailable ? (
                      <Badge tone="signal">{t("Root offline")}</Badge>
                    ) : (
                      <Badge tone="outline">{formatBytes(version.size)}</Badge>
                    )}
                  </span>
                </div>
              ))}
              {(detail.data?.versions.length ?? 0) + (detail.data?.missingVersions?.length ?? 0) === 0 && (
                <p className="py-3 text-[12.5px] text-ink-3">{t("No version rows.")}</p>
              )}
            </div>
          )}
        </div>

        <div className="flex flex-wrap gap-2">
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            onClick={() => onAction("refresh_metadata", [row.id])}
          >
            {t("Refresh metadata")}
          </Button>
          <Button
            variant="default"
            size="sm"
            onClick={() => onAction("delete_missing_versions", [row.id])}
          >
            {t("Delete missing versions")}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

export function AdminGamesScreen() {
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { push } = useToast();
  const filters = readAdminGameFilters(params);
  const query = toAdminGamesQuery(filters);

  const [search, setSearch] = useState(filters.q);
  useEffect(() => setSearch(filters.q), [filters.q]);

  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [drawer, setDrawer] = useState<AdminGameRow | null>(null);
  const [pending, setPending] = useState<{
    action: AdminBulkAction;
    ids: number[];
    preview: AdminBulkPreview | null;
  } | null>(null);
  const [typed, setTyped] = useState("");
  const [jobId, setJobId] = useState<string | null>(null);

  const list = useQuery({
    queryKey: ["admin-games", query],
    queryFn: () => api.adminGames(query),
  });

  // A filter change invalidates any selection from the previous result set.
  // Depend on the serialised query, not the URLSearchParams identity.
  const paramString = params.toString();
  useEffect(() => {
    setSelected(new Set());
  }, [paramString]);

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["admin-games"] });
    void queryClient.invalidateQueries({ queryKey: ["games"] });
    void queryClient.invalidateQueries({ queryKey: ["games-missing-count"] });
    void queryClient.invalidateQueries({ queryKey: ["stats"] });
  };

  const run = useMutation({
    mutationFn: (body: { ids: number[]; action: AdminBulkAction }) =>
      api.bulkAdminGames({ ...body, confirm: true }),
    onSuccess: (response, variables) => {
      invalidate();
      if (response.jobId) {
        setJobId(response.jobId);
        setPending(null);
        return;
      }
      const { succeeded, failed, files, bytes } = response.totals;
      push({
        title:
          failed > 0
            ? t("{succeeded} of {games} games updated", { succeeded, games: response.totals.games })
            : t("{count} games updated", { count: succeeded }),
        description: [
          files > 0 ? tn(files, "1 file deleted", "{count} files deleted") : null,
          bytes > 0 ? formatBytes(bytes) : null,
          failed > 0 ? t("{count} failed; the others were not rolled back.", { count: failed }) : null,
          actionLabel(variables.action),
        ]
          .filter(Boolean)
          .join(" · "),
        tone: failed > 0 ? "danger" : "success",
      });
      setPending(null);
      // Drop the rows that succeeded; failed ids stay selected for a retry.
      const succeededIds = new Set(
        response.results.filter((result) => result.ok).map((result) => result.gameId),
      );
      setSelected((current) => new Set([...current].filter((id) => !succeededIds.has(id))));
    },
    onError: (error) =>
      push({ title: t("The bulk action failed"), description: errorMessage(error), tone: "danger" }),
  });

  const preview = useMutation({
    mutationFn: (body: { ids: number[]; action: AdminBulkAction }) =>
      api.bulkAdminGames({ ...body, dryRun: true }),
    onError: (error) =>
      push({ title: t("Could not build the preview"), description: errorMessage(error), tone: "danger" }),
  });

  const job = useQuery({
    queryKey: ["admin-bulk-job", jobId],
    queryFn: () => api.adminGamesJob(jobId!),
    enabled: Boolean(jobId),
    refetchInterval: (queryState) =>
      queryState.state.data?.status === "running" ? 800 : false,
  });

  useEffect(() => {
    const report = job.data;
    if (!report || report.status === "running") return;
    invalidate();
    push({
      title:
        report.failed > 0
          ? t("Bulk deletion finished with {count} failures", { count: report.failed })
          : t("Bulk deletion finished"),
      description: t("{done} of {total} games · {files} files · {bytes}", {
        done: report.done,
        total: report.total,
        files: report.results.reduce((sum, result) => sum + result.deletedFiles, 0),
        bytes: formatBytes(report.results.reduce((sum, result) => sum + result.freedBytes, 0)),
      }),
      tone: report.failed > 0 ? "danger" : "success",
    });
    setJobId(null);
    setSelected(new Set());
    // Run once per completed report; the query is disabled again afterwards.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [job.data?.status, job.data?.id]);

  const update = (patch: Partial<AdminGameFilters>) => {
    setParams(writeAdminGameFilters(withFilter(filters, patch)), { replace: true });
  };

  const startAction = (action: AdminBulkAction, ids: number[]) => {
    if (ids.length === 0) return;
    setDrawer(null);
    if (!DESTRUCTIVE_ACTIONS.includes(action)) {
      run.mutate({ ids, action });
      return;
    }
    setTyped("");
    setPending({ action, ids, preview: null });
    preview.mutate(
      { ids, action },
      {
        onSuccess: (response) =>
          setPending((current) => (current ? { ...current, preview: response.preview } : current)),
      },
    );
  };

  const rows = list.data?.items ?? [];
  const counts = list.data?.counts;
  const total = list.data?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / (list.data?.limit ?? ADMIN_PAGE_SIZE)));
  const allSelected = rows.length > 0 && rows.every((row) => selected.has(row.id));
  const toggle = (id: number) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const statusTabs = useMemo(
    () =>
      ADMIN_STATUS_VALUES.map((value) => ({
        value,
        label: statusLabel(value),
        count:
          value === "all"
            ? counts?.total
            : value === "available"
              ? counts?.available
              : value === "partial"
                ? counts?.partiallyMissing
                : counts?.missing,
      })),
    [counts],
  );

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Games")}
        meta={
          <>
            <span className="num">{tn(total, "1 game", "{count} games")}</span>
            {counts && (
              <>
                <span className="num">{t("{count} missing", { count: counts.missing })}</span>
                <span className="num">{t("{count} partially missing", { count: counts.partiallyMissing })}</span>
              </>
            )}
          </>
        }
        description={t("Find, repair and remove any game in the library. File deletions never leave the resolved library root.")}
        actions={
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            onClick={() => {
              void api
                .startScan()
                .then((result) =>
                  push({
                    title: result.started ? t("Rescan queued") : t("A scan is already running"),
                    description: t("The library list refreshes when the scan finishes."),
                  }),
                )
                .catch((cause: Error) =>
                  push({ title: t("Could not start the scan"), description: cause.message, tone: "danger" }),
                );
            }}
          >
            {t("Rescan library")}
          </Button>
        }
      />

      <div className="mt-5 flex flex-wrap items-center gap-2 border-b border-line pb-3">
        {statusTabs.map((tab) => (
          <button
            key={tab.value}
            type="button"
            aria-pressed={filters.status === tab.value}
            onClick={() => update({ status: tab.value })}
            className={cn(
              "inline-flex h-8 items-center gap-2 rounded-[2px] border px-2.5 font-mono text-[10.5px] uppercase tracking-[0.08em] transition-colors",
              filters.status === tab.value
                ? "border-signal/60 bg-signal/12 text-signal-text"
                : "border-line-2 text-ink-3 hover:border-ink-3 hover:text-ink-2",
            )}
          >
            {tab.label}
            {tab.count !== undefined && <span className="num text-[10px] opacity-70">{tab.count}</span>}
          </button>
        ))}
        <form
          className="ms-auto flex min-w-[220px] flex-1 items-center gap-2 sm:max-w-[320px]"
          onSubmit={(event) => {
            event.preventDefault();
            update({ q: search });
          }}
        >
          <div className="relative flex-1">
            <Search className="pointer-events-none absolute start-2.5 top-1/2 size-3.5 -translate-y-1/2 text-ink-3" aria-hidden="true" />
            <Input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              onBlur={() => {
                if (search !== filters.q) update({ q: search });
              }}
              placeholder={t("Search titles")}
              aria-label={t("Search titles")}
              className="h-9 ps-8 text-[12.5px]"
            />
          </div>
        </form>
        <div className="w-[130px]">
          <Select
            label={t("Filter by kind")}
            value={filters.kind}
            onChange={(value) => update({ kind: value })}
            options={[
              { value: "", label: t("All kinds") },
              { value: "game", label: t("Games") },
              { value: "rom", label: t("ROMs") },
            ]}
            className="h-9 text-[12.5px]"
          />
        </div>
        <SortButton filters={filters} onChange={update} />
        <Button
          variant="ghost"
          size="sm"
          onClick={() => update({ order: filters.order === "asc" ? "desc" : "asc" })}
          aria-label={filters.order === "asc" ? t("Ascending") : t("Descending")}
        >
          {filters.order === "asc" ? t("A–Z") : t("Z–A")}
        </Button>
      </div>

      {selected.size > 0 && (
        <div className="sticky top-0 z-20 mt-3 flex flex-wrap items-center gap-2 border border-signal/40 bg-signal/8 px-3 py-2">
          <span className="font-mono text-[10.5px] uppercase tracking-[0.1em] text-signal-text">
            {rowSelectionLabel(rows, selected)}
          </span>
          <Button variant="ghost" size="sm" onClick={() => setSelected(new Set())}>
            {t("Clear")}
          </Button>
          <div className="ms-auto flex flex-wrap items-center gap-2">
            <Button
              variant="default"
              size="sm"
              icon={<RefreshCw className="size-3.5" />}
              onClick={() => startAction("refresh_metadata", [...selected])}
            >
              {t("Refresh metadata")}
            </Button>
            <Button
              variant="default"
              size="sm"
              onClick={() => startAction("delete_missing_versions", [...selected])}
            >
              {t("Delete missing versions")}
            </Button>
            <Button
              variant="danger"
              size="sm"
              icon={<Trash2 className="size-3.5" />}
              onClick={() => startAction("remove_from_library", [...selected])}
            >
              {t("Remove from library")}
            </Button>
            <Button
              variant="danger"
              size="sm"
              icon={<FolderX className="size-3.5" />}
              onClick={() => startAction("delete_archives", [...selected])}
            >
              {t("Delete archives")}
            </Button>
          </div>
        </div>
      )}

      {list.isPending ? (
        <div className="mt-4 flex flex-col gap-2">
          {Array.from({ length: 6 }).map((_, index) => (
            <Skeleton key={index} className="h-11" />
          ))}
        </div>
      ) : list.isError ? (
        <div className="mt-4">
          <ErrorState
            message={errorMessage(list.error)}
            onRetry={() => void list.refetch()}
          />
        </div>
      ) : rows.length === 0 ? (
        <EmptyState
          className="mt-6"
          title={filters.status === "available" && !filters.q ? t("No games in the library") : t("No games match these filters")}
          description={
            filters.status === "available" && !filters.q
              ? t("Add archives to a library root and run a scan. Games whose file was deleted outside Ludex stay in the database under the Missing tab.")
              : t("Adjust the search or the status filter. The counts above still reflect the other filters.")
          }
        >
          {filters.status !== "available" || filters.q ? (
            <Button
              variant="default"
              onClick={() =>
                setParams(
                  writeAdminGameFilters(
                    withFilter(filters, { q: "", status: "available", provider: "", kind: "" }),
                  ),
                  { replace: true },
                )
              }
            >
              {t("Clear filters")}
            </Button>
          ) : (
            <Button variant="default" onClick={() => navigate("/admin/scan")}>
              {t("Open library scan")}
            </Button>
          )}
        </EmptyState>
      ) : (
        <div className="mt-4 overflow-x-auto">
          <table className="w-full min-w-[880px] border-collapse text-left text-[12.5px]">
            <thead className="sticky top-0 z-10 bg-bg">
              <tr className="border-b border-line-2 font-mono text-[10px] uppercase tracking-[0.1em] text-ink-3">
                <th className="w-8 py-2 pe-2">
                  <Checkbox
                    checked={allSelected}
                    onCheckedChange={(checked) =>
                      setSelected(checked ? new Set(rows.map((row) => row.id)) : new Set())
                    }
                    label={t("Select every game on this page")}
                  />
                </th>
                <th className="py-2 pe-3">{t("Title")}</th>
                <th className="py-2 pe-3">{t("Kind")}</th>
                <th className="py-2 pe-3">{t("Provider")}</th>
                <th className="py-2 pe-3 text-end">{t("Versions")}</th>
                <th className="py-2 pe-3 text-end">{t("Size")}</th>
                <th className="py-2 pe-3 text-end">{t("Installed")}</th>
                <th className="py-2 pe-3">{t("Updated")}</th>
                <th className="w-20 py-2 text-end">{t("Actions")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.id} className={cn("border-b border-line align-middle transition-colors hover:bg-surface/50", selected.has(row.id) && "bg-signal/5")}>
                  <td className="py-1.5 pe-2">
                    <Checkbox
                      checked={selected.has(row.id)}
                      onCheckedChange={() => toggle(row.id)}
                      label={t("Select {title}", { title: row.title })}
                    />
                  </td>
                  <td className="max-w-[320px] py-1.5 pe-3">
                    <button
                      type="button"
                      className="flex min-w-0 flex-col text-left"
                      onClick={() => setDrawer(row)}
                    >
                      <span className="truncate font-medium text-ink hover:underline">{row.title}</span>
                      <span className="flex items-center gap-2 text-[11px] text-ink-3">
                        {formatYear(row.releaseDate) ?? "—"}
                        {statusBadge(row.status)}
                      </span>
                    </button>
                  </td>
                  <td className="py-1.5 pe-3 font-mono text-[10.5px] uppercase tracking-[0.06em] text-ink-2">
                    {row.kind === "rom" ? row.platform ?? "rom" : t("Game")}
                  </td>
                  <td className="py-1.5 pe-3 text-ink-2">{row.providerSlug ?? "—"}</td>
                  <td className="py-1.5 pe-3 text-end">
                    <span className="num">{row.availableVersionCount}</span>
                    {row.missingVersionCount > 0 && (
                      <span className="num ms-1 text-danger">+{row.missingVersionCount}</span>
                    )}
                  </td>
                  <td className="num py-1.5 pe-3 text-end text-ink-2">{formatBytes(row.totalSize)}</td>
                  <td className="num py-1.5 pe-3 text-end text-ink-2">{row.installedReports}</td>
                  <td className="py-1.5 pe-3 text-ink-3">{formatRelative(row.updatedAt)}</td>
                  <td className="py-1.5 text-end">
                    <Button variant="ghost" size="sm" onClick={() => setDrawer(row)}>
                      {t("Details")}
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="mt-4 flex flex-wrap items-center justify-between gap-3 text-[12px] text-ink-3">
        <span className="num">
          {t("Page {page} of {pages}", { page: filters.page, pages })}
        </span>
        <div className="flex items-center gap-2">
          <Button
            variant="default"
            size="sm"
            icon={<ChevronLeft className="rtl:-scale-x-100 size-3.5" />}
            disabled={filters.page <= 1}
            onClick={() => update({ page: filters.page - 1 })}
          >
            {t("Previous")}
          </Button>
          <Button
            variant="default"
            size="sm"
            disabled={filters.page >= pages}
            onClick={() => update({ page: filters.page + 1 })}
          >
            {t("Next")}
            <ChevronRight className="rtl:-scale-x-100 ms-1 size-3.5" aria-hidden="true" />
          </Button>
        </div>
      </div>

      {drawer && <Drawer row={drawer} onClose={() => setDrawer(null)} onAction={startAction} />}

      <Dialog
        open={pending !== null}
        onOpenChange={(open) => {
          if (!open) {
            setPending(null);
            setTyped("");
          }
        }}
        title={pending ? actionLabel(pending.action) : t("Bulk action")}
        description={pending ? actionWarning(pending.action) : undefined}
        size="md"
        footer={
          <>
            <Button variant="ghost" onClick={() => setPending(null)}>
              {t("Cancel")}
            </Button>
            <Button
              variant={pending?.action === "delete_archives" ? "danger" : "primary"}
              loading={run.isPending}
              disabled={!pending?.preview || !confirmationMatches(typed, pending.preview.games)}
              onClick={() => pending && run.mutate({ ids: pending.ids, action: pending.action })}
            >
              {pending ? actionLabel(pending.action) : t("Confirm")}
            </Button>
          </>
        }
      >
        {pending && (
          <div className="flex flex-col gap-4">
            {pending.action === "delete_archives" && (
              <p className="flex items-start gap-2 border border-danger/40 bg-danger/10 p-3 text-[12.5px] text-danger">
                <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
                {t("Only files that resolve below a library root are deleted. Symlinks and paths outside the root are refused, and a file that is already gone is not an error.")}
              </p>
            )}
            {preview.isPending || !pending.preview ? (
              <Skeleton className="h-24" />
            ) : (
              <>
                <dl className="grid grid-cols-3 gap-3">
                  <div className="border border-line p-3">
                    <dt className="font-mono text-[9.5px] uppercase tracking-[0.1em] text-ink-3">{t("Games")}</dt>
                    <dd className="num mt-1 text-[22px]">{pending.preview.games}</dd>
                  </div>
                  <div className="border border-line p-3">
                    <dt className="font-mono text-[9.5px] uppercase tracking-[0.1em] text-ink-3">{t("Files")}</dt>
                    <dd className="num mt-1 text-[22px]">{pending.preview.files}</dd>
                  </div>
                  <div className="border border-line p-3">
                    <dt className="font-mono text-[9.5px] uppercase tracking-[0.1em] text-ink-3">{t("Size")}</dt>
                    <dd className="num mt-1 text-[22px]">{formatBytes(pending.preview.bytes)}</dd>
                  </div>
                </dl>
                {pending.preview.items.some((item) => !item.title) && (
                  <p className="text-[12px] text-danger">
                    {t("{count} selected games no longer exist and will be reported as failures.", {
                      count: pending.preview.items.filter((item) => !item.title).length,
                    })}
                  </p>
                )}
                <div className="max-h-44 overflow-y-auto border-y border-line">
                  {pending.preview.items.map((item) => (
                    <div key={item.gameId} className="flex items-baseline justify-between gap-3 border-b border-line py-1.5 text-[12px] last:border-0">
                      <span className="min-w-0 truncate">{item.title || `#${item.gameId}`}</span>
                      <span className="num shrink-0 text-ink-3">
                        {t("{files} files · {size}", { files: item.files, size: formatBytes(item.bytes) })}
                      </span>
                    </div>
                  ))}
                </div>
                <Field
                  label={t("Type {count} to confirm", { count: pending.preview.games })}
                  hint={t("This many games will be changed. Nothing runs until the count matches.")}
                >
                  <Input
                    value={typed}
                    onChange={(event) => setTyped(event.target.value)}
                    inputMode="numeric"
                    autoComplete="off"
                    placeholder={String(pending.preview.games)}
                  />
                </Field>
              </>
            )}
          </div>
        )}
      </Dialog>

      {job.data && job.data.status === "running" && (
        <div className="fixed inset-x-4 bottom-4 z-30 mx-auto max-w-[560px] border border-line-2 bg-surface p-3 shadow-plate">
          <p className="flex items-center justify-between gap-3 font-mono text-[10.5px] uppercase tracking-[0.1em] text-ink-2">
            <span>{t("Deleting archives")}</span>
            <span className="num">
              {job.data.done} / {job.data.total}
            </span>
          </p>
          <div className="mt-2 h-1 w-full bg-line">
            <div
              className="h-full bg-signal transition-[width] duration-300"
              style={{ width: `${job.data.total > 0 ? Math.round((job.data.done / job.data.total) * 100) : 0}%` }}
            />
          </div>
        </div>
      )}
    </div>
  );
}

/** Re-exported for the route module: keeps the filters type close to the screen. */
export type { AdminGameFilters };
