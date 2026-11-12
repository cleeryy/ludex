import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, CheckCircle2, Eye, FolderOpen, Gamepad2, Layers, RefreshCw, ServerOff, ShieldCheck, Trash2, Tv, Wifi } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { needsSunshineElevation } from "@/lib/sunshine";
import type { LocalSettings, SunshinePreviewEntry } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Input, Skeleton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { SettingsSwitchRow } from "@/components/settings/settings-section";
import { t } from "@/lib/i18n";

/**
 * Settings → Streaming (#97, #98).
 *
 * Sunshine and Moonlight are external applications. This panel only reports
 * what was detected and, for Sunshine, writes the app list after an explicit
 * click; every real write is backed up first.
 */
export function StreamingPanel({
  draft,
  update,
}: {
  draft: LocalSettings;
  update: (patch: Partial<LocalSettings>) => void;
}) {
  return (
    <section className="mt-10" aria-labelledby="streaming-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h3 id="streaming-heading" className="text-[15px] font-semibold tracking-[-0.01em] text-ink">
            {t("Game streaming")}
          </h3>
          <p className="mt-1 max-w-[70ch] text-[12px] leading-relaxed text-ink-3">
            {t("Publish the Ludex library as Sunshine apps and start streams from Moonlight. Ludex never bundles either application.")}
          </p>
        </div>
      </div>
      <div className="mt-4 grid gap-8 xl:grid-cols-2">
        <SunshinePanel draft={draft} update={update} />
        <MoonlightPanel draft={draft} update={update} />
      </div>
      <ConsoleModePanel draft={draft} update={update} />
    </section>
  );
}

/**
 * Console mode (#172): one Sunshine app that opens Big Picture, instead of one
 * entry per game. The sync section above applies the settings on its next run.
 */
function ConsoleModePanel({
  draft,
  update,
}: {
  draft: LocalSettings;
  update: (patch: Partial<LocalSettings>) => void;
}) {
  return (
    <div className="mt-8 border border-line p-5">
      <div className="flex items-start justify-between gap-4">
        <div className="flex items-center gap-2">
          <Gamepad2 className="size-4 text-ink-3" aria-hidden="true" />
          <h4 className="text-[13.5px] font-semibold text-ink">{t("Console mode")}</h4>
          <Badge tone={draft.consoleMode ? "ok" : "outline"}>
            {draft.consoleMode ? t("on") : t("off")}
          </Badge>
        </div>
      </div>
      <p className="mt-1 max-w-[76ch] text-[12px] leading-relaxed text-ink-3">
        {t("Publish one Sunshine app that opens Big Picture fullscreen on the host. Games are installed, launched and stopped from there, and quitting the console returns to the host desktop. Per-game Sunshine entries are removed by the next sync.")}
      </p>

      <div className="mt-4">
        <SettingsSwitchRow
          title={t("Publish one Ludex console app")}
          description={t("Moonlight lists a single app. Turn this off to go back to one Sunshine app per game.")}
          checked={draft.consoleMode}
          onCheckedChange={(consoleMode) => update({ consoleMode })}
        />
        <SettingsSwitchRow
          title={t("Fullscreen console")}
          description={t("The published command opens the console fullscreen. Off passes --windowed, which is useful when testing on a desktop.")}
          checked={draft.consoleFullscreen}
          onCheckedChange={(consoleFullscreen) => update({ consoleFullscreen })}
        />
        <SettingsSwitchRow
          title={t("Publish per-game entries too (expert)")}
          description={t("Keep one Sunshine app per installed game next to the console app. Not needed for Moonlight: the console launches every game.")}
          checked={draft.consolePerGameApps}
          onCheckedChange={(consolePerGameApps) => update({ consolePerGameApps })}
        />
      </div>

      <div className="mt-5 grid gap-4 lg:grid-cols-2">
        <label className="flex flex-col gap-1.5">
          <span className="text-[12.5px] font-medium text-ink">{t("Console app name")}</span>
          <Input
            value={draft.consoleAppName}
            onChange={(event) => update({ consoleAppName: event.target.value })}
            placeholder="Ludex"
            spellCheck={false}
            className="h-9"
          />
          <span className="text-[11px] text-ink-3">
            {t("Name Moonlight shows for the console app. Empty uses Ludex.")}
          </span>
        </label>
        <label className="flex flex-col gap-1.5">
          <span className="text-[12.5px] font-medium text-ink">{t("Exit shortcut")}</span>
          <Input
            value={draft.consoleExitShortcut}
            onChange={(event) => update({ consoleExitShortcut: event.target.value })}
            placeholder="f12"
            spellCheck={false}
            className="h-9 font-mono text-[11px]"
          />
          <span className="text-[11px] text-ink-3">
            {t("Key combination that quits the console back to the desktop, for example f12 or ctrl+shift+q.")}
          </span>
        </label>
      </div>
      <p className="mt-3 text-[11px] leading-relaxed text-ink-3">
        {t("Save the desktop settings first, then publish the library above. Sunshine starts the console with the Ludex binary and --console; the app must be installed on the streaming host itself.")}
      </p>
    </div>
  );
}

function ActionBadge({ action }: { action: SunshinePreviewEntry["action"] }) {
  if (action === "add") return <Badge tone="signal">{t("add")}</Badge>;
  if (action === "update") return <Badge tone="default">{t("update")}</Badge>;
  return <Badge tone="outline">{t("unchanged")}</Badge>;
}

function SunshinePanel({
  draft,
  update,
}: {
  draft: LocalSettings;
  update: (patch: Partial<LocalSettings>) => void;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<"sync" | "preview" | "remove" | null>(null);
  const [granting, setGranting] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [backupPath, setBackupPath] = useState<string | null>(null);
  const [syncError, setSyncError] = useState<{ message: string; needsElevation: boolean } | null>(null);

  const status = useQuery({
    queryKey: ["sunshine-status"],
    queryFn: () => native.sunshineStatus(),
    retry: false,
    staleTime: 15_000,
  });
  const preview = useQuery({
    queryKey: ["sunshine-preview", status.data?.appsPath ?? "none"],
    queryFn: () => native.sunshinePreview(),
    retry: false,
    enabled: Boolean(status.data?.appsPath),
    staleTime: 15_000,
  });

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: ["sunshine-status"] });
    await queryClient.invalidateQueries({ queryKey: ["sunshine-preview"] });
    await queryClient.invalidateQueries({ queryKey: ["stream-target"] });
  };

  const sync = async (dryRun: boolean) => {
    setBusy(dryRun ? "preview" : "sync");
    try {
      const report = await native.sunshineSync({ dryRun });
      setSyncError(null);
      setBackupPath(report.backupPath ?? null);
      await refresh();
      push({
        title: report.written ? t("Library published") : t("Nothing was written"),
        description: report.message,
        tone: report.written ? "success" : "default",
      });
    } catch (cause) {
      setSyncError({ message: errorMessage(cause), needsElevation: needsSunshineElevation(cause) });
      push({ title: t("Sunshine sync failed"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  /**
   * #182: explicit one-time UAC grant for a Sunshine install under Program
   * Files, then a single retry. Cancelling changes nothing and keeps the error
   * with its manual alternatives.
   */
  const grantAccess = async () => {
    setGranting(true);
    try {
      const grant = await native.grantSunshineWriteAccess();
      if (grant.status === "granted") {
        push({ title: t("Write access granted"), description: grant.message, tone: "success" });
        setSyncError(null);
        await sync(false);
        return;
      }
      if (grant.status === "cancelled") {
        push({ title: t("Elevation cancelled"), description: grant.message, tone: "default" });
        return;
      }
      push({ title: t("Could not grant write access"), description: grant.message, tone: "danger" });
    } catch (cause) {
      push({ title: t("Could not grant write access"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setGranting(false);
    }
  };

  const removeManaged = async () => {
    setBusy("remove");
    try {
      const report = await native.sunshineRemoveManaged();
      setBackupPath(report.backupPath ?? null);
      setConfirmRemove(false);
      await refresh();
      push({
        title: report.written ? t("Ludex apps removed") : t("Nothing to remove"),
        description: report.message,
        tone: report.written ? "success" : "default",
      });
    } catch (cause) {
      push({ title: t("Could not remove the Ludex apps"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="border border-line p-5">
      <div className="flex items-start justify-between gap-4">
        <div className="flex items-center gap-2">
          <Tv className="size-4 text-ink-3" aria-hidden="true" />
          <h4 className="text-[13.5px] font-semibold text-ink">{t("Sunshine host")}</h4>
          {status.data && (
            <Badge tone={status.data.detected ? "ok" : "outline"}>
              {status.data.detected ? t("detected") : t("not found")}
            </Badge>
          )}
        </div>
        <Button
          size="sm"
          variant="ghost"
          icon={<RefreshCw className="size-3.5" />}
          loading={status.isFetching}
          onClick={() => void refresh()}
        >
          {t("Rescan")}
        </Button>
      </div>

      {status.isPending && (
        <div className="mt-4 flex flex-col gap-2">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      )}
      {status.isError && (
        <p role="alert" className="mt-4 border-l-2 border-danger pl-3 text-[12px] text-danger">
          {errorMessage(status.error)}
        </p>
      )}

      {status.data && (
        <>
          <p className="mt-3 text-[12px] leading-relaxed text-ink-3">{status.data.message}</p>
          <dl className="mt-4 border-t border-line">
            {[
              [t("Sunshine"), status.data.binary ?? t("not detected")],
              [t("apps.json"), status.data.appsPath ?? t("not found")],
              [t("sunshine.conf"), status.data.confPath ?? t("not found")],
              [
                t("Apps"),
                status.data.appsFound
                  ? t("{count} entries, {managed} published by Ludex", {
                      count: status.data.entries,
                      managed: status.data.managedEntries,
                    })
                  : t("no apps.json yet"),
              ],
            ].map(([label, value]) => (
              <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2">
                <dt className="shrink-0 text-[12px] text-ink-3">{label}</dt>
                <dd className="min-w-0 truncate font-mono text-[10.5px] text-ink-2" title={value}>
                  {value}
                </dd>
              </div>
            ))}
          </dl>

          {status.data.warnings.length > 0 && (
            <ul className="mt-3 flex flex-col gap-1.5 text-[11.5px] leading-relaxed text-ink-3">
              {status.data.warnings.map((warning) => (
                <li key={warning} className="flex items-start gap-2 border-l-2 border-line-2 pl-2">
                  <AlertTriangle className="mt-0.5 size-3 shrink-0 text-warn" aria-hidden="true" />
                  <span>{warning}</span>
                </li>
              ))}
            </ul>
          )}

          <div className="mt-4 flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="primary"
              icon={<Layers className="size-3.5" />}
              loading={busy === "sync"}
              disabled={!status.data.appsPath || Boolean(busy)}
              onClick={() => void sync(false)}
            >
              {t("Publish library")}
            </Button>
            <Button
              size="sm"
              variant="default"
              icon={<Eye className="size-3.5" />}
              loading={busy === "preview"}
              disabled={!status.data.appsPath || Boolean(busy)}
              onClick={() => void sync(true)}
            >
              {t("Preview changes")}
            </Button>
            <Button
              size="sm"
              variant="danger"
              icon={<Trash2 className="size-3.5" />}
              disabled={!status.data.appsPath || status.data.managedEntries === 0 || Boolean(busy)}
              onClick={() => setConfirmRemove(true)}
            >
              {t("Remove Ludex apps")}
            </Button>
          </div>

          {syncError && (
            <div role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12px] text-danger">
              <p className="leading-relaxed">{syncError.message}</p>
              {syncError.needsElevation && isNative() && status.data.platform === "windows" && (
                <div className="mt-2">
                  <p className="max-w-[68ch] text-[11px] leading-relaxed text-ink-3">
                    {t("This grants your Windows account modify rights on Sunshine's config folder, including sunshine.conf. Only needed for a Program Files install; Windows asks once, and cancelling changes nothing.")}
                  </p>
                  <Button
                    size="sm"
                    variant="default"
                    className="mt-2"
                    icon={<ShieldCheck className="size-3.5" />}
                    loading={granting}
                    disabled={Boolean(busy)}
                    onClick={() => void grantAccess()}
                  >
                    {t("Grant write access")}
                  </Button>
                </div>
              )}
            </div>
          )}

          {backupPath && (
            <p className="mt-3 flex items-center gap-2 text-[11px] text-ink-3">
              <CheckCircle2 className="size-3 shrink-0 text-ok" aria-hidden="true" />
              <span className="min-w-0">
                {t("Backup:")} <span className="break-all font-mono">{backupPath}</span>
              </span>
            </p>
          )}

          {preview.isError && (
            <p role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12px] text-danger">
              {errorMessage(preview.error)}
            </p>
          )}

          {preview.data && (
            <div className="mt-4">
              <p className="text-[11.5px] text-ink-3">{preview.data.message}</p>
              {preview.data.entries.length > 0 && (
                <ul className="mt-2 max-h-[240px] overflow-y-auto border-t border-line">
                  {preview.data.entries.map((entry) => (
                    <li key={entry.gameId} className="flex items-center gap-2 border-b border-line py-1.5">
                      <ActionBadge action={entry.action} />
                      <span className="min-w-0 flex-1 truncate text-[12px] text-ink" title={entry.name}>
                        {entry.name}
                      </span>
                      {entry.console && <Badge tone="signal">{t("console")}</Badge>}
                      {entry.needsLudexScheme && (
                        <span title={t("This entry opens a ludex:// URI; add an executable to launch it directly.")}>
                          <AlertTriangle className="size-3 text-warn" aria-label={t("Needs a ludex:// handler")} />
                        </span>
                      )}
                      <span className="max-w-[45%] truncate font-mono text-[10px] text-ink-3" title={entry.cmd}>
                        {entry.cmd}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              {preview.data.removals.length > 0 && (
                <p className="mt-2 text-[11.5px] text-ink-3">
                  {t("To remove: {names}", { names: preview.data.removals.map((item) => item.name).join(", ") })}
                </p>
              )}
              {preview.data.skipped.length > 0 && (
                <ul className="mt-2 flex flex-col gap-1 text-[11.5px] text-ink-3">
                  {preview.data.skipped.map((item) => (
                    <li key={item.gameId}>
                      {item.title}: {item.reason}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}

          <div className="mt-5 grid gap-4">
            <label className="flex flex-col gap-1.5">
              <span className="text-[12.5px] font-medium text-ink">{t("apps.json override")}</span>
              <Input
                value={draft.sunshineAppsPath}
                onChange={(event) => update({ sunshineAppsPath: event.target.value })}
                placeholder={status.data.appsPath ?? "/home/me/.config/sunshine/apps.json"}
                spellCheck={false}
                className="h-9 font-mono text-[11px]"
              />
              <span className="text-[11px] text-ink-3">
                {t("Save the desktop settings, then rescan. Empty = detect Sunshine automatically.")}
              </span>
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-[12.5px] font-medium text-ink">{t("sunshine.conf override")}</span>
              <Input
                value={draft.sunshineConfPath}
                onChange={(event) => update({ sunshineConfPath: event.target.value })}
                placeholder={status.data.confPath ?? "/home/me/.config/sunshine/sunshine.conf"}
                spellCheck={false}
                className="h-9 font-mono text-[11px]"
              />
            </label>
          </div>

          <p className="mt-3 flex items-start gap-2 text-[11px] leading-relaxed text-ink-3">
            <FolderOpen className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
            <span>
              {t("Only entries carrying the Ludex marker are updated or removed. Your own Sunshine apps are never touched, and nothing is written without this button.")}
            </span>
          </p>
          <p className="mt-2 flex items-start gap-2 text-[11px] leading-relaxed text-ink-3">
            <ShieldCheck className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
            <span>
              {t("When Windows denies a write to a Program Files install, the publish error offers a one-time elevation that grants this account write access; nothing is elevated automatically.")}
            </span>
          </p>
        </>
      )}

      <Dialog
        open={confirmRemove}
        onOpenChange={setConfirmRemove}
        title={t("Remove the Ludex apps from Sunshine?")}
        description={t("Every entry Ludex published is removed from apps.json. User-created apps stay. A backup is written first.")}
        size="sm"
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setConfirmRemove(false)}>
              {t("Cancel")}
            </Button>
            <Button variant="danger" loading={busy === "remove"} onClick={() => void removeManaged()}>
              {t("Remove Ludex apps")}
            </Button>
          </div>
        }
      />
    </div>
  );
}

function MoonlightPanel({
  draft,
  update,
}: {
  draft: LocalSettings;
  update: (patch: Partial<LocalSettings>) => void;
}) {
  const status = useQuery({
    queryKey: ["moonlight-status"],
    queryFn: () => native.moonlightStatus(),
    retry: false,
    staleTime: 15_000,
  });

  return (
    <div className="border border-line p-5">
      <div className="flex items-start justify-between gap-4">
        <div className="flex items-center gap-2">
          <Wifi className="size-4 text-ink-3" aria-hidden="true" />
          <h4 className="text-[13.5px] font-semibold text-ink">{t("Moonlight client")}</h4>
          {status.data && (
            <Badge tone={status.data.detected ? "ok" : "outline"}>
              {status.data.detected ? t("detected") : t("not found")}
            </Badge>
          )}
        </div>
        <Button
          size="sm"
          variant="ghost"
          icon={<RefreshCw className="size-3.5" />}
          loading={status.isFetching}
          onClick={() => void status.refetch()}
        >
          {t("Rescan")}
        </Button>
      </div>

      {status.isPending && (
        <div className="mt-4 flex flex-col gap-2">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      )}
      {status.isError && (
        <p role="alert" className="mt-4 border-l-2 border-danger pl-3 text-[12px] text-danger">
          {errorMessage(status.error)}
        </p>
      )}

      {status.data && (
        <>
          <p className="mt-3 text-[12px] leading-relaxed text-ink-3">{status.data.message}</p>
          <dl className="mt-4 border-t border-line">
            {[
              [t("Moonlight"), status.data.binary ?? t("not detected")],
              [t("Host list"), status.data.configPath ?? status.data.configSource ?? t("not found")],
            ].map(([label, value]) => (
              <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2">
                <dt className="shrink-0 text-[12px] text-ink-3">{label}</dt>
                <dd className="min-w-0 truncate font-mono text-[10.5px] text-ink-2" title={value}>
                  {value}
                </dd>
              </div>
            ))}
          </dl>

          {status.data.configSource && status.data.configPath && (
            <p className="mt-2 text-[11px] text-ink-3">
              {t("Read from {source}.", { source: status.data.configSource })}
            </p>
          )}

          {status.data.hosts.length > 0 ? (
            <ul className="mt-3 border-t border-line">
              {status.data.hosts.map((host) => (
                <li key={`${host.name}-${host.address}`} className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-line py-2">
                  <span className="text-[12.5px] text-ink">{host.name}</span>
                  <span className="font-mono text-[10.5px] text-ink-3">{host.address}</span>
                  <span className="ml-auto">
                    <Badge tone={host.paired ? "ok" : "outline"}>{host.paired ? t("paired") : t("not paired")}</Badge>
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="mt-3 flex items-start gap-2 text-[11.5px] text-ink-3">
              <ServerOff className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
              <span>{t("No host yet. Pair a Sunshine host in Moonlight once; Ludex then reuses that pairing.")}</span>
            </p>
          )}

          {status.data.warnings.length > 0 && (
            <ul className="mt-3 flex flex-col gap-1.5 text-[11.5px] leading-relaxed text-ink-3">
              {status.data.warnings.map((warning) => (
                <li key={warning} className="flex items-start gap-2 border-l-2 border-line-2 pl-2">
                  <AlertTriangle className="mt-0.5 size-3 shrink-0 text-warn" aria-hidden="true" />
                  <span>{warning}</span>
                </li>
              ))}
            </ul>
          )}

          <p className="mt-3 text-[11px] leading-relaxed text-ink-3">
            {t("Published games gain a Stream action on their page. Moonlight opens the stream; Ludex stays out of the video path.")}
          </p>

          <div className="mt-5 grid gap-4">
            <label className="flex flex-col gap-1.5">
              <span className="text-[12.5px] font-medium text-ink">{t("Moonlight override")}</span>
              <Input
                value={draft.moonlightBinary}
                onChange={(event) => update({ moonlightBinary: event.target.value })}
                placeholder={status.data.binary ?? "/usr/bin/moonlight"}
                spellCheck={false}
                className="h-9 font-mono text-[11px]"
              />
              <span className="text-[11px] text-ink-3">
                {t("Save the desktop settings, then rescan. Empty = look on PATH and in the usual install folders.")}
              </span>
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-[12.5px] font-medium text-ink">{t("Moonlight config override")}</span>
              <Input
                value={draft.moonlightConfigPath}
                onChange={(event) => update({ moonlightConfigPath: event.target.value })}
                placeholder={status.data.configPath ?? "~/.config/Moonlight Game Streaming Project/Moonlight.conf"}
                spellCheck={false}
                className="h-9 font-mono text-[11px]"
              />
              <span className="text-[11px] text-ink-3">
                {t("Optional. Portable builds keep Moonlight.ini next to Moonlight.exe; installed Windows builds keep their settings in the registry, which Ludex reads without writing.")}
              </span>
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-[12.5px] font-medium text-ink">{t("Manual Moonlight host")}</span>
              <Input
                value={draft.moonlightHost}
                onChange={(event) => update({ moonlightHost: event.target.value })}
                placeholder={status.data.manualHost ?? status.data.hosts[0]?.address ?? "192.168.1.20"}
                spellCheck={false}
                className="h-9 font-mono text-[11px]"
              />
              <span className="text-[11px] text-ink-3">
                {t("Host name or IP used when Ludex cannot read Moonlight's saved hosts. The Stream action still runs moonlight stream <host> <app>.")}
              </span>
            </label>
          </div>
        </>
      )}
    </div>
  );
}
