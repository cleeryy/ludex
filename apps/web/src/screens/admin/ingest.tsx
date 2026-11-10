import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { AlertTriangle, Check, Copy, Download, FileArchive, Link2, RefreshCw, Trash2, X } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { cn } from "@/lib/cn";
import { formatBytes, formatDate, formatEta, formatRelative, formatSpeed, formatPercent } from "@/lib/format";
import type { IngestJob, IngestStatus } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, IconButton, Input, Progress, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

function ingestStatusLabel(status: IngestStatus): string {
  switch (status) {
    case "queued":
      return t("Queued");
    case "downloading":
      return t("Downloading");
    case "verifying":
      return t("Verifying");
    case "done":
      return t("Done");
    case "error":
      return t("Error");
    case "cancelled":
      return t("Cancelled");
    default:
      return status;
  }
}

function statusTone(status: IngestStatus): "default" | "signal" | "danger" | "ok" | "outline" {
  if (status === "downloading" || status === "verifying" || status === "queued") return "signal";
  if (status === "done") return "ok";
  if (status === "error") return "danger";
  return "outline";
}

export function IngestScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const [url, setUrl] = useState("");
  const [title, setTitle] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const [fileFilter, setFileFilter] = useState("");
  const [verifyState, setVerifyState] = useState<{ name: string; ok: boolean; log: string } | null>(null);
  const [verifying, setVerifying] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [replaceTarget, setReplaceTarget] = useState<IngestJob | null>(null);
  const [replaceUrl, setReplaceUrl] = useState("");

  const jobsQuery = useQuery({
    queryKey: ["jobs"],
    queryFn: () => api.listJobs(),
    refetchInterval: 2000,
  });
  const filesQuery = useQuery({
    queryKey: ["files"],
    queryFn: () => api.listFiles(),
    refetchInterval: 10_000,
  });
  const statsQuery = useQuery({
    queryKey: ["ingest-stats"],
    queryFn: () => api.ingestStats(),
    refetchInterval: 5000,
  });

  const jobs = jobsQuery.data ?? [];
  const active = jobs.filter((job) => job.status === "downloading" || job.status === "verifying" || job.status === "queued");
  const history = jobs.filter((job) => job.status === "done" || job.status === "error" || job.status === "cancelled");
  const stats = statsQuery.data;

  const files = useMemo(() => {
    const list = filesQuery.data ?? [];
    const q = fileFilter.trim().toLowerCase();
    return q ? list.filter((file) => file.name.toLowerCase().includes(q)) : list;
  }, [filesQuery.data, fileFilter]);

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    void queryClient.invalidateQueries({ queryKey: ["files"] });
    void queryClient.invalidateQueries({ queryKey: ["ingest-stats"] });
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setFormError(null);
    if (!/^https?:\/\/\S+$/i.test(url.trim())) {
      setFormError(t("Use a full http or https URL."));
      return;
    }
    setSubmitting(true);
    try {
      await api.createJob({ url: url.trim(), title: title.trim() || undefined });
      setUrl("");
      setTitle("");
      push({ title: t("Download queued"), description: t("It will appear in Active until the archive is verified."), tone: "success" });
      invalidate();
    } catch (cause) {
      setFormError(errorMessage(cause));
    } finally {
      setSubmitting(false);
    }
  };

  const cancel = async (job: IngestJob) => {
    try {
      await api.cancelJob(job.id);
      push({ title: t("Download cancelled"), description: t("The partial file was removed.") });
      invalidate();
    } catch (cause) {
      push({ title: t("Could not cancel"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const retry = async (job: IngestJob) => {
    try {
      await api.retryJob(job.id);
      push({ title: t("Download resumed"), description: t("It continues from the last byte received."), tone: "success" });
      invalidate();
    } catch (cause) {
      push({ title: t("Could not resume"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const verify = async (name: string) => {
    setVerifying(name);
    setVerifyState(null);
    try {
      const result = await api.verifyFile(name);
      setVerifyState({ name, ...result });
    } catch (cause) {
      setVerifyState({ name, ok: false, log: errorMessage(cause) });
    } finally {
      setVerifying(null);
    }
  };

  const removeFile = async (name: string) => {
    try {
      await api.deleteFile(name);
      push({ title: t("File deleted"), description: name });
      setDeleteTarget(null);
      invalidate();
    } catch (cause) {
      push({ title: t("Could not delete the file"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const replaceLink = async () => {
    if (!replaceTarget) return;
    try {
      await api.replaceJobUrl(replaceTarget.id, replaceUrl.trim());
      push({ title: t("Link replaced"), description: t("The download was requeued and keeps its partial file."), tone: "success" });
      setReplaceTarget(null);
      invalidate();
    } catch (cause) {
      push({ title: t("Could not replace the link"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const copy = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value);
      push({ title: t("{label} copied", { label }) });
    } catch {
      push({ title: t("Copy failed"), description: t("Your browser blocked clipboard access."), tone: "danger" });
    }
  };

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Ingest")}
        meta={
          <>
            <span className="num">{t("{count} active", { count: active.length })}</span>
            <span className="num">{t("{count} in history", { count: history.length })}</span>
            {stats && <span className="num">{t("{size} free", { size: formatBytes(stats.diskFree) })}</span>}
          </>
        }
        description={t("Paste a download link. Ludex fetches the archive with resume support, verifies it, renames it to the game title and indexes it.")}
        actions={
          <Button variant="ghost" size="sm" icon={<RefreshCw className="size-3.5" />} onClick={() => navigate("/admin/scan")}>
            {t("Library scan")}
          </Button>
        }
      />

      <div className="mt-8 grid gap-10 lg:grid-cols-[1fr_290px]">
        <div className="min-w-0">
          <section aria-labelledby="add-heading">
            <SectionHeader title={t("Add a download")} />
            <form onSubmit={submit} className="border border-line bg-surface/40 p-4 sm:p-5">
              <div className="flex flex-col gap-3">
                <label htmlFor="ingest-url" className="text-[12px] text-ink-2">
                  {t("Download URL")}
                </label>
                <Input
                  id="ingest-url"
                  value={url}
                  onChange={(event) => setUrl(event.target.value)}
                  placeholder="https://files.example.net/Game%20(2026)%20v1.0.zip"
                  autoFocus
                  spellCheck={false}
                  className="h-12 font-mono text-[12.5px]"
                  aria-invalid={Boolean(formError)}
                />
                <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
                  <div className="flex-1">
                    <label htmlFor="ingest-title" className="mb-1.5 block text-[12px] text-ink-2">
                      {t("Title")} <span className="text-ink-3">{t("optional")}</span>
                    </label>
                    <Input
                      id="ingest-title"
                      value={title}
                      onChange={(event) => setTitle(event.target.value)}
                      placeholder={t("Defaults to the file name")}
                      maxLength={120}
                      className="h-11"
                    />
                  </div>
                  <Button type="submit" variant="primary" size="lg" loading={submitting} icon={<Download className="size-4" />}>
                    {t("Download")}
                  </Button>
                </div>
                {formError && (
                  <p role="alert" className="flex items-center gap-2 text-[12.5px] text-danger">
                    <AlertTriangle className="size-3.5" aria-hidden="true" />
                    {formError}
                  </p>
                )}
                <p className="text-[11.5px] leading-relaxed text-ink-3">
                  {t("http or https only. The format is read from the file header, not the URL. A successful download triggers a library scan.")}
                </p>
              </div>
            </form>
          </section>

          <section className="mt-10" aria-labelledby="active-heading">
            <SectionHeader
              title={t("Active")}
              count={active.length}
              actions={<span className="font-mono text-[10.5px] text-ink-3">{t("Refreshing every 2s")}</span>}
            />
            <div id="active-heading" className="border-t border-line">
              {jobsQuery.isPending && (
                <div className="flex flex-col gap-3 py-4">
                  {Array.from({ length: 2 }).map((_, i) => (
                    <Skeleton key={i} className="h-24 w-full" />
                  ))}
                </div>
              )}
              {jobsQuery.isError && (
                <ErrorState message={errorMessage(jobsQuery.error)} onRetry={() => void jobsQuery.refetch()} />
              )}
              {!jobsQuery.isPending && !jobsQuery.isError && active.length === 0 && (
                <p className="py-6 text-[13px] text-ink-3">{t("Nothing downloading. Past downloads are below.")}</p>
              )}
              {active.map((job) => (
                <JobRow
                  key={job.id}
                  job={job}
                  onCancel={() => void cancel(job)}
                  onRetry={() => void retry(job)}
                  onReplace={() => {
                    setReplaceTarget(job);
                    setReplaceUrl(job.url ?? "");
                  }}
                  onCopy={() => job.url && void copy(job.url, "URL")}
                />
              ))}
            </div>
          </section>

          {history.length > 0 && (
            <section className="mt-10" aria-labelledby="history-heading">
              <SectionHeader title={t("History")} count={history.length} />
              <details id="history-heading" className="group border-t border-line">
                <summary className="flex cursor-pointer list-none items-center justify-between gap-3 py-3 text-[12.5px] text-ink-3 hover:text-ink-2">
                  <span>{t("Finished, failed and cancelled downloads")}</span>
                  <span className="font-mono text-[10.5px] group-open:hidden">{t("Show")}</span>
                  <span className="hidden font-mono text-[10.5px] group-open:inline">{t("Hide")}</span>
                </summary>
                <div className="border-t border-line">
                  {history.map((job) => (
                    <JobRow
                      key={job.id}
                      job={job}
                      compact
                      onCancel={() => void cancel(job)}
                      onRetry={() => void retry(job)}
                      onReplace={() => {
                        setReplaceTarget(job);
                        setReplaceUrl(job.url ?? "");
                      }}
                      onCopy={() => job.url && void copy(job.url, "URL")}
                    />
                  ))}
                </div>
              </details>
            </section>
          )}

          <section className="mt-10" aria-labelledby="files-heading">
            <SectionHeader
              title={t("Library files")}
              count={filesQuery.data?.length ?? 0}
              actions={
                <div className="w-[220px]">
                  <Input
                    value={fileFilter}
                    onChange={(event) => setFileFilter(event.target.value)}
                    placeholder={t("Filter by name")}
                    aria-label={t("Filter library files")}
                    className="h-8 text-[12px]"
                  />
                </div>
              }
            />
            <div id="files-heading" className="border-t border-line">
              {filesQuery.isError && (
                <ErrorState
                  message={errorMessage(filesQuery.error)}
                  onRetry={() => void filesQuery.refetch()}
                />
              )}
              {!filesQuery.isPending && !filesQuery.isError && files.length === 0 && (
                <p className="py-6 text-[13px] text-ink-3">
                  {fileFilter ? t("No files match that filter.") : t("The games folder is empty.")}
                </p>
              )}
              {files.map((file) => (
                <div key={file.name} className="border-b border-line py-3">
                  <div className="flex flex-wrap items-baseline justify-between gap-3">
                    <div className="flex min-w-0 items-baseline gap-2.5">
                      <FileArchive className="size-3.5 shrink-0 self-center text-ink-3" aria-hidden="true" />
                      <div className="min-w-0">
                        <p className="truncate font-mono text-[11.5px] text-ink">{file.name}</p>
                        <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-ink-3">
                          <span>{formatBytes(file.size)}</span>
                          <span>{formatDate(file.modifiedAt)}</span>
                          {file.partial && <span className="text-signal-text">{t("partial — resumes automatically")}</span>}
                        </p>
                      </div>
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      {!file.partial && (
                        <Button
                          variant="ghost"
                          size="sm"
                          loading={verifying === file.name}
                          onClick={() => void verify(file.name)}
                          className="text-ink-2"
                        >
                          {t("Test")}
                        </Button>
                      )}
                      <IconButton
                        label={t("Delete {name}", { name: file.name })}
                        variant="ghost"
                        className="size-8 text-ink-3 hover:text-danger"
                        onClick={() => setDeleteTarget(file.name)}
                      >
                        <Trash2 className="size-3.5" />
                      </IconButton>
                    </div>
                  </div>
                  {verifyState?.name === file.name && (
                    <pre
                      className={cn(
                        "mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-words border-l-2 bg-bg-deep/60 px-3 py-2 font-mono text-[10.5px] leading-relaxed",
                        verifyState.ok ? "border-ok text-ink-2" : "border-danger text-ink-2",
                      )}
                    >
                      {verifyState.ok ? t("Archive verified") : t("Archive test failed")}
                      {"\n"}
                      {verifyState.log}
                    </pre>
                  )}
                </div>
              ))}
            </div>
          </section>
        </div>

        <aside className="lg:sticky lg:top-6 lg:self-start">
          <div className="border border-line bg-surface/40 p-4">
            <h2 className="text-[12px] font-medium text-ink-2">{t("Storage")}</h2>
            {stats ? (
              <>
                <p className="mt-3 flex items-baseline gap-1.5">
                  <span className="num text-[26px] leading-none text-ink">{formatBytes(stats.diskFree)}</span>
                  <span className="text-[11.5px] text-ink-3">{t("free")}</span>
                </p>
                <div className="mt-3">
                  <Progress value={stats.diskTotal - stats.diskFree} max={stats.diskTotal} label={t("Disk usage")} />
                  <p className="mt-1.5 flex justify-between font-mono text-[10.5px] text-ink-3">
                    <span>{t("{size} used", { size: formatBytes(stats.diskTotal - stats.diskFree) })}</span>
                    <span>{t("{size} total", { size: formatBytes(stats.diskTotal) })}</span>
                  </p>
                </div>
                <dl className="mt-4 flex flex-col gap-2 border-t border-line pt-3 text-[12px]">
                  <div className="flex items-baseline justify-between gap-3">
                    <dt className="text-ink-3">{t("Library")}</dt>
                    <dd className="num text-ink-2">{formatBytes(stats.libraryBytes)}</dd>
                  </div>
                  <div className="flex items-baseline justify-between gap-3">
                    <dt className="text-ink-3">{t("Games")}</dt>
                    <dd className="num text-ink-2">{stats.gameCount}</dd>
                  </div>
                  <div className="flex items-baseline justify-between gap-3">
                    <dt className="text-ink-3">{t("Active jobs")}</dt>
                    <dd className="num text-ink-2">{stats.activeJobs}</dd>
                  </div>
                </dl>
                <div className="mt-4 border-t border-line pt-3">
                  <p className="text-[10.5px] text-ink-3">{t("Games folder")}</p>
                  <div className="mt-1 flex items-baseline gap-2">
                    <code className="min-w-0 flex-1 truncate font-mono text-[10.5px] text-ink-2">{stats.gamesDir}</code>
                    <button
                      type="button"
                      onClick={() => void copy(stats.gamesDir, "Path")}
                      aria-label={t("Copy games folder path")}
                      className="text-ink-3 hover:text-ink"
                    >
                      <Copy className="size-3.5" />
                    </button>
                  </div>
                </div>
              </>
            ) : statsQuery.isError ? (
              <p className="mt-3 text-[12px] leading-relaxed text-ink-3">{t("Storage information is unavailable. The download jobs are unaffected.")}</p>
            ) : (
              <div className="mt-3 space-y-2">
                <Skeleton className="h-6 w-24" />
                <Skeleton className="h-1 w-full" />
                <Skeleton className="h-3 w-full" />
              </div>
            )}
          </div>

          <p className="mt-4 text-[11.5px] leading-relaxed text-ink-3">
            {t("Jobs survive a server restart. Partial files are kept and resumed, and an archive is only renamed once its header and CRC check pass.")}
          </p>
        </aside>
      </div>

      <ConfirmDialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("Delete this file?")}
        description={deleteTarget ? `${deleteTarget} will be removed from the games folder. Progress and metadata for the game stay in the database.` : ""}
        confirmLabel={t("Delete file")}
        onConfirm={() => deleteTarget && void removeFile(deleteTarget)}
      />

      <Dialog
        open={Boolean(replaceTarget)}
        onOpenChange={(open) => !open && setReplaceTarget(null)}
        title={t("Replace the link")}
        description={t("Useful when a signed URL expired. The partial file is kept and the download resumes.")}
        size="sm"
        footer={
          <>
            <Button variant="ghost" onClick={() => setReplaceTarget(null)}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" onClick={() => void replaceLink()} disabled={!replaceUrl.trim()}>
              {t("Replace and resume")}
            </Button>
          </>
        }
      >
        <label htmlFor="replace-url" className="mb-1.5 block text-[12px] text-ink-2">
          {t("New URL")}
        </label>
        <Input
          id="replace-url"
          value={replaceUrl}
          onChange={(event) => setReplaceUrl(event.target.value)}
          className="font-mono text-[12px]"
          autoFocus
        />
      </Dialog>
    </div>
  );
}

function JobRow({
  job,
  compact,
  onCancel,
  onRetry,
  onReplace,
  onCopy,
}: {
  job: IngestJob;
  compact?: boolean;
  onCancel: () => void;
  onRetry: () => void;
  onReplace: () => void;
  onCopy: () => void;
}) {
  const active = job.status === "downloading" || job.status === "verifying" || job.status === "queued";
  const pct = job.totalBytes ? Math.min(100, (job.downloadedBytes / job.totalBytes) * 100) : null;
  const eta = active ? formatEta(job.downloadedBytes, job.totalBytes, job.speedBps) : null;
  const retryable = job.status === "error" || job.status === "cancelled";

  return (
    <article className={cn("border-b border-line", compact ? "py-2.5" : "py-4")} aria-label={t("{title}, {phase}", { title: job.title, phase: ingestStatusLabel(job.status) })}>
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h3 className={cn("min-w-0 truncate font-medium text-ink", compact ? "text-[13px]" : "text-[14.5px]")}>{job.title}</h3>
        <Badge tone={statusTone(job.status)}>{ingestStatusLabel(job.status)}</Badge>
      </div>

      {!compact && job.url && (
        <div className="mt-1 flex items-baseline gap-2">
          <a
            href={job.url}
            target="_blank"
            rel="noopener noreferrer"
            className="min-w-0 truncate font-mono text-[10.5px] text-ink-3 underline-offset-2 hover:text-ink-2 hover:underline"
          >
            {job.url}
          </a>
          <button type="button" onClick={onCopy} aria-label={t("Copy download URL")} className="shrink-0 text-ink-3 hover:text-ink">
            <Copy className="size-3" />
          </button>
        </div>
      )}

      {active && (
        <>
          <div className="mt-2.5">
            <Progress
              value={pct}
              label={t("Download progress for {title}", { title: job.title })}
              tone={job.status === "error" ? "danger" : "signal"}
            />
          </div>
          <div className="mt-1.5 flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
            <span>
              {job.totalBytes
                ? `${formatBytes(job.downloadedBytes)} / ${formatBytes(job.totalBytes)}`
                : t("{size} received", { size: formatBytes(job.downloadedBytes) })}
            </span>
            {job.speedBps > 0 && <span>{formatSpeed(job.speedBps)}</span>}
            {pct !== null && <span>{formatPercent(pct, 1)}</span>}
            {eta && <span>{t("{duration} left", { duration: eta })}</span>}
            {job.attempt > 1 && <span>{t("attempt")} {job.attempt}</span>}
            {!job.totalBytes && <span>{t("size unknown")}</span>}
          </div>
        </>
      )}

      {compact && (
        <p className="mt-1 font-mono text-[10.5px] text-ink-3">
          {formatBytes(job.downloadedBytes)}
          {job.totalBytes ? ` / ${formatBytes(job.totalBytes)}` : ""} · {formatRelative(job.updatedAt)}
        </p>
      )}

      {job.error && (
        <p className={cn("mt-1.5 flex items-start gap-1.5 text-[12px]", job.status === "error" ? "text-danger" : "text-ink-3")}>
          <AlertTriangle className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
          {job.error}
        </p>
      )}

      <div className="mt-2 flex flex-wrap items-center gap-1">
        {active && (
          <button
            type="button"
            onClick={onCancel}
            className="flex h-8 items-center gap-1.5 text-[12px] text-danger underline-offset-4 hover:underline"
          >
            <X className="size-3.5" aria-hidden="true" />
            {t("Cancel")}
          </button>
        )}
        {retryable && (
          <>
            <Button variant="ghost" size="sm" className="text-signal-text" onClick={onRetry} icon={<RefreshCw className="size-3.5" />}>
              {t("Resume")}
            </Button>
            <Button variant="ghost" size="sm" className="text-ink-3" onClick={onReplace} icon={<Link2 className="size-3.5" />}>
              {t("New link")}
            </Button>
          </>
        )}
        {job.status === "done" && (
          <span className="flex items-center gap-1.5 text-[11.5px] text-ok">
            <Check className="size-3.5" aria-hidden="true" />
            {t("Indexed")}
          </span>
        )}
      </div>
    </article>
  );
}
