import { useCallback, useSyncExternalStore } from "react";
import { scopedStorageKey } from "@/lib/server-storage";
import {
  CONSOLE_APPEARANCE_KEY,
  DEFAULT_CONSOLE_APPEARANCE,
  readConsoleAppearance,
  writeConsoleAppearance,
  type ConsoleAppearance,
} from "@/theme/console-appearance";
import type { StorageLike } from "@/theme/storage";

/* --------------------------------------------------------------------------
   Console appearance store (#229).

   One module-level value shared by the running console and the Settings
   panels, mirrored into localStorage per server so two console windows and the
   desktop app agree. Same shape as `use-layout.ts`: the write path sanitises,
   announces the change and re-renders every subscriber through
   `useSyncExternalStore` — no reload, no preview/apply split.
   -------------------------------------------------------------------------- */

function storage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function storageKey(): string {
  try {
    return scopedStorageKey(CONSOLE_APPEARANCE_KEY);
  } catch {
    return CONSOLE_APPEARANCE_KEY;
  }
}

let current: ConsoleAppearance | null = null;
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) listener();
}

export function getConsoleAppearance(): ConsoleAppearance {
  if (current === null) current = readConsoleAppearance(storage(), storageKey());
  return current;
}

/** Applies and persists. Safe outside React (command palette, tests). */
export function setConsoleAppearance(appearance: ConsoleAppearance): ConsoleAppearance {
  const next = writeConsoleAppearance(storage(), appearance, storageKey());
  if (next.preset === current?.preset && next.accent === current?.accent && next.background === current?.background && next.radius === current?.radius) {
    return next;
  }
  current = next;
  emit();
  return next;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);

  const onStorage = (event: StorageEvent) => {
    if (event.key !== null && event.key !== storageKey()) return;
    const next = readConsoleAppearance(storage(), storageKey());
    if (next === current) return;
    current = next;
    emit();
  };

  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", onStorage);
  };
}

export function useConsoleAppearance(): [ConsoleAppearance, (appearance: ConsoleAppearance) => void] {
  const appearance = useSyncExternalStore(
    subscribe,
    getConsoleAppearance,
    () => DEFAULT_CONSOLE_APPEARANCE,
  );
  const set = useCallback((next: ConsoleAppearance) => {
    setConsoleAppearance(next);
  }, []);
  return [appearance, set];
}

/** Tests reset the module store between cases. */
export function resetConsoleAppearanceStore(): void {
  current = null;
}
