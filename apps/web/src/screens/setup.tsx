import { useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ExternalLink,
  FolderOpen,
  HardDrive,
  KeyRound,
  Loader2,
  RotateCcw,
  Server,
} from "lucide-react";
import { api, assetUrl, setApiBase, tokenStore, USING_MOCKS } from "@/lib/api";
import { readDesktopStateFromSearch, pendingDesktopState, rememberDesktopState } from "@/lib/desktop-handoff";
import { errorMessage } from "@/lib/http";
import { DEFAULT_LOCAL_SETTINGS, isNative, native } from "@/lib/native";
import {
  setupAuthOptions,
  setupHandoffBlocker,
  setupHandoffView,
  type SetupAuthOption,
  type SetupHandoffPhase,
} from "@/lib/setup-auth";
import type { LocalSettings, SetupStatus } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { AuthLayout } from "@/components/auth-layout";
import { Button, Field, Input } from "@/components/ui/controls";
import { cn } from "@/lib/cn";
import { t, tr } from "@/lib/i18n";

type Stage = "choose" | "local" | "remote" | "create" | "browser" | "login";

function normalizeUrl(input: string): string {
  const trimmed = input.trim().replace(/\/$/, "");
  if (!trimmed) return "";
  return /^https?:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
}

export function SetupScreen() {
  const { setup, nativeFirstRun } = useAuth();
  const [params] = useSearchParams();
  // #175: the desktop wizard opens `#/login?desktop_state=…`; with no users yet
  // the gate reroutes to /setup, so this page must keep the state (across the
  // OIDC redirect too) and hand the session back once an account exists. The
  // sessionStorage copy also covers a reload that drops the query string.
  const [desktopState] = useState(() => readDesktopStateFromSearch(params) ?? pendingDesktopState());
  const config = useQuery({
    queryKey: ["config"],
    queryFn: () => api.config(),
    enabled: Boolean(desktopState),
    staleTime: 60_000,
    retry: 0,
  });

  useEffect(() => {
    if (desktopState) rememberDesktopState(desktopState);
  }, [desktopState]);

  if (nativeFirstRun && isNative()) {
    return <DesktopWizard />;
  }

  return (
    <AuthLayout
      statement={t("One account. Then the library.")}
      note={`${setup?.serverName ?? t("This server")} has no users yet. The account you create now is the administrator, and it stays on this machine.`}
    >
      <CreateAdminForm
        serverName={setup?.serverName ?? "Ludex"}
        desktopHandoff={Boolean(desktopState)}
        options={setupAuthOptions({
          needsSetup: true,
          oidcEnabled: Boolean(config.data?.features.oidc),
          displayName: config.data?.oidcDisplayName,
        })}
      />
    </AuthLayout>
  );
}

/* ------------------------- browser / configured server -------------------- */

function CreateAdminForm({
  serverName,
  desktopHandoff,
  options,
}: {
  serverName: string;
  /** A desktop handoff is pending: sign in for it, then create or return. */
  desktopHandoff: boolean;
  options: SetupAuthOption[];
}) {
  const { completeSetup } = useAuth();
  const navigate = useNavigate();
  const [name, setName] = useState(serverName);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const sso = options.find((option) => option.method === "sso");

  /** First-run SSO (#175): let the browser run the provider, then hand back. */
  const startOidc = () => {
    if (USING_MOCKS) {
      navigate("/login?oidc_code=demo-sso", { replace: true });
      return;
    }
    const url = assetUrl("/api/auth/oidc/start");
    if (url) window.location.assign(url);
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (username.trim().length < 3) return setError(t("Use at least three characters for the username."));
    if (password.length < 8) return setError(t("Use at least eight characters for the password."));
    if (password !== confirm) return setError(t("The two passwords do not match."));
    setBusy(true);
    try {
      await completeSetup({ username: username.trim(), password, serverName: name.trim() || undefined });
      // With a pending handoff, /login binds the fresh session to the desktop
      // app (#157); the gate would otherwise send this page home.
      navigate(desktopHandoff ? "/login" : "/", { replace: true });
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

      {desktopHandoff && sso && (
        <div className="flex flex-col gap-3 border border-line bg-surface p-4">
          <p className="text-[12.5px] leading-relaxed text-ink-3">
            {t("The desktop app opened this page. Sign in with your provider, or create the administrator below; the session returns to the app either way.")}
          </p>
          <Button variant="default" size="lg" className="w-full" icon={<KeyRound className="size-4" />} onClick={startOidc}>
            {sso.label}
          </Button>
        </div>
      )}

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
  // #175: a server with no users can still have a provider; setup then runs
  // through the browser login instead of the in-app form.
  const [oidcDisplayName, setOidcDisplayName] = useState<string | null>(null);

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
      // The provider is a public setting; a stale or older server that does not
      // answer it simply keeps the password-only flow.
      const config = await api.config().catch(() => null);
      setOidcDisplayName(config?.features.oidc ? config.oidcDisplayName?.trim() || "SSO" : null);
      // An existing server signs in through its real web page (SSO or
      // password); a server with no users offers both the browser setup and
      // the in-app administrator form (#175).
      setStage(next.needsSetup ? "create" : "browser");
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

  if (stage === "browser") {
    const url = normalizeUrl(remoteUrl);
    return (
      <AuthLayout
        statement={t("Connected to your own shelf.")}
        note={`${status?.serverName ?? t("Server")} at ${url}. The account stays on that server.`}
        footer={
          <button type="button" onClick={back} className="flex items-center gap-1.5 hover:text-ink">
            <ArrowLeft className="rtl:-scale-x-100 size-3" />
            {t("Use another address")}
          </button>
        }
      >
        <BrowserSignIn
          url={url}
          saveRemote={saveRemote}
          onCancel={back}
          onFallback={() => {
            setError(null);
            setStage("login");
          }}
        />
      </AuthLayout>
    );
  }

  if (stage === "create" || stage === "login") {
    const options = setupAuthOptions({
      needsSetup: stage === "create",
      oidcEnabled: Boolean(oidcDisplayName),
      displayName: oidcDisplayName,
    });
    const sso = options.find((option) => option.method === "sso");
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
          sso={
            sso && stage === "create" ? (
              <SetupSsoPanel option={sso} url={normalizeUrl(remoteUrl)} saveRemote={saveRemote} />
            ) : sso ? (
              <div className="flex flex-col gap-2">
                <Button
                  variant="default"
                  size="lg"
                  className="w-full"
                  icon={<KeyRound className="size-4" />}
                  onClick={() => setStage("browser")}
                >
                  {sso.label}
                </Button>
                <p className="text-[11.5px] leading-relaxed text-ink-3">
                  {t("If you only have an SSO account, an administrator must create it on this server before you can sign in here.")}
                </p>
              </div>
            ) : null
          }
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

/* -------------------- first-run browser sign-in (#175) -------------------- */

/**
 * One browser handoff attempt (#157), shared by the first-run setup panel and
 * the "connect to an existing server" screen. It opens the browser, polls the
 * claim endpoint and redeems the code through the exchange; `onDone` persists
 * the returned session. Nothing is written before the browser claims it.
 */
function useBrowserHandoff({
  url,
  autoStart = false,
  fallbackError,
  onDone,
}: {
  /** Candidate server; passed to `oidc_login_start` before settings exist. */
  url: string;
  /** The "connect" screen starts immediately; the setup form waits for a click. */
  autoStart?: boolean;
  /** Copy used when the poll fails without an actionable server message. */
  fallbackError: string;
  onDone: () => Promise<void>;
}) {
  const { signInWithOidc } = useAuth();
  const [phase, setPhase] = useState<SetupHandoffPhase>(autoStart ? "starting" : "idle");
  const [message, setMessage] = useState<string | null>(null);
  const [webLoginUrl, setWebLoginUrl] = useState<string | null>(null);
  // The exact page `oidc_login_start` opened, including `desktop_state`; the
  // plain `webLoginUrl` cannot hand a session back, so reopening must use this.
  const [handoffUrl, setHandoffUrl] = useState<string | null>(null);

  const aliveRef = useRef(true);
  const startedRef = useRef(false);
  const sessionRef = useRef<string | null>(null);
  const generationRef = useRef(0);

  const run = async () => {
    const generation = ++generationRef.current;
    setPhase("starting");
    setMessage(null);
    setWebLoginUrl(null);
    setHandoffUrl(null);
    try {
      const start = await native.oidcLoginStart(url);
      if (!aliveRef.current || generation !== generationRef.current) {
        void native.oidcLoginCancel(start.sessionId).catch(() => {});
        return;
      }
      sessionRef.current = start.sessionId;
      setWebLoginUrl(start.webLoginUrl);
      setHandoffUrl(`${start.webLoginUrl}?desktop_state=${encodeURIComponent(start.sessionId)}`);
      setPhase("waiting");

      const poll = await native.oidcLoginPoll(start.sessionId);
      if (!aliveRef.current || generation !== generationRef.current) return;
      sessionRef.current = null;

      if (poll.status === "claimed" && poll.code) {
        // The exchange stays the only place a token is issued (#157).
        await signInWithOidc(poll.code);
        await onDone();
        return;
      }
      setPhase("error");
      if (poll.status === "cancelled") {
        setMessage(t("Sign-in cancelled."));
      } else if (poll.status === "expired") {
        setMessage(poll.message ?? t("The sign-in window expired. Start again from Ludex."));
      } else {
        setMessage(poll.message ?? fallbackError);
      }
    } catch (cause) {
      if (!aliveRef.current || generation !== generationRef.current) return;
      sessionRef.current = null;
      setPhase("error");
      // If the browser never opened there is no claim URL; keep a plain web
      // login target so the "Open web login" fallback still works.
      setWebLoginUrl((current) => current ?? `${url.replace(/\/$/, "")}/#/login`);
      const detail = errorMessage(cause);
      setMessage(detail === t("Something went wrong") ? fallbackError : detail);
    }
  };

  useEffect(() => {
    aliveRef.current = true;
    // React StrictMode runs the effect twice in development; the handoff must
    // open one browser page and one poll, so only the first run starts it.
    if (autoStart && !startedRef.current) {
      startedRef.current = true;
      void run();
    }
    return () => {
      aliveRef.current = false;
      // StrictMode runs setup → cleanup → setup synchronously: cancel only when
      // no later setup revived the component (real unmount or stage change).
      queueMicrotask(() => {
        if (aliveRef.current) return;
        const session = sessionRef.current;
        sessionRef.current = null;
        if (session) void native.oidcLoginCancel(session).catch(() => {});
      });
    };
    // `run` is stable for the lifetime of the panel; retries call it directly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Stop the pending poll; the panel stays mounted. */
  const cancel = () => {
    generationRef.current += 1;
    const session = sessionRef.current;
    sessionRef.current = null;
    if (session) void native.oidcLoginCancel(session).catch(() => {});
    setPhase("idle");
    setMessage(null);
    setWebLoginUrl(null);
    setHandoffUrl(null);
  };

  /** Surface a failure raised outside the poll (for example a refused open). */
  const fail = (detail: string) => {
    setPhase("error");
    setMessage(detail);
  };

  return { phase, message, webLoginUrl, handoffUrl, start: run, cancel, fail };
}

/**
 * "Sign in with <provider>" on the first-run setup screen (#175).
 *
 * A server with no users can still have a provider. The handoff runs in the
 * browser (`/setup`, then the provider) and the session comes back here; the
 * local administrator form below stays the way out when the provider cannot
 * create the first, administrator account.
 */
function SetupSsoPanel({
  option,
  url,
  saveRemote,
}: {
  option: SetupAuthOption;
  url: string;
  saveRemote: (url: string, token: string) => Promise<void>;
}) {
  const { adoptSession } = useAuth();
  const navigate = useNavigate();
  const handoff = useBrowserHandoff({
    url,
    fallbackError: t("The browser sign-in did not finish. Try again or create the local administrator."),
    onDone: async () => {
      const token = tokenStore.get();
      if (!token) throw new Error(t("The sign-in did not return a session. Try again."));
      await saveRemote(url, token);
      await adoptSession({ serverUrl: url, token });
      navigate("/", { replace: true });
    },
  });
  const view = setupHandoffView({
    phase: handoff.phase,
    message: handoff.message,
    webLoginUrl: handoff.handoffUrl ?? handoff.webLoginUrl,
  });
  const blocked = setupHandoffBlocker({
    native: isNative(),
    oidcEnabled: true,
    serverUrl: url,
    phase: handoff.phase,
  });

  /** Reopen the claim page, or the plain web login when no URL was received. */
  const openWebLogin = () => {
    const target =
      handoff.handoffUrl ?? handoff.webLoginUrl ?? `${url.replace(/\/$/, "")}/#/login`;
    void native.openExternalUrl(target).catch((cause) => handoff.fail(errorMessage(cause)));
  };

  return (
    <div className="flex flex-col gap-3">
      <Button
        variant="primary"
        size="lg"
        className="w-full"
        icon={<KeyRound className="size-4" />}
        loading={view.progress !== null}
        disabled={blocked !== null}
        onClick={() => void handoff.start()}
      >
        {option.label}
      </Button>

      {view.progress && (
        <p role="status" className="flex items-center gap-2 text-[12.5px] text-ink-2">
          <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
          {view.progress}
        </p>
      )}

      {view.error && (
        <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
          {view.error}
        </p>
      )}

      {(view.canRetry || view.canOpenWebLogin || view.canCancel) && (
        <div className="flex flex-wrap items-center gap-2">
          {view.canRetry && (
            <Button variant="default" size="sm" icon={<RotateCcw className="size-3.5" />} onClick={() => void handoff.start()}>
              {t("Try again")}
            </Button>
          )}
          {view.canOpenWebLogin && (
            <Button variant="quiet" size="sm" className="px-0 text-ink-2" icon={<ExternalLink className="size-3.5" />} onClick={openWebLogin}>
              {t("Open web login")}
            </Button>
          )}
          {view.canCancel && (
            <Button variant="quiet" size="sm" onClick={handoff.cancel}>
              {t("Cancel")}
            </Button>
          )}
        </div>
      )}

      <p className="text-[11.5px] leading-relaxed text-ink-3">
        {t("The provider must create the first account with the administrator role (an admin group in the provider settings). If it cannot, create the local administrator below, then add SSO accounts from the app.")}
      </p>
    </div>
  );
}

/**
 * Signs in to an existing server through its real web login (#157 handoff).
 * The address is not written to the settings until the browser claims the
 * session; until then the wizard can still go back to another server.
 */
function BrowserSignIn({
  url,
  saveRemote,
  onCancel,
  onFallback,
}: {
  url: string;
  saveRemote: (url: string, token: string) => Promise<void>;
  onCancel: () => void;
  onFallback: () => void;
}) {
  const { adoptSession } = useAuth();
  const navigate = useNavigate();
  const handoff = useBrowserHandoff({
    url,
    autoStart: true,
    fallbackError: t("The browser sign-in did not finish. Try again or use your server password."),
    onDone: async () => {
      const token = tokenStore.get();
      if (!token) throw new Error(t("The sign-in did not return a session. Try again."));
      await saveRemote(url, token);
      await adoptSession({ serverUrl: url, token });
      navigate("/", { replace: true });
    },
  });

  /** Stop the pending poll and leave the stage. */
  const cancel = () => {
    handoff.cancel();
    onCancel();
  };

  /** The page opened in the browser is gone: reopen the same handoff page. */
  const openAgain = () => {
    if (!handoff.handoffUrl) return;
    void native.openExternalUrl(handoff.handoffUrl).catch((cause) => handoff.fail(errorMessage(cause)));
  };

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-2">
        <h1 className="text-[34px] font-semibold leading-none tracking-[-0.03em]">
          {t("Finish signing in in your browser")}
        </h1>
        <p className="measure text-[13px] leading-relaxed text-ink-3">
          {t("The login page opened in your system browser. This window updates itself when you finish there.")}
        </p>
      </div>

      {handoff.phase === "error" && (
        <div className="flex flex-col gap-2 border-l-2 border-danger pl-3">
          <p role="alert" className="text-[13px] text-danger">
            {handoff.message}
          </p>
        </div>
      )}

      <div className="flex flex-col gap-3 border border-line bg-surface p-4">
        {handoff.phase !== "error" && (
          <p className="flex items-center gap-2 text-[12.5px] text-ink-2" role="status">
            <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
            {t("Waiting for your browser")}
          </p>
        )}
        <div className="flex flex-col gap-1">
          <span className="text-[11px] uppercase tracking-[0.12em] text-ink-3">{t("Server address")}</span>
          <span className="break-all font-mono text-[11.5px] text-ink-2">{url}</span>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {handoff.phase === "error" && (
          <Button
            variant="primary"
            size="lg"
            onClick={() => void handoff.start()}
            icon={<RotateCcw className="size-4" />}
          >
            {t("Try again")}
          </Button>
        )}
        <Button
          variant={handoff.phase === "error" ? "default" : "primary"}
          size="lg"
          onClick={openAgain}
          disabled={!handoff.handoffUrl}
          icon={<ExternalLink className="size-4" />}
        >
          {t("Open the browser again")}
        </Button>
        <Button variant="quiet" size="lg" onClick={cancel}>
          {t("Cancel")}
        </Button>
      </div>

      <div className="border-t border-line pt-4">
        <Button variant="quiet" size="sm" className="px-0 text-ink-2" onClick={onFallback}>
          {t("Sign in here instead")}
        </Button>
      </div>
    </div>
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
  sso,
}: {
  mode: "create" | "login";
  serverName: string;
  onCreate: (body: { username: string; password: string; serverName?: string }) => Promise<void>;
  onLogin: (body: { username: string; password: string }) => Promise<void>;
  /** Provider handoff rendered above the password fields (#175). */
  sso?: React.ReactNode;
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
      {sso && (
        <div className="flex flex-col gap-4">
          {sso}
          <div className="flex items-center gap-3 text-[11.5px] text-ink-3" aria-hidden="true">
            <span className="h-px flex-1 bg-line" />
            <span>{mode === "create" ? t("or create a local administrator") : t("or use a password")}</span>
            <span className="h-px flex-1 bg-line" />
          </div>
        </div>
      )}
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
