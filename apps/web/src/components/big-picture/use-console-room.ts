import { useCallback, useSyncExternalStore } from "react";
import { scopedStorageKey } from "@/lib/server-storage";
import {
  CONSOLE_ROOM_KEY,
  DEFAULT_CONSOLE_ROOM,
  readConsoleRoom,
  writeConsoleRoom,
  type ConsoleRoomSettings,
} from "@/theme/console-room";
import type { StorageLike } from "@/theme/storage";

/* --------------------------------------------------------------------------
   Cabinet room store (#268).

   One module-level value shared by the running console and the Settings rows,
   mirrored into localStorage per server so two console windows and the desktop
   app agree. Same shape as `use-layout.ts`: the write path sanitises, announces
   the change and re-renders every subscriber through `useSyncExternalStore` —
   the preview *is* the applied value, so there is no Apply button to forget.
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
    return scopedStorageKey(CONSOLE_ROOM_KEY);
  } catch {
    return CONSOLE_ROOM_KEY;
  }
}

let current: ConsoleRoomSettings | null = null;
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) listener();
}

function same(a: ConsoleRoomSettings, b: ConsoleRoomSettings): boolean {
  return (
    a.room === b.room &&
    a.accent === b.accent &&
    a.wall === b.wall &&
    a.corners === b.corners &&
    a.density === b.density &&
    a.motion === b.motion &&
    a.sound === b.sound &&
    a.haptics === b.haptics
  );
}

export function getConsoleRoom(): ConsoleRoomSettings {
  if (current === null) current = readConsoleRoom(storage(), storageKey());
  return current;
}

/** Applies and persists. Safe outside React (tests, command palette). */
export function setConsoleRoom(settings: ConsoleRoomSettings): ConsoleRoomSettings {
  const next = writeConsoleRoom(storage(), settings, storageKey());
  if (current && same(next, current)) return next;
  current = next;
  emit();
  return next;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);

  const onStorage = (event: StorageEvent) => {
    if (event.key !== null && event.key !== storageKey()) return;
    const next = readConsoleRoom(storage(), storageKey());
    if (current && same(next, current)) return;
    current = next;
    emit();
  };

  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", onStorage);
  };
}

export function useConsoleRoom(): [ConsoleRoomSettings, (settings: ConsoleRoomSettings) => void] {
  const settings = useSyncExternalStore(subscribe, getConsoleRoom, () => DEFAULT_CONSOLE_ROOM);
  const set = useCallback((next: ConsoleRoomSettings) => {
    setConsoleRoom(next);
  }, []);
  return [settings, set];
}

/** Tests reset the module store between cases. */
export function resetConsoleRoomStore(): void {
  current = null;
}
