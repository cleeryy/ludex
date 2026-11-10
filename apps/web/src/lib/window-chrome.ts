import { useEffect, useSyncExternalStore } from "react";
import { isNative, native } from "@/lib/native";

/**
 * Window chrome for the desktop app (#111).
 *
 * Windows and Linux run undecorated (`decorations: false` in the Tauri config)
 * and Ludex draws its own title bar; the browser build, and macOS, keep the
 * native window frame. Settings → Appearance flips to the native title bar for
 * troubleshooting, screen readers, remote desktop and Windows 11 Snap Layouts.
 *
 * The preference lives in the desktop settings file, but the first frame cannot
 * wait for Tauri IPC: the chrome is mirrored in `localStorage` so a saved
 * preference is honoured before the settings round trip lands. The desktop
 * setting stays authoritative and overwrites the mirror on every load.
 */

export type ChromePlatform = "windows" | "linux" | "macos" | "unknown";

const MIRROR_KEY = "ludex.nativeTitleBar";

type Listener = () => void;
const listeners = new Set<Listener>();

function readMirror(): boolean | null {
  try {
    const raw = localStorage.getItem(MIRROR_KEY);
    if (raw === "true") return true;
    if (raw === "false") return false;
    return null;
  } catch {
    return null;
  }
}

let preference: boolean | null = readMirror();
let hydrationStarted = false;

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** `null` until the preference is known (first frame of a fresh profile). */
export function titleBarPreferenceSnapshot(): boolean | null {
  return preference;
}

/** Update the mirrored preference and re-render every chrome consumer. */
export function setTitleBarPreference(value: boolean | null): void {
  if (value === preference) return;
  preference = value;
  if (value !== null) {
    try {
      localStorage.setItem(MIRROR_KEY, String(value));
    } catch {
      /* private mode: the preference is session-only */
    }
  }
  for (const listener of listeners) listener();
}

/** Read the persisted preference once per page load. */
export function hydrateTitleBarPreference(): void {
  if (hydrationStarted || !isNative()) return;
  hydrationStarted = true;
  void native
    .getSettings()
    .then((settings) => {
      if (settings) setTitleBarPreference(Boolean(settings.nativeTitleBar));
    })
    .catch(() => {
      // Let a later mount retry: the window keeps the custom bar meanwhile.
      hydrationStarted = false;
    });
}

/**
 * Platform of the running webview. Only used to decide *before* the settings
 * round trip whether a custom title bar can exist at all; Tauri gives the
 * webview the stock engine user agent, so this is exact in practice.
 */
export function chromePlatform(): ChromePlatform {
  if (!isNative()) return "unknown";
  const userAgent = navigator.userAgent;
  if (/Windows/i.test(userAgent)) return "windows";
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "macos";
  if (/Linux|X11/i.test(userAgent)) return "linux";
  return "unknown";
}

/** Ludex can draw its own title bar on this platform. */
export function supportsCustomTitleBar(): boolean {
  const platform = chromePlatform();
  return platform === "windows" || platform === "linux";
}

/**
 * True when the app should draw its title bar and window controls: a desktop
 * build on Windows/Linux where the native title bar was not requested. Always
 * false in the browser build and on macOS.
 */
export function useCustomTitleBar(): boolean {
  const stored = useSyncExternalStore(subscribe, titleBarPreferenceSnapshot, () => null);
  useEffect(() => {
    hydrateTitleBarPreference();
  }, []);
  if (!supportsCustomTitleBar()) return false;
  return stored !== true;
}
