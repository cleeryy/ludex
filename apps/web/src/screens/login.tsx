import { useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, CheckCircle2, KeyRound, Loader2, RotateCcw } from "lucide-react";
import { api, assetUrl, USING_MOCKS } from "@/lib/api";
import { MOCK_PASSWORD } from "@/lib/mock";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import {
  createDesktopHandoff,
  forgetDesktopState,
  pendingDesktopState,
  readDesktopStateFromSearch,
  rememberDesktopState,
} from "@/lib/desktop-handoff";
import { useAuth } from "@/state/auth";
import { AuthLayout } from "@/components/auth-layout";
import { Button, Field, Input } from "@/components/ui/controls";
import { tr, t } from "@/lib/i18n";

type HandoffPhase = "idle" | "sending" | "done" | "error";

export function LoginScreen() {
  const { user, signIn, signInWithOidc, setup, setHoldLogin } = useAuth();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [oidcBusy, setOidcBusy] = useState(false);
  const [oidcError, setOidcError] = useState<string | null>(null);
  const [desktopAddress, setDesktopAddress] = useState<string | null>(null);
  const exchanged = useRef<string | null>(null);
  const desktopSession = useRef<string | null>(null);
  const handoffStarted = useRef(false);

  // #157: the desktop app opened this page with `?desktop_state=…`. The state
  // must survive the OIDC redirect (the callback only carries `oidc_code`), so
  // it is remembered for this tab and read back after the provider returns.
  const [desktopState] = useState<string | null>(() => {
    const fromUrl = readDesktopStateFromSearch(params);
    return fromUrl ?? pendingDesktopState();
  });
  const [handoffPhase, setHandoffPhase] = useState<HandoffPhase>("idle");
  const [handoffError, setHandoffError] = useState<string | null>(null);

  const config = useQuery({
    queryKey: ["config"],
    queryFn: () => api.config(),
    staleTime: 60_000,
    retry: 0,
  });

  const oidcEnabled = Boolean(config.data?.features.oidc);
  const displayName = config.data?.oidcDisplayName?.trim() || "SSO";
  const code = params.get("oidc_code");

  useEffect(() => {
    if (!desktopState) return;
    rememberDesktopState(desktopState);
    setHoldLogin(true);
    return () => setHoldLogin(false);
  }, [desktopState, setHoldLogin]);

  /**
   * After a successful sign-in, hand the session to the desktop app when this
   * page was opened for it. Without a state, the normal navigation applies and
   * no handoff endpoint is called.
   */
  const finishSignIn = async (force = false) => {
    if (!desktopState) {
      navigate("/", { replace: true });
      return;
    }
    // The password submit, the OIDC code effect and the "already signed in"
    // effect can all reach this point: bind the state exactly once.
    if (handoffStarted.current && !force) return;
    handoffStarted.current = true;
    setHandoffPhase("sending");
    setHandoffError(null);
    try {
      await createDesktopHandoff(desktopState);
      forgetDesktopState();
      setHandoffPhase("done");
    } catch (cause) {
      setHandoffPhase("error");
      setHandoffError(errorMessage(cause));
    }
  };

  // Already signed in in this browser: hand the session back immediately
  // instead of asking the user to sign in again (#157).
  useEffect(() => {
    if (code || !user || !desktopState || handoffPhase !== "idle") return;
    void finishSignIn();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code, user, desktopState, handoffPhase]);

  // The server redirects back to /#/login?oidc_code=... after the provider.
  useEffect(() => {
    if (!code || exchanged.current === code) return;
    exchanged.current = code;
    let cancelled = false;
    setOidcBusy(true);
    setOidcError(null);

    void signInWithOidc(code)
      .then(() => {
        if (!cancelled) return finishSignIn();
      })
      .catch((cause) => {
        if (cancelled) return;
        setOidcError(errorMessage(cause));
        navigate("/login", { replace: true });
      })
      .finally(() => {
        if (!cancelled) setOidcBusy(false);
      });

    return () => {
      cancelled = true;
    };
    // `finishSignIn` is recreated every render; the code is the only trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code, signInWithOidc, navigate]);

  /** Desktop app: open the web login in the system browser and wait for it. */
  const startDesktopOidc = async () => {
    setOidcError(null);
    setOidcBusy(true);
    try {
      const start = await native.oidcLoginStart();
      desktopSession.current = start.sessionId;
      setDesktopAddress(start.webLoginUrl);
      const poll = await native.oidcLoginPoll(start.sessionId);
      desktopSession.current = null;
      if (poll.status === "claimed" && poll.code) {
        // The exchange stays the only place a token is issued (#157).
        await signInWithOidc(poll.code);
        await finishSignIn();
      } else if (poll.status === "cancelled") {
        setOidcError(t("Sign-in cancelled."));
      } else if (poll.status === "expired") {
        setOidcError(poll.message ?? t("The sign-in window expired. Start again from Ludex."));
      } else {
        setOidcError(
          poll.message ?? t("The browser sign-in did not finish. Try again or use your server password."),
        );
      }
    } catch (cause) {
      desktopSession.current = null;
      setOidcError(errorMessage(cause));
    } finally {
      setOidcBusy(false);
    }
  };

  const cancelDesktopOidc = async () => {
    const session = desktopSession.current;
    if (!session) return;
    try {
      await native.oidcLoginCancel(session);
    } catch {
      /* the poll also stops on its own timeout */
    }
  };

  const startOidc = () => {
    setOidcError(null);
    if (isNative()) {
      void startDesktopOidc();
      return;
    }
    if (USING_MOCKS) {
      navigate("/login?oidc_code=demo-sso", { replace: true });
      return;
    }
    const url = assetUrl("/api/auth/oidc/start");
    if (url) window.location.assign(url);
  };

  /** "Open web login" fallback: the same page, without the handoff. */
  const openWebLogin = () => {
    const url = desktopAddress ?? assetUrl("/#/login");
    if (!url) return;
    if (isNative()) {
      void native.openExternalUrl(url).catch((cause) => setOidcError(errorMessage(cause)));
    } else {
      window.location.assign(url);
    }
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy || handoffPhase === "sending") return;
    setBusy(true);
    setError(null);
    try {
      await signIn(username, password);
      await finishSignIn();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  if (handoffPhase === "done") {
    return (
      <AuthLayout
        statement={t("The shelf you actually own.")}
        note={t("Ludex indexes the archives on your server and serves them to your machines. No storefront, no account anywhere else.")}
      >
        <div className="flex flex-col gap-6">
          <div className="flex flex-col gap-3">
            <CheckCircle2 className="size-6 text-signal" aria-hidden="true" />
            <h1 className="text-[34px] font-semibold leading-none tracking-[-0.03em]">
              {t("Return to Ludex; you can close this tab.")}
            </h1>
            <p className="text-[14px] leading-relaxed text-ink-2">
              {t("Ludex has your session. Switch back to the app to continue.")}
            </p>
            <p className="text-[12.5px] leading-relaxed text-ink-3">
              {t("If the app is still waiting, start the sign-in again from Ludex.")}
            </p>
          </div>
        </div>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout
      statement={t("The shelf you actually own.")}
      note={t("Ludex indexes the archives on your server and serves them to your machines. No storefront, no account anywhere else.")}
    >
      <form onSubmit={submit} className="flex flex-col gap-6">
        <div className="flex flex-col gap-2">
          <h1 className="text-[34px] font-semibold leading-none tracking-[-0.03em]">{t("Sign in")}</h1>
          <p className="flex items-center gap-2 text-[13px] text-ink-3">
            <span>{setup?.serverName ?? "Ludex"}</span>
            <span className="h-3 w-px bg-line-2" aria-hidden="true" />
            <span className="num">{`v${setup?.version ?? "0.1.0"}`}</span>
          </p>
        </div>

        {oidcBusy && (
          <p className="flex items-center gap-2 text-[12.5px] text-ink-2" role="status">
            <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
            {isNative() ? t("Waiting for your browser") : t("Completing sign-in…")}
          </p>
        )}

        {oidcError && (
          <div className="flex flex-col gap-2 border-l-2 border-danger pl-3">
            <p role="alert" className="text-[13px] text-danger">
              {oidcError}
            </p>
            {isNative() && (
              <div className="flex flex-wrap gap-2">
                <Button variant="quiet" size="sm" className="px-0 text-ink-2" onClick={openWebLogin}>
                  {t("Open web login")}
                </Button>
                <Button
                  variant="quiet"
                  size="sm"
                  className="px-0 text-ink-2"
                  icon={<RotateCcw className="size-3.5" />}
                  onClick={startOidc}
                >
                  {t("Try again")}
                </Button>
              </div>
            )}
          </div>
        )}

        {oidcEnabled && (
          <div className="flex flex-col gap-4">
            <Button
              variant="default"
              size="lg"
              className="w-full"
              icon={<KeyRound className="size-4" />}
              loading={oidcBusy}
              onClick={startOidc}
            >
              {t("Sign in with")} {displayName}
            </Button>
            {isNative() && oidcBusy && (
              <div className="flex flex-col gap-2">
                <p className="text-[12px] leading-relaxed text-ink-3">
                  {t("Finish the sign-in in your browser, then come back here. This window waits up to five minutes.")}
                </p>
                <Button variant="quiet" size="sm" className="self-start px-0 text-ink-2" onClick={cancelDesktopOidc}>
                  {t("Cancel sign-in")}
                </Button>
              </div>
            )}
            {isNative() && !oidcBusy && !oidcError && (
              <p className="text-[12px] leading-relaxed text-ink-3">
                {t("Single sign-on opens your browser. Password sign-in still works here.")}
              </p>
            )}
            <div className="flex items-center gap-3 text-[11.5px] text-ink-3" aria-hidden="true">
              <span className="h-px flex-1 bg-line" />
              <span>{t("or use a password")}</span>
              <span className="h-px flex-1 bg-line" />
            </div>
          </div>
        )}

        <div className="flex flex-col gap-4">
          <Field label={t("Username")} htmlFor="login-username">
            <Input
              id="login-username"
              name="username"
              autoComplete="username"
              autoFocus
              required
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              placeholder={t("Your username")}
            />
          </Field>
          <Field label={t("Password")} htmlFor="login-password">
            <Input
              id="login-password"
              name="password"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              placeholder={t("Your password")}
            />
          </Field>
        </div>

        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
            {error}
          </p>
        )}

        {handoffPhase === "sending" && (
          <p className="flex items-center gap-2 text-[12.5px] text-ink-2" role="status">
            <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
            {t("Sending the session to Ludex…")}
          </p>
        )}

        {handoffPhase === "error" && handoffError && (
          <div className="flex flex-col gap-2 border-l-2 border-danger pl-3">
            <p role="alert" className="text-[13px] text-danger">
              {handoffError}
            </p>
            <p className="text-[12.5px] leading-relaxed text-ink-3">
              {t("You are signed in here, but Ludex did not receive the session. Try again, or close this tab and sign in from the app.")}
            </p>
            <Button
              variant="quiet"
              size="sm"
              className="self-start px-0 text-ink-2"
              onClick={() => {
                handoffStarted.current = false;
                void finishSignIn();
              }}
            >
              {t("Send the session again")}
            </Button>
          </div>
        )}

        <Button
          type="submit"
          variant="primary"
          size="lg"
          loading={busy || handoffPhase === "sending"}
          iconRight={<ArrowRight className="rtl:-scale-x-100 size-4" />}
        >
          {t("Sign in")}
        </Button>

        {USING_MOCKS && (
          <div className="border-t border-line pt-4">
            <p className="text-[12.5px] leading-relaxed text-ink-3">
              {tr("Demo data is on. Sign in as {user} with the password {password}, or use the single sign-on button.", {
                user: <span className="font-mono text-ink-2">mara</span>,
                password: <span className="font-mono text-ink-2">{MOCK_PASSWORD}</span>,
              })}
            </p>
            <Button
              variant="quiet"
              size="sm"
              className="mt-2 px-0 text-ink-2"
              onClick={() => {
                setUsername("mara");
                setPassword(MOCK_PASSWORD);
                setError(null);
              }}
            >
              {t("Fill demo credentials")}
            </Button>
          </div>
        )}
      </form>
    </AuthLayout>
  );
}
