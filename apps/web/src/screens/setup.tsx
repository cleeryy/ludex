import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { ArrowLeft, ArrowRight, Check, FolderOpen, HardDrive, Server } from "lucide-react";
import { api, setApiBase, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { DEFAULT_LOCAL_SETTINGS, isNative, native } from "@/lib/native";
import type { LocalSettings, SetupStatus } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { AuthLayout } from "@/components/auth-layout";
import { Button, Field, Input } from "@/components/ui/controls";
import { cn } from "@/lib/cn";
import { t, tr } from "@/lib/i18n";

type Stage = "choose" | "local" | "remote" | "create" | "login";

function normalizeUrl(input: string): string {
  const trimmed = input.trim().replace(/\/$/, "");
  if (!trimmed) return "";
  return /^https?:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
}

export function SetupScreen() {
  const { setup, nativeFirstRun } = useAuth();

  if (nativeFirstRun && isNative()) {
    return <DesktopWizard />;
  }

  return (
    <AuthLayout
      statement={t("One account. Then the library.")}
      note={`${setup?.serverName ?? t("This server")} has no users yet. The account you create now is the administrator, and it stays on this machine.`}
    >
      <CreateAdminForm serverName={setup?.serverName ?? "Ludex"} />
    </AuthLayout>
  );
}

/* ------------------------- browser / configured server -------------------- */

function CreateAdminForm({ serverName }: { serverName: string }) {
  const { completeSetup } = useAuth();
  const navigate = useNavigate();
  const [name, setName] = useState(serverName);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (username.trim().length < 3) return setError(t("Use at least three characters for the username."));
    if (password.length < 8) return setError(t("Use at least eight characters for the password."));
    if (password !== confirm) return setError(t("The two passwords do not match."));
    setBusy(true);
    try {
      await completeSetup({ username: username.trim(), password, serverName: name.trim() || undefined });
      navigate("/", { replace: true });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-6">
      <div className="flex flex-col gap-2">
        <h1 className="text-[34px] font-semibold leading-none tracking-[-0.03em]">{t("Set up this server")}</h1>
        <p className="measure text-[13px] leading-relaxed text-ink-3">
          {t("This is the first run. Create the administrator account; everything else is managed from inside the app.")}
        </p>
      </div>

      <div className="flex flex-col gap-4">
        <Field label={t("Server name")} htmlFor="setup-server" hint={t("Shown in the app header and on the login screen.")}>
          <Input id="setup-server" value={name} onChange={(event) => setName(event.target.value)} maxLength={60} />
        </Field>
        <Field label={t("Username")} htmlFor="setup-username">
          <Input
            id="setup-username"
            autoComplete="username"
            autoFocus
            required
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            placeholder={t("Your username")}
          />
        </Field>
        <Field label={t("Password")} htmlFor="setup-password" hint={t("At least eight characters.")}>
          <Input
            id="setup-password"
            type="password"
            autoComplete="new-password"
            required
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </Field>
        <Field label={t("Confirm password")} htmlFor="setup-confirm">
          <Input
            id="setup-confirm"
            type="password"
            autoComplete="new-password"
            required
            value={confirm}
            onChange={(event) => setConfirm(event.target.value)}
          />
        </Field>
      </div>

      {error && (
        <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
          {error}
        </p>
      )}

      <Button type="submit" variant="primary" size="lg" loading={busy} iconRight={<ArrowRight className="rtl:-scale-x-100 size-4" />}>
        {t("Create administrator")}
      </Button>

      {USING_MOCKS && (
        <p className="text-[12px] leading-relaxed text-ink-3">
          {t("Demo build. This writes a demo administrator to local storage; nothing leaves the browser.")}
        </p>
      )}
    </form>
  );
}

/* ------------------------------ desktop wizard ---------------------------- */

function DesktopWizard() {
  const { setup, adoptSession } = useAuth();
  const navigate = useNavigate();
  const [stage, setStage] = useState<Stage>("choose");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [remoteUrl, setRemoteUrl] = useState("http://");
  const [baseSettings, setBaseSettings] = useState<LocalSettings | null>(null);

  const [gamesDir, setGamesDir] = useState<string | null>(null);
  const [installDir, setInstallDir] = useState<string | null>(null);
  const [downloadDir, setDownloadDir] = useState<string | null>(null);

  const loadSettings = async (): Promise<LocalSettings> => {
    if (baseSettings) return baseSettings;
    const current = (await native.getSettings()) ?? DEFAULT_LOCAL_SETTINGS;
    setBaseSettings(current);
    return current;
  };

  const pick = async (setter: (path: string) => void, title: string) => {
    const path = await native.pickFolder(title);
    if (path) setter(path);
  };

  const startLocal = async () => {
    setError(null);
    if (!gamesDir) return setError(t("Choose the folder where your game archives live."));
    if (!installDir) return setError(t("Choose the folder where games should be installed."));
    setBusy(true);
    try {
      const current = await loadSettings();
      const settings: LocalSettings = {
        ...current,
        mode: "local",
        gamesDir,
        installDir,
        downloadDir: downloadDir ?? installDir,
        serverPort: 0,
      };
      await native.saveSettings(settings);
      const server = await native.startLocalServer();
      await native.saveSettings({ ...settings, serverUrl: server.url, token: server.token });
      await adoptSession({ serverUrl: server.url, token: server.token });
      navigate("/", { replace: true });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const connect = async () => {
    setError(null);
    const url = normalizeUrl(remoteUrl);
    if (!url) return setError(t("Enter the address of your Ludex server."));
    setBusy(true);
    try {
      setApiBase(url);
      const next = await api.setupStatus();
      setStatus(next);
      setStage(next.needsSetup ? "create" : "login");
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const saveRemote = async (url: string, token: string) => {
    const current = await loadSettings();
    await native.saveSettings({ ...current, mode: "remote", serverUrl: url, token });
  };

  const remoteCreate = async (body: { username: string; password: string; serverName?: string }) => {
    const url = normalizeUrl(remoteUrl);
    const result = await api.setup(body);
    if (!result.token || !result.user) throw new Error(t("The server did not return a session."));
    await saveRemote(url, result.token);
    await adoptSession({ serverUrl: url, token: result.token, user: result.user });
    navigate("/", { replace: true });
  };

  const remoteLogin = async (body: { username: string; password: string }) => {
    const url = normalizeUrl(remoteUrl);
    const result = await api.login(body);
    if (!result.token || !result.user) throw new Error(t("The server did not return a session."));
    await saveRemote(url, result.token);
    await adoptSession({ serverUrl: url, token: result.token, user: result.user });
    navigate("/", { replace: true });
  };

  const back = () => {
    setError(null);
    setStage(stage === "remote" || stage === "local" ? "choose" : "remote");
  };

  if (stage === "create" || stage === "login") {
    return (
      <AuthLayout
        statement={t("Connected to your own shelf.")}
        note={`${status?.serverName ?? t("Server")} at ${normalizeUrl(remoteUrl)}. The account stays on that server.`}
        footer={
          <button type="button" onClick={back} className="flex items-center gap-1.5 hover:text-ink">
            <ArrowLeft className="rtl:-scale-x-100 size-3" />
            {t("Use another address")}
          </button>
        }
      >
        <RemoteAuthForm
          mode={stage}
          serverName={status?.serverName ?? "Ludex"}
          onCreate={remoteCreate}
          onLogin={remoteLogin}
        />
      </AuthLayout>
    );
  }

  return (
    <AuthLayout
      statement={t("Your library, on your hardware.")}
      note={t("Local mode runs the Ludex server inside this app on 127.0.0.1. Nothing is exposed to the network unless you choose a remote server.")}
    >
      <div className="flex flex-col gap-6">
        <div className="flex flex-col gap-2">
          <h1 className="text-[34px] font-semibold leading-none tracking-[-0.03em]">{t("Set up Ludex")}</h1>
          <p className="text-[13px] leading-relaxed text-ink-3">
            {stage === "choose" ? t("Choose where the library lives.") : stage === "local" ? t("Point the app at your folders.") : t("Connect to a server you already run.")}
          </p>
        </div>

        {stage !== "choose" && (
          <button type="button" onClick={back} className="flex w-fit items-center gap-1.5 text-[12.5px] text-ink-3 hover:text-ink">
            <ArrowLeft className="rtl:-scale-x-100 size-3.5" />
            {t("Back")}
          </button>
        )}

        {stage === "choose" && (
          <div className="flex flex-col gap-2">
            <ModePlate
              title={t("Local library")}
              description={t("Run the server in this app and point it at a folder of archives.")}
              icon={<HardDrive className="size-4" />}
              onClick={() => setStage("local")}
            />
            <ModePlate
              title={t("Connect to a server")}
              description={t("Use a Ludex server on your network or the internet.")}
              icon={<Server className="size-4" />}
              onClick={() => setStage("remote")}
            />
            {USING_MOCKS && (
              <p className="pt-2 text-[12px] leading-relaxed text-ink-3">
                {t("Demo build. The embedded server is not running here, so local mode will report that it is unavailable. Remote mode and the browser setup work against the mock API.")}
              </p>
            )}
          </div>
        )}

        {stage === "local" && (
          <div className="flex flex-col gap-4">
            <PathPicker
              label={t("Games folder")}
              hint={t("Archives and portable executables. Ludex scans three levels deep.")}
              value={gamesDir}
              onPick={() => void pick(setGamesDir, "Choose the games folder")}
            />
            <PathPicker
              label={t("Install folder")}
              hint={t("Where extracted games are placed.")}
              value={installDir}
              onPick={() => void pick(setInstallDir, "Choose the install folder")}
            />
            <PathPicker
              label={t("Download cache")}
              hint={t("Optional. Archives are deleted after extraction by default.")}
              value={downloadDir}
              onPick={() => void pick(setDownloadDir, "Choose the download folder")}
            />
            <Button variant="primary" size="lg" onClick={() => void startLocal()} loading={busy} iconRight={<ArrowRight className="rtl:-scale-x-100 size-4" />}>
              {t("Start the local library")}
            </Button>
          </div>
        )}

        {stage === "remote" && (
          <div className="flex flex-col gap-4">
            <Field label={t("Server address")} htmlFor="remote-url" hint="For example http://192.168.1.20:8080">
              <Input
                id="remote-url"
                value={remoteUrl}
                onChange={(event) => setRemoteUrl(event.target.value)}
                autoFocus
                spellCheck={false}
                inputMode="url"
                onKeyDown={(event) => {
                  if (event.key === "Enter") void connect();
                }}
              />
            </Field>
            <Button variant="primary" size="lg" onClick={() => void connect()} loading={busy} iconRight={<ArrowRight className="rtl:-scale-x-100 size-4" />}>
              {t("Connect")}
            </Button>
          </div>
        )}

        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
            {error}
          </p>
        )}

        <p className="border-t border-line pt-4 text-[11.5px] leading-relaxed text-ink-3">
          {tr("{name} stores settings in the desktop app, not in this page. You can change the mode later from Settings.", { name: setup?.serverName ?? "Ludex" })}
        </p>
      </div>
    </AuthLayout>
  );
}

function ModePlate({
  title,
  description,
  icon,
  onClick,
}: {
  title: string;
  description: string;
  icon: React.ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "group flex items-start gap-3 border border-line bg-surface p-4 text-left transition-colors duration-[140ms]",
        "hover:border-line-2 hover:bg-surface-2",
      )}
    >
      <span className="mt-0.5 grid size-8 shrink-0 place-items-center border border-line text-ink-2">{icon}</span>
      <span className="flex-1">
        <span className="flex items-center justify-between gap-3">
          <span className="text-[14.5px] font-medium text-ink">{title}</span>
          <ArrowRight className="rtl:-scale-x-100 size-3.5 text-ink-3 transition-transform group-hover:translate-x-0.5" />
        </span>
        <span className="mt-1 block text-[12.5px] leading-relaxed text-ink-3">{description}</span>
      </span>
    </button>
  );
}

function PathPicker({
  label,
  hint,
  value,
  onPick,
}: {
  label: string;
  hint: string;
  value: string | null;
  onPick: () => void;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-[13px] font-medium text-ink">{label}</span>
        {value && <Check className="size-3.5 text-ok" aria-label={t("Selected")} />}
      </div>
      <button
        type="button"
        onClick={onPick}
        className="flex h-11 items-center gap-2.5 border border-line bg-surface px-3 text-left transition-colors hover:border-line-2"
      >
        <FolderOpen className="size-4 shrink-0 text-ink-3" aria-hidden="true" />
        <span className={cn("min-w-0 flex-1 truncate font-mono text-[11.5px]", value ? "text-ink-2" : "text-ink-3")}>
          {value ?? t("Choose a folder")}
        </span>
        <span className="shrink-0 text-[12px] text-ink-3">{t("Browse")}</span>
      </button>
      <p className="text-[11.5px] leading-relaxed text-ink-3">{hint}</p>
    </div>
  );
}

function RemoteAuthForm({
  mode,
  serverName,
  onCreate,
  onLogin,
}: {
  mode: "create" | "login";
  serverName: string;
  onCreate: (body: { username: string; password: string; serverName?: string }) => Promise<void>;
  onLogin: (body: { username: string; password: string }) => Promise<void>;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (mode === "create" && password !== confirm) return setError(t("The two passwords do not match."));
    if (password.length < 8) return setError(t("Use at least eight characters for the password."));
    setBusy(true);
    try {
      if (mode === "create") await onCreate({ username: username.trim(), password, serverName });
      else await onLogin({ username: username.trim(), password });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-6">
      <div className="flex flex-col gap-2">
        <h1 className="text-[34px] font-semibold leading-none tracking-[-0.03em]">
          {mode === "create" ? t("Create the administrator") : t("Sign in")}
        </h1>
        <p className="text-[13px] leading-relaxed text-ink-3">
          {mode === "create" ? `${serverName} has no users yet.` : `Signing in to ${serverName}.`}
        </p>
      </div>
      <div className="flex flex-col gap-4">
        <Field label={t("Username")} htmlFor="remote-username">
          <Input id="remote-username" autoComplete="username" autoFocus required value={username} onChange={(event) => setUsername(event.target.value)} />
        </Field>
        <Field label={t("Password")} htmlFor="remote-password">
          <Input id="remote-password" type="password" autoComplete="current-password" required value={password} onChange={(event) => setPassword(event.target.value)} />
        </Field>
        {mode === "create" && (
          <Field label={t("Confirm password")} htmlFor="remote-confirm">
            <Input id="remote-confirm" type="password" autoComplete="new-password" required value={confirm} onChange={(event) => setConfirm(event.target.value)} />
          </Field>
        )}
      </div>
      {error && (
        <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
          {error}
        </p>
      )}
      <Button type="submit" variant="primary" size="lg" loading={busy} iconRight={<ArrowRight className="rtl:-scale-x-100 size-4" />}>
        {mode === "create" ? t("Create administrator") : t("Sign in")}
      </Button>
    </form>
  );
}
