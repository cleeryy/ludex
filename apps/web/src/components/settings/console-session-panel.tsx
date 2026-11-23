import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import type {
  ConsoleSessionEntryPlan,
  ConsoleSessionEntryState,
  ConsoleSessionPlan,
  ConsoleSessionStatus,
} from "@/lib/types";
import { t } from "@/lib/i18n";
import {
  canToggle,
  entryAction,
  managerLabel,
  needsSeatAttention,
  planCounts,
  plannedFileKind,
  toggleExplanation,
  type ToggleExplanation,
} from "@/lib/console-session";
import { Button } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import {
  DesktopOnlyNote,
  SettingsGroup,
  SettingsLoading,
  SettingsSwitchRow,
} from "@/components/settings/settings-section";

function entryStateLabel(state: ConsoleSessionEntryState): string {
  switch (state) {
    case "installed":
      return t("installed");
    case "differs":
      return t("a different file already exists");
    case "missing":
      return t("not installed");
  }
}

/**
 * Settings → Console (#286): the shipped Wayland session, and the autologin
 * configuration that makes it the boot target. The switch never applies a
 * plan the user has not seen: the backend's dry-run is the dialog's body.
 */
export function ConsoleSessionPanel() {
  const [status, setStatus] = useState<ConsoleSessionStatus | null>(null);
  const [entry, setEntry] = useState<ConsoleSessionEntryPlan | null>(null);
  const [plan, setPlan] = useState<ConsoleSessionPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const nextStatus = await native.consoleSessionStatus();
      setStatus(nextStatus);
      setEntry(await native.consoleSessionEntryPlan().catch(() => null));
      setError(null);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (isNative()) void refresh();
  }, [refresh]);

  const requestToggle = async (enable: boolean) => {
    setBusy(true);
    setError(null);
    try {
      setPlan(await native.consoleSessionPlan({ enable }));
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const confirmPlan = async () => {
    if (!plan) return;
    setBusy(true);
    setError(null);
    try {
      const next = await native.consoleSessionApply({ enable: plan.enable, confirm: true });
      setStatus(next);
      setPlan(null);
      setEntry(await native.consoleSessionEntryPlan().catch(() => null));
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const changeEntry = async (install: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const next = install
        ? await native.consoleSessionInstallEntry({ confirm: true })
        : await native.consoleSessionUninstallEntry({ confirm: true });
      setEntry(next);
      setStatus(await native.consoleSessionStatus());
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  if (!isNative()) {
    return (
      <div className="border-t border-line pt-5">
        <DesktopOnlyNote message={t("The boot-into-console switch is available in the Ludex desktop app on Linux.")} />
      </div>
    );
  }
  if (loading && !status) return <SettingsLoading label={t("Reading the display manager…")} />;
  if (!status) {
    return (
      <p role="alert" className="border-t border-line pt-5 text-body text-danger">
        {error ?? t("The console session status is unavailable.")}
      </p>
    );
  }

  const explained = toggleExplanation(status);
  const counts = plan ? planCounts(plan) : null;
  const entryMode = entryAction(status.entryState);

  return (
    <div className="border-t border-line pt-5">
      {error && (
        <p role="alert" className="mb-4 border border-danger/40 bg-danger/5 px-3 py-2 text-label text-danger">
          {error}
        </p>
      )}

      <div className="flex flex-wrap items-baseline gap-x-5 gap-y-1 font-mono text-micro text-ink-3">
        <span>
          {t("Session manager:")} {managerLabel(status) ?? t("not detected")}
        </span>
        {status.user && (
          <span>
            {t("Autologin user:")} {status.user}
          </span>
        )}
        <span>
          {t("Session entry:")} {entryStateLabel(status.entryState)}
        </span>
      </div>

      <div className="mt-5">
        <SettingsSwitchRow
          title={t("Boot straight into console mode")}
          description={<ToggleDescription explained={explained} />}
          checked={status.enabled}
          disabled={!canToggle(status) || busy}
          onCheckedChange={(checked) => void requestToggle(checked)}
        />
      </div>

      {needsSeatAttention(status) && (
        <p className="mt-3 max-w-[70ch] border-s-2 border-signal/60 ps-3 text-meta leading-relaxed text-ink-3">
          {status.seat.note}
        </p>
      )}

      <SettingsGroup
        title={t("Wayland session entry")}
        description={t("The session that runs `ludex --console`. Install it to pick Ludex Console from the display manager's session list; the boot switch installs it automatically.")}
      >
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line pb-4">
          <div className="min-w-0">
            <p className="break-all font-mono text-micro text-ink-3">{status.entryPath}</p>
            <p className="mt-1 text-meta leading-relaxed text-ink-3">
              {entry?.message ?? t("Session entry state is unavailable.")}
            </p>
          </div>
          <div className="flex shrink-0 gap-2">
            {entryMode === "install" && (
              <Button onClick={() => void changeEntry(true)} loading={busy}>
                {t("Install session entry")}
              </Button>
            )}
            {entryMode === "remove" && (
              <Button variant="ghost" onClick={() => void changeEntry(false)} loading={busy}>
                {t("Remove session entry")}
              </Button>
            )}
            {entryMode === "conflict" && (
              <span className="text-meta text-danger">
                {t("Another program's session file owns this name; Ludex will not overwrite it.")}
              </span>
            )}
          </div>
        </div>
        {entry && entryMode !== "conflict" && (
          <details className="mt-3">
            <summary className="cursor-pointer text-meta text-ink-3">
              {t("Exact commands for the session entry")}
            </summary>
            <pre className="mt-2 overflow-x-auto border border-line bg-surface-2 p-3 font-mono text-micro leading-relaxed text-ink-2">
              {entry.installCommand}
              {"\n"}
              {entry.uninstallCommand}
            </pre>
          </details>
        )}
      </SettingsGroup>

      <p className="mt-6 max-w-[70ch] text-meta leading-relaxed text-ink-3">
        {t("Quit console mode returns to the desktop; suspend, restart and shut down live in the Console Power surface.")}
      </p>

      <Dialog
        open={plan !== null}
        onOpenChange={(open) => {
          if (!open) setPlan(null);
        }}
        title={
          plan?.enable
            ? t("Enable boot into console mode")
            : t("Disable boot into console mode")
        }
        size="lg"
        footer={
          <>
            <Button variant="ghost" onClick={() => setPlan(null)}>
              {t("Cancel")}
            </Button>
            <Button
              variant={plan?.enable ? "primary" : "danger"}
              onClick={() => void confirmPlan()}
              loading={busy}
              autoFocus
            >
              {plan?.enable ? t("Enable") : t("Disable")}
            </Button>
          </>
        }
      >
        {plan && (
          <div className="space-y-4">
            <p className="text-label leading-relaxed text-ink-2">{plan.message}</p>
            {counts && (
              <p className="font-mono text-micro text-ink-3">
                {t("{writes} write(s), {restores} restore(s), {removes} removal(s), {backups} backup(s)", {
                  writes: counts.writes,
                  restores: counts.restores,
                  removes: counts.removes,
                  backups: counts.backups,
                })}
              </p>
            )}
            <ul className="border-t border-line">
              {plan.files.map((file) => (
                <li key={file.path} className="border-b border-line py-3">
                  <div className="flex flex-wrap items-baseline justify-between gap-2">
                    <span className="text-label font-medium text-ink">{file.description}</span>
                    <span className="text-micro uppercase tracking-[0.08em] text-ink-3">
                      {plannedFileKind(file, plan.enable) === "write"
                        ? t("write")
                        : plannedFileKind(file, plan.enable) === "restore"
                          ? t("restore")
                          : t("remove")}
                    </span>
                  </div>
                  <p className="mt-1 break-all font-mono text-micro text-ink-3">{file.path}</p>
                  {file.backup && (
                    <p className="mt-1 break-all font-mono text-micro text-ink-3">
                      {t("Backup: {path}", { path: file.backup })}
                    </p>
                  )}
                  {file.action === "write" && (
                    <details className="mt-2">
                      <summary className="cursor-pointer text-meta text-ink-3">
                        {t("Contents")}
                      </summary>
                      <pre className="mt-1 max-h-56 overflow-auto border border-line bg-surface-2 p-2 font-mono text-micro leading-relaxed text-ink-2">
                        {file.contents}
                      </pre>
                    </details>
                  )}
                </li>
              ))}
            </ul>
            {plan.warnings.map((warning) => (
              <p key={warning} className="text-meta leading-relaxed text-ink-3">
                {warning}
              </p>
            ))}
            <details>
              <summary className="cursor-pointer text-meta text-ink-3">
                {t("Commands run with administrator rights")}
              </summary>
              <pre className="mt-2 max-h-56 overflow-auto border border-line bg-surface-2 p-2 font-mono text-micro leading-relaxed text-ink-2">
                pkexec /bin/sh -c
                {"\n"}
                {plan.commands.join("\n")}
              </pre>
            </details>
          </div>
        )}
      </Dialog>
    </div>
  );
}

function ToggleDescription({ explained }: { explained: ToggleExplanation }) {
  switch (explained.kind) {
    case "unsupported":
      return <>{explained.reason}</>;
    case "polkit":
      return <>{t("Changing display manager configuration needs `pkexec` (polkit). Install polkit to enable this switch.")}</>;
    case "refusal":
      return <>{explained.reason}</>;
    case "enabled":
      return (
        <>
          {t("Boot-into-console is enabled through {manager}. Disable it to restore the previous boot behaviour.", {
            manager: explained.manager,
          })}
        </>
      );
    case "ready":
      return (
        <>
          {t("Ludex will configure {manager} to log this user in and start the console session. Every file is backed up first, and the dry-run is shown before anything changes.", {
            manager: explained.manager,
          })}
        </>
      );
  }
}
