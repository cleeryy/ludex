import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FolderOpen, HardDrive, Monitor, Moon, Sun } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { formatBytes, formatDate } from "@/lib/format";
import { cn } from "@/lib/cn";
import { useAuth } from "@/state/auth";
import { useSettings, type ThemePref } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { PageHeader, SectionHeader } from "@/components/page";
import { Button, Field, Input } from "@/components/ui/controls";

export function SettingsScreen() {
  const { user, refreshUser } = useAuth();
  const { theme, setTheme, resolved } = useSettings();
  const { push } = useToast();
  const nativeMode = isNative();

  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const { data: settings } = useQuery({
    queryKey: ["native-settings"],
    queryFn: () => native.getSettings(),
    enabled: nativeMode,
  });

  const { data: disk } = useQuery({
    queryKey: ["disk", settings?.installDir],
    queryFn: () => native.getDiskUsage(settings!.installDir),
    enabled: nativeMode && Boolean(settings?.installDir),
  });

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

      <div className="mt-8 grid max-w-[980px] gap-10 lg:grid-cols-[1fr_1fr]">
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

          {nativeMode && (
            <div className="mt-10">
              <SectionHeader title="Desktop" description="Paths and storage on this machine." />
              <dl className="border-t border-line">
                {[
                  ["Mode", settings?.mode === "local" ? "Local library" : "Remote server"],
                  ["Server", settings?.serverUrl || "—"],
                  ["Games folder", settings?.gamesDir ?? "—"],
                  ["Install folder", settings?.installDir || "—"],
                  ["Download cache", settings?.downloadDir || "—"],
                ].map(([label, value]) => (
                  <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
                    <dt className="shrink-0 text-[12.5px] text-ink-3">{label}</dt>
                    <dd className="truncate font-mono text-[11.5px] text-ink-2" title={value}>
                      {value}
                    </dd>
                  </div>
                ))}
              </dl>
              {disk && (
                <p className="mt-3 flex items-center gap-2 text-[12px] text-ink-3">
                  <HardDrive className="size-3.5" aria-hidden="true" />
                  <span className="num">{formatBytes(disk.freeBytes)} free</span>
                  <span>of</span>
                  <span className="num">{formatBytes(disk.totalBytes)}</span>
                </p>
              )}
              {settings?.installDir && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="mt-2 px-0"
                  icon={<FolderOpen className="size-3.5" />}
                  onClick={() => void native.openPath(settings.installDir).catch((cause: Error) => push({ title: "Could not open the folder", description: cause.message, tone: "danger" }))}
                >
                  Open install folder
                </Button>
              )}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
