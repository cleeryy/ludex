import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { api, setApiBase, tokenStore, UNAUTHORIZED_EVENT } from "@/lib/api";
import { isNative, native } from "@/lib/native";
import type { SetupStatus, User } from "@/lib/types";
import { t } from "@/lib/i18n";

interface AuthValue {
  user: User | null;
  token: string | null;
  ready: boolean;
  bootError: string | null;
  setup: SetupStatus | null;
  nativeFirstRun: boolean;
  adoptSession: (args: { serverUrl: string; token: string; user?: User }) => Promise<void>;
  signIn: (username: string, password: string) => Promise<void>;
  signInWithOidc: (code: string) => Promise<void>;
  completeSetup: (body: { username: string; password: string; serverName?: string }) => Promise<void>;
  signOut: () => Promise<void>;
  refreshUser: () => Promise<void>;
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
  const [attempt, setAttempt] = useState(0);

  const retryBoot = useCallback(() => {
    setBootError(null);
    setReady(false);
    setAttempt((n) => n + 1);
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function boot() {
      try {
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

        const current = tokenStore.get();
        if (current) {
          try {
            const me = await api.me();
            if (cancelled) return;
            setUser(me);
            setToken(current);
          } catch {
            tokenStore.clear();
            if (!cancelled) {
              setToken(null);
              setUser(null);
            }
          }
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

  const signIn = useCallback(async (username: string, password: string) => {
    const result = await api.login({ username, password });
    tokenStore.set(result.token);
    setToken(result.token);
    setUser(result.user);
    setSetup((current) => (current ? { ...current, needsSetup: false } : current));
  }, []);

  const signInWithOidc = useCallback(async (code: string) => {
    const result = await api.oidcExchange(code);
    tokenStore.set(result.token);
    setToken(result.token);
    setUser(result.user);
    setSetup((current) => (current ? { ...current, needsSetup: false } : current));
  }, []);

  const completeSetup = useCallback(async (body: { username: string; password: string; serverName?: string }) => {
    const result = await api.setup(body);
    tokenStore.set(result.token);
    setToken(result.token);
    setUser(result.user);
    setSetup((current) => (current ? { ...current, needsSetup: false, serverName: body.serverName || current.serverName } : current));
  }, []);

  const signOut = useCallback(async () => {
    try {
      await api.logout();
    } catch {
      /* the token is discarded either way */
    }
    tokenStore.clear();
    setToken(null);
    setUser(null);
  }, []);

  const adoptSession = useCallback(async ({ serverUrl, token: nextToken, user: nextUser }: { serverUrl: string; token: string; user?: User }) => {
    setApiBase(serverUrl);
    tokenStore.set(nextToken);
    setToken(nextToken);
    if (nextUser) {
      setUser(nextUser);
    } else {
      const me = await api.me();
      setUser(me);
    }
    setNativeFirstRun(false);
    setSetup((current) => ({ needsSetup: false, serverName: current?.serverName ?? "Ludex", version: current?.version ?? "0.2.0" }));
    setReady(true);
  }, []);

  const refreshUser = useCallback(async () => {
    const me = await api.me();
    setUser(me);
  }, []);

  const value = useMemo(
    () => ({ user, token, ready, bootError, setup, nativeFirstRun, adoptSession, signIn, signInWithOidc, completeSetup, signOut, refreshUser, retryBoot }),
    [user, token, ready, bootError, setup, nativeFirstRun, adoptSession, signIn, signInWithOidc, completeSetup, signOut, refreshUser, retryBoot],
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
