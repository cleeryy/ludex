import { useCallback, useSyncExternalStore } from "react";
import { scopedStorageKey } from "@/lib/server-storage";
import {
  BIG_PICTURE_CONTROLLER_KEY,
  defaultControllerPrefs,
  readControllerPrefs,
  sanitizeControllerPrefs,
  writeControllerPrefs,
  type BigPictureControllerPrefs,
  type ControllerStorage,
} from "./controller";

/* --------------------------------------------------------------------------
   The Big Picture preferences, shared by the screen and Settings (#198).

   One module store for the profile, haptics and sound: the console screen and
   the desktop Appearance panel read the same value, so muting in either place
   applies to a running Big Picture without a reload. The value stays
   device-local and per server (`scopedStorageKey`), exactly like the layout.

   `createPrefsStore` takes its storage so the persistence rules are testable
   without a browser; the exported singleton uses `localStorage` and mirrors
   changes from another window through the `storage` event.
   -------------------------------------------------------------------------- */

export interface PrefsStore {
  get(): BigPictureControllerPrefs;
  set(next: BigPictureControllerPrefs): BigPictureControllerPrefs;
  update(patch: Partial<BigPictureControllerPrefs>): BigPictureControllerPrefs;
  subscribe(listener: () => void): () => void;
}

export interface PrefsStoreOptions {
  storage?: ControllerStorage | null;
  /** A fixed key (tests) or a resolver, so a server switch re-scopes the key. */
  key?: string | (() => string);
  /** Listen for another window writing the same key. Off in tests. */
  listen?: boolean;
}

function defaultStorage(): ControllerStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function defaultKey(): string {
  try {
    return scopedStorageKey(BIG_PICTURE_CONTROLLER_KEY);
  } catch {
    return BIG_PICTURE_CONTROLLER_KEY;
  }
}

export function createPrefsStore(options: PrefsStoreOptions = {}): PrefsStore {
  const storage = options.storage === undefined ? defaultStorage() : options.storage;
  const fixedKey = typeof options.key === "function" ? null : options.key;
  const resolveKey = typeof options.key === "function" ? options.key : () => fixedKey ?? defaultKey();
  const listen = options.listen ?? true;
  let current: BigPictureControllerPrefs | null = null;
  const listeners = new Set<() => void>();

  const emit = () => {
    for (const listener of listeners) listener();
  };

  const get = (): BigPictureControllerPrefs => {
    if (current === null) current = readControllerPrefs(storage, resolveKey());
    return current;
  };

  return {
    get,

    set(next) {
      current = writeControllerPrefs(storage, next, resolveKey());
      emit();
      return current;
    },

    update(patch) {
      const merged = { ...get(), ...patch };
      // A partial `sound` patch would drop the other field; callers pass the
      // whole object, and sanitize fills anything missing.
      current = writeControllerPrefs(storage, sanitizeControllerPrefs(merged), resolveKey());
      emit();
      return current;
    },

    subscribe(listener) {
      listeners.add(listener);
      if (!listen || typeof window === "undefined") {
        return () => {
          listeners.delete(listener);
        };
      }

      const onStorage = (event: StorageEvent) => {
        if (event.key !== null && event.key !== resolveKey()) return;
        const next = readControllerPrefs(storage, resolveKey());
        if (JSON.stringify(next) === JSON.stringify(current)) return;
        current = next;
        emit();
      };
      window.addEventListener("storage", onStorage);
      return () => {
        listeners.delete(listener);
        window.removeEventListener("storage", onStorage);
      };
    },
  };
}

export const bigPicturePrefs = createPrefsStore();

/** Stable identity for `useSyncExternalStore`'s server snapshot. */
const SERVER_SNAPSHOT: BigPictureControllerPrefs = defaultControllerPrefs();

/** [value, patch] — the patch is shallow, so pass a whole `sound` object. */
export function useBigPicturePrefs(): readonly [
  BigPictureControllerPrefs,
  (patch: Partial<BigPictureControllerPrefs>) => void,
] {
  const prefs = useSyncExternalStore(
    bigPicturePrefs.subscribe,
    bigPicturePrefs.get,
    () => SERVER_SNAPSHOT,
  );
  const update = useCallback((patch: Partial<BigPictureControllerPrefs>) => {
    bigPicturePrefs.update(patch);
  }, []);
  return [prefs, update] as const;
}
