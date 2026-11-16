import { useSyncExternalStore } from "react";

/**
 * The `console_v2` shell flag (#267).
 *
 * Phase 1 ships the new `/console/*` shell behind a device-local flag: the
 * choice belongs to the machine in front of the television, not to the
 * account, so it lives in `localStorage` and never in the preference document
 * the server stores. Default is **off**: the route is reachable by deep link
 * and by Console mode while the shell is validated, but a desktop home stays
 * the desktop home until the device asks otherwise.
 *
 * The key is versioned (`ludex.console.shell`), so Phase 2 can promote the
 * default and delete the flag without colliding with an older experiment.
 */
export const CONSOLE_SHELL_STORAGE_KEY = "ludex.console.shell";

type Listener = () => void;
const listeners = new Set<Listener>();

function readRaw(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(CONSOLE_SHELL_STORAGE_KEY) === "1";
  } catch {
    // A browser that refuses storage (private mode, quota) keeps the default.
    return false;
  }
}

let cached = readRaw();

export function consoleShellEnabled(): boolean {
  const next = readRaw();
  if (next !== cached) cached = next;
  return next;
}

export function setConsoleShellEnabled(enabled: boolean): void {
  if (typeof window !== "undefined") {
    try {
      if (enabled) window.localStorage.setItem(CONSOLE_SHELL_STORAGE_KEY, "1");
      else window.localStorage.removeItem(CONSOLE_SHELL_STORAGE_KEY);
    } catch {
      /* storage is optional: the in-memory flag still applies this session */
    }
  }
  const next = enabled;
  if (next === cached) return;
  cached = next;
  for (const listener of listeners) listener();
}

export function subscribeConsoleShell(listener: Listener): () => void {
  listeners.add(listener);
  // Another tab (or the desktop settings page) may change it too.
  const onStorage = (event: StorageEvent) => {
    if (event.key !== null && event.key !== CONSOLE_SHELL_STORAGE_KEY) return;
    const next = readRaw();
    if (next === cached) return;
    cached = next;
    for (const entry of listeners) entry();
  };
  if (typeof window !== "undefined") window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    if (typeof window !== "undefined") window.removeEventListener("storage", onStorage);
  };
}

/** The device flag, live across components. */
export function useConsoleShellFlag(): boolean {
  return useSyncExternalStore(subscribeConsoleShell, consoleShellEnabled, () => false);
}
