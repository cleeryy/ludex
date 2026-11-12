import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Eye, ShieldCheck, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate } from "@/lib/format";
import { t } from "@/lib/i18n";
import { isNative, native } from "@/lib/native";
import type { DiagnosticsPreview } from "@/lib/types";
import { useIsAdmin } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { Badge, Button, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { SettingsSwitchRow } from "@/components/settings/settings-section";

/**
 * Privacy and diagnostics (#171): the opt-in toggle, what is collected, the
 * exact payload preview, and deletion. Local capture happens on the device;
 * sending is always an explicit button press.
 */
export function DiagnosticsPrivacyPanel() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [preview, setPreview] = useState<DiagnosticsPreview | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const statusQuery = useQuery({
    queryKey: ["diagnostics-status"],
    queryFn: () => native.diagnosticsStatus(),
    enabled: isNative(),
  });
  const status = statusQuery.data;

  const refresh = () => queryClient.invalidateQueries({ queryKey: ["diagnostics-status"] });

  const toggle = async (enabled: boolean) => {
    setBusy(true);
    try {
      const next = await native.setDiagnosticsEnabled(enabled);
      queryClient.setQueryData(["diagnostics-status"], next);
      push({
        title: enabled ? t("Anonymous diagnostics enabled") : t("Anonymous diagnostics disabled"),
        description: enabled
          ? t("Nothing is sent until you review a report and press Send.")
          : t("Local reports and the install id were deleted."),
        tone: "success",
      });
    } catch (cause) {
      setBusy(false);
      push({ title: t("Could not change the diagnostics setting"), description: errorMessage(cause), tone: "danger" });
      return;
    }
    setBusy(false);
  };

  const openPreview = async (reportId: string) => {
    try {
      setPreview(await native.diagnosticsPreview(reportId));
    } catch (cause) {
      push({ title: t("Could not build the preview"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const send = async () => {
    if (!preview) return;
    setBusy(true);
    try {
      await native.diagnosticsSend(preview.reportId);
      push({ title: t("Report sent"), description: t("Thank you. The report contains no account, path or token."), tone: "success" });
      setPreview(null);
      await refresh();
    } catch (cause) {
      push({ title: t("Could not send the report"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  const deleteAll = async () => {
    setBusy(true);
    try {
      await native.diagnosticsDeleteReports();
      setConfirmDelete(false);
      await refresh();
      push({ title: t("Local reports deleted"), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not delete local reports"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  if (!isNative()) {
    return (
      <p className="text-[12.5px] leading-relaxed text-ink-3">
        {t("Crash reports are captured by the Ludex desktop app. The browser build has no local crash store and never sends diagnostics.")}
      </p>
    );
  }

  if (statusQuery.isLoading || !status) {
    return (
      <div className="space-y-2">
        <Skeleton className="h-12 w-full" />
        <Skeleton className="h-12 w-full" />
      </div>
    );
  }

  return (
    <div>
      <div className="flex items-start gap-3 border-b border-line py-4">
        <ShieldCheck className="mt-0.5 size-4 shrink-0 text-ok" aria-hidden="true" />
        <div className="min-w-0">
          <p className="text-[13px] font-medium text-ink">{t("Off by default. Nothing is sent automatically.")}</p>
          <p className="mt-1 max-w-[64ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("Ludex keeps a short local history of crashes and failed commands so a bug can be reported without copying logs by hand. Reports leave this device only when you press Send, and only while the toggle below is on.")}
          </p>
        </div>
      </div>

      <SettingsSwitchRow
        title={t("Share anonymous diagnostics and crash reports")}
        description={t("App and OS version, error kind and a redacted summary. Turning this off deletes the local queue and the random install id.")}
        checked={status.enabled}
        onCheckedChange={(checked) => void toggle(checked)}
      />

      <dl className="border-b border-line py-3 text-[12px]">
        <div className="flex flex-wrap items-baseline justify-between gap-2 py-1">
          <dt className="text-ink-3">{t("App version")}</dt>
          <dd className="num text-ink">v{status.appVersion} · {status.platform} · {status.os}</dd>
        </div>
        <div className="flex flex-wrap items-baseline justify-between gap-2 py-1">
          <dt className="text-ink-3">{t("Random install id")}</dt>
          <dd className="break-all font-mono text-[11px] text-ink-2">{status.installId || t("generated locally")}</dd>
        </div>
        <div className="flex flex-wrap items-baseline justify-between gap-2 py-1">
          <dt className="text-ink-3">{t("Stored locally")}</dt>
          <dd className="num text-ink">{status.reports.length} / 20 · {t("{count} not sent", { count: status.unsent })}</dd>
        </div>
      </dl>

      <div className="pb-2 pt-4">
        <h4 className="text-[12.5px] font-medium text-ink">{t("What is collected")}</h4>
        <ul className="mt-2 list-disc space-y-1 ps-5 text-[11.5px] leading-relaxed text-ink-3">
          <li>{t("App version, operating system, CPU architecture, UI locale and release channel.")}</li>
          <li>{t("Error kind, a one-line summary, an optional stack trace, the last log lines and the command name.")}</li>
          <li>{t("A game id and title only when the failure happened in that game's install, launch or update.")}</li>
          <li className="text-ink-2">
            {t("Removed before storage: home directory, usernames, hostnames, tokens, cookies, Authorization headers, server URLs, IP addresses, e-mail addresses and file paths (replaced by a stable hash).")}
          </li>
          <li>{t("The random install id is not derived from hardware, your account or the hostname. Turning the toggle off forgets it.")}</li>
        </ul>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-3 border-t border-line pt-3">
        <p className="text-[11.5px] text-ink-3">{t("Reports are kept on this device (last 20) and, once sent, on the server for 90 days.")}</p>
        <Button
          variant="ghost"
          size="sm"
          icon={<Trash2 className="size-3.5" />}
          disabled={busy || status.reports.length === 0}
          onClick={() => setConfirmDelete(true)}
        >
          {t("Delete local reports")}
        </Button>
      </div>

      {status.reports.length > 0 ? (
        <ul className="mt-3 border-t border-line">
          {status.reports.map((report) => (
            <li key={report.id} className="flex items-start justify-between gap-4 border-b border-line py-3">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge tone={report.kind === "panic" ? "danger" : "outline"}>{report.kind}</Badge>
                  <span className="text-[11px] text-ink-3">{formatDate(report.createdAt)}</span>
                  {report.sent && <Badge tone="ok">{t("Sent")}</Badge>}
                </div>
                <p className="mt-1 line-clamp-2 text-[12px] leading-relaxed text-ink-2">{report.summary}</p>
              </div>
              <Button
                variant="default"
                size="sm"
                icon={<Eye className="size-3.5" />}
                onClick={() => void openPreview(report.id)}
              >
                {t("Preview")}
              </Button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-3 border-t border-line pt-3 text-[12px] text-ink-3">
          {t("No local report yet. Crashes and failed commands will appear here.")}
        </p>
      )}

      <ConfirmDialog
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title={t("Delete local reports?")}
        description={t("Every report stored on this device is removed. Reports already sent stay on the server until the administrator deletes them or retention expires.")}
        confirmLabel={t("Delete reports")}
        loading={busy}
        onConfirm={() => void deleteAll()}
      />

      <Dialog
        open={preview !== null}
        onOpenChange={(open) => {
          if (!open) setPreview(null);
        }}
        title={t("What will be sent")}
        description={t("This is the exact JSON body. Paths, usernames, hostnames, tokens, URLs, IPs and e-mails are already removed.")}
        size="lg"
        footer={
          <>
            <Button variant="ghost" onClick={() => setPreview(null)} disabled={busy}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" onClick={() => void send()} loading={busy}>
              {t("Send this report")}
            </Button>
          </>
        }
      >
        <pre className="max-h-[46vh] overflow-auto whitespace-pre-wrap break-all rounded-[3px] border border-line bg-surface p-3 font-mono text-[11px] leading-relaxed text-ink-2">
          {preview ? JSON.stringify(preview.payload, null, 2) : ""}
        </pre>
      </Dialog>
    </div>
  );
}

/**
 * Admin view of stored reports (#171): totals, version/kind/platform
 * distribution and deletion. No account or IP is ever attached to a report.
 */
export function AdminDiagnosticsPanel() {
  const admin = useIsAdmin();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [deleting, setDeleting] = useState<number | null>(null);

  const pageQuery = useQuery({
    queryKey: ["admin-diagnostics"],
    queryFn: () => api.listDiagnostics(),
    enabled: admin,
  });

  const remove = useMutation({
    mutationFn: (id: number) => api.deleteDiagnosticsReport(id),
    onSuccess: async () => {
      setDeleting(null);
      await queryClient.invalidateQueries({ queryKey: ["admin-diagnostics"] });
      push({ title: t("Report deleted"), tone: "success" });
    },
    onError: (error) => {
      setDeleting(null);
      push({ title: t("Could not delete the report"), description: errorMessage(error), tone: "danger" });
    },
  });

  if (!admin) return null;
  if (pageQuery.isLoading) return <Skeleton className="h-16 w-full" />;
  if (pageQuery.isError) {
    return <p className="text-[12.5px] text-danger">{errorMessage(pageQuery.error)}</p>;
  }
  const page = pageQuery.data;
  if (!page) return null;

  return (
    <div>
      <p className="max-w-[64ch] text-[11.5px] leading-relaxed text-ink-3">
        {t("Reports are anonymous: the server stores a random install id, never an account, IP address or server log. Counts show which version and error kind to fix first. Retention is {days} days.", { days: page.retentionDays })}
      </p>
      <dl className="mt-3 grid grid-cols-2 gap-px overflow-hidden rounded-[3px] border border-line bg-line sm:grid-cols-4">
        {[
          [t("Stored"), page.total],
          [t("Versions"), page.byVersion.length],
          [t("Error kinds"), page.byKind.length],
          [t("Platforms"), page.byPlatform.length],
        ].map(([label, value]) => (
          <div key={String(label)} className="bg-surface px-3 py-2.5">
            <dt className="text-[10.5px] uppercase tracking-[0.1em] text-ink-3">{label}</dt>
            <dd className="num mt-1 text-[18px] text-ink">{value}</dd>
          </div>
        ))}
      </dl>

      <div className="mt-3 grid gap-3 md:grid-cols-2">
        <Distribution title={t("By version")} rows={page.byVersion} />
        <Distribution title={t("By error kind")} rows={page.byKind} />
      </div>

      {page.items.length === 0 ? (
        <p className="mt-4 border-t border-line pt-3 text-[12px] text-ink-3">
          {t("No report has been sent to this server yet.")}
        </p>
      ) : (
        <ul className="mt-4 border-t border-line">
          {page.items.map((report) => (
            <li key={report.id} className="flex items-start justify-between gap-4 border-b border-line py-3">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge tone={report.errorKind === "panic" ? "danger" : "outline"}>{report.errorKind}</Badge>
                  <span className="num text-[11px] text-ink-3">v{report.appVersion}</span>
                  <span className="text-[11px] text-ink-3">{report.platform} · {report.os}</span>
                  <span className="text-[11px] text-ink-3">{formatDate(report.createdAt)}</span>
                </div>
                <p className="mt-1 line-clamp-2 text-[12px] leading-relaxed text-ink-2">{report.summary}</p>
                <p className="mt-1 font-mono text-[10.5px] text-ink-3">install {report.installId.slice(0, 8)}…</p>
              </div>
              <Button
                variant="ghost"
                size="sm"
                icon={<Trash2 className="size-3.5" />}
                loading={deleting === report.id && remove.isPending}
                onClick={() => {
                  setDeleting(report.id);
                  remove.mutate(report.id);
                }}
              >
                {t("Delete")}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Distribution({ title, rows }: { title: string; rows: { value: string; count: number }[] }) {
  if (rows.length === 0) {
    return (
      <div className="rounded-[3px] border border-line p-3">
        <p className="text-[11px] uppercase tracking-[0.1em] text-ink-3">{title}</p>
        <p className="mt-1 text-[12px] text-ink-3">{t("Nothing yet")}</p>
      </div>
    );
  }
  const max = Math.max(...rows.map((row) => row.count), 1);
  return (
    <div className="rounded-[3px] border border-line p-3">
      <p className="text-[11px] uppercase tracking-[0.1em] text-ink-3">{title}</p>
      <ul className="mt-2 space-y-1.5">
        {rows.slice(0, 6).map((row) => (
          <li key={row.value} className="grid grid-cols-[1fr_auto] items-center gap-2 text-[11.5px]">
            <div className="min-w-0">
              <div className="flex items-baseline justify-between gap-2">
                <span className="truncate text-ink-2">{row.value || "—"}</span>
                <span className="num text-ink-3">{row.count}</span>
              </div>
              <div className="mt-1 h-px bg-line">
                <div className="h-px bg-signal" style={{ width: `${Math.round((row.count / max) * 100)}%` }} />
              </div>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
