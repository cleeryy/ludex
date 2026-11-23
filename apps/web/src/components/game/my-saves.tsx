import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { HardDriveDownload, Plus, RotateCcw, Trash2 } from "lucide-react";
import { useToast } from "@/state/toasts";
import { Button, Input } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { t } from "@/lib/i18n";
import { errorMessage } from "@/lib/http";
import { cn } from "@/lib/cn";
import { formatBytes, formatDate } from "@/lib/format";
import { qk } from "@/lib/query-keys";
import { shortDeviceId } from "@/lib/cloud-saves";
import {
  clampRetention,
  pointsToPrune,
  SAVE_MANAGER_DESKTOP_ONLY,
  saveHistory,
  saveHistoryAvailable,
} from "@/lib/my-saves";
import type { CloudSaveStatus, SaveRestorePoint } from "@/lib/types";

/**
 * "My saves" (#295): what this device has, from when, and a one-click rollback.
 *
 * The panel is the save-manager surface the sync engine was missing. It reads
 * the local per-session restore points, shows the newest local change, the last
 * sync and the device that produced it, and never deletes the newest point.
 * Conflict policy belongs to #296: this panel only reports whether the sync
 * engine currently sees a conflict.
 */
export function MySavesPanel({
  gameId,
  status,
  onChanged,
}: {
  gameId: number;
  /** Cloud-save status of the same game; `null` while it loads or offline. */
  status: CloudSaveStatus | null;
  /** Called after a restore/prune so the parent can refresh its status query. */
  onChanged: () => void;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const native = saveHistoryAvailable();
  const points = useQuery({
    queryKey: qk.saveRestorePoints.forGame(gameId),
    queryFn: () => saveHistory.list(gameId),
    enabled: native,
    staleTime: 15_000,
    retry: false,
  });
  const view = points.data ?? null;

  const [retentionDraft, setRetentionDraft] = useState("5");
  const [savingRetention, setSavingRetention] = useState(false);
  const [creating, setCreating] = useState(false);
  const [restoreTarget, setRestoreTarget] = useState<SaveRestorePoint | null>(null);
  const [restoring, setRestoring] = useState(false);
  const [pruneOpen, setPruneOpen] = useState(false);
  const [pruning, setPruning] = useState(false);
  const retention = view?.retention;

  useEffect(() => {
    if (retention !== undefined) setRetentionDraft(String(retention));
  }, [retention]);

  const invalidate = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: qk.saveRestorePoints.forGame(gameId) }),
      queryClient.invalidateQueries({ queryKey: qk.cloudSaveStatus.forGame(gameId) }),
      queryClient.invalidateQueries({ queryKey: qk.saves.forGame(gameId) }),
    ]);
    onChanged();
  };

  const createPoint = async () => {
    if (creating) return;
    setCreating(true);
    try {
      const point = await saveHistory.capture(gameId);
      await invalidate();
      push(
        point
          ? { title: t("Restore point created"), description: formatDate(point.createdAt), tone: "success" }
          : { title: t("The save already matches the newest restore point.") },
      );
    } catch (cause) {
      push({
        title: t("Could not create a restore point"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setCreating(false);
    }
  };

  const restore = async () => {
    if (!restoreTarget || restoring) return;
    setRestoring(true);
    try {
      const outcome = await saveHistory.restore(gameId, restoreTarget.id);
      await invalidate();
      push({ title: t("Save restored"), description: outcome.message, tone: "success" });
      setRestoreTarget(null);
    } catch (cause) {
      push({
        title: t("Could not restore the save"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setRestoring(false);
    }
  };

  const saveRetention = async () => {
    const sessions = clampRetention(Number(retentionDraft));
    if (savingRetention) return;
    setSavingRetention(true);
    try {
      const saved = await saveHistory.setRetention(sessions);
      setRetentionDraft(String(saved));
      await invalidate();
    } catch (cause) {
      push({
        title: t("Could not save the setting"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setSavingRetention(false);
    }
  };

  const prune = async () => {
    if (pruning) return;
    setPruning(true);
    try {
      const report = await saveHistory.prune(gameId, clampRetention(Number(retentionDraft)));
      await invalidate();
      push({
        title: t("Restore points pruned"),
        description: `${report.removed} · ${report.kept}`,
        tone: "success",
      });
      setPruneOpen(false);
    } catch (cause) {
      push({
        title: t("Could not prune the restore points"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setPruning(false);
    }
  };

  const count = view?.points.length ?? 0;
  const conflict = status?.conflict;
  const lastLocalChange = status?.localNewestMtime ?? view?.lastCaptureAt ?? null;
  const lastSynced = status?.lastUploadAt ?? status?.latest?.createdAt ?? null;
  const device = status?.latest?.deviceId ?? status?.deviceId ?? null;
  const removable = view ? pointsToPrune(view.points.length, Number(retentionDraft)) : 0;

  return (
    <div className="border-b border-line px-4 py-4">
      <SectionHeader
        level={3}
        title={t("My saves")}
        count={count > 0 ? count : undefined}
        description={
          native
            ? t("One restore point is kept after each session. The newest copy is never deleted.")
            : SAVE_MANAGER_DESKTOP_ONLY
        }
        actions={
          native ? (
            <div className="flex items-center gap-2">
              <Button
                variant="ghost"
                size="sm"
                icon={<Trash2 className="size-3.5" />}
                disabled={count === 0}
                onClick={() => setPruneOpen(true)}
              >
                {t("Prune")}
              </Button>
              <Button
                variant="default"
                size="sm"
                icon={<Plus className="size-3.5" />}
                loading={creating}
                onClick={() => void createPoint()}
              >
                {t("Create restore point")}
              </Button>
            </div>
          ) : undefined
        }
      />

      {native && (
        <div className="mt-4">
          <dl className="grid grid-cols-2 gap-x-6 gap-y-2 text-label sm:grid-cols-3">
            <div className="flex items-baseline justify-between gap-3 border-b border-line pb-1.5">
              <dt className="text-ink-3">{t("Last local change")}</dt>
              <dd className="font-mono text-meta text-ink-2" title={lastLocalChange ?? undefined}>
                {lastLocalChange ? formatDate(lastLocalChange) : "—"}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-3 border-b border-line pb-1.5">
              <dt className="text-ink-3">{t("Last synced")}</dt>
              <dd className="font-mono text-meta text-ink-2" title={lastSynced ?? undefined}>
                {lastSynced ? formatDate(lastSynced) : t("None")}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-3 border-b border-line pb-1.5">
              <dt className="text-ink-3">{t("Device")}</dt>
              <dd className="truncate font-mono text-meta text-ink-2" title={device ?? undefined}>
                {shortDeviceId(device)}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-3 border-b border-line pb-1.5">
              <dt className="text-ink-3">{t("Size")}</dt>
              <dd className="font-mono text-meta text-ink-2">
                {formatBytes(status?.localBytes ?? 0)}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-3 border-b border-line pb-1.5">
              <dt className="text-ink-3">{t("Restore points")}</dt>
              <dd className="font-mono text-meta text-ink-2">{count}</dd>
            </div>
            <div className="flex items-baseline justify-between gap-3 border-b border-line pb-1.5">
              <dt className="text-ink-3">{t("Conflict state")}</dt>
              <dd
                className={cn(
                  "font-mono text-meta",
                  conflict ? "text-warn" : "text-ink-2",
                )}
              >
                {conflict ? t("Detected") : t("None")}
              </dd>
            </div>
          </dl>

          <div className="mt-3 flex flex-wrap items-end justify-between gap-3">
            <label className="flex items-center gap-2 text-label text-ink-3">
              {t("Sessions kept")}
              <Input
                type="number"
                min={1}
                max={50}
                value={retentionDraft}
                className="w-20"
                aria-label={t("Sessions kept")}
                disabled={savingRetention}
                onChange={(event) => setRetentionDraft(event.target.value)}
                onBlur={() => void saveRetention()}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void saveRetention();
                }}
              />
            </label>
            <p className="text-meta text-ink-3">
              {count === 0
                ? t("No restore points yet. One is created after each session.")
                : `${formatBytes(view?.totalBytes ?? 0)} · ${count}`}
            </p>
          </div>

          {points.isError && (
            <p className="mt-3 text-label text-ink-2" role="alert">
              {errorMessage(points.error)}
            </p>
          )}

          {count > 0 && (
            <ul className="mt-3 flex flex-col divide-y divide-line border-t border-line">
              {view?.points.map((point) => (
                <li
                  key={point.id}
                  className="flex flex-wrap items-center justify-between gap-3 py-2"
                >
                  <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
                    <HardDriveDownload className="size-3.5 shrink-0 text-ink-3" aria-hidden="true" />
                    <span className="font-mono text-meta text-ink-2">
                      {formatDate(point.createdAt)}
                    </span>
                    <span className="text-micro uppercase tracking-[0.08em] text-ink-3">
                      {point.source}
                    </span>
                    {point.label && (
                      <span className="truncate text-meta text-ink-3">{point.label}</span>
                    )}
                    <span className="font-mono text-meta text-ink-3">
                      {shortDeviceId(point.deviceId)} · {formatBytes(point.sizeBytes)}
                    </span>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    icon={<RotateCcw className="size-3.5" />}
                    onClick={() => setRestoreTarget(point)}
                  >
                    {t("Restore")}
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <ConfirmDialog
        open={Boolean(restoreTarget)}
        onOpenChange={(open) => !open && setRestoreTarget(null)}
        title={t("Restore this save?")}
        description={t(
          "Restoring replaces the save files on this device. The current files are kept as a new restore point.",
        )}
        confirmLabel={t("Restore")}
        tone="primary"
        loading={restoring}
        onConfirm={() => void restore()}
      />

      <ConfirmDialog
        open={pruneOpen}
        onOpenChange={setPruneOpen}
        title={t("Prune restore points?")}
        description={`${t("Older restore points are deleted. The newest one is always kept.")} (${removable})`}
        confirmLabel={t("Prune")}
        loading={pruning}
        onConfirm={() => void prune()}
      />
    </div>
  );
}
