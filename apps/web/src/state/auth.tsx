import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { api, setApiBase, tokenStore, UNAUTHORIZED_EVENT } from "@/lib/api";
import { ApiError } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { bootstrapBrowserServer } from "@/lib/servers";
import type { AuthResult, SetupStatus, User } from "@/lib/types";
import { applyAccountLocale, applyServerDefaultLocale, chooseAccountLocale, t } from "@/lib/i18n";

interface AuthValue {
  user: User | null;
  token: string | null;
  ready: boolean;
  bootError: string | null;
  setup: SetupStatus | null;
  nativeFirstRun: boolean;
  /**
   * Keeps `/login` mounted after a successful sign-in while a desktop handoff
   * finishes (#157); the router redirects authenticated users otherwise.
   */
  holdLogin: boolean;
  setHoldLogin: (hold: boolean) => void;
  adoptSession: (args: { serverUrl: string; token: string; user?: User }) => Promise<void>;
  /**
   * Password step. Returns `{ mfaRequired: true }` without signing in when the
   * account has a second factor (#166); the caller then asks for the code.
   */
  signIn: (username: string, password: string) => Promise<AuthResult>;
  /** Second step of a 2FA login (#166). */
  completeMfaSignIn: (challengeToken: string, code: string) => Promise<void>;
  /** Redeems an invitation link and signs the new account in (#166). */
  acceptInvitation: (
    token: string,
    body: { username: string; password: string },
  ) => Promise<void>;
  signInWithOidc: (code: string) => Promise<void>;
  completeSetup: (body: { username: string; password: string; serverName?: string }) => Promise<void>;
  signOut: () => Promise<void>;
  refreshUser: () => Promise<void>;
  /**
   * Persists the wizard preference (#119). The local account is updated first
   * so the UI never blocks on the request; a failed write keeps the optimistic
   * value for this session and retries on the next explicit action.
   */
  updateOnboarding: (body: { completed?: boolean; step?: number }) => Promise<void>;
  /**
   * Persists the account interface language (#188). `null` clears it so the
   * account follows the server default again; the UI applies it immediately.
   */
  updateLocale: (locale: string | null) => Promise<void>;
  retryBoot: () => void;
}

const AuthContext = createContext<AuthValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [token, setToken] = useState<string | null>(() => tokenStore.get());
  const [setup, setSetup] = useState<SetupStatus | null>(null);
  const [ready, setReady] = useState(false);
  const [bootError, setBootError] = useState<string | null>(null);
  const [nativeFirstRun, setNativeFirstRun] = useState(false);
  const [holdLogin, setHoldLogin] = useState(false);
  const [attempt, setAttempt] = useState(0);

  const retryBoot = useCallback(() => {
    setBootError(null);
    setUser(null);
    setToken(tokenStore.get());
    setReady(false);
    setAttempt((n) => n + 1);
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function boot() {
      try {
        // #24: a browser can follow several saved servers; point the client at
        // the active one before the first request.
        if (!isNative()) bootstrapBrowserServer();
        if (isNative()) {
          const settings = await native.getSettings();
          if (settings?.serverUrl) setApiBase(settings.serverUrl);
          if (settings?.token) tokenStore.set(settings.token);
          else if (settings?.mode === "remote") tokenStore.clear();

          // First run in the desktop app: no embedded server yet, so there is
          // nothing to call. The setup wizard starts it (or connects to one).
          if (!settings?.serverUrl) {
            if (!cancelled) {
              setNativeFirstRun(true);
              setSetup({ needsSetup: true, serverName: "Ludex", version: "0.2.0" });
              setReady(true);
            }
            return;
          }
        }
        const status = await api.setupStatus();
        if (cancelled) return;
        setSetup(status);

        // #188: the public config carries the server default language, which
        // the login/setup screens use before any account exists. The cache is
        // written first so a reload bootstraps without a flash.
        try {
          const config = await api.config();
          if (!cancelled) await applyServerDefaultLocale(config.defaultLocale);
        } catch {
          /* older server or offline: keep the cached/browser locale */
        }

        const current = tokenStore.get();
        if (current) {
          try {
            const me = await api.me();
            if (cancelled) return;
            setUser(me);
            setToken(current);
            await applyAccountLocale(me.locale);
          } catch (cause) {
            tokenStore.clear();
            if (!cancelled) {
              setToken(null);
              setUser(null);
            }
            // Keep the cached account language when the server is merely
            // unreachable (the desktop must still boot in the last language);
            // drop it when the session is actually rejected.
            const rejected = cause instanceof ApiError && cause.status === 401;
            if (rejected) await applyAccountLocale(null);
          }
        } else {
          await applyAccountLocale(null);
        }
        if (!cancelled) setReady(true);
      } catch (error) {
        if (cancelled) return;
        setBootError(error instanceof Error ? error.message : t("Cannot reach the server"));
        setReady(true);
      }
    }

    void boot();
    return () => {
      cancelled = true;
    };
  }, [attempt]);

  useEffect(() => {
    const onUnauthorized = () => {
      setUser(null);
      setToken(null);
    };
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
  }, []);

  /** Stores a completed sign-in in the token store and in memory. */
  const adoptAuthResult = useCallback((result: AuthResult) => {
    const { token: nextToken, user: nextUser } = result;
    if (!nextToken || !nextUser) return false;
    tokenStore.set(nextToken);
    setToken(nextToken);
    setUser(nextUser);
    setSetup((current) => (current ? { ...current, needsSetup: false } : current));
    // #188: cache and apply the account language as soon as login answers.
    void applyAccountLocale(nextUser.locale);
    return true;
  }, []);

  const signIn = useCallback(
    async (username: string, password: string) => {
      const result = await api.login({ username, password });
      adoptAuthResult(result);
      return result;
    },
    [adoptAuthResult],
  );

  const completeMfaSignIn = useCallback(
    async (challengeToken: string, code: string) => {
      const result = await api.loginMfa({ challengeToken, code });
      if (!adoptAuthResult(result)) {
        throw new Error(t("The sign-in challenge was not accepted. Start again."));
      }
      const issuedToken = result.token;
      if (isNative() && issuedToken) {
        try {
          const current = await native.getSettings();
          if (current) await native.saveSettings({ ...current, token: issuedToken });
        } catch {
          // The session still works for this run.
        }
      }
    },
    [adoptAuthResult],
  );

  const acceptInvitation = useCallback(
    async (inviteToken: string, body: { username: string; password: string }) => {
      const result = await api.acceptInvitation(inviteToken, body);
      if (!adoptAuthResult(result)) {
        throw new Error(t("The invitation was not accepted."));
      }
    },
    [adoptAuthResult],
  );

  const signInWithOidc = useCallback(
    async (code: string) => {
      const result = await api.oidcExchange(code);
      if (!adoptAuthResult(result)) {
        throw new Error(t("The server did not return a session."));
      }
      // Desktop SSO (#157): the exchanged session must reach the active server
      // profile (its token goes to the OS keychain or the local secret store), so
      // signing in on the login screen survives a restart.
      const issuedToken = result.token;
      if (isNative() && issuedToken) {
        try {
          const current = await native.getSettings();
          if (current) await native.saveSettings({ ...current, token: issuedToken });
        } catch {
          // The session still works for this run; the next save persists it.
        }
      }
    },
    [adoptAuthResult],
  );

  const completeSetup = useCallback(
    async (body: { username: string; password: string; serverName?: string }) => {
      const result = await api.setup(body);
      if (!adoptAuthResult(result)) {
        throw new Error(t("The server did not return a session."));
      }
      setSetup((current) =>
        current
          ? { ...current, needsSetup: false, serverName: body.serverName || current.serverName }
          : current,
      );
    },
    [adoptAuthResult],
  );

  const signOut = useCallback(async () => {
    try {
      await api.logout();
    } catch {
      /* the token is discarded either way */
    }
    tokenStore.clear();
    setToken(null);
    setUser(null);
    // #188: the next visitor on this device must not inherit the account
    // language; the server/browser default applies again.
    await applyAccountLocale(null);
  }, []);

  const adoptSession = useCallback(async ({ serverUrl, token: nextToken, user: nextUser }: { serverUrl: string; token: string; user?: User }) => {
    setApiBase(serverUrl);
    tokenStore.set(nextToken);
    setToken(nextToken);
    if (nextUser) {
      setUser(nextUser);
      await applyAccountLocale(nextUser.locale);
    } else {
      const me = await api.me();
      setUser(me);
      await applyAccountLocale(me.locale);
    }
    setNativeFirstRun(false);
    setSetup((current) => ({ needsSetup: false, serverName: current?.serverName ?? "Ludex", version: current?.version ?? "0.2.0" }));
    setReady(true);
  }, []);

  const refreshUser = useCallback(async () => {
    const me = await api.me();
    setUser(me);
    await applyAccountLocale(me.locale);
  }, []);

  const updateLocale = useCallback(async (locale: string | null) => {
    const next = await api.updateAccountProfile({ locale });
    setUser(next);
    // An explicit account choice also drops a per-device override, so the
    // picker never leaves the UI on a language the user just moved away from.
    await chooseAccountLocale(next.locale);
  }, []);

  const updateOnboarding = useCallback(async (body: { completed?: boolean; step?: number }) => {
    setUser((current) => {
      if (!current) return current;
      return {
        ...current,
        onboardingStep: body.step ?? current.onboardingStep,
        onboardingCompletedAt:
          body.completed === undefined
            ? current.onboardingCompletedAt
            : body.completed
              ? new Date().toISOString()
              : null,
      };
    });
    try {
      const next = await api.updateOnboarding(body);
      setUser(next);
    } catch {
      // Offline or old server: the optimistic value stands for this session.
    }
  }, []);

  const value = useMemo(
    () => ({ user, token, ready, bootError, setup, nativeFirstRun, holdLogin, setHoldLogin, adoptSession, signIn, completeMfaSignIn, acceptInvitation, signInWithOidc, completeSetup, signOut, refreshUser, updateOnboarding, updateLocale, retryBoot }),
    [user, token, ready, bootError, setup, nativeFirstRun, holdLogin, adoptSession, signIn, completeMfaSignIn, acceptInvitation, signInWithOidc, completeSetup, signOut, refreshUser, updateOnboarding, updateLocale, retryBoot],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthValue {
  const value = useContext(AuthContext);
  if (!value) throw new Error("useAuth must be used inside AuthProvider");
  return value;
}

export function useIsEditor(): boolean {
  const { user } = useAuth();
  return user?.role === "admin" || user?.role === "editor";
}

export function useIsAdmin(): boolean {
  const { user } = useAuth();
  return user?.role === "admin";
}
