import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderOpen, HardDrive, Monitor, Moon, Sun } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { installBridge, installsAvailable } from "@/lib/install";
import { isNative, native } from "@/lib/native";
import { formatBytes, formatDate } from "@/lib/format";
import type { DiskUsage, LocalSettings } from "@/lib/types";
import { cn } from "@/lib/cn";
import { useAuth } from "@/state/auth";
import { useSettings, type ThemePref } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { PageHeader, SectionHeader } from "@/components/page";
import { Button, Field, Input, Switch, Textarea } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { DesktopUpdatePanel } from "@/components/settings/desktop-update";
import { CompatibilityPanel } from "@/components/settings/compatibility-panel";
import { SteamImportPanel } from "@/components/settings/steam-import";
import { PresenceStatus } from "@/components/settings/presence-status";

export function SettingsScreen() {
  const { user, refreshUser } = useAuth();
  const { theme, setTheme, resolved } = useSettings();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const desktopAvailable = installsAvailable();

  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [draft, setDraft] = useState<LocalSettings | null>(null);
  const [saving, setSaving] = useState(false);

  const profileQuery = useQuery({ queryKey: ["my-profile"], queryFn: () => api.myProfile() });
  const [publicProfile, setPublicProfile] = useState(false);
  const [displayName, setDisplayName] = useState("");
  const [bio, setBio] = useState("");
  const [savingProfile, setSavingProfile] = useState(false);

  useEffect(() => {
    if (!profileQuery.data) return;
    setPublicProfile(profileQuery.data.publicProfile);
    setDisplayName(profileQuery.data.displayName ?? "");
    setBio(profileQuery.data.bio ?? "");
  }, [profileQuery.data]);

  const settingsQuery = useQuery({
    queryKey: ["desktop-settings"],
    queryFn: () => installBridge.settings(),
    enabled: desktopAvailable,
  });
  const settings = settingsQuery.data ?? null;

  useEffect(() => {
    if (settings) setDraft(settings);
  }, [settings]);

  const dirty = Boolean(draft && settings && JSON.stringify(draft) !== JSON.stringify(settings));

  const installDisk = useQuery<DiskUsage>({
    queryKey: ["disk", draft?.installDir],
    queryFn: () => installBridge.diskUsage(draft!.installDir),
    enabled: desktopAvailable && Boolean(draft?.installDir),
    staleTime: 30_000,
  });
  const downloadDisk = useQuery<DiskUsage>({
    queryKey: ["disk", draft?.downloadDir],
    queryFn: () => installBridge.diskUsage(draft!.downloadDir),
    enabled: desktopAvailable && Boolean(draft?.downloadDir),
    staleTime: 30_000,
  });

  const update = (patch: Partial<LocalSettings>) => setDraft((current) => (current ? { ...current, ...patch } : current));

  const pick = async (key: "installDir" | "downloadDir" | "steamRoot", title: string) => {
    const path = await native.pickFolder(title);
    if (path) update({ [key]: path } as Partial<LocalSettings>);
    else if (!isNative()) {
      push({
        title: "Folder picker needs the desktop app",
        description: "You can type a path here in the browser.",
      });
    }
  };

  const saveDesktop = async () => {
    if (!draft) return;
    setSaving(true);
    try {
      await installBridge.saveSettings(draft);
      await queryClient.invalidateQueries({ queryKey: ["desktop-settings"] });
      await queryClient.invalidateQueries({ queryKey: ["compat-scan"] });
      push({ title: "Desktop settings saved", tone: "success" });
    } catch (cause) {
      push({ title: "Could not save the settings", description: errorMessage(cause), tone: "danger" });
    } finally {
      setSaving(false);
    }
  };

  const saveProfile = async () => {
    setSavingProfile(true);
    try {
      await api.updateProfile({
        publicProfile,
        displayName: displayName.trim() || null,
        bio: bio.trim() || null,
      });
      await queryClient.invalidateQueries({ queryKey: ["my-profile"] });
      await queryClient.invalidateQueries({ queryKey: ["community-users"] });
      await queryClient.invalidateQueries({ queryKey: ["community-feed"] });
      push({ title: "Community profile saved", tone: "success" });
    } catch (cause) {
      push({ title: "Could not save the profile", description: errorMessage(cause), tone: "danger" });
    } finally {
      setSavingProfile(false);
    }
  };

  const changePassword = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (newPassword.length < 8) return setError("Use at least eight characters for the new password.");
    if (newPassword !== confirm) return setError("The two passwords do not match.");
    setBusy(true);
    try {
      await api.changePassword({ currentPassword, newPassword });
      setCurrentPassword("");
      setNewPassword("");
      setConfirm("");
      push({ title: "Password changed", tone: "success" });
      await refreshUser();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page page-in">
      <PageHeader title="Settings" meta={user ? <span className="num">Signed in as {user.username}</span> : undefined} />

      <div className="mt-8 grid max-w-[980px] gap-10 lg:grid-cols-2">
        <section>
          <SectionHeader title="Account" />
          <dl className="border-t border-line">
            {[
              ["Username", user?.username ?? "—"],
              ["Email", user?.email ?? "Not set"],
              ["Role", user ? (user.role === "admin" ? "Administrator" : user.role === "editor" ? "Editor" : "Member") : "—"],
              ["Member since", user ? formatDate(user.createdAt) : "—"],
            ].map(([label, value]) => (
              <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
                <dt className="text-[12.5px] text-ink-3">{label}</dt>
                <dd className="text-[13.5px] text-ink">{value}</dd>
              </div>
            ))}
          </dl>

          <div className="mt-8">
            <SectionHeader title="Appearance" description="Applies to this browser or app window." />
            <div className="grid grid-cols-3 gap-2">
              {(
                [
                  { value: "dark", label: "Dark", icon: <Moon className="size-4" /> },
                  { value: "light", label: "Light", icon: <Sun className="size-4" /> },
                  { value: "system", label: "System", icon: <Monitor className="size-4" /> },
                ] as { value: ThemePref; label: string; icon: React.ReactNode }[]
              ).map((option) => (
                <button
                  key={option.value}
                  type="button"
                  onClick={() => setTheme(option.value)}
                  aria-pressed={theme === option.value}
                  className={cn(
                    "flex flex-col gap-2 border p-3 text-left transition-colors",
                    theme === option.value ? "border-signal bg-surface-2" : "border-line hover:border-line-2",
                  )}
                >
                  <span className={theme === option.value ? "text-signal-text" : "text-ink-3"}>{option.icon}</span>
                  <span className="text-[13px] text-ink">{option.label}</span>
                </button>
              ))}
            </div>
            <p className="mt-2 text-[11.5px] text-ink-3">
              Currently rendering the {resolved} theme. Reduced motion follows your system setting.
            </p>
          </div>
        </section>

        <section>
          <SectionHeader title="Password" description="Eight characters or more." />
          <form onSubmit={changePassword} className="flex flex-col gap-4 border-t border-line pt-4">
            <Field label="Current password" htmlFor="current-password">
              <Input
                id="current-password"
                type="password"
                autoComplete="current-password"
                value={currentPassword}
                onChange={(event) => setCurrentPassword(event.target.value)}
                required
              />
            </Field>
            <Field label="New password" htmlFor="new-password">
              <Input
                id="new-password"
                type="password"
                autoComplete="new-password"
                value={newPassword}
                onChange={(event) => setNewPassword(event.target.value)}
                required
              />
            </Field>
            <Field label="Confirm new password" htmlFor="confirm-password">
              <Input
                id="confirm-password"
                type="password"
                autoComplete="new-password"
                value={confirm}
                onChange={(event) => setConfirm(event.target.value)}
                required
              />
            </Field>
            {error && (
              <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
                {error}
              </p>
            )}
            <div>
              <Button type="submit" variant="default" loading={busy}>
                Change password
              </Button>
            </div>
          </form>
        </section>
      </div>

      <section className="mt-12 max-w-[980px]" aria-labelledby="community-profile-heading">
        <SectionHeader
          title="Community profile"
          description="Opt in to a public page. Private accounts never appear in the feed or the member list."
          actions={
            publicProfile ? (
              <Link
                to={`/community/u/${user?.id ?? ""}`}
                className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
              >
                View public page
              </Link>
            ) : undefined
          }
        />
        <div id="community-profile-heading" className="border-t border-line pt-5">
          {profileQuery.isPending ? (
            <p className="text-[13px] text-ink-3">Loading your profile…</p>
          ) : (
            <div className="grid gap-6 lg:grid-cols-2">
              <div className="flex flex-col gap-4">
                <div className="flex items-start justify-between gap-6 border-b border-line pb-4">
                  <div>
                    <p className="text-[13px] font-medium text-ink">Public profile</p>
                    <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                      Your display name, bio, library, public collections and activity become visible to other accounts.
                      Playtime totals and private collections stay private.
                    </p>
                  </div>
                  <Switch checked={publicProfile} onCheckedChange={setPublicProfile} label="Public profile" />
                </div>
                <Field label="Display name" htmlFor="display-name" hint="Shown instead of your login name. Optional.">
                  <Input
                    id="display-name"
                    value={displayName}
                    onChange={(event) => setDisplayName(event.target.value)}
                    placeholder={user?.username ?? "Mara"}
                    maxLength={64}
                  />
                </Field>
              </div>
              <div className="flex flex-col gap-4">
                <Field label="Bio" htmlFor="profile-bio" hint="A sentence or two. Optional.">
                  <Textarea
                    id="profile-bio"
                    value={bio}
                    onChange={(event) => setBio(event.target.value)}
                    placeholder="Puzzle games and long nights."
                    rows={4}
                    maxLength={500}
                    className="min-h-[110px]"
                  />
                </Field>
                <div className="flex flex-wrap items-center gap-3">
                  <Button variant="default" loading={savingProfile} onClick={() => void saveProfile()}>
                    Save profile
                  </Button>
                  <span className="text-[11.5px] text-ink-3">
                    {publicProfile ? "Your page is listed in Community." : "Your page is visible to you only."}
                  </span>
                </div>
              </div>
            </div>
          )}
        </div>
      </section>

      {desktopAvailable && (
        <section className="mt-12 max-w-[980px]">
          <SectionHeader
            title="Desktop"
            description="Where games are installed, how archives are downloaded, and how passwords are tried."
          />
          <div className="border-t border-line pt-5">
            {settingsQuery.isPending && <p className="text-[13px] text-ink-3">Loading desktop settings…</p>}
            {settingsQuery.isError && (
              <p className="text-[13px] text-danger">{errorMessage(settingsQuery.error)}</p>
            )}

            {draft && (
              <>
                <div className="mb-6 flex flex-wrap items-baseline gap-x-6 gap-y-1 font-mono text-[10.5px] text-ink-3">
                  <span>Mode: {draft.mode === "local" ? "Local library" : "Remote server"}</span>
                  {draft.serverUrl && <span className="truncate">Server: {draft.serverUrl}</span>}
                  {!isNative() && <span className="text-signal-text">Preview values from the demo adapter</span>}
                </div>

                <div className="grid gap-8 lg:grid-cols-2">
                  <div className="flex flex-col gap-5">
                    <PathField
                      label="Install folder"
                      hint="Each game gets its own folder and version subfolder."
                      value={draft.installDir}
                      onChange={(value) => update({ installDir: value })}
                      onPick={() => void pick("installDir", "Choose the install folder")}
                      disk={installDisk.data}
                    />
                    <PathField
                      label="Download folder"
                      hint="Temporary archives. Ignored when stream installs are on."
                      value={draft.downloadDir}
                      onChange={(value) => update({ downloadDir: value })}
                      onPick={() => void pick("downloadDir", "Choose the download folder")}
                      disk={downloadDisk.data}
                    />
                    <PathField
                      label="Steam folder"
                      hint="Optional. Only used by the Steam import when auto-detection fails."
                      value={draft.steamRoot}
                      onChange={(value) => update({ steamRoot: value })}
                      onPick={() => void pick("steamRoot", "Choose the Steam folder")}
                      disk={undefined}
                    />
                  </div>

                  <div>
                    <div className="flex items-start justify-between gap-6 border-b border-line pb-4">
                      <div>
                        <p className="text-[13px] font-medium text-ink">Stream installs from the server</p>
                        <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                          ZIP archives only. Files are written as they arrive, so the archive never touches the disk. If the
                          network drops, a stream install cannot be resumed — it restarts from zero. Use “Retry as download” in
                          Downloads to continue through the resumable normal download path. Uses more bandwidth; non-ZIP
                          archives fall back to a normal download.
                        </p>
                      </div>
                      <Switch
                        checked={draft.streamInstall}
                        onCheckedChange={(streamInstall) => update({ streamInstall })}
                        label="Stream installs from the server"
                      />
                    </div>

                    <div className="flex items-start justify-between gap-6 border-b border-line py-4">
                      <div>
                        <p className="text-[13px] font-medium text-ink">Keep downloaded archives</p>
                        <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                          Leave the archive in the download folder after a successful install. Off by default; the desktop
                          deletes it to save space.
                        </p>
                      </div>
                      <Switch
                        checked={draft.keepArchives}
                        onCheckedChange={(keepArchives) => update({ keepArchives })}
                        label="Keep downloaded archives"
                      />
                    </div>

                    <div className="border-b border-line py-4">
                      <label htmlFor="bandwidth-limit" className="text-[13px] font-medium text-ink">
                        Bandwidth limit
                      </label>
                      <p className="mt-1 mb-2 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                        Caps every download (installs, repairs and stream installs) in KiB/s. Use 0 for no limit.
                      </p>
                      <div className="flex items-center gap-2">
                        <Input
                          id="bandwidth-limit"
                          type="number"
                          min={0}
                          step={64}
                          inputMode="numeric"
                          value={draft.bandwidthLimitKib}
                          onChange={(event) =>
                            update({ bandwidthLimitKib: Math.max(0, Math.round(Number(event.target.value) || 0)) })
                          }
                          className="h-10 w-[150px] font-mono text-[12px]"
                        />
                        <span className="text-[11.5px] text-ink-3">KiB/s</span>
                        <span className="num text-[11px] text-ink-3">
                          {draft.bandwidthLimitKib > 0
                            ? `≈ ${formatBytes(draft.bandwidthLimitKib * 1024)}/s`
                            : "Unlimited"}
                        </span>
                      </div>
                    </div>

                    <div className="pt-4">
                      <label htmlFor="archive-passwords" className="text-[13px] font-medium text-ink">
                        Archive passwords
                      </label>
                      <p className="mt-1 mb-2 text-[11.5px] leading-relaxed text-ink-3">
                        Tried in order when an archive is encrypted. One per line, or separated by commas.
                      </p>
                      <Textarea
                        id="archive-passwords"
                        value={draft.archivePasswords.join("\n")}
                        onChange={(event) =>
                          update({
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
                          ? "No passwords. Encrypted archives will fail to extract."
                          : `${draft.archivePasswords.length} ${draft.archivePasswords.length === 1 ? "password" : "passwords"} stored locally in the desktop settings.`}
                      </p>
                    </div>

                    <div className="flex items-start justify-between gap-6 border-b border-line py-4">
                      <div>
                        <p className="text-[13px] font-medium text-ink">Close to tray</p>
                        <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                          Closing the window keeps Ludex running in the system tray so downloads continue. Quit from the
                          tray menu.
                        </p>
                      </div>
                      <Switch
                        checked={draft.closeToTray}
                        onCheckedChange={(closeToTray) => update({ closeToTray })}
                        label="Close to tray"
                      />
                    </div>

                    <div className="flex items-start justify-between gap-6 border-b border-line py-4">
                      <div>
                        <p className="text-[13px] font-medium text-ink">Check for updates at startup</p>
                        <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                          At most once every 24 hours. Manual checks from this page always run.
                        </p>
                      </div>
                      <Switch
                        checked={draft.checkUpdatesOnStartup}
                        onCheckedChange={(checkUpdatesOnStartup) => update({ checkUpdatesOnStartup })}
                        label="Check for updates at startup"
                      />
                    </div>

                    <div className="border-b border-line py-4">
                      <div className="flex items-start justify-between gap-6">
                        <div>
                          <p className="text-[13px] font-medium text-ink">Discord Rich Presence</p>
                          <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                            Show the running game, its cover and the elapsed time on your Discord profile. Uses the
                            local Discord app, never a bot token. Off by default.
                          </p>
                        </div>
                        <Switch
                          checked={draft.discordPresence}
                          onCheckedChange={(discordPresence) => update({ discordPresence })}
                          label="Discord Rich Presence"
                        />
                      </div>
                      <div className="mt-3 flex flex-wrap items-center gap-3">
                        <Input
                          id="discord-client-id"
                          value={draft.discordClientId}
                          onChange={(event) => update({ discordClientId: event.target.value })}
                          placeholder="Discord application id"
                          spellCheck={false}
                          className="h-9 w-[240px] font-mono text-[11.5px]"
                        />
                        <span className="text-[11px] text-ink-3">
                          An application id labels the activity; it is not a secret.
                        </span>
                      </div>
                      <PresenceStatus enabled={draft.discordPresence} />
                    </div>

                    <div className="border-b border-line py-4">
                      <p className="text-[13px] font-medium text-ink">Silent installer arguments</p>
                      <p className="mt-1 mb-2 max-w-[52ch] text-[11.5px] leading-relaxed text-ink-3">
                        Tried in order when a game ships a setup program, one argument set per line. Leave empty for
                        the built-in presets (<span className="font-mono">/S</span>,{" "}
                        <span className="font-mono">/silent</span>,{" "}
                        <span className="font-mono">/VERYSILENT /SUPPRESSMSGBOXES /NORESTART</span>,{" "}
                        <span className="font-mono">--silent</span>).
                      </p>
                      <Textarea
                        id="installer-args"
                        value={draft.installerSilentArgs.map((set) => set.join(" ")).join("\n")}
                        onChange={(event) =>
                          update({
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
                          Timeout per attempt
                        </label>
                        <Input
                          id="installer-timeout"
                          type="number"
                          min={30}
                          max={7200}
                          step={30}
                          inputMode="numeric"
                          value={draft.installerTimeoutSecs}
                          onChange={(event) =>
                            update({
                              installerTimeoutSecs: Math.max(30, Math.round(Number(event.target.value) || 600)),
                            })
                          }
                          className="h-9 w-[110px] font-mono text-[12px]"
                        />
                        <span className="text-[11.5px] text-ink-3">seconds</span>
                      </div>
                    </div>

                    <div className="py-4">
                      <p className="text-[13px] font-medium text-ink">Update channel</p>
                      <p className="mt-1 mb-2 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                        Early access builds come from the <span className="font-mono">edge</span> prerelease and can be
                        rough. Save the settings to switch.
                      </p>
                      <div className="w-[220px]">
                        <Select
                          label="Update channel"
                          value={draft.updateChannel}
                          onChange={(updateChannel) => update({ updateChannel })}
                          options={[
                            { value: "stable", label: "Stable", hint: "Latest release" },
                            { value: "early", label: "Early access", hint: "edge prerelease" },
                          ]}
                        />
                      </div>
                    </div>
                  </div>
                </div>

                {isNative() && <CompatibilityPanel draft={draft} update={update} />}

                <DesktopUpdatePanel />

                <div className="mt-6 flex flex-wrap items-center gap-3 border-t border-line pt-4">
                  <Button variant="primary" onClick={() => void saveDesktop()} loading={saving} disabled={!dirty}>
                    Save desktop settings
                  </Button>
                  {dirty && <span className="text-[11.5px] text-ink-3">Unsaved changes</span>}
                  {draft.installDir && (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="ml-auto text-ink-3"
                      icon={<FolderOpen className="size-3.5" />}
                      onClick={() => void installBridge.openPath(draft.installDir).catch(() => push({ title: "Folder paths live on the desktop" }))}
                    >
                      Open install folder
                    </Button>
                  )}
                </div>
              </>
            )}
          </div>
        </section>
      )}

      {isNative() && (
        <section className="mt-12 max-w-[980px]" aria-labelledby="integrations-heading">
          <SectionHeader
            title="Integrations"
            description="Import a Steam library or drive Ludex from another launcher."
          />
          <div id="integrations-heading" className="border-t border-line pt-5">
            <SteamImportPanel />
          </div>
        </section>
      )}

      {!desktopAvailable && (
        <p className="mt-10 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
          Install folders, stream installs and archive passwords are available in the Ludex desktop app.
        </p>
      )}
    </div>
  );
}

function PathField({
  label,
  hint,
  value,
  onChange,
  onPick,
  disk,
}: {
  label: string;
  hint: string;
  value: string;
  onChange: (value: string) => void;
  onPick: () => void;
  disk?: DiskUsage;
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
          className="h-10 font-mono text-[11.5px]"
          placeholder="/home/user/Games"
        />
        <Button variant="default" size="sm" className="h-10 shrink-0" onClick={onPick} icon={<FolderOpen className="size-3.5" />}>
          Browse
        </Button>
      </div>
      <p className="flex flex-wrap items-baseline gap-x-2 text-[11.5px] leading-relaxed text-ink-3">
        <span>{hint}</span>
        {disk && (
          <span className="num inline-flex items-center gap-1 text-ink-3">
            <HardDrive className="size-3" aria-hidden="true" />
            {formatBytes(disk.freeBytes)} free of {formatBytes(disk.totalBytes)}
          </span>
        )}
      </p>
    </div>
  );
}
