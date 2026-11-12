import { useSyncExternalStore } from "react";
import { isNative } from "@/lib/native";

/**
 * PWA glue: install prompt, update prompt and service-worker registration.
 * The service worker is production-only so `vite dev` never serves a stale
 * shell. All state is exposed through a tiny external store so the shell can
 * render the prompts with `useSyncExternalStore`.
 *
 * Inside the desktop app the service worker is never registered: WebView2
 * persists the profile across Windows updates, so an old worker would keep
 * serving the previous frontend (#177).
 */

interface BeforeInstallPromptEvent extends Event {
  readonly platforms: string[];
  prompt: () => Promise<void>;
  userChoice: Promise<{ outcome: "accepted" | "dismissed"; platform: string }>;
}

export interface PwaState {
  canInstall: boolean;
  installed: boolean;
  updateReady: boolean;
  offlineReady: boolean;
}

const INSTALL_DISMISS_KEY = "ludex.pwa.installDismissedAt";
const INSTALL_DISMISS_DAYS = 30;

let deferredPrompt: BeforeInstallPromptEvent | null = null;
let registration: ServiceWorkerRegistration | null = null;
let reloading = false;
let initialized = false;
let snapshot: PwaState = { canInstall: false, installed: false, updateReady: false, offlineReady: false };
const listeners = new Set<() => void>();

function dismissedRecently(): boolean {
  try {
    const at = Number(localStorage.getItem(INSTALL_DISMISS_KEY) ?? 0);
    return Number.isFinite(at) && Date.now() - at < INSTALL_DISMISS_DAYS * 86_400_000;
  } catch {
    return false;
  }
}

function emit(patch: Partial<PwaState>): void {
  const next = { ...snapshot, ...patch };
  if (
    next.canInstall === snapshot.canInstall &&
    next.installed === snapshot.installed &&
    next.updateReady === snapshot.updateReady &&
    next.offlineReady === snapshot.offlineReady
  ) {
    return;
  }
  snapshot = next;
  for (const listener of listeners) listener();
}

export function subscribePwa(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getPwaSnapshot(): PwaState {
  return snapshot;
}

export function usePwa(): PwaState {
  return useSyncExternalStore(subscribePwa, getPwaSnapshot, getPwaSnapshot);
}

export function initPwa(): void {
  if (initialized || typeof window === "undefined") return;
  initialized = true;

  // Desktop (#177): drop anything an older build left in the WebView2 profile
  // and never register a worker there. Tauri serves its own assets, so the
  // install/update prompts do not apply either.
  if (isNative()) {
    void clearNativeWorker();
    return;
  }

  window.addEventListener("beforeinstallprompt", (event) => {
    event.preventDefault();
    deferredPrompt = event as BeforeInstallPromptEvent;
    emit({ canInstall: !dismissedRecently() });
  });

  window.addEventListener("appinstalled", () => {
    deferredPrompt = null;
    emit({ canInstall: false, installed: true });
  });

  if (window.matchMedia("(display-mode: standalone)").matches) {
    emit({ installed: true, canInstall: false });
  }

  if (!("serviceWorker" in navigator) || !import.meta.env.PROD) return;

  void registerWorker();
}

/**
 * Remove the worker and shell caches a previous desktop build left in the
 * WebView2 profile (#177). Only `ludex-*` caches are deleted: other caches in
 * that profile are not ours. Failures are swallowed — cleanup must never break
 * startup.
 */
async function clearNativeWorker(): Promise<void> {
  try {
    if ("serviceWorker" in navigator) {
      for (const registration of await navigator.serviceWorker.getRegistrations()) {
        await registration.unregister();
      }
    }
    if ("caches" in window) {
      for (const key of await caches.keys()) {
        if (key.startsWith("ludex-")) await caches.delete(key);
      }
    }
  } catch {
    // A stale worker must never break the app.
  }
}

async function registerWorker(): Promise<void> {
  try {
    const url = new URL("sw.js", window.location.href);
    registration = await navigator.serviceWorker.register(url, { scope: new URL("./", url).pathname });

    if (registration.waiting) emit({ updateReady: true });
    registration.addEventListener("updatefound", () => {
      const installing = registration?.installing;
      if (!installing) return;
      installing.addEventListener("statechange", () => {
        if (installing.state === "installed" && navigator.serviceWorker.controller) {
          emit({ updateReady: true });
        }
      });
    });

    navigator.serviceWorker.ready.then(() => emit({ offlineReady: true }));

    navigator.serviceWorker.addEventListener("controllerchange", () => {
      if (reloading) window.location.reload();
    });

    // Check for a new build while the tab stays open, without hammering.
    window.setInterval(() => void registration?.update().catch(() => undefined), 60 * 60 * 1000);
  } catch {
    // A failed registration must never break the app.
  }
}

export async function promptInstall(): Promise<boolean> {
  if (!deferredPrompt) return false;
  await deferredPrompt.prompt();
  const choice = await deferredPrompt.userChoice;
  deferredPrompt = null;
  emit({ canInstall: false, installed: choice.outcome === "accepted" });
  return choice.outcome === "accepted";
}

export function dismissInstall(): void {
  try {
    localStorage.setItem(INSTALL_DISMISS_KEY, String(Date.now()));
  } catch {
    /* private mode */
  }
  emit({ canInstall: false });
}

export function applyUpdate(): void {
  reloading = true;
  const waiting = registration?.waiting;
  if (waiting) waiting.postMessage({ type: "SKIP_WAITING" });
  else window.location.reload();
}

export function dismissUpdate(): void {
  emit({ updateReady: false });
}
