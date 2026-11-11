import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderOpen, Monitor, Moon, Sun } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { installBridge, installsAvailable } from "@/lib/install";
import { isNative, native } from "@/lib/native";
import { setTitleBarPreference, supportsCustomTitleBar } from "@/lib/window-chrome";
import { formatBytes, formatDate } from "@/lib/format";
import type { LocalSettings } from "@/lib/types";
import { cn } from "@/lib/cn";
import { useAuth } from "@/state/auth";
import { useSettings, type ThemePref } from "@/state/settings";
import { coverageFor, LOCALES, statusLabel, useI18n, tr } from "@/lib/i18n";
import { useToast } from "@/state/toasts";
import { PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Field, Input, Switch, Textarea } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { DesktopUpdatePanel } from "@/components/settings/desktop-update";
import { CompatibilityPanel } from "@/components/settings/compatibility-panel";
import { LauncherImportPanel } from "@/components/settings/launcher-import";
import { InstalledPanel } from "@/components/settings/installed-games";
import { PresenceStatus } from "@/components/settings/presence-status";
import { StreamingPanel } from "@/components/settings/streaming-panel";
import { ServersPanel } from "@/components/shell/server-switcher";
import { GettingStartedCard } from "@/components/onboarding/getting-started";
import { LocationsPanel } from "@/components/settings/locations-panel";
import { openOnboarding, restoreGettingStarted } from "@/lib/onboarding";
import { ControllerPanel } from "@/components/settings/controller-panel";
import { RomEmulatorsPanel } from "@/components/settings/rom-emulators";
import { t } from "@/lib/i18n";

export function SettingsScreen() {
  const { user, refreshUser } = useAuth();
  const { theme, setTheme, resolved } = useSettings();
  const { locale: activeLocale, setLocale } = useI18n();
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


  const update = (patch: Partial<LocalSettings>) => setDraft((current) => (current ? { ...current, ...patch } : current));

  /**
   * The title bar toggle lives in Appearance but belongs to the desktop
   * settings: it is applied and persisted on its own, so it never waits for the
   * "Save desktop settings" button further down (#111).
   */
  const applyNativeTitleBar = async (next: boolean) => {
    update({ nativeTitleBar: next });
    // The window chrome (AppShell and the bare screens) reads this mirror.
    setTitleBarPreference(next);
    try {
      await native.setNativeTitleBar(next);
      // Refresh the saved snapshot so "Unsaved changes" stays honest. Skipped
      // while the form holds other edits: the refetch resets the draft.
      if (!dirty) await queryClient.invalidateQueries({ queryKey: ["desktop-settings"] });
    } catch (cause) {
      update({ nativeTitleBar: !next });
      setTitleBarPreference(!next);
      push({ title: t("Could not change the title bar"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const pick = async (key: "steamRoot", title: string) => {
    const path = await native.pickFolder(title);
    if (path) update({ [key]: path } as Partial<LocalSettings>);
    else if (!isNative()) {
      push({
        title: t("Folder picker needs the desktop app"),
        description: t("You can type a path here in the browser."),
      });
    }
  };

  const saveDesktop = async () => {
    if (!draft) return;
    setSaving(true);
    try {
      await installBridge.saveSettings(draft);
      await queryClient.invalidateQueries({ queryKey: ["desktop-settings"] });
      await queryClient.invalidateQueries({ queryKey: ["desktop-locations"] });
      await queryClient.invalidateQueries({ queryKey: ["title-bar-info"] });
      await queryClient.invalidateQueries({ queryKey: ["compat-scan"] });
      await queryClient.invalidateQueries({ queryKey: ["cloud-save-status"] });
      push({ title: t("Desktop settings saved"), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not save the settings"), description: errorMessage(cause), tone: "danger" });
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
      push({ title: t("Community profile saved"), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not save the profile"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setSavingProfile(false);
    }
  };

  const changePassword = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (newPassword.length < 8) return setError(t("Use at least eight characters for the new password."));
    if (newPassword !== confirm) return setError(t("The two passwords do not match."));
    setBusy(true);
    try {
      await api.changePassword({ currentPassword, newPassword });
      setCurrentPassword("");
      setNewPassword("");
      setConfirm("");
      push({ title: t("Password changed"), tone: "success" });
      await refreshUser();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page page-in">
      <PageHeader title={t("Settings")} meta={user ? <span className="num">{t("Signed in as")} {user.username}</span> : undefined} />

      <div className="mt-8 grid max-w-[980px] gap-10 lg:grid-cols-2">
        <section>
          <SectionHeader title={t("Account")} />
          <dl className="border-t border-line">
            {[
              [t("Username"), user?.username ?? "—"],
              [t("Email"), user?.email ?? t("Not set")],
              [t("Role"), user ? (user.role === "admin" ? t("Administrator") : user.role === "editor" ? t("Editor") : t("Member")) : "—"],
              [t("Member since"), user ? formatDate(user.createdAt) : "—"],
            ].map(([label, value]) => (
              <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
                <dt className="text-[12.5px] text-ink-3">{label}</dt>
                <dd className="text-[13.5px] text-ink">{value}</dd>
              </div>
            ))}
          </dl>

          <div className="mt-8">
            <SectionHeader title={t("Appearance")} description={t("Applies to this browser or app window.")} />
            <div className="grid grid-cols-3 gap-2">
              {(
                [
                  { value: "dark", label: t("Dark"), icon: <Moon className="size-4" /> },
                  { value: "light", label: t("Light"), icon: <Sun className="size-4" /> },
                  { value: "system", label: t("System"), icon: <Monitor className="size-4" /> },
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
              {t("Currently rendering the {theme} theme. Reduced motion follows your system setting.", {
                theme: resolved === "dark" ? t("Dark") : t("Light"),
              })}
            </p>

            {isNative() && supportsCustomTitleBar() && draft && (
              <div className="mt-6 flex items-start justify-between gap-6 border-t border-line pt-4">
                <div>
                  <p className="text-[13px] font-medium text-ink">{t("Use the native title bar")}</p>
                  <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                    {t("Draw the system title bar instead of Ludex's own. Applies immediately, no restart. Turn it on for Windows Snap Layouts, screen readers or remote desktop.")}
                  </p>
                </div>
                <Switch
                  checked={draft.nativeTitleBar}
                  onCheckedChange={(nativeTitleBar) => void applyNativeTitleBar(nativeTitleBar)}
                  label={t("Use the native title bar")}
                />
              </div>
            )}
          </div>

          <div className="mt-8">
            <SectionHeader
              title={t("Language")}
              description={t("Applies to this browser or app window. Community translations fall back to English for missing strings.")}
            />
            {[
              { status: "official" as const, title: t("Official languages") },
              { status: "community" as const, title: t("Community translations") },
            ].map((group) => (
              <div key={group.status} className="mt-4 first:mt-0">
                <p className="mb-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{group.title}</p>
                <div className="grid gap-1.5 sm:grid-cols-2">
                  {LOCALES.filter((entry) => entry.status === group.status).map((entry) => {
                    const coverage = entry.code === "en" ? { percent: 100 } : coverageFor(entry.code);
                    const selected = activeLocale === entry.code;
                    return (
                      <button
                        key={entry.code}
                        type="button"
                        lang={entry.code}
                        dir={entry.dir}
                        aria-pressed={selected}
                        onClick={() => void setLocale(entry.code)}
                        className={cn(
                          "flex min-h-11 items-center justify-between gap-3 border px-3 py-2 text-start transition-colors",
                          selected ? "border-signal bg-surface-2" : "border-line hover:border-line-2 hover:bg-surface",
                        )}
                      >
                        <span className="min-w-0">
                          <span className="block truncate text-[13px] text-ink">{entry.label}</span>
                          <span className="block truncate font-mono text-[10px] text-ink-3">
                            {entry.englishLabel}
                            {entry.dir === "rtl" ? " · RTL" : ""}
                          </span>
                        </span>
                        <span className="flex shrink-0 items-center gap-1.5">
                          {entry.code !== "en" && coverage && (
                            <span className="num text-[10px] text-ink-3">{coverage.percent}%</span>
                          )}
                          <Badge tone={selected ? "signal" : "outline"}>{statusLabel(entry)}</Badge>
                        </span>
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
            <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
              {t("Community locales are contributed as JSON files. See docs/I18N.md for the contribution flow; Arabic is machine-seeded and unreviewed.")}
            </p>
          </div>
        </section>

        <section>
          <SectionHeader title={t("Password")} description={t("Eight characters or more.")} />
          <form onSubmit={changePassword} className="flex flex-col gap-4 border-t border-line pt-4">
            <Field label={t("Current password")} htmlFor="current-password">
              <Input
                id="current-password"
                type="password"
                autoComplete="current-password"
                value={currentPassword}
                onChange={(event) => setCurrentPassword(event.target.value)}
                required
              />
            </Field>
            <Field label={t("New password")} htmlFor="new-password">
              <Input
                id="new-password"
                type="password"
                autoComplete="new-password"
                value={newPassword}
                onChange={(event) => setNewPassword(event.target.value)}
                required
              />
            </Field>
            <Field label={t("Confirm new password")} htmlFor="confirm-password">
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
                {t("Change password")}
              </Button>
            </div>
          </form>
        </section>
      </div>

      <section className="mt-12 max-w-[980px]" aria-labelledby="community-profile-heading">
        <SectionHeader
          title={t("Community profile")}
          description={t("Opt in to a public page. Private accounts never appear in the feed or the member list.")}
          actions={
            publicProfile ? (
              <Link
                to={`/community/u/${user?.id ?? ""}`}
                className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
              >
                {t("View public page")}
              </Link>
            ) : undefined
          }
        />
        <div id="community-profile-heading" className="border-t border-line pt-5">
          {profileQuery.isPending ? (
            <p className="text-[13px] text-ink-3">{t("Loading your profile…")}</p>
          ) : (
            <div className="grid gap-6 lg:grid-cols-2">
              <div className="flex flex-col gap-4">
                <div className="flex items-start justify-between gap-6 border-b border-line pb-4">
                  <div>
                    <p className="text-[13px] font-medium text-ink">{t("Public profile")}</p>
                    <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                      {t("Your display name, bio, library, public collections and activity become visible to other accounts. Playtime totals and private collections stay private.")}
                    </p>
                  </div>
                  <Switch checked={publicProfile} onCheckedChange={setPublicProfile} label={t("Public profile")} />
                </div>
                <Field label={t("Display name")} htmlFor="display-name" hint={t("Shown instead of your login name. Optional.")}>
                  <Input
                    id="display-name"
                    value={displayName}
                    onChange={(event) => setDisplayName(event.target.value)}
                    placeholder={user?.username ?? t("Mara")}
                    maxLength={64}
                  />
                </Field>
              </div>
              <div className="flex flex-col gap-4">
                <Field label={t("Bio")} htmlFor="profile-bio" hint={t("A sentence or two. Optional.")}>
                  <Textarea
                    id="profile-bio"
                    value={bio}
                    onChange={(event) => setBio(event.target.value)}
                    placeholder={t("Puzzle games and long nights.")}
                    rows={4}
                    maxLength={500}
                    className="min-h-[110px]"
                  />
                </Field>
                <div className="flex flex-wrap items-center gap-3">
                  <Button variant="default" loading={savingProfile} onClick={() => void saveProfile()}>
                    {t("Save profile")}
                  </Button>
                  <span className="text-[11.5px] text-ink-3">
                    {publicProfile ? t("Your page is listed in Community.") : t("Your page is visible to you only.")}
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
            title={t("Desktop")}
            description={t("Where games are installed, how archives are downloaded, and how passwords are tried.")}
          />
          <div className="border-t border-line pt-5">
            {settingsQuery.isPending && <p className="text-[13px] text-ink-3">{t("Loading desktop settings…")}</p>}
            {settingsQuery.isError && (
              <p className="text-[13px] text-danger">{errorMessage(settingsQuery.error)}</p>
            )}

            {draft && (
              <>
                <div className="mb-6 flex flex-wrap items-baseline gap-x-6 gap-y-1 font-mono text-[10.5px] text-ink-3">
                  <span>{t("Mode:")} {draft.mode === "local" ? t("Local library") : t("Remote server")}</span>
                  {draft.serverUrl && <span className="truncate">{t("Server:")} {draft.serverUrl}</span>}
                  {!isNative() && <span className="text-signal-text">{t("Preview values from the demo adapter")}</span>}
                </div>

                <div className="mb-8">
                  <LocationsPanel
                    locations={draft.locations ?? []}
                    onChange={(locations) => update({ locations })}
                  />
                </div>

                <div className="grid gap-8 lg:grid-cols-2">
                  <div className="flex flex-col gap-5">
                    <PathField
                      label={t("Steam folder")}
                      hint={t("Optional. Only used by the Steam import when auto-detection fails.")}
                      value={draft.steamRoot}
                      onChange={(value) => update({ steamRoot: value })}
                      onPick={() => void pick("steamRoot", "Choose the Steam folder")}
                    />
                  </div>

                  <div>
                    <div className="flex items-start justify-between gap-6 border-b border-line pb-4">
                      <div>
                        <p className="text-[13px] font-medium text-ink">{t("Stream installs from the server")}</p>
                        <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                          {t("ZIP archives only. Files are written as they arrive, so the archive never touches the disk. A dropped connection, a pause or an app restart resumes at file granularity: completed files are kept and only missing ones are downloaded again. 7z/RAR archives, and servers without the per-entry API, fall back to the normal resumable download.")}
                        </p>
                      </div>
                      <Switch
                        checked={draft.streamInstall}
                        onCheckedChange={(streamInstall) => update({ streamInstall })}
                        label={t("Stream installs from the server")}
                      />
                    </div>

                    <div className="flex items-start justify-between gap-6 border-b border-line py-4">
                      <div>
                        <p className="text-[13px] font-medium text-ink">{t("Keep downloaded archives")}</p>
                        <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                          {t("Leave the archive in the download folder after a successful install. Off by default; the desktop deletes it to save space.")}
                        </p>
                      </div>
                      <Switch
                        checked={draft.keepArchives}
                        onCheckedChange={(keepArchives) => update({ keepArchives })}
                        label={t("Keep downloaded archives")}
                      />
                    </div>

                    <div className="border-b border-line py-4">
                      <label htmlFor="bandwidth-limit" className="text-[13px] font-medium text-ink">
                        {t("Bandwidth limit")}
                      </label>
                      <p className="mt-1 mb-2 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                        {t("Caps every download (installs, repairs and stream installs) in KiB/s. Use 0 for no limit.")}
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
                        <span className="text-[11.5px] text-ink-3">{t("KiB/s")}</span>
                        <span className="num text-[11px] text-ink-3">
                          {draft.bandwidthLimitKib > 0
                            ? `≈ ${formatBytes(draft.bandwidthLimitKib * 1024)}/s`
                            : t("Unlimited")}
                        </span>
                      </div>
                    </div>

                    <div className="pt-4">
                      <label htmlFor="archive-passwords" className="text-[13px] font-medium text-ink">
                        {t("Archive passwords")}
                      </label>
                      <p className="mt-1 mb-2 text-[11.5px] leading-relaxed text-ink-3">
                        {t("Tried in order when an archive is encrypted. One per line, or separated by commas.")}
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
                          ? t("No passwords. Encrypted archives will fail to extract.")
                          : `${draft.archivePasswords.length} ${draft.archivePasswords.length === 1 ? "password" : "passwords"} stored locally in the desktop settings.`}
                      </p>
                    </div>

                    <div className="flex items-start justify-between gap-6 border-b border-line py-4">
                      <div>
                        <p className="text-[13px] font-medium text-ink">{t("Close to tray")}</p>
                        <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                          {t("Closing the window keeps Ludex running in the system tray so downloads continue. Quit from the tray menu.")}
                        </p>
                      </div>
                      <Switch
                        checked={draft.closeToTray}
                        onCheckedChange={(closeToTray) => update({ closeToTray })}
                        label={t("Close to tray")}
                      />
                    </div>

                    <div className="flex items-start justify-between gap-6 border-b border-line py-4">
                      <div>
                        <p className="text-[13px] font-medium text-ink">{t("Check for updates at startup")}</p>
                        <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                          {t("At most once every 24 hours. Manual checks from this page always run.")}
                        </p>
                      </div>
                      <Switch
                        checked={draft.checkUpdatesOnStartup}
                        onCheckedChange={(checkUpdatesOnStartup) => update({ checkUpdatesOnStartup })}
                        label={t("Check for updates at startup")}
                      />
                    </div>

                    <div className="border-b border-line py-4">
                      <div className="flex items-start justify-between gap-6">
                        <div>
                          <p className="text-[13px] font-medium text-ink">{t("Discord Rich Presence")}</p>
                          <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                            {t("Show the running game, its cover and the elapsed time on your Discord profile. Uses the local Discord app, never a bot token. Off by default.")}
                          </p>
                        </div>
                        <Switch
                          checked={draft.discordPresence}
                          onCheckedChange={(discordPresence) => update({ discordPresence })}
                          label={t("Discord Rich Presence")}
                        />
                      </div>
                      <div className="mt-3 flex flex-wrap items-center gap-3">
                        <Input
                          id="discord-client-id"
                          value={draft.discordClientId}
                          onChange={(event) => update({ discordClientId: event.target.value })}
                          placeholder={t("Discord application id")}
                          spellCheck={false}
                          className="h-9 w-[240px] font-mono text-[11.5px]"
                        />
                        <span className="text-[11px] text-ink-3">
                          {t("An application id labels the activity; it is not a secret.")}
                        </span>
                      </div>
                      <PresenceStatus enabled={draft.discordPresence} />
                    </div>

                    <div className="border-b border-line py-4">
                      <p className="text-[13px] font-medium text-ink">{t("Silent installer arguments")}</p>
                      <p className="mt-1 mb-2 max-w-[52ch] text-[11.5px] leading-relaxed text-ink-3">
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
                          {t("Timeout per attempt")}
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
                        <span className="text-[11.5px] text-ink-3">{t("seconds")}</span>
                      </div>
                    </div>

                    <div className="py-4">
                      <p className="text-[13px] font-medium text-ink">{t("Update channel")}</p>
                      <p className="mt-1 mb-2 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
                        {tr("Early access builds come from the {channel} prerelease and can be rough. Save the settings to switch.", {
                          channel: <span className="font-mono">edge</span>,
                        })}
                      </p>
                      <div className="w-[220px]">
                        <Select
                          label={t("Update channel")}
                          value={draft.updateChannel}
                          onChange={(updateChannel) => update({ updateChannel })}
                          options={[
                            { value: "stable", label: t("Stable"), hint: t("Latest release") },
                            { value: "early", label: t("Early access"), hint: t("edge prerelease") },
                          ]}
                        />
                      </div>
                    </div>
                  </div>
                </div>

                {isNative() && <CompatibilityPanel draft={draft} update={update} />}

                <div className="mt-6 border-t border-line pt-5">
                  <div className="flex flex-wrap items-start justify-between gap-6">
                    <div>
                      <p className="text-[13px] font-medium text-ink">Automatic cloud saves</p>
                      <p className="mt-1 max-w-[62ch] text-[11.5px] leading-relaxed text-ink-3">
                        {isNative()
                          ? "Back up the detected save folders after each session. Individual games can turn this off on their page; the server keeps the newest 10 automatic snapshots per game and never prunes manual ones."
                          : "Automatic cloud saves need the Ludex desktop app: they read and write save files on this machine. The browser can still download and upload manual snapshots."}
                      </p>
                    </div>
                    {isNative() && (
                      <Switch
                        checked={draft.cloudAutoSync}
                        onCheckedChange={(cloudAutoSync) => update({ cloudAutoSync })}
                        label="Automatic cloud saves"
                      />
                    )}
                  </div>

                  {isNative() && (
                    <div className="mt-4 grid gap-4 sm:grid-cols-2">
                      <label className="flex flex-col gap-1.5">
                        <span className="text-[12px] text-ink-2">Device name</span>
                        <Input
                          value={draft.cloudDeviceLabel ?? ""}
                          maxLength={60}
                          placeholder="Living room PC"
                          onChange={(event) => update({ cloudDeviceLabel: event.target.value || null })}
                          className="h-10 text-[13px]"
                        />
                        <span className="text-[10.5px] leading-relaxed text-ink-3">
                          Shown on automatic snapshots. Leave empty to use the computer name.
                        </span>
                      </label>
                      <div className="flex flex-col gap-1.5">
                        <span className="text-[12px] text-ink-2">Device identifier</span>
                        <p className="truncate border border-line bg-surface px-3 py-2.5 font-mono text-[11.5px] text-ink-3">
                          {draft.cloudDeviceId || "Generated on first upload"}
                        </p>
                        <span className="text-[10.5px] leading-relaxed text-ink-3">
                          A random id stored in this app's settings, not a hardware fingerprint.
                        </span>
                      </div>
                    </div>
                  )}

                  <p className="mt-4 max-w-[86ch] border-l-2 border-line-2 pl-3 text-[11px] leading-relaxed text-ink-3">
                    Privacy: snapshots are uploaded to the Ludex server you are signed in to, under your account, and
                    only you can list or download them. The server stores the archive as-is and never opens it. Host
                    paths, device names and file contents stay between this machine and your server.
                  </p>
                </div>

                <DesktopUpdatePanel />

                <div className="mt-6 flex flex-wrap items-center gap-3 border-t border-line pt-4">
                  <Button variant="primary" onClick={() => void saveDesktop()} loading={saving} disabled={!dirty}>
                    {t("Save desktop settings")}
                  </Button>
                  {dirty && <span className="text-[11.5px] text-ink-3">{t("Unsaved changes")}</span>}
                  {draft.installDir && (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="ml-auto text-ink-3"
                      icon={<FolderOpen className="size-3.5" />}
                      onClick={() => void installBridge.openPath(draft.installDir).catch(() => push({ title: t("Folder paths live on the desktop") }))}
                    >
                      {t("Open install folder")}
                    </Button>
                  )}
                </div>
              </>
            )}
          </div>
        </section>
      )}

      {isNative() && draft && (
        <section className="mt-12 max-w-[980px]">
          <StreamingPanel draft={draft} update={update} />
        </section>
      )}

      <ServersPanel />

      {isNative() && (
        <section className="mt-12 max-w-[980px]" aria-labelledby="installed-heading">
          <SectionHeader
            title={t("Installed")}
            description={t("What this machine has, and the GameVault folders Ludex can link without copying them.")}
          />
          <div id="installed-heading" className="border-t border-line pt-5">
            <InstalledPanel />
          </div>
        </section>
      )}

      {isNative() && (
        <section className="mt-12 max-w-[980px]" aria-labelledby="integrations-heading">
          <SectionHeader
            title={t("Integrations")}
            description={t("Import games another launcher installed, Steam included, without copying a byte.")}
          />
          <div id="integrations-heading" className="border-t border-line pt-5">
            <LauncherImportPanel />
          </div>
        </section>
      )}

      {!desktopAvailable && (
        <p className="mt-10 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
          {t("Install folders, stream installs and archive passwords are available in the Ludex desktop app.")}
        </p>
      )}

      <section className="mt-12 max-w-[980px]" id="help" aria-labelledby="help-heading">
        <SectionHeader
          title={t("Help")}
          description={t("Replay the first-run tour or finish the recommended setup steps.")}
        />
        <div className="border-t border-line pt-5">
          <div className="flex flex-wrap items-center gap-3">
            <Button
              variant="default"
              onClick={() => {
                restoreGettingStarted();
                openOnboarding({ restart: true });
              }}
            >
              {t("Restart onboarding")}
            </Button>
            <span className="text-[11.5px] text-ink-3">
              {t("This clears the completed flag and reopens the wizard at the first step.")}
            </span>
          </div>
          <GettingStartedCard className="mt-5" />
        </div>
      </section>

      {desktopAvailable && (
        <div className="mt-12 max-w-[980px]">
          <SectionHeader
            title={t("Emulators")}
            description={t("Local commands used to launch ROMs. Ludex never bundles an emulator binary.")}
          />
          <div className="mt-4">
            <RomEmulatorsPanel />
          </div>
        </div>
      )}

      <div className="mt-12 max-w-[980px]">
        <SectionHeader
          title={t("Controllers")}
          description={t("Deadzone, sensitivity, remapping and vibration apply in the library and Big Picture.")}
        />
        <div className="mt-4">
          <ControllerPanel />
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
          className="h-10 font-mono text-[11.5px]"
          placeholder="/home/user/Games"
        />
        <Button variant="default" size="sm" className="h-10 shrink-0" onClick={onPick} icon={<FolderOpen className="size-3.5" />}>
          {t("Browse")}
        </Button>
      </div>
      <p className="text-[11.5px] leading-relaxed text-ink-3">{hint}</p>
    </div>
  );
}
