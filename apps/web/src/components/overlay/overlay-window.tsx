import { useEffect, useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { setApiBase, tokenStore } from "@/lib/api";
import { isNative, native } from "@/lib/native";
import { I18nProvider } from "@/lib/i18n";
import { OverlayScreen } from "@/components/overlay/overlay-screen";

/**
 * Entry point of the floating `overlay` window (#144).
 *
 * The window loads the same bundle as the main app with `?overlay=1` in the
 * query string. It gets its own tiny provider tree instead of the full app:
 * no router, no auth gate, no toasts. The server URL and token are copied from
 * the desktop settings so achievements can load, and the document background is
 * made transparent so the game stays visible around the panel.
 */

export function isOverlayEntry(search?: string): boolean {
  const query = search ?? (typeof window === "undefined" ? "" : window.location.search);
  return new URLSearchParams(query).get("overlay") === "1";
}

const overlayQueryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 0,
      staleTime: 10_000,
      refetchOnWindowFocus: false,
    },
  },
});

export function OverlayWindowApp() {
  // A transparent Tauri window still paints the document background, which
  // would hide the game completely. Set it inline: this window is the only
  // place that wants transparency, so no global stylesheet rule is needed.
  if (typeof document !== "undefined") {
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
  }
  return (
    <QueryClientProvider client={overlayQueryClient}>
      <I18nProvider>
        <OverlayBootstrapper />
      </I18nProvider>
    </QueryClientProvider>
  );
}

function OverlayBootstrapper() {
  const ready = useOverlayBootstrap();
  if (!ready) return <div className="h-dvh w-full bg-transparent" />;
  return <OverlayScreen mode="window" onClose={() => undefined} />;
}

/**
 * Copy the server URL/token from the desktop settings once, before the overlay
 * queries run. A failure is not fatal: library and downloads are local.
 */
function useOverlayBootstrap(): boolean {
  const [ready, setReady] = useState(() => !isNative());
  useEffect(() => {
    if (!isNative()) return;
    let cancelled = false;
    void (async () => {
      try {
        const settings = await native.getSettings();
        if (settings?.serverUrl) setApiBase(settings.serverUrl);
        if (settings?.token) tokenStore.set(settings.token);
      } catch {
        /* local content still works */
      }
      if (!cancelled) setReady(true);
    })();
    return () => {
      cancelled = true;
    };
  }, []);
  return ready;
}
