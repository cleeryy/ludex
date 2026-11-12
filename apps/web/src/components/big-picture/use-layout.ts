import { useCallback, useSyncExternalStore } from "react";
import { notifyPreferencesChanged } from "@/lib/preferences-events";
import { scopedStorageKey } from "@/lib/server-storage";
import {
  BIG_PICTURE_LAYOUT_KEY,
  DEFAULT_BIG_PICTURE_LAYOUT,
  readLayout,
  writeLayout,
  type BigPictureLayout,
  type LayoutStorage,
} from "./layout";

/* --------------------------------------------------------------------------
   The Big Picture layout preference, shared by the screen and Settings (#141).

   A module-level store keeps one value for the whole app: Settings writes it
   and the running Big Picture screen re-renders through
   `useSyncExternalStore`, so switching layouts never needs a reload. The value
   is mirrored into `localStorage` (scoped per server) and picked up from other
   windows through the `storage` event.
   -------------------------------------------------------------------------- */

function storage(): LayoutStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function storageKey(): string {
  try {
    return scopedStorageKey(BIG_PICTURE_LAYOUT_KEY);
  } catch {
    return BIG_PICTURE_LAYOUT_KEY;
  }
}

let current: BigPictureLayout | null = null;
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) listener();
}

/** Current preference, read from storage on first use. */
export function getBigPictureLayout(): BigPictureLayout {
  if (current === null) current = readLayout(storage(), storageKey());
  return current;
}

/** Applies and persists. Safe to call outside React (command palette, tests). */
export function setBigPictureLayout(layout: BigPictureLayout): BigPictureLayout {
  const next = writeLayout(storage(), layout, storageKey());
  if (next === current) return next;
  current = next;
  emit();
  notifyPreferencesChanged();
  return next;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);

  // `storage` fires in the other windows only, which is exactly the case this
  // listener covers: two Big Picture windows on the same server.
  const onStorage = (event: StorageEvent) => {
    if (event.key !== null && event.key !== storageKey()) return;
    const next = readLayout(storage(), storageKey());
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

export function useBigPictureLayout(): [BigPictureLayout, (layout: BigPictureLayout) => void] {
  const layout = useSyncExternalStore(
    subscribe,
    getBigPictureLayout,
    () => DEFAULT_BIG_PICTURE_LAYOUT,
  );
  const set = useCallback((next: BigPictureLayout) => {
    setBigPictureLayout(next);
  }, []);
  return [layout, set];
}

/**
 * Viewport width, for the grid geometry. Big Picture is always full-bleed, so
 * the window width is the grid width; the listener is passive and only fires
 * when the number actually changes.
 */
export function useViewportWidth(): number {
  const subscribeToResize = useCallback((notify: () => void) => {
    window.addEventListener("resize", notify);
    return () => window.removeEventListener("resize", notify);
  }, []);
  return useSyncExternalStore(
    subscribeToResize,
    () => (typeof window === "undefined" ? 1920 : window.innerWidth),
    () => 1920,
  );
}
