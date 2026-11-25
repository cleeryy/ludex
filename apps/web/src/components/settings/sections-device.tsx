import type { ReactNode } from "react";
import { FolderOpen } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative } from "@/lib/native";
import { setAdvancedTiersUnlocked, useAdvancedTiersUnlocked } from "@/lib/settings-tiers";
import type { LocalSettings } from "@/lib/types";
import { t, tr } from "@/lib/i18n";
import { useAuth } from "@/state/auth";
import { usePreferences } from "@/state/preferences";
import { Button, Input } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { CompatibilityPanel } from "@/components/settings/compatibility-panel";
import { DesktopUpdatePanel } from "@/components/settings/desktop-update";
import { HostPreflightPanel } from "@/components/settings/host-preflight-panel";
import { InstalledPanel } from "@/components/settings/installed-games";
import { KeepAwakeStatus } from "@/components/settings/keep-awake-panel";
import { LauncherImportPanel } from "@/components/settings/launcher-import";
import { LocationsPanel } from "@/components/settings/locations-panel";
import { PresenceStatus } from "@/components/settings/presence-status";
import { SaveDiscoveryPanel } from "@/components/settings/save-discovery-panel";
import { SaveManifestPanel } from "@/components/settings/save-manifest-panel";
import { StreamingPanel } from "@/components/settings/streaming-panel";
import {
  DesktopOnlyNote,
  DesktopSaveBar,
  SettingsGroup,
  SettingsLoading,
  SettingsSwitchRow,
} from "@/components/settings/settings-section";
import type { DesktopSettingsController } from "@/components/settings/use-desktop-settings";
import { ServersPanel } from "@/components/shell/server-switcher";

/**
 * The categories that edit the desktop settings file. They share one draft and
 * one save (see `useDesktopSettings`), and the save bar is rendered next to the
 * controls it commits.
 */
export function DesktopBody({
  desktop,
  note,
  children,
}: {
  desktop: DesktopSettingsController;
  note?: string;
  children: (draft: LocalSettings) => ReactNode;
}) {
  if (!desktop.available) {
    return (
      <div className="border-t border-line pt-5">
        <DesktopOnlyNote message={note} />
      </div>
    );
  }
  if (desktop.pending) return <SettingsLoading label={t("Loading desktop settings…")} />;
  if (desktop.error) {
    return (
      <p role="alert" className="border-t border-line pt-5 text-body text-danger">
        {errorMessage(desktop.error)}
      </p>
    );
  }
  if (!desktop.draft) return null;
  return <div className="border-t border-line pt-5">{children(desktop.draft)}</div>;
}

export function DesktopModeLine({ draft }: { draft: LocalSettings }) {
  return (
    <div className="mb-6 flex flex-wrap items-baseline gap-x-6 gap-y-1 font-mono text-micro text-ink-3">
      <span>
        {t("Mode:")} {draft.mode === "local" ? t("Local library") : t("Remote server")}
      </span>
      {draft.serverUrl && (
        <span className="truncate">
          {t("Server:")} {draft.serverUrl}
        </span>
      )}
      {!isNative() && <span className="text-signal-text">{t("Preview values from the demo adapter")}</span>}
    </div>
  );
}

/** Storage: install roots, what is already on this machine, GameVault folders. */
export function StorageBody({ desktop }: { desktop: DesktopSettingsController }) {
  return (
    <DesktopBody desktop={desktop}>
      {(draft) => (
        <>
          <DesktopModeLine draft={draft} />
          <LocationsPanel
            locations={draft.locations ?? []}
            // Any edit of the folder list is an explicit install-root choice
            // (#145): later installs keep their saved default.
            onChange={(locations) => desktop.update({ locations, installRootConfirmed: true })}
          />
          {isNative() && (
            <SettingsGroup
              title={t("Installed")}
              description={t("What this machine has, and the GameVault folders Ludex can link without copying them.")}
            >
              <InstalledPanel />
            </SettingsGroup>
          )}
          <DesktopSaveBar desktop={desktop} openFolder />
        </>
      )}
    </DesktopBody>
  );
}

export { DownloadsBody } from "@/components/settings/downloads-section";

/** Saves and cloud: automatic snapshots, the save database and save locations. */
export function SavesBody({ desktop }: { desktop: DesktopSettingsController }) {
  const unlocked = useAdvancedTiersUnlocked();
  return (
    <DesktopBody
      desktop={desktop}
      note={t("Automatic cloud saves and the save database need the Ludex desktop app. Manual snapshots still work in the browser.")}
    >
      {(draft) => (
        <>
          <SettingsGroup
            title={t("Automatic cloud saves")}
            description={
              isNative()
                ? t("Back up the detected save folders after each session. Individual games can turn this off on their page; the server keeps the newest 10 automatic snapshots per game and never prunes manual ones.")
                : t("Automatic cloud saves need the Ludex desktop app: they read and write save files on this machine. The browser can still download and upload manual snapshots.")
            }
          >
            {isNative() && (
              <SettingsSwitchRow
                title={t("Back up automatically")}
                description={t("Runs after each session, in the folders detected for the game.")}
                checked={draft.cloudAutoSync}
                onCheckedChange={(cloudAutoSync) => desktop.update({ cloudAutoSync })}
              />
            )}
            {isNative() && (
              <div className="grid gap-4 pt-4 sm:grid-cols-2">
                <label className="flex flex-col gap-1.5">
                  <span className="text-label text-ink-2">{t("Device name")}</span>
                  <Input
                    value={draft.cloudDeviceLabel ?? ""}
                    maxLength={60}
                    placeholder={t("Living room PC")}
                    onChange={(event) => desktop.update({ cloudDeviceLabel: event.target.value || null })}
                    className="h-11 text-body"
                  />
                  <span className="text-micro leading-relaxed text-ink-3">
                    {t("Shown on automatic snapshots. Leave empty to use the computer name.")}
                  </span>
                </label>
                {unlocked && (
                  <div className="flex flex-col gap-1.5">
                    <span className="text-label text-ink-2">{t("Device identifier")}</span>
                    <p className="truncate border border-line bg-surface px-3 py-2.5 font-mono text-meta text-ink-3">
                      {draft.cloudDeviceId || t("Generated on first upload")}
                    </p>
                    <span className="text-micro leading-relaxed text-ink-3">
                      {t("A random id stored in this app's settings, not a hardware fingerprint.")}
                    </span>
                  </div>
                )}
              </div>
            )}
          </SettingsGroup>

          <p className="mt-6 max-w-[86ch] border-l-2 border-line-2 pl-3 text-meta leading-relaxed text-ink-3">
            {t("Privacy: snapshots are uploaded to the Ludex server you are signed in to, under your account, and only you can list or download them. The server stores the archive as-is and never opens it. Host paths, device names and file contents stay between this machine and your server.")}
          </p>

          <div className="mt-8 border-t border-line pt-5">
            <SaveDiscoveryPanel />
          </div>

          {unlocked ? (
            <div className="mt-8 border-t border-line pt-5">
              <SaveManifestPanel draft={draft} update={desktop.update} />
            </div>
          ) : (
            <SettingsGroup
              title={t("Advanced save settings")}
              description={t("The device identifier and Ludusavi save manifest database are hidden while Power-user mode is off.")}
            >
              <Button variant="default" size="sm" onClick={() => setAdvancedTiersUnlocked(true)}>
                {t("Show advanced save options")}
              </Button>
            </SettingsGroup>
          )}

          <DesktopSaveBar desktop={desktop} />
        </>
      )}
    </DesktopBody>
  );
}

/** Integrations: other launchers, Discord, and the servers this app knows. */
export function IntegrationsBody({ desktop }: { desktop: DesktopSettingsController }) {
  return (
    <>
      <DesktopBody
        desktop={desktop}
        note={t("Launcher imports, the Steam folder and Discord presence need the Ludex desktop app. Servers work here.")}
      >
        {(draft) => (
          <>
            {isNative() ? (
              <>
                <SettingsGroup
                  title={t("Launchers")}
                  description={t("Import games another launcher installed, Steam included, without copying a byte.")}
                >
                  <LauncherImportPanel settings={draft} onSourcesChange={desktop.applyLauncherSources} />
                </SettingsGroup>

                <SettingsGroup title={t("Steam")}>
                  <PathField
                    label={t("Steam folder")}
                    hint={t("Optional. Only used by the Steam import when auto-detection fails.")}
                    value={draft.steamRoot}
                    onChange={(value) => desktop.update({ steamRoot: value })}
                    onPick={() => void desktop.pick("steamRoot", t("Choose the Steam folder"))}
                  />
                </SettingsGroup>

                <SettingsGroup
                  title={t("Discord Rich Presence")}
                  description={t("Show the running game, its cover and the elapsed time on your Discord profile. Uses the local Discord app, never a bot token. Off by default.")}
                >
                  <SettingsSwitchRow
                    title={t("Discord Rich Presence")}
                    description={t("Show the running game, its cover and the elapsed time on your Discord profile.")}
                    checked={draft.discordPresence}
                    onCheckedChange={(discordPresence) => desktop.update({ discordPresence })}
                  />
                  <div className="flex flex-wrap items-center gap-3 pt-4">
                    <div className="w-[240px]">
                      <Input
                        id="discord-client-id"
                        aria-label={t("Discord application id")}
                        value={draft.discordClientId}
                        onChange={(event) => desktop.update({ discordClientId: event.target.value })}
                        placeholder={t("Discord application id")}
                        spellCheck={false}
                        className="font-mono text-meta"
                      />
                    </div>
                    <span className="text-meta text-ink-3">{t("An application id labels the activity; it is not a secret.")}</span>
                  </div>
                  <PresenceStatus enabled={draft.discordPresence} />
                </SettingsGroup>

                <SettingsGroup
                  title={t("Remote installs")}
                  description={t("This PC appears on the phone's remote-install screen once Ludex is signed in. The name below is what you pick there.")}
                >
                  <div className="grid gap-4 sm:grid-cols-2">
                    <label className="flex flex-col gap-1.5">
                      <span className="text-label text-ink-2">{t("Device name")}</span>
                      <Input
                        value={draft.deviceName ?? ""}
                        maxLength={80}
                        placeholder={t("Gaming PC")}
                        onChange={(event) => desktop.update({ deviceName: event.target.value || null })}
                        className="h-11 text-body"
                      />
                      <span className="text-micro leading-relaxed text-ink-3">
                        {t("Shown on the phone. Leave empty to use the computer name.")}
                      </span>
                    </label>
                    <div className="flex flex-col gap-1.5">
                      <span className="text-label text-ink-2">{t("Device identifier")}</span>
                      <p className="truncate border border-line bg-surface px-3 py-2.5 font-mono text-meta text-ink-3">
                        {draft.deviceId || t("Generated on first heartbeat")}
                      </p>
                      <span className="text-micro leading-relaxed text-ink-3">
                        {t("A random id stored in this app's settings, not a hardware fingerprint.")}
                      </span>
                    </div>
                  </div>
                </SettingsGroup>

                <DesktopSaveBar desktop={desktop} />
              </>
            ) : (
              <DesktopOnlyNote
                message={t("Launcher imports, the Steam folder and Discord presence need the Ludex desktop app. Servers work here.")}
              />
            )}
          </>
        )}
      </DesktopBody>

      <div className="mt-10 border-t border-line pt-5">
        <ServersPanel />
      </div>
    </>
  );
}

/** Updates: the build, its channel, and when it looks for a new one. */
export function UpdatesBody({ desktop }: { desktop: DesktopSettingsController }) {
  const { setup } = useAuth();
  const { preferences, update } = usePreferences();
  return (
    <>
      <div className="border-t border-line pt-5">
        <dl>
          {[
            [t("Version"), `v${setup?.version ?? "0.1.0"}`],
            [t("Server"), setup?.serverName ?? "Ludex"],
          ].map(([label, value]) => (
            <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
              <dt className="text-label text-ink-3">{label}</dt>
              <dd className="num text-label text-ink">{value}</dd>
            </div>
          ))}
        </dl>
      </div>

      <SettingsGroup
        title={t("Update reminders")}
        description={t("Follows your account. The update channel and the versions you skipped stay on this device.")}
      >
        <div className="w-[260px]">
          <Select
            label={t("Update reminders")}
            value={preferences.updates.prompt}
            onChange={(prompt) => update({ updates: { prompt } })}
            options={[
              { value: "always", label: t("Always") },
              { value: "notify", label: t("Badge only") },
              { value: "off", label: t("Off") },
            ]}
          />
        </div>
      </SettingsGroup>

      <SettingsGroup
        title={t("Update notifications")}
        description={t("When a new build is published. The badge and the update center ignore this: the update is never downloaded on its own.")}
      >
        <div className="w-[260px]">
          <Select
            label={t("Update notifications")}
            value={preferences.updates.notifications}
            onChange={(notifications) => update({ updates: { notifications } })}
            options={[
              {
                value: "app-and-os",
                label: t("In-app and OS notifications"),
                hint: isNative() ? t("Desktop only") : undefined,
              },
              { value: "app", label: t("In-app only") },
              { value: "off", label: t("Off") },
            ]}
          />
        </div>
      </SettingsGroup>

      <DesktopBody
        desktop={desktop}
        note={t("App updates need the desktop build: the browser always runs the build your server serves. Release notes and downloads are on GitHub.")}
      >
        {(draft) => (
          <>
            <SettingsGroup title={t("Update channel")}>
              <p className="mb-2 max-w-[54ch] text-meta leading-relaxed text-ink-3">
                {tr("Early access builds come from the {channel} prerelease and can be rough. Save the settings to switch.", {
                  channel: <span className="font-mono">edge</span>,
                })}
              </p>
              <div className="w-[240px]">
                <Select
                  label={t("Update channel")}
                  value={draft.updateChannel}
                  onChange={(updateChannel) => desktop.update({ updateChannel })}
                  options={[
                    { value: "stable", label: t("Stable"), hint: t("Latest release") },
                    { value: "early", label: t("Early access"), hint: t("edge prerelease") },
                  ]}
                />
              </div>
            </SettingsGroup>

            <SettingsGroup title={t("At startup")}>
              <SettingsSwitchRow
                title={t("Check for updates at startup")}
                description={t("At most once every 24 hours. Manual checks from this page always run.")}
                checked={draft.checkUpdatesOnStartup}
                onCheckedChange={(checkUpdatesOnStartup) => desktop.update({ checkUpdatesOnStartup })}
              />
            </SettingsGroup>

            <div className="mt-8 border-t border-line pt-5">
              <DesktopUpdatePanel />
            </div>

            <DesktopSaveBar desktop={desktop} />
          </>
        )}
      </DesktopBody>
    </>
  );
}

/** Streaming: Sunshine and Moonlight (desktop only, hidden elsewhere). */
export function StreamingBody({ desktop }: { desktop: DesktopSettingsController }) {
  return (
    <DesktopBody desktop={desktop}>
      {(draft) => (
        <>
          <StreamingPanel draft={draft} update={desktop.update} />
          <DesktopSaveBar desktop={desktop} />
        </>
      )}
    </DesktopBody>
  );
}

/** Compatibility: Steam, Proton, UMU and Wine detection. */
export function CompatibilityBody({ desktop }: { desktop: DesktopSettingsController }) {
  return (
    <DesktopBody desktop={desktop}>
      {(draft) => (
        <>
          <CompatibilityPanel draft={draft} update={desktop.update} />
          <DesktopSaveBar desktop={desktop} />
        </>
      )}
    </DesktopBody>
  );
}

/**
 * Host preflight (#285): read-only checks plus an explicit install consent.
 * Nothing here edits the settings draft, so there is no save bar.
 */
export function HostBody({ desktop }: { desktop: DesktopSettingsController }) {
  if (!desktop.available) {
    return (
      <div className="border-t border-line pt-5">
        <DesktopOnlyNote message={t("Host checks need the Ludex desktop app on Linux.")} />
      </div>
    );
  }
  return (
    <div className="border-t border-line pt-5">
      <HostPreflightPanel />
      <div className="mt-6 border-t border-line pt-5">
        <h3 className="text-body font-medium text-ink">{t("Keep the machine awake")}</h3>
        <div className="mt-2">
          <KeepAwakeStatus />
        </div>
      </div>
    </div>
  );
}

function PathField({
  label,
  hint,
  value,
  onChange,
  onPick,
}: {  label: string;
  hint: string;
  value: string;
  onChange: (value: string) => void;
  onPick: () => void;
}) {
  const id = `path-${label.toLowerCase().replace(/\s+/g, "-")}`;
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-body font-medium text-ink" htmlFor={id}>
        {label}
      </label>
      <div className="flex items-stretch gap-2">
        <Input
          id={id}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          spellCheck={false}
          className="h-11 font-mono text-meta"
          placeholder="/home/user/Games"
        />
        <Button variant="default" size="sm" className="h-11 shrink-0" onClick={onPick} icon={<FolderOpen className="size-3.5" />}>
          {t("Browse")}
        </Button>
      </div>
      <p className="text-meta leading-relaxed text-ink-3">{hint}</p>
    </div>
  );
}
