import { CloudOff, RefreshCw, RotateCcw } from "lucide-react";
import { t } from "@/lib/i18n";
import { formatDate } from "@/lib/format";
import { usePreferences } from "@/state/preferences";
import { Button } from "@/components/ui/controls";
import { SettingsGroup } from "@/components/settings/settings-section";

/**
 * Sync state of the account preference document (#201).
 *
 * Lives in Settings → Account: it reports where the current values came from,
 * when they last reached the server and whether a write is still pending. The
 * reset action only touches the account copy; the device's install paths,
 * window chrome and tokens stay as they are.
 */
export function PreferencesSyncPanel() {
  const {
    status,
    lastSyncedAt,
    source,
    conflictRevision,
    resetToServer,
    useDeviceSettings,
    resolveConflict,
  } = usePreferences();

  const statusLabel = (() => {
    switch (status) {
      case "idle":
        return t("Not signed in");
      case "loading":
        return t("Checking the server…");
      case "synced":
        return t("Synced");
      case "pending":
        return t("Saving…");
      case "offline":
        return t("Offline");
      case "conflict":
        return t("Conflict");
      case "error":
        return t("Sync failed");
    }
  })();

  const sourceLabel = (() => {
    switch (source) {
      case "server":
        return t("Server");
      case "cache":
        return t("Local cache");
      case "device":
        return t("This device");
      case "default":
        return t("Defaults");
    }
  })();

  return (
    <SettingsGroup
      title={t("Settings sync")}
      description={t(
        "Theme, language, Big Picture and update preferences follow your account. Install paths, bandwidth, streaming paths and tokens stay on this device.",
      )}
      actions={
        status === "conflict" ? undefined : (
          <Button
            variant="ghost"
            size="sm"
            icon={<RotateCcw className="size-3.5" />}
            onClick={() => void resetToServer()}
          >
            {t("Reset to server")}
          </Button>
        )
      }
    >
      <dl>
        {[
          [t("Status"), statusLabel],
          [t("Last synced"), lastSyncedAt ? formatDate(lastSyncedAt) : t("Never")],
          [t("Source"), sourceLabel],
        ].map(([label, value]) => (
          <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
            <dt className="text-[12.5px] text-ink-3">{label}</dt>
            <dd className="text-[13px] text-ink">{value}</dd>
          </div>
        ))}
      </dl>

      {status === "offline" && (
        <p className="mt-3 flex items-start gap-2 text-[11.5px] leading-relaxed text-ink-3">
          <CloudOff className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          {t("Changes are queued on this device and sync when the connection returns.")}
        </p>
      )}

      {status === "conflict" ? (
        <div className="mt-4 border-l-2 border-warn pl-3">
          <p className="text-[12.5px] leading-relaxed text-ink">
            {t("Another device changed these settings first. Choose which copy to keep.")}
          </p>
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <Button variant="primary" size="sm" onClick={() => void resolveConflict("device")}>
              {t("Keep this device")}
            </Button>
            <Button variant="default" size="sm" onClick={() => void resolveConflict("server")}>
              {t("Use the server")}
            </Button>
            <span className="num text-[11px] text-ink-3">
              {conflictRevision === null ? null : `r${conflictRevision}`}
            </span>
          </div>
        </div>
      ) : (
        <div className="mt-4 flex flex-wrap items-center gap-3">
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            onClick={() => void useDeviceSettings()}
          >
            {t("Use this device's settings")}
          </Button>
          <span className="text-[11.5px] leading-relaxed text-ink-3">
            {t("Reset removes the account copy and restores the defaults everywhere. This device keeps its paths, window and network settings.")}
          </span>
        </div>
      )}
    </SettingsGroup>
  );
}
