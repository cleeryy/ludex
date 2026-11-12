import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Archive,
  Download,
  Plus,
  RotateCcw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatDate, formatRelative } from "@/lib/format";
import { useToast } from "@/state/toasts";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Field, Input, Progress, Skeleton, Switch } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import type {
  BackupEntry,
  BackupRestoreResult,
  BackupVerificationState,
} from "@/lib/types";
import { t } from "@/lib/i18n";

const QUERY_KEY = ["admin", "backups"] as const;

const VERIFICATION_TONE: Record<BackupVerificationState, "ok" | "danger" | "outline"> = {
  verified: "ok",
  failed: "danger",
  unverified: "outline",
};

function verificationLabel(state: BackupVerificationState): string {
  switch (state) {
    case "verified":
      return t("Verified");
    case "failed":
      return t("Failed");
    default:
      return t("Not verified");
  }
}

function kindLabel(kind: BackupEntry["kind"]): string {
  switch (kind) {
    case "manual":
      return t("Manual");
    case "pre_restore":
      return t("Pre-restore");
    default:
      return t("Archive");
  }
}

/** Seconds since the mutation started, shown next to an indeterminate bar. */
function Elapsed({ active }: { active: boolean }) {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    if (!active) {
      setSeconds(0);
      return;
    }
    const started = Date.now();
    const timer = window.setInterval(() => setSeconds(Math.floor((Date.now() - started) / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return <span className="num text-[11.5px] text-ink-3">{t("{seconds}s", { seconds })}</span>;
}

export function BackupsScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const backups = useQuery({
    queryKey: QUERY_KEY,
    queryFn: () => api.adminBackups(),
    refetchInterval: 30_000,
  });

  const invalidate = () => void queryClient.invalidateQueries({ queryKey: QUERY_KEY });

  const [createOpen, setCreateOpen] = useState(false);
  const [includeMedia, setIncludeMedia] = useState(true);
  const [uploadS3, setUploadS3] = useState(false);
  const create = useMutation({
    mutationFn: (dryRun: boolean) => api.createAdminBackup({ dryRun, includeMedia, uploadS3 }),
    onSuccess: (result, dryRun) => {
      if (dryRun) {
        push({
          title: t("Dry run finished"),
          description: t("The archive would contain {files} files ({size}).", {
            files: result.files,
            size: formatBytes(result.bytes),
          }),
        });
        return;
      }
      push({
        title: t("Backup created"),
        description: t("{name} · {size}", { name: result.name, size: formatBytes(result.bytes) }),
        tone: "success",
      });
      setCreateOpen(false);
      invalidate();
    },
    onError: (error: Error) => push({ title: t("Backup failed"), description: error.message, tone: "danger" }),
  });

  const verify = useMutation({
    mutationFn: (name: string) => api.verifyAdminBackup(name),
    onSuccess: (verification) => {
      push({
        title: t("Archive verified"),
        description: t("{games} games · {versions} versions · {users} users", {
          games: verification.games ?? 0,
          versions: verification.versions ?? 0,
          users: verification.users ?? 0,
        }),
        tone: "success",
      });
      invalidate();
    },
    onError: (error: Error) => {
      push({ title: t("Verification failed"), description: error.message, tone: "danger" });
      invalidate();
    },
  });

  const [deleting, setDeleting] = useState<BackupEntry | null>(null);
  const remove = useMutation({
    mutationFn: (entry: BackupEntry) => api.deleteAdminBackup(entry.name, entry.name),
    onSuccess: (_result, entry) => {
      push({ title: t("Backup deleted"), description: entry.name, tone: "success" });
      setDeleting(null);
      invalidate();
    },
    onError: (error: Error) => push({ title: t("Delete failed"), description: error.message, tone: "danger" }),
  });

  const [downloading, setDownloading] = useState<string | null>(null);
  const download = async (entry: BackupEntry) => {
    setDownloading(entry.name);
    try {
      await api.downloadAdminBackup(entry.name);
    } catch (error) {
      push({ title: t("Download failed"), description: errorMessage(error), tone: "danger" });
    } finally {
      setDownloading(null);
    }
  };

  const [restoring, setRestoring] = useState<BackupEntry | null>(null);
  const [restored, setRestored] = useState<BackupRestoreResult | null>(null);

  const entries = backups.data?.backups ?? [];

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Backups")}
        meta={
          <>
            <span>{t("{count} archives", { count: entries.length })}</span>
            {backups.data && (
              <code className="truncate font-mono text-[11px] text-ink-2">{backups.data.directory}</code>
            )}
            {USING_MOCKS && <Badge tone="outline">{t("Demo data")}</Badge>}
          </>
        }
        description={t(
          "Create consistent snapshots of the database, saves and media cache, verify them, download them for an offsite copy, or restore one. Game archives stay on their library roots or S3 bucket and are never included.",
        )}
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreateOpen(true)}>
            {t("Create backup")}
          </Button>
        }
      />

      {backups.isError && (
        <ErrorState message={errorMessage(backups.error)} onRetry={() => void backups.refetch()} />
      )}

      {restored && (
        <div className="mt-6 flex items-start gap-3 border border-signal/40 bg-signal/10 px-4 py-3">
          <RotateCcw className="mt-0.5 size-4 shrink-0 text-signal-text" aria-hidden="true" />
          <div className="text-[12.5px] leading-relaxed text-ink-2">
            <p className="font-medium text-ink">{t("Restore complete — restart the server")}</p>
            <p className="mt-1">
              {t(
                "The database was replaced on disk. Until the server restarts it keeps serving the previous data.",
              )}{" "}
              {restored.safetyBackup && (
                <>
                  {t("Safety copy:")} <code className="font-mono text-[11px]">{restored.safetyBackup}</code>.
                </>
              )}
            </p>
          </div>
          <Button variant="ghost" size="sm" className="ms-auto" onClick={() => setRestored(null)}>
            {t("Dismiss")}
          </Button>
        </div>
      )}

      <section className="mt-8">
        <SectionHeader title={t("Archives")} count={entries.length} />
        {backups.isPending && (
          <div className="flex flex-col gap-2">
            {Array.from({ length: 3 }).map((_, index) => (
              <Skeleton key={index} className="h-16 w-full" />
            ))}
          </div>
        )}
        {!backups.isPending && entries.length === 0 && (
          <div className="border border-dashed border-line-2 px-6 py-10 text-center">
            <Archive className="mx-auto size-6 text-ink-3" aria-hidden="true" />
            <p className="mt-3 text-[14px] font-medium">{t("No backups yet")}</p>
            <p className="mt-1 text-[12.5px] text-ink-3">
              {t("Create one before a risky operation, then verify and download a copy.")}
            </p>
            <Button
              className="mt-4"
              variant="primary"
              icon={<Plus className="size-4" />}
              onClick={() => setCreateOpen(true)}
            >
              {t("Create backup")}
            </Button>
          </div>
        )}

        {entries.length > 0 && (
          <div className="border border-line">
            {entries.map((entry) => (
              <div key={entry.name} className="border-b border-line p-4 last:border-b-0">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p className="truncate font-mono text-[12.5px] text-ink" title={entry.name}>
                      {entry.name}
                    </p>
                    <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px] text-ink-3">
                      <Badge tone="outline">{kindLabel(entry.kind)}</Badge>
                      <Badge tone={VERIFICATION_TONE[entry.verification.state]}>
                        {verificationLabel(entry.verification.state)}
                      </Badge>
                      <span className="num">{formatBytes(entry.size)}</span>
                      <span title={formatDate(entry.createdAt)}>{formatRelative(entry.createdAt)}</span>
                      {entry.verification.checkedAt && (
                        <span>
                          {t("checked {when}", { when: formatRelative(entry.verification.checkedAt) })}
                        </span>
                      )}
                    </div>
                    {entry.verification.state === "failed" && entry.verification.message && (
                      <p className="mt-2 flex items-start gap-2 text-[11.5px] text-danger">
                        <AlertTriangle className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
                        <span className="break-all">{entry.verification.message}</span>
                      </p>
                    )}
                  </div>
                  <div className="flex flex-wrap items-center gap-1">
                    <Button
                      variant="quiet"
                      size="sm"
                      icon={<ShieldCheck className="size-3.5" />}
                      loading={verify.isPending && verify.variables === entry.name}
                      onClick={() => verify.mutate(entry.name)}
                    >
                      {t("Verify")}
                    </Button>
                    <Button
                      variant="quiet"
                      size="sm"
                      icon={<Download className="size-3.5" />}
                      loading={downloading === entry.name}
                      onClick={() => void download(entry)}
                    >
                      {t("Download")}
                    </Button>
                    <Button
                      variant="quiet"
                      size="sm"
                      icon={<RotateCcw className="size-3.5" />}
                      onClick={() => setRestoring(entry)}
                    >
                      {t("Restore")}
                    </Button>
                    <Button
                      variant="quiet"
                      size="sm"
                      icon={<Trash2 className="size-3.5" />}
                      onClick={() => setDeleting(entry)}
                    >
                      {t("Delete")}
                    </Button>
                  </div>
                </div>
                {entry.verification.state === "verified" && (
                  <p className="mt-2 text-[11.5px] text-ink-3">
                    {t("Contains {games} games, {versions} versions and {users} users.", {
                      games: entry.verification.games ?? 0,
                      versions: entry.verification.versions ?? 0,
                      users: entry.verification.users ?? 0,
                    })}
                  </p>
                )}
              </div>
            ))}
          </div>
        )}
      </section>

      <section className="mt-10 grid gap-8 lg:grid-cols-2">
        <div>
          <SectionHeader title={t("What is archived")} />
          <ul className="flex flex-col gap-3 border-t border-line pt-4 text-[13px] leading-relaxed text-ink-2">
            <li>{t("The SQLite database, copied with VACUUM INTO for a consistent snapshot.")}</li>
            <li>{t("Cloud save snapshots and, unless disabled, the media cache.")}</li>
            <li>{t("A manifest with SHA-256 checksums, versions and row counters.")}</li>
            <li>{t("No secrets: JWT, IGDB, OIDC and S3 credentials never enter an archive.")}</li>
          </ul>
        </div>
        <div>
          <SectionHeader title={t("Restoring")} />
          <ul className="flex flex-col gap-3 border-t border-line pt-4 text-[13px] leading-relaxed text-ink-2">
            <li>{t("A dry-run preview verifies every checksum before anything is written.")}</li>
            <li>{t("A real restore requires typing the archive name and creates a safety copy first.")}</li>
            <li>{t("Restores are refused while a library scan is running.")}</li>
            <li>{t("Restart the server afterwards so it opens the restored database.")}</li>
          </ul>
        </div>
      </section>

      <Dialog
        open={createOpen}
        onOpenChange={(open) => {
          if (!create.isPending) setCreateOpen(open);
        }}
        title={t("Create backup")}
        description={t("Choose what to include. A dry run measures the archive without writing it.")}
        dismissible={!create.isPending}
        footer={
          <>
            <Button variant="ghost" onClick={() => setCreateOpen(false)} disabled={create.isPending}>
              {t("Cancel")}
            </Button>
            <Button
              onClick={() => create.mutate(true)}
              loading={create.isPending && create.variables === true}
              disabled={create.isPending}
            >
              {t("Dry run")}
            </Button>
            <Button
              variant="primary"
              onClick={() => create.mutate(false)}
              loading={create.isPending && create.variables === false}
              disabled={create.isPending}
            >
              {t("Create backup")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-5">
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-[13px] font-medium">{t("Include media cache")}</p>
              <p className="mt-0.5 text-[11.5px] text-ink-3">
                {t("Covers, backgrounds and screenshots. Disable for a smaller archive.")}
              </p>
            </div>
            <Switch
              checked={includeMedia}
              onCheckedChange={setIncludeMedia}
              label={t("Include media cache")}
              disabled={create.isPending}
            />
          </div>
          <div className="flex items-center justify-between gap-4">
            <div>
              <p className="text-[13px] font-medium">{t("Upload to S3")}</p>
              <p className="mt-0.5 text-[11.5px] text-ink-3">
                {t("Uses the configured archive storage; requires the S3 backend.")}
              </p>
            </div>
            <Switch
              checked={uploadS3}
              onCheckedChange={setUploadS3}
              label={t("Upload to S3")}
              disabled={create.isPending}
            />
          </div>
          {create.isPending && (
            <div className="flex flex-col gap-2 border border-line p-3">
              <div className="flex items-center justify-between gap-3">
                <span className="text-[12px] text-ink-2">
                  {create.variables === true ? t("Measuring the archive…") : t("Writing the archive…")}
                </span>
                <Elapsed active={create.isPending} />
              </div>
              <Progress value={null} label={t("Backup in progress")} />
              <p className="text-[11px] text-ink-3">
                {t("Large media caches take a while; the server keeps serving requests.")}
              </p>
            </div>
          )}
        </div>
      </Dialog>

      {deleting && (
        <ConfirmDialog
          open={deleting !== null}
          onOpenChange={(open) => {
            if (!open) setDeleting(null);
          }}
          title={t("Delete this backup?")}
          description={t("The archive {name} is removed from disk. This cannot be undone.", {
            name: deleting.name,
          })}
          confirmLabel={t("Delete backup")}
          loading={remove.isPending}
          onConfirm={() => remove.mutate(deleting)}
        />
      )}

      {restoring && (
        <RestoreDialog
          entry={restoring}
          onClose={() => setRestoring(null)}
          onRestored={(result) => {
            setRestoring(null);
            setRestored(result);
            invalidate();
          }}
        />
      )}
    </div>
  );
}

function RestoreDialog({
  entry,
  onClose,
  onRestored,
}: {
  entry: BackupEntry;
  onClose: () => void;
  onRestored: (result: BackupRestoreResult) => void;
}) {
  const { push } = useToast();
  const [confirmation, setConfirmation] = useState("");
  const preview = useQuery({
    queryKey: ["admin", "backups", entry.name, "restore-preview"],
    queryFn: () => api.restoreAdminBackup(entry.name, { dryRun: true, includeMedia: true }),
  });
  const restore = useMutation({
    mutationFn: () =>
      api.restoreAdminBackup(entry.name, {
        dryRun: false,
        includeMedia: true,
        confirmation,
      }),
    onSuccess: (result) => {
      push({
        title: t("Restore complete"),
        description: t("Restart the server to serve the restored database."),
        tone: "success",
      });
      onRestored(result);
    },
    onError: (error: Error) =>
      push({ title: t("Restore failed"), description: error.message, tone: "danger" }),
  });

  const confirmed = confirmation === entry.name;
  const data = preview.data;

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !restore.isPending) onClose();
      }}
      title={t("Restore this backup?")}
      description={t("This replaces the live database, saves and media with the archive contents.")}
      dismissible={!restore.isPending}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={restore.isPending}>
            {t("Cancel")}
          </Button>
          <Button
            variant="danger"
            onClick={() => restore.mutate()}
            disabled={!confirmed || preview.isError}
            loading={restore.isPending}
          >
            {t("Restore backup")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        <div className="flex items-start gap-3 border border-danger/40 bg-danger/10 px-3 py-2.5">
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden="true" />
          <p className="text-[12px] leading-relaxed text-ink-2">
            {t(
              "A safety copy is written to the backup directory before the database is replaced. Restart the server afterwards.",
            )}
          </p>
        </div>

        {preview.isPending && (
          <div className="flex flex-col gap-2">
            <Skeleton className="h-3 w-40" />
            <Skeleton className="h-16 w-full" />
          </div>
        )}
        {preview.isError && <ErrorState message={errorMessage(preview.error)} onRetry={() => void preview.refetch()} />}
        {data && (
          <dl className="border-t border-line">
            {(
              [
                [t("Created"), formatDate(data.createdAt)],
                [t("Games"), String(data.games)],
                [t("Versions"), String(data.versions)],
                [t("Users"), String(data.users)],
                [t("Archive size"), formatBytes(data.bytes)],
                [t("Destination"), data.overwrote ? t("Existing data will be overwritten") : t("Empty destination")],
              ] as const
            ).map(([label, value]) => (
              <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2">
                <dt className="text-[11.5px] text-ink-3">{label}</dt>
                <dd className="num text-[12px] text-ink-2">{value}</dd>
              </div>
            ))}
          </dl>
        )}

        <Field
          label={t("Type the archive file name to confirm")}
          hint={entry.name}
          htmlFor="restore-confirmation"
          error={confirmation.length > 0 && !confirmed ? t("The name does not match") : null}
        >
          <Input
            id="restore-confirmation"
            value={confirmation}
            onChange={(event) => setConfirmation(event.target.value)}
            autoComplete="off"
            spellCheck={false}
            disabled={restore.isPending}
            placeholder={entry.name}
          />
        </Field>

        {restore.isPending && (
          <div className="flex flex-col gap-2">
            <div className="flex items-center justify-between gap-3">
              <span className="text-[12px] text-ink-2">{t("Restoring…")}</span>
              <Elapsed active={restore.isPending} />
            </div>
            <Progress value={null} label={t("Restore in progress")} />
          </div>
        )}
      </div>
    </Dialog>
  );
}
