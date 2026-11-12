import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { ChevronRight, FolderOpen } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative } from "@/lib/native";
import { formatBytes } from "@/lib/format";
import type { LocalSettings } from "@/lib/types";
import { t, tn, tr } from "@/lib/i18n";
import { useAuth } from "@/state/auth";
import { usePreferences } from "@/state/preferences";
import { Button, Input, Textarea } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { CompatibilityPanel } from "@/components/settings/compatibility-panel";
import { DesktopUpdatePanel } from "@/components/settings/desktop-update";
import { InstalledPanel } from "@/components/settings/installed-games";
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
function DesktopBody({
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
      <p role="alert" className="border-t border-line pt-5 text-[13px] text-danger">
        {errorMessage(desktop.error)}
      </p>
    );
  }
  if (!desktop.draft) return null;
  return <div className="border-t border-line pt-5">{children(desktop.draft)}</div>;
}

export function DesktopModeLine({ draft }: { draft: LocalSettings }) {
  return (
    <div className="mb-6 flex flex-wrap items-baseline gap-x-6 gap-y-1 font-mono text-[10.5px] text-ink-3">
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

/** Downloads: how archives arrive, how they are kept and how quiet installers run. */
export function DownloadsBody({ desktop }: { desktop: DesktopSettingsController }) {
  return (
    <DesktopBody desktop={desktop}>
      {(draft) => (
        <>
          <SettingsGroup title={t("Installs")}>
            <SettingsSwitchRow
              title={t("Stream installs from the server")}
              description={t("ZIP archives only. Files are written as they arrive, so the archive never touches the disk. A dropped connection, a pause or an app restart resumes at file granularity: completed files are kept and only missing ones are downloaded again. 7z/RAR archives, and servers without the per-entry API, fall back to the normal resumable download.")}
              checked={draft.streamInstall}
              onCheckedChange={(streamInstall) => desktop.update({ streamInstall })}
            />
            <SettingsSwitchRow
              title={t("Keep downloaded archives")}
              description={t("Leave the archive in the download folder after a successful install. Off by default; the desktop deletes it to save space.")}
              checked={draft.keepArchives}
              onCheckedChange={(keepArchives) => desktop.update({ keepArchives })}
            />
          </SettingsGroup>

          <SettingsGroup
            title={t("Bandwidth limit")}
            description={t("Caps every download (installs, repairs and stream installs) in KiB/s. Use 0 for no limit.")}
          >
            <div className="flex flex-wrap items-center gap-2">
              <div className="w-[150px]">
                <Input
                  id="bandwidth-limit"
                  aria-label={t("Bandwidth limit")}
                  type="number"
                  min={0}
                  step={64}
                  inputMode="numeric"
                  value={draft.bandwidthLimitKib}
                  onChange={(event) =>
                    desktop.update({ bandwidthLimitKib: Math.max(0, Math.round(Number(event.target.value) || 0)) })
                  }
                  className="font-mono text-[12px]"
                />
              </div>
              <span className="text-[11.5px] text-ink-3">{t("KiB/s")}</span>
              <span className="num text-[11px] text-ink-3">
                {draft.bandwidthLimitKib > 0 ? `≈ ${formatBytes(draft.bandwidthLimitKib * 1024)}/s` : t("Unlimited")}
              </span>
            </div>
          </SettingsGroup>

          <SettingsGroup title={t("Encrypted archives")}>
            <label htmlFor="archive-passwords" className="text-[13px] font-medium text-ink">
              {t("Archive passwords")}
            </label>
            <p className="mb-2 mt-1 max-w-[54ch] text-[11.5px] leading-relaxed text-ink-3">
              {t("Tried in order when an archive is encrypted. One per line, or separated by commas.")}
            </p>
            <Textarea
              id="archive-passwords"
              value={draft.archivePasswords.join("\n")}
              onChange={(event) =>
                desktop.update({
                  archivePasswords: event.target.value
                    .split(/[\n,]+/)
                    .map((entry) => entry.trim())
                    .filter(Boolean),
                })
              }
              placeholder={"first-password\nsecond-password"}
              rows={3}
              className="min-h-[84px] font-mono text-[12px]"
            />
            <p className="mt-1 text-[11px] text-ink-3">
              {draft.archivePasswords.length === 0
                ? t("No passwords. Encrypted archives will fail to extract.")
                : tn(draft.archivePasswords.length, "1 password stored locally in the desktop settings.", "{count} passwords stored locally in the desktop settings.")}
            </p>
          </SettingsGroup>

          <SettingsGroup title={t("Quiet installers")}>
            <p className="text-[13px] font-medium text-ink">{t("Silent installer arguments")}</p>
            <p className="mb-2 mt-1 max-w-[54ch] text-[11.5px] leading-relaxed text-ink-3">
              {tr("Tried in order when a game ships a setup program, one argument set per line. Leave empty for the built-in presets ({presets}).", {
                presets: (
                  <>
                    <span className="font-mono">/S</span>, <span className="font-mono">/silent</span>,{" "}
                    <span className="font-mono">/VERYSILENT /SUPPRESSMSGBOXES /NORESTART</span>,{" "}
                    <span className="font-mono">--silent</span>
                  </>
                ),
              })}
            </p>
            <Textarea
              id="installer-args"
              value={draft.installerSilentArgs.map((set) => set.join(" ")).join("\n")}
              onChange={(event) =>
                desktop.update({
                  installerSilentArgs: event.target.value
                    .split("\n")
                    .map((line) => line.trim())
                    .filter(Boolean)
                    .map((line) => line.split(/\s+/)),
                })
              }
              placeholder={"/S\n/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"}
              rows={3}
              className="min-h-[78px] font-mono text-[12px]"
            />
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <label htmlFor="installer-timeout" className="text-[12.5px] text-ink-2">
                {t("Timeout per attempt")}
              </label>
              <div className="w-[110px]">
                <Input
                  id="installer-timeout"
                  type="number"
                  min={30}
                  max={7200}
                  step={30}
                  inputMode="numeric"
                  value={draft.installerTimeoutSecs}
                  onChange={(event) =>
                    desktop.update({ installerTimeoutSecs: Math.max(30, Math.round(Number(event.target.value) || 600)) })
                  }
                  className="font-mono text-[12px]"
                />
              </div>
              <span className="text-[11.5px] text-ink-3">{t("seconds")}</span>
            </div>
          </SettingsGroup>

          <div className="mt-6 flex flex-wrap items-center gap-3 border-t border-line pt-4">
            <span className="text-[11.5px] text-ink-3">{t("Finished downloads, history and the queue live on their own screen.")}</span>
            <Link
              to="/downloads"
              className="inline-flex h-11 items-center gap-1 text-[12px] text-ink-2 underline-offset-4 hover:text-ink hover:underline"
            >
              {t("Open the downloads screen")}
              <ChevronRight className="rtl:-scale-x-100 size-3" aria-hidden="true" />
            </Link>
          </div>

          <DesktopSaveBar desktop={desktop} />
        </>
      )}
    </DesktopBody>
  );
}

/** Saves and cloud: automatic snapshots, the save database and save locations. */
export function SavesBody({ desktop }: { desktop: DesktopSettingsController }) {
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
                  <span className="text-[12px] text-ink-2">{t("Device name")}</span>
                  <Input
                    value={draft.cloudDeviceLabel ?? ""}
                    maxLength={60}
                    placeholder={t("Living room PC")}
                    onChange={(event) => desktop.update({ cloudDeviceLabel: event.target.value || null })}
                    className="h-11 text-[13px]"
                  />
                  <span className="text-[10.5px] leading-relaxed text-ink-3">
                    {t("Shown on automatic snapshots. Leave empty to use the computer name.")}
                  </span>
                </label>
                <div className="flex flex-col gap-1.5">
                  <span className="text-[12px] text-ink-2">{t("Device identifier")}</span>
                  <p className="truncate border border-line bg-surface px-3 py-2.5 font-mono text-[11.5px] text-ink-3">
                    {draft.cloudDeviceId || t("Generated on first upload")}
                  </p>
                  <span className="text-[10.5px] leading-relaxed text-ink-3">
                    {t("A random id stored in this app's settings, not a hardware fingerprint.")}
                  </span>
                </div>
              </div>
            )}
          </SettingsGroup>

          <p className="mt-6 max-w-[86ch] border-l-2 border-line-2 pl-3 text-[11px] leading-relaxed text-ink-3">
            {t("Privacy: snapshots are uploaded to the Ludex server you are signed in to, under your account, and only you can list or download them. The server stores the archive as-is and never opens it. Host paths, device names and file contents stay between this machine and your server.")}
          </p>

          <div className="mt-8 border-t border-line pt-5">
            <SaveManifestPanel draft={draft} update={desktop.update} />
          </div>
          <div className="mt-8 border-t border-line pt-5">
            <SaveDiscoveryPanel />
          </div>

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
                        className="font-mono text-[11.5px]"
                      />
                    </div>
                    <span className="text-[11px] text-ink-3">{t("An application id labels the activity; it is not a secret.")}</span>
                  </div>
                  <PresenceStatus enabled={draft.discordPresence} />
                </SettingsGroup>

                <SettingsGroup
                  title={t("Remote installs")}
                  description={t("This PC appears on the phone's remote-install screen once Ludex is signed in. The name below is what you pick there.")}
                >
                  <div className="grid gap-4 sm:grid-cols-2">
                    <label className="flex flex-col gap-1.5">
                      <span className="text-[12px] text-ink-2">{t("Device name")}</span>
                      <Input
                        value={draft.deviceName ?? ""}
                        maxLength={80}
                        placeholder={t("Gaming PC")}
                        onChange={(event) => desktop.update({ deviceName: event.target.value || null })}
                        className="h-11 text-[13px]"
                      />
                      <span className="text-[10.5px] leading-relaxed text-ink-3">
                        {t("Shown on the phone. Leave empty to use the computer name.")}
                      </span>
                    </label>
                    <div className="flex flex-col gap-1.5">
                      <span className="text-[12px] text-ink-2">{t("Device identifier")}</span>
                      <p className="truncate border border-line bg-surface px-3 py-2.5 font-mono text-[11.5px] text-ink-3">
                        {draft.deviceId || t("Generated on first heartbeat")}
                      </p>
                      <span className="text-[10.5px] leading-relaxed text-ink-3">
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
              <dt className="text-[12.5px] text-ink-3">{label}</dt>
              <dd className="num text-[12.5px] text-ink">{value}</dd>
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

      <DesktopBody
        desktop={desktop}
        note={t("App updates need the desktop build: the browser always runs the build your server serves. Release notes and downloads are on GitHub.")}
      >
        {(draft) => (
          <>
            <SettingsGroup title={t("Update channel")}>
              <p className="mb-2 max-w-[54ch] text-[11.5px] leading-relaxed text-ink-3">
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

function PathField({
  label,
  hint,
  value,
  onChange,
  onPick,
}: {
  label: string;
  hint: string;
  value: string;
  onChange: (value: string) => void;
  onPick: () => void;
}) {
  const id = `path-${label.toLowerCase().replace(/\s+/g, "-")}`;
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-[13px] font-medium text-ink" htmlFor={id}>
        {label}
      </label>
      <div className="flex items-stretch gap-2">
        <Input
          id={id}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          spellCheck={false}
          className="h-11 font-mono text-[11.5px]"
          placeholder="/home/user/Games"
        />
        <Button variant="default" size="sm" className="h-11 shrink-0" onClick={onPick} icon={<FolderOpen className="size-3.5" />}>
          {t("Browse")}
        </Button>
      </div>
      <p className="text-[11.5px] leading-relaxed text-ink-3">{hint}</p>
    </div>
  );
}
