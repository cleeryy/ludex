import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Keyboard } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { isNative } from "@/lib/native";
import { supportsCustomTitleBar } from "@/lib/window-chrome";
import { formatDate } from "@/lib/format";
import { openOnboarding, restoreGettingStarted } from "@/lib/onboarding";
import { openShortcutsHelp } from "@/lib/ui-events";
import { t } from "@/lib/i18n";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { Button, Field, Input, Switch, Textarea } from "@/components/ui/controls";
import { AppearancePanel } from "@/components/settings/appearance-panel";
import { ControllerPanel } from "@/components/settings/controller-panel";
import { RomEmulatorsPanel } from "@/components/settings/rom-emulators";
import { SettingsGroup, SettingsSwitchRow } from "@/components/settings/settings-section";
import type { DesktopSettingsController } from "@/components/settings/use-desktop-settings";
import { GettingStartedCard } from "@/components/onboarding/getting-started";

/** Account: who you are, the password, and the page other members can see. */
export function AccountBody() {
  const { user, refreshUser } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();

  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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

  return (
    <div className="border-t border-line pt-5">
      <dl>
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

      <SettingsGroup title={t("Password")} description={t("Eight characters or more.")}>
        <form onSubmit={changePassword} className="flex max-w-[420px] flex-col gap-4">
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
      </SettingsGroup>

      <SettingsGroup
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
      >
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
      </SettingsGroup>
    </div>
  );
}

/** Appearance: theme, accent, and the desktop window that hosts it. */
export function AppearanceBody({ desktop }: { desktop: DesktopSettingsController }) {
  const titleBar = isNative() && supportsCustomTitleBar();
  return (
    <div className="border-t border-line pt-5">
      <AppearancePanel />
      {isNative() && desktop.draft && (
        <SettingsGroup
          title={t("Window")}
          description={t("Desktop window chrome. Applies immediately, no restart.")}
        >
          {titleBar && (
            <SettingsSwitchRow
              title={t("Use the native title bar")}
              description={t("Draw the system title bar instead of Ludex's own. Applies immediately, no restart. Turn it on for Windows Snap Layouts, screen readers or remote desktop.")}
              checked={desktop.draft.nativeTitleBar}
              onCheckedChange={(next) => void desktop.applyNativeTitleBar(next)}
            />
          )}
          <SettingsSwitchRow
            title={t("Close to tray")}
            description={t("Closing the window keeps Ludex running in the system tray so downloads continue. Quit from the tray menu.")}
            checked={desktop.draft.closeToTray}
            onCheckedChange={(closeToTray) => desktop.update({ closeToTray })}
          />
        </SettingsGroup>
      )}
    </div>
  );
}

/** Profiles: the controller mapping and the emulators this device launches. */
export function ProfilesBody({ desktop }: { desktop: DesktopSettingsController }) {
  return (
    <div className="border-t border-line pt-6">
      <SettingsGroup
        title={t("Controllers")}
        description={t("Deadzone, sensitivity, remapping and vibration apply in the library and Big Picture.")}
      >
        <ControllerPanel />
      </SettingsGroup>
      {desktop.available && (
        <SettingsGroup
          title={t("Emulators")}
          description={t("Local commands used to launch ROMs. Ludex never bundles an emulator binary.")}
        >
          <RomEmulatorsPanel />
        </SettingsGroup>
      )}
    </div>
  );
}

/** Help and about: onboarding, shortcuts, and the build you are running. */
export function HelpBody() {
  const { setup } = useAuth();
  return (
    <div className="border-t border-line pt-5">
      <SettingsGroup
        title={t("Onboarding")}
        description={t("Replay the first-run tour or finish the recommended setup steps.")}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="default"
              size="sm"
              onClick={() => {
                restoreGettingStarted();
                openOnboarding({ restart: true });
              }}
            >
              {t("Restart onboarding")}
            </Button>
            <Button variant="ghost" size="sm" icon={<Keyboard className="size-3.5" />} onClick={openShortcutsHelp}>
              {t("Keyboard shortcuts")}
            </Button>
          </div>
        }
      >
        <GettingStartedCard />
        <p className="mt-3 text-[11.5px] text-ink-3">
          {t("This clears the completed flag and reopens the wizard at the first step.")}
        </p>
      </SettingsGroup>

      <SettingsGroup title={t("About this app")}>
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
      </SettingsGroup>
    </div>
  );
}
