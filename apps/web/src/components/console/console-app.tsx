import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { isNative, native } from "@/lib/native";
import { useAuth } from "@/state/auth";
import { ConsoleNoticeProvider } from "@/components/console/surfaces/console-notice";
import { ConsoleKeepAwakeNotice } from "@/components/console/surfaces/console-keep-awake";
import { ConsoleRecoveryNotice } from "@/components/console/surfaces/console-recovery-notice";
import { ConsoleRoutes } from "@/components/console/routes";
import {
  ConsoleBoot,
  ConsoleNeedsSetup,
  ConsoleNoServer,
  ConsoleOffline,
  ConsoleSignedOut,
} from "@/components/console/shell/console-gate";

/**
 * The lazy `/console/*` entry (#267).
 *
 * This is a sibling of the desktop shell in the same SPA, not a second webview:
 * `app.tsx` mounts it outside `Gate`, so it owns its boot and gate screens and
 * never shows the desktop login. Providers stay global (one QueryClient, Auth,
 * Settings, I18n, downloads store); the console adds only its own notice
 * surface. Desktop globals (toasts, onboarding, PWA prompts, update dialog,
 * overlay host) live behind `AppShell` and therefore cannot paint here.
 */
/**
 * Native navigation (#267): the desktop sets the hash and emits
 * `ui://navigate`, because a hash assignment alone does not wake
 * `HashRouter`. The desktop shell subscribes through `useNativeEvents`; the
 * Console is its own shell, so it subscribes here. The floating overlay's
 * "Console mode" also arrives on the main window through `overlay://navigate`.
 */
function useConsoleNativeNavigation() {
  const navigate = useNavigate();
  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    const offs: Array<() => void> = [];
    const add = (pending: Promise<() => void>) => {
      void pending
        .then((off) => {
          if (disposed) off();
          else offs.push(off);
        })
        .catch(() => undefined);
    };
    add(native.onNavigate((path) => navigate(path)));
    add(native.onOverlayNavigate((path) => navigate(path)));
    return () => {
      disposed = true;
      for (const off of offs) off();
    };
  }, [navigate]);
}

export function ConsoleApp() {
  const { ready, bootError, user, setup, retryBoot } = useAuth();
  useConsoleNativeNavigation();
  const [online, setOnline] = useState(() => (typeof navigator === "undefined" ? true : navigator.onLine));

  useEffect(() => {
    const update = () => setOnline(navigator.onLine);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);

  if (!ready) return <ConsoleBoot />;
  if (bootError) return <ConsoleNoServer message={bootError} />;
  if (setup?.needsSetup) return <ConsoleNeedsSetup />;
  // When signed in, allow booting into Console Home offline so locally
  // installed games still play; cold signed-out offline stays on the gate.
  if (!online && !user) return <ConsoleOffline onRetry={retryBoot} />;
  if (!user) return <ConsoleSignedOut />;

  return (
    <ConsoleNoticeProvider>
      <ConsoleKeepAwakeNotice />
      <ConsoleRecoveryNotice />
      <ConsoleRoutes />
    </ConsoleNoticeProvider>
  );
}
