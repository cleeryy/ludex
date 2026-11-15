import {
  identityLabel,
  isGlyphTheme,
  isPadFamily,
  padFingerprint,
  type GlyphTheme,
  type PadIdentity,
} from "@/lib/controller-identity";

/* --------------------------------------------------------------------------
   The device map (#230).

   A profile is remembered per controller fingerprint, device-local and
   unscoped: `ludex.controllers.v1` belongs to this browser/device, not to a
   server or an account. The document is deliberately small and separate from
   `ControllerProfile`, which stays the shared wire document the server
   validates:

     { version: 1, pads: { [fingerprint]: entry }, last: fingerprint | null }

   An entry records where the profile comes from (automatic, the account
   profile, a per-game override or a built-in preset), the glyph theme and the
   Nintendo-style A/B swap. Local custom profiles would need a server
   migration, so they are not part of this document yet.
   -------------------------------------------------------------------------- */

export const CONTROLLER_DEVICES_KEY = "ludex.controllers.v1";

/** Where the profile for a pad comes from, in the same id vocabulary as the
 * Console profile overlay (`auto`, `account`, `game:<id>`, `preset:<id>`). */
export type ControllerDeviceSource =
  | { kind: "auto" }
  | { kind: "account" }
  | { kind: "game"; gameId: number }
  | { kind: "preset"; id: string };

export interface ControllerDeviceEntry {
  /** Display name; defaults to the pad family label on first assignment. */
  label: string;
  identity: PadIdentity;
  source: ControllerDeviceSource;
  glyphTheme: GlyphTheme;
  swapAB: boolean;
  /** Epoch milliseconds, informational only. */
  updatedAt: number;
}

export interface ControllerDevices {
  version: 1;
  pads: Record<string, ControllerDeviceEntry>;
  /** Fingerprint of the pad that drove the console last, for ordering. */
  last: string | null;
}

export const EMPTY_CONTROLLER_DEVICES: ControllerDevices = { version: 1, pads: {}, last: null };

export type ControllerDevicePatch = Partial<Pick<ControllerDeviceEntry, "label" | "source" | "glyphTheme" | "swapAB">>;

export type ControllerStorage = Pick<Storage, "getItem" | "setItem">;

/* ------------------------------- source ids ------------------------------- */

export const AUTO_SOURCE_ID = "auto";

export function sourceId(source: ControllerDeviceSource): string {
  switch (source.kind) {
    case "auto":
      return AUTO_SOURCE_ID;
    case "account":
      return "account";
    case "game":
      return `game:${source.gameId}`;
    case "preset":
      return `preset:${source.id}`;
  }
}

/** Parses the overlay/manager id vocabulary; unknown ids fall back to auto. */
export function sourceFromId(id: string | null | undefined): ControllerDeviceSource {
  const value = typeof id === "string" ? id.trim() : "";
  if (!value || value === AUTO_SOURCE_ID) return { kind: "auto" };
  if (value === "account") return { kind: "account" };
  const game = value.match(/^game:(\d+)$/);
  if (game) {
    const gameId = Number(game[1]);
    if (Number.isSafeInteger(gameId) && gameId > 0) return { kind: "game", gameId };
    return { kind: "auto" };
  }
  const preset = value.match(/^preset:(.+)$/);
  if (preset && preset[1].trim()) return { kind: "preset", id: preset[1].trim() };
  return { kind: "auto" };
}

/* ------------------------------- sanitising ------------------------------- */

function sanitizeSource(value: unknown): ControllerDeviceSource {
  if (!value || typeof value !== "object") return { kind: "auto" };
  const record = value as Record<string, unknown>;
  switch (record.kind) {
    case "account":
      return { kind: "account" };
    case "game": {
      const gameId = Number(record.gameId);
      return Number.isSafeInteger(gameId) && gameId > 0 ? { kind: "game", gameId } : { kind: "auto" };
    }
    case "preset": {
      const id = typeof record.id === "string" ? record.id.trim() : "";
      return id ? { kind: "preset", id: id.slice(0, 64) } : { kind: "auto" };
    }
    default:
      return { kind: "auto" };
  }
}

export function sanitizeControllerDeviceEntry(
  value: unknown,
  fingerprint?: string,
): ControllerDeviceEntry | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  const rawIdentity = record.identity;
  const identityRecord =
    rawIdentity && typeof rawIdentity === "object" ? (rawIdentity as Record<string, unknown>) : {};
  const rawId = typeof identityRecord.rawId === "string" ? identityRecord.rawId.trim() : "";
  if (!rawId) return null;
  const family = isPadFamily(identityRecord.family) ? identityRecord.family : undefined;
  // Recompute from the raw id: stored vendor/product/family are display sugar,
  // never the source of truth.
  const identity = padFingerprint(rawId, family, {
    mapping: identityRecord.standard === true ? "standard" : undefined,
  });
  if (fingerprint && identity.fingerprint !== fingerprint) return null;
  const label = typeof record.label === "string" && record.label.trim()
    ? record.label.trim().slice(0, 64)
    : identityLabel(identity);
  const updatedAt = Number(record.updatedAt);
  return {
    label,
    identity,
    source: sanitizeSource(record.source),
    glyphTheme: isGlyphTheme(record.glyphTheme) ? record.glyphTheme : "auto",
    swapAB: record.swapAB === true,
    updatedAt: Number.isFinite(updatedAt) && updatedAt > 0 ? updatedAt : 0,
  };
}

export function sanitizeControllerDevices(value: unknown): ControllerDevices {
  if (!value || typeof value !== "object") return { ...EMPTY_CONTROLLER_DEVICES, pads: {} };
  const record = value as Record<string, unknown>;
  const rawPads = record.pads;
  const pads: Record<string, ControllerDeviceEntry> = {};
  if (rawPads && typeof rawPads === "object") {
    for (const [key, entry] of Object.entries(rawPads as Record<string, unknown>)) {
      const sanitized = sanitizeControllerDeviceEntry(entry, key);
      if (sanitized) pads[sanitized.identity.fingerprint] = sanitized;
    }
  }
  const last = typeof record.last === "string" && record.last.trim() ? record.last.trim() : null;
  return { version: 1, pads, last };
}

export function readControllerDevices(
  storage: ControllerStorage | null | undefined,
  key: string = CONTROLLER_DEVICES_KEY,
): ControllerDevices {
  if (!storage) return { ...EMPTY_CONTROLLER_DEVICES, pads: {} };
  try {
    const raw = storage.getItem(key);
    if (!raw) return { ...EMPTY_CONTROLLER_DEVICES, pads: {} };
    return sanitizeControllerDevices(JSON.parse(raw));
  } catch {
    // Private mode, a blocked origin or hand-edited JSON: start empty.
    return { ...EMPTY_CONTROLLER_DEVICES, pads: {} };
  }
}

function writeControllerDevices(
  storage: ControllerStorage | null | undefined,
  devices: ControllerDevices,
  key: string,
): ControllerDevices {
  const next = sanitizeControllerDevices(devices);
  try {
    storage?.setItem(key, JSON.stringify(next));
  } catch {
    /* the map is session-only when storage is unavailable */
  }
  return next;
}

/* --------------------------------- store ---------------------------------- */

export interface ControllerDevicesStore {
  get(): ControllerDevices;
  set(next: ControllerDevices): ControllerDevices;
  /** Creates or updates the entry for a pad, keeping a stable label. */
  assign(identity: PadIdentity, patch: ControllerDevicePatch): ControllerDevices;
  update(fingerprint: string, patch: ControllerDevicePatch): ControllerDevices;
  remove(fingerprint: string): ControllerDevices;
  /** Records the last pad that drove the console; no-op when unchanged. */
  touch(fingerprint: string): ControllerDevices;
  subscribe(listener: () => void): () => void;
}

export interface ControllerDevicesStoreOptions {
  storage?: ControllerStorage | null;
  key?: string;
  /** Mirrors another window writing the same key. Off in tests. */
  listen?: boolean;
  /** Clock override for deterministic tests. */
  now?: () => number;
}

function defaultStorage(): ControllerStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function createControllerDevicesStore(
  options: ControllerDevicesStoreOptions = {},
): ControllerDevicesStore {
  const storage = options.storage === undefined ? defaultStorage() : options.storage;
  const key = options.key ?? CONTROLLER_DEVICES_KEY;
  const listen = options.listen ?? true;
  const now = options.now ?? (() => Date.now());
  let current: ControllerDevices | null = null;
  const listeners = new Set<() => void>();

  const emit = () => {
    for (const listener of listeners) listener();
  };

  const get = (): ControllerDevices => {
    if (current === null) current = readControllerDevices(storage, key);
    return current;
  };

  const commit = (next: ControllerDevices): ControllerDevices => {
    current = writeControllerDevices(storage, next, key);
    emit();
    return current;
  };

  const upsert = (identity: PadIdentity, patch: ControllerDevicePatch, base: ControllerDevices): ControllerDevices => {
    const previous = base.pads[identity.fingerprint];
    const entry: ControllerDeviceEntry = {
      label: patch.label?.trim() ? patch.label.trim().slice(0, 64) : previous?.label ?? identityLabel(identity),
      identity,
      source: patch.source ?? previous?.source ?? { kind: "auto" },
      glyphTheme: patch.glyphTheme ?? previous?.glyphTheme ?? "auto",
      swapAB: patch.swapAB ?? previous?.swapAB ?? false,
      updatedAt: now(),
    };
    return { ...base, pads: { ...base.pads, [identity.fingerprint]: entry } };
  };

  return {
    get,

    set(next) {
      return commit(next);
    },

    assign(identity, patch) {
      // `last` is input history, not edit history: only the console touches it.
      return commit(upsert(identity, patch, get()));
    },

    update(fingerprint, patch) {
      const base = get();
      const previous = base.pads[fingerprint];
      if (!previous) return base;
      return commit(upsert(previous.identity, patch, base));
    },

    remove(fingerprint) {
      const base = get();
      if (!base.pads[fingerprint]) return base;
      const pads = { ...base.pads };
      delete pads[fingerprint];
      return commit({ ...base, pads, last: base.last === fingerprint ? null : base.last });
    },

    touch(fingerprint) {
      const base = get();
      if (!fingerprint || base.last === fingerprint) return base;
      return commit({ ...base, last: fingerprint });
    },

    subscribe(listener) {
      listeners.add(listener);
      if (!listen || typeof window === "undefined") {
        return () => {
          listeners.delete(listener);
        };
      }
      const onStorage = (event: StorageEvent) => {
        if (event.key !== null && event.key !== key) return;
        const next = readControllerDevices(storage, key);
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

/** The singleton the app reads; unscoped on purpose (see the header). */
export const controllerDevices = createControllerDevicesStore();
