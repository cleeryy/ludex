import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { HashRouter, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { api } from "@/lib/api";
import { isNative, native } from "@/lib/native";
import { useAuth, useIsAdmin, useIsEditor } from "@/state/auth";
import { SettingsProvider } from "@/state/settings";
import { ToastProvider } from "@/state/toasts";
import { AuthProvider } from "@/state/auth";
import { DownloadsProvider } from "@/state/downloads";
import { Wordmark } from "@/components/brand";
import { Button } from "@/components/ui/controls";
import { ToastViewport } from "@/components/ui/toast";
import { AppShell } from "@/components/shell/app-shell";
import { LoginScreen } from "@/screens/login";
import { SetupScreen } from "@/screens/setup";
import { HomeScreen } from "@/screens/home";
import { LibraryScreen } from "@/screens/library";
import { DownloadsScreen } from "@/screens/downloads";
import { BigPictureScreen } from "@/screens/big-picture";
import { GameDetailScreen } from "@/screens/game-detail";
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
  return (
    <div className="grid min-h-dvh place-items-center bg-bg">
      <div className="flex flex-col items-center gap-5">
        <Wordmark height={22} />
        <div className="h-px w-44 overflow-hidden bg-line">
          <div className="h-full w-1/3 animate-[slide_1.4s_ease-in-out_infinite] bg-signal" />
        </div>
        <p className="text-[11.5px] text-ink-3">Opening the library</p>
      </div>
    </div>
  );
}

function BootError({ message }: { message: string }) {
  const { retryBoot } = useAuth();
  return (
    <div className="flex min-h-dvh flex-col bg-bg">
      <div className="px-6 pt-6">
        <Wordmark height={18} />
      </div>
      <div className="flex flex-1 items-center px-6">
        <div className="w-full max-w-[480px]">
          <h1 className="text-[30px] font-semibold leading-tight tracking-[-0.03em]">The server did not answer</h1>
          <p className="mt-3 text-[14px] leading-relaxed text-ink-2">{message}</p>
          <p className="mt-2 text-[12.5px] leading-relaxed text-ink-3">
            Check that the Ludex server is running, then try again. In the desktop app you can also reset the connection and
            point it at another address.
          </p>
          <div className="mt-6 flex flex-wrap gap-2">
            <Button variant="primary" onClick={retryBoot}>
              Try again
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
                Reset connection
              </Button>
            )}
          </div>
        </div>
      </div>
      <div className="px-6 pb-6 font-mono text-[11px] text-ink-3">{api ? "api: configured" : null}</div>
    </div>
  );
}

function Gate({ children }: { children: React.ReactNode }) {
  const { ready, bootError, user, setup } = useAuth();
  const location = useLocation();

  if (!ready) return <BootScreen />;
  if (bootError) return <BootError message={bootError} />;

  const needsSetup = setup?.needsSetup ?? false;
  const at = location.pathname;

  if (needsSetup && at !== "/setup") return <Navigate to="/setup" replace />;
  if (!needsSetup && at === "/setup") return <Navigate to={user ? "/" : "/login"} replace />;
  if (!needsSetup && !user && at !== "/login") return <Navigate to="/login" replace />;
  if (user && at === "/login") return <Navigate to="/" replace />;

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

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <SettingsProvider>
        <ToastProvider>
          <AuthProvider>
            <DownloadsProvider>
            <HashRouter>
              <Routes>
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
                  <Route path="game/:id" element={<GameDetailScreen />} />
                  <Route path="collections" element={<CollectionsScreen />} />
                  <Route path="collections/:id" element={<CollectionScreen />} />
                  <Route path="community" element={<CommunityScreen />} />
                  <Route path="community/u/:id" element={<ProfileScreen />} />
                  <Route path="news" element={<NewsScreen />} />
                  <Route path="settings" element={<SettingsScreen />} />
                  <Route
                    path="admin/ingest"
                    element={
                      <RequireEditor>
                        <IngestScreen />
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
                  <Route path="*" element={<NotFoundScreen />} />
                </Route>
              </Routes>
            </HashRouter>
            <ToastViewport />
            </DownloadsProvider>
          </AuthProvider>
        </ToastProvider>
      </SettingsProvider>
    </QueryClientProvider>
  );
}
