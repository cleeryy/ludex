import { useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, KeyRound, Loader2 } from "lucide-react";
import { api, assetUrl, USING_MOCKS } from "@/lib/api";
import { MOCK_PASSWORD } from "@/lib/mock";
import { errorMessage } from "@/lib/http";
import { isNative } from "@/lib/native";
import { useAuth } from "@/state/auth";
import { AuthLayout } from "@/components/auth-layout";
import { Button, Field, Input } from "@/components/ui/controls";
import { tr, t } from "@/lib/i18n";

export function LoginScreen() {
  const { signIn, signInWithOidc, setup } = useAuth();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [oidcBusy, setOidcBusy] = useState(false);
  const [oidcError, setOidcError] = useState<string | null>(null);
  const exchanged = useRef<string | null>(null);

  const config = useQuery({
    queryKey: ["config"],
    queryFn: () => api.config(),
    staleTime: 60_000,
    retry: 0,
  });

  const oidcEnabled = Boolean(config.data?.features.oidc);
  const displayName = config.data?.oidcDisplayName?.trim() || "SSO";
  const code = params.get("oidc_code");

  // The server redirects back to /#/login?oidc_code=... after the provider.
  useEffect(() => {
    if (!code || exchanged.current === code) return;
    exchanged.current = code;
    let cancelled = false;
    setOidcBusy(true);
    setOidcError(null);

    void signInWithOidc(code)
      .then(() => {
        if (!cancelled) navigate("/", { replace: true });
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
  }, [code, signInWithOidc, navigate]);

  const startOidc = () => {
    setOidcError(null);
    if (USING_MOCKS) {
      navigate("/login?oidc_code=demo-sso", { replace: true });
      return;
    }
    const url = assetUrl("/api/auth/oidc/start");
    if (url) window.location.assign(url);
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await signIn(username, password);
      navigate("/", { replace: true });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

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
            {t("Completing sign-in…")}
          </p>
        )}

        {oidcError && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
            {oidcError}
          </p>
        )}

        {oidcEnabled && !isNative() && (
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
            <div className="flex items-center gap-3 text-[11.5px] text-ink-3" aria-hidden="true">
              <span className="h-px flex-1 bg-line" />
              <span>{t("or use a password")}</span>
              <span className="h-px flex-1 bg-line" />
            </div>
          </div>
        )}

        {oidcEnabled && isNative() && (
          <p className="border-l-2 border-line pl-3 text-[11.5px] leading-relaxed text-ink-3">
            {t("Single sign-on is configured for this server. It runs in the web interface; in the desktop app, use your server password.")}
          </p>
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

        <Button type="submit" variant="primary" size="lg" loading={busy} iconRight={<ArrowRight className="rtl:-scale-x-100 size-4" />}>
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
