import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { HashRouter, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { useEffect } from "react";
import { api } from "@/lib/api";
import { pendingDesktopState, readDesktopStateFromSearch } from "@/lib/desktop-handoff";
import { isNative, native } from "@/lib/native";
import { useAuth, useIsAdmin, useIsEditor } from "@/state/auth";
import { SettingsProvider } from "@/state/settings";
import { I18nProvider, useI18n } from "@/lib/i18n";
import { UpdateProvider } from "@/components/updates/update-provider";
import { UpdateNotifier } from "@/components/updates/update-notifier";
import { UpdatesScreen } from "@/screens/updates";
import { ToastProvider } from "@/state/toasts";
import { AuthProvider } from "@/state/auth";
import { DownloadsProvider } from "@/state/downloads";
import { Wordmark } from "@/components/brand";
import { Button } from "@/components/ui/controls";
import { ToastViewport } from "@/components/ui/toast";
import { PwaPrompts } from "@/components/pwa-prompts";
import { AppShell } from "@/components/shell/app-shell";
import { FloatingWindowControls } from "@/components/shell/window-controls";
import { useCustomTitleBar } from "@/lib/window-chrome";
import { LoginScreen } from "@/screens/login";
import { SetupScreen } from "@/screens/setup";
import { HomeScreen } from "@/screens/home";
import { LibraryScreen } from "@/screens/library";
import { DownloadsScreen } from "@/screens/downloads";
import { BigPictureScreen } from "@/screens/big-picture";
import { GameDetailRoute } from "@/screens/game-detail";
import { NewsScreen } from "@/screens/news";
import { CollectionsScreen } from "@/screens/collections";
import { CollectionScreen } from "@/screens/collection";
import { CommunityScreen } from "@/screens/community";
import { ProfileScreen } from "@/screens/profile";
import { SettingsScreen } from "@/screens/settings";
import { NotFoundScreen } from "@/screens/not-found";
import { IngestScreen } from "@/screens/admin/ingest";
import { ScanScreen } from "@/screens/admin/scan";
import { UsersScreen } from "@/screens/admin/users";
import { AdminNewsScreen } from "@/screens/admin/news";
import { ServerScreen } from "@/screens/admin/server";
import { AuthenticationScreen } from "@/screens/admin/authentication";
import { MetadataProvidersScreen } from "@/screens/admin/metadata";
import { UploadScreen } from "@/screens/admin/upload";
import { InvitationsScreen } from "@/screens/admin/invitations";
import { ShareScreen } from "@/screens/share";
import { DownloadScreen } from "@/screens/download";
import { RegisterScreen } from "@/screens/register";
import { t } from "@/lib/i18n";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      staleTime: 20_000,
      refetchOnWindowFocus: false,
    },
  },
});

function BootScreen() {
  const customChrome = useCustomTitleBar();
  return (
    <div
      data-tauri-drag-region={customChrome ? "deep" : undefined}
      className="grid min-h-dvh place-items-center bg-bg"
    >
      <div className="flex flex-col items-center gap-5">
        <Wordmark height={22} />
        <div className="h-px w-44 overflow-hidden bg-line">
          <div className="h-full w-1/3 animate-[slide_1.4s_ease-in-out_infinite] bg-signal" />
        </div>
        <p className="text-[11.5px] text-ink-3">{t("Opening the library")}</p>
      </div>
      <FloatingWindowControls />
    </div>
  );
}

function BootError({ message }: { message: string }) {
  const { retryBoot } = useAuth();
  const customChrome = useCustomTitleBar();
  return (
    <div className="flex min-h-dvh flex-col bg-bg">
      {/* The wordmark row doubles as the drag strip: the message below stays
          selectable, which matters when it has to be copied. */}
      <div data-tauri-drag-region={customChrome ? "deep" : undefined} className="px-6 pt-6 pb-2">
        <Wordmark height={18} />
      </div>
      <div className="flex flex-1 items-center px-6">
        <div className="w-full max-w-[480px]">
          <h1 className="text-[30px] font-semibold leading-tight tracking-[-0.03em]">{t("The server did not answer")}</h1>
          <p className="mt-3 text-[14px] leading-relaxed text-ink-2">{message}</p>
          <p className="mt-2 text-[12.5px] leading-relaxed text-ink-3">
            {t("Check that the Ludex server is running, then try again. In the desktop app you can also reset the connection and point it at another address.")}
          </p>
          <div className="mt-6 flex flex-wrap gap-2">
            <Button variant="primary" onClick={retryBoot}>
              {t("Try again")}
            </Button>
            {isNative() && (
              <Button
                variant="default"
                onClick={async () => {
                  const current = await native.getSettings();
                  if (current) await native.saveSettings({ ...current, serverUrl: "", token: null });
                  window.location.reload();
                }}
              >
                {t("Reset connection")}
              </Button>
            )}
          </div>
        </div>
      </div>
      <div className="px-6 pb-6 font-mono text-[11px] text-ink-3">{api ? "api: configured" : null}</div>
      <FloatingWindowControls />
    </div>
  );
}

function Gate({ children }: { children: React.ReactNode }) {
  const { ready, bootError, user, setup, holdLogin } = useAuth();
  const location = useLocation();

  if (!ready) return <BootScreen />;
  if (bootError) return <BootError message={bootError} />;

  const needsSetup = setup?.needsSetup ?? false;
  const at = location.pathname;
  // #157: a desktop handoff opens /login in the user's browser. It must stay
  // reachable both before signing in and when that browser already has a
  // session (the page then hands the session back immediately).
  const desktopHandoff =
    readDesktopStateFromSearch(location.search) !== null || pendingDesktopState() !== null;

  if (needsSetup && at !== "/setup") return <Navigate to="/setup" replace />;
  if (!needsSetup && at === "/setup") return <Navigate to={user ? "/" : "/login"} replace />;
  if (!needsSetup && !user && at !== "/login") return <Navigate to="/login" replace />;
  // #157: a desktop handoff keeps the login page mounted just long enough to
  // show "Return to Ludex", even though the account is already signed in.
  if (user && at === "/login" && !holdLogin && !desktopHandoff) return <Navigate to="/" replace />;

  return <>{children}</>;
}

function RequireEditor({ children }: { children: React.ReactNode }) {
  const isEditor = useIsEditor();
  if (!isEditor) return <Navigate to="/" replace />;
  return <>{children}</>;
}

function RequireAdmin({ children }: { children: React.ReactNode }) {
  const isAdmin = useIsAdmin();
  if (!isAdmin) return <Navigate to="/" replace />;
  return <>{children}</>;
}

/**
 * Mirror the app locale and profile display name into the desktop settings
 * (#148), so a game that opted into "keep in sync" gets the current values on
 * its next launch even when its settings page is never opened.
 */
let lastSyncedDefaults = "";

function ConfigSyncDefaults() {
  const { locale } = useI18n();
  const { user } = useAuth();
  const profile = useQuery({
    queryKey: ["my-profile"],
    queryFn: () => api.myProfile(),
    enabled: isNative() && Boolean(user),
    staleTime: 60_000,
    retry: false,
  });
  const name = profile.data?.displayName?.trim() || user?.username || "";

  useEffect(() => {
    if (!isNative()) return;
    const key = `${locale}\u0000${name}`;
    if (key === lastSyncedDefaults) return;
    void native
      .setConfigSyncDefaults(locale, name)
      .then(() => {
        lastSyncedDefaults = key;
      })
      .catch(() => {
        /* the mirror is best effort; the game settings page still works */
      });
  }, [locale, name]);

  return null;
}

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <SettingsProvider>
        <ToastProvider>
          <AuthProvider>
            <DownloadsProvider>
            <I18nProvider>
            <UpdateProvider>
            <ConfigSyncDefaults />
            <HashRouter>
              <UpdateNotifier />
              <Routes>
                {/* Public and account-free: must stay outside the Gate (#9). */}
                <Route path="/share/:token" element={<ShareScreen />} />
                {/* Public download page (#164): reachable logged out, never gated. */}
                <Route path="/download" element={<DownloadScreen />} />
                {/* Invitation landing page (#166): the invitee has no account yet. */}
                <Route path="/register" element={<RegisterScreen />} />
                <Route
                  path="/setup"
                  element={
                    <Gate>
                      <SetupScreen />
                    </Gate>
                  }
                />
                <Route
                  path="/bigpicture"
                  element={
                    <Gate>
                      <BigPictureScreen />
                    </Gate>
                  }
                />
                <Route
                  path="/login"
                  element={
                    <Gate>
                      <LoginScreen />
                    </Gate>
                  }
                />
                <Route
                  element={
                    <Gate>
                      <AppShell />
                    </Gate>
                  }
                >
                  <Route index element={<HomeScreen />} />
                  <Route path="library" element={<LibraryScreen mode="library" />} />
                  <Route path="downloads" element={<DownloadsScreen />} />
                  <Route path="recent" element={<LibraryScreen mode="recent" />} />
                  <Route path="bookmarks" element={<LibraryScreen mode="bookmarks" />} />
                  <Route path="all" element={<LibraryScreen mode="index" />} />
                  <Route path="game/:id" element={<GameDetailRoute />} />
                  <Route path="collections" element={<CollectionsScreen />} />
                  <Route path="collections/:id" element={<CollectionScreen />} />
                  <Route path="community" element={<CommunityScreen />} />
                  <Route path="community/u/:id" element={<ProfileScreen />} />
                  <Route path="news" element={<NewsScreen />} />
                  <Route path="settings" element={<SettingsScreen />} />
                  <Route path="updates" element={<UpdatesScreen />} />
                  <Route
                    path="admin/ingest"
                    element={
                      <RequireEditor>
                        <IngestScreen />
                      </RequireEditor>
                    }
                  />
                  <Route
                    path="admin/upload"
                    element={
                      <RequireEditor>
                        <UploadScreen />
                      </RequireEditor>
                    }
                  />
                  <Route
                    path="admin/scan"
                    element={
                      <RequireEditor>
                        <ScanScreen />
                      </RequireEditor>
                    }
                  />
                  <Route
                    path="admin/users"
                    element={
                      <RequireAdmin>
                        <UsersScreen />
                      </RequireAdmin>
                    }
                  />
                  <Route
                    path="admin/invitations"
                    element={
                      <RequireAdmin>
                        <InvitationsScreen />
                      </RequireAdmin>
                    }
                  />
                  <Route
                    path="admin/news"
                    element={
                      <RequireAdmin>
                        <AdminNewsScreen />
                      </RequireAdmin>
                    }
                  />
                  <Route
                    path="admin/server"
                    element={
                      <RequireAdmin>
                        <ServerScreen />
                      </RequireAdmin>
                    }
                  />
                  <Route
                    path="admin/authentication"
                    element={
                      <RequireAdmin>
                        <AuthenticationScreen />
                      </RequireAdmin>
                    }
                  />
                  <Route
                    path="admin/metadata"
                    element={
                      <RequireAdmin>
                        <MetadataProvidersScreen />
                      </RequireAdmin>
                    }
                  />
                  <Route path="*" element={<NotFoundScreen />} />
                </Route>
              </Routes>
            </HashRouter>
            <ToastViewport />
            <PwaPrompts />
            </UpdateProvider>
            </I18nProvider>
            </DownloadsProvider>
          </AuthProvider>
        </ToastProvider>
      </SettingsProvider>
    </QueryClientProvider>
  );
}
