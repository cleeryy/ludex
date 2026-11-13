import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useBigPictureLayout } from "@/components/big-picture/use-layout";
import {
  readControllerPrefs,
  writeControllerPrefs,
} from "@/components/big-picture/controller";
import { api } from "@/lib/api";
import { ApiError } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import {
  PREFERENCES_CHANGED_EVENT,
  defaultPreferences,
  mergePreferences,
  preferencesEqual,
  readCachedPreferences,
  readLocalPreferences,
  readUpdatePrompt,
  readSoundPreference,
  readUpdateNotifications,
  sanitizePreferences,
  themeAvailable,
  writeCachedPreferences,
  writeSoundPreference,
  writeUpdateNotifications,
  writeUpdatePrompt,
  type AccountPreferences,
  type PreferencesPatch,
  type UpdateNotificationPreference,
  type UpdatePromptPreference,
} from "@/lib/preferences";
import { scopedStorageKey } from "@/lib/server-storage";
import { useAuth } from "@/state/auth";
import { useSettings } from "@/state/settings";
import type { LocalSettings } from "@/lib/types";
import type { ThemeSelection } from "@/theme/types";

/**
 * Account preference sync (#201).
 *
 * Boot order: the per-account cache paints first (no flash, no waiting on the
 * network), then the server document is fetched and applied. A server document
 * wins over the cache for account-scoped keys; device-scoped values are never
 * touched. On the very first sign-in the server has no document yet, so the
 * local account-scoped values are uploaded instead of being reset.
 *
 * Writes are optimistic and debounced, and carry the revision read from the
 * server. A 409 means another device wrote in between: the provider stops
 * retrying and surfaces the conflict so the user can keep this device's values
 * or take the server's. Offline writes stay queued (and cached) until the
 * browser reports it is back.
 */

export type PreferencesStatus =
  | "idle"
  | "loading"
  | "synced"
  | "pending"
  | "offline"
  | "conflict"
  | "error";

export type PreferencesSource = "server" | "cache" | "device" | "default";

export interface PreferencesValue {
  preferences: AccountPreferences;
  status: PreferencesStatus;
  /** Revision the last successful read or write returned. */
  revision: number;
  lastSyncedAt: string | null;
  /** Revision the server holds while a conflict is unresolved. */
  conflictRevision: number | null;
  source: PreferencesSource;
  /** Optimistic local change; flushed to the server after a short debounce. */
  update: (patch: PreferencesPatch) => void;
  /** Drops the account document on the server; local device settings stay. */
  resetToServer: () => Promise<void>;
  /** Uploads this device's account-scoped values as the new document. */
  useDeviceSettings: () => Promise<void>;
  /** Resolves a 409 in favour of this device or the server. */
  resolveConflict: (choice: "device" | "server") => Promise<void>;
}

const PreferencesContext = createContext<PreferencesValue | null>(null);

const DEBOUNCE_MS = 700;

function isOffline(): boolean {
  return typeof navigator !== "undefined" && navigator.onLine === false;
}

/** Mirrors the desktop `settings.json` fields owned by the account document. */
export function preferencesFromDesktop(settings: LocalSettings | null): AccountPreferences | null {
  if (!settings) return null;
  const hasMirror =
    Boolean(settings.accountTheme) ||
    Boolean(settings.accountBigPictureLayout) ||
    Boolean(settings.accountLibraryView) ||
    typeof settings.accountSound === "boolean" ||
    typeof settings.accountHaptics === "boolean" ||
    Boolean(settings.accountUpdatePrompt) ||
    Boolean(settings.accountUpdateNotifications);
  if (!hasMirror) return null;
  return sanitizePreferences({
    theme: { selection: settings.accountTheme },
    bigPicture: {
      layout: settings.accountBigPictureLayout,
      sound: settings.accountSound,
      haptics: settings.accountHaptics,
    },
    library: { view: settings.accountLibraryView },
    updates: {
      prompt: settings.accountUpdatePrompt,
      notifications: settings.accountUpdateNotifications,
    },
  });
}

export function PreferencesProvider({ children }: { children: ReactNode }) {
  const { user } = useAuth();
  const {
    theme,
    accent,
    tint,
    view,
    customThemes,
    setAppearance,
    setView,
  } = useSettings();
  const [layout, setLayout] = useBigPictureLayout();

  const [preferences, setPreferences] = useState<AccountPreferences>(() => readLocalPreferences());
  const [status, setStatus] = useState<PreferencesStatus>("idle");
  const [revision, setRevision] = useState(0);
  const [lastSyncedAt, setLastSyncedAt] = useState<string | null>(null);
  const [conflictRevision, setConflictRevision] = useState<number | null>(null);
  const [source, setSource] = useState<PreferencesSource>("device");
  const [updatePrompt, setUpdatePrompt] = useState<UpdatePromptPreference>(() => readUpdatePrompt());
  const [updateNotifications, setUpdateNotificationsState] = useState<UpdateNotificationPreference>(
    () => readUpdateNotifications(),
  );

  const documentRef = useRef(preferences);
  const revisionRef = useRef(0);
  const pendingRef = useRef(false);
  const hydratedRef = useRef(false);
  const inFlightRef = useRef(false);
  const conflictRef = useRef<number | null>(null);
  const userIdRef = useRef<number | null>(null);
  const signedInRef = useRef(false);
  // True while a server document is being written into the settings UI; the
  // storage events fired by those writes must not queue a write back.
  const applyingRef = useRef(false);
  const timerRef = useRef<number | null>(null);
  // `schedule` lives in a ref so `flush`, `settle` and `queue` can call it
  // without a circular hook dependency.
  const scheduleRef = useRef<(delay?: number) => void>(() => {});

  // The setters below change identity when the theme changes; effects keep
  // the latest pair without re-running their boot logic.
  const applyRef = useRef<(document: AccountPreferences) => void>(() => {});

  const applyDocument = useCallback(
    (document: AccountPreferences) => {
      applyingRef.current = true;
      const next = { ...document };
      // A custom theme only exists where it was imported; never leave the user
      // with a selection that renders as the default palette.
      if (!themeAvailable(next.theme.selection, customThemes)) {
        next.theme = { ...next.theme, selection: theme };
      }
      setAppearance({ ...next.theme, selection: next.theme.selection as ThemeSelection });
      setView(next.library.view);
      setLayout(next.bigPicture.layout);
      const controllerKey = scopedStorageKey("ludex.bigPicture.controller");
      const controller = readControllerPrefs(localStorage, controllerKey);
      if (controller.haptics !== next.bigPicture.haptics) {
        writeControllerPrefs(localStorage, { ...controller, haptics: next.bigPicture.haptics }, controllerKey);
      }
      writeSoundPreference(next.bigPicture.sound);
      writeUpdatePrompt(next.updates.prompt);
      setUpdatePrompt(next.updates.prompt);
      writeUpdateNotifications(next.updates.notifications);
      setUpdateNotificationsState(next.updates.notifications);
      queueMicrotask(() => {
        applyingRef.current = false;
      });
    },
    [customThemes, setAppearance, setLayout, setView, theme],
  );
  applyRef.current = applyDocument;

  const mirrorDesktop = useCallback((document: AccountPreferences) => {
    if (!isNative()) return;
    void native
      .applyAccountPreferences({
        theme: { selection: document.theme.selection, accent: document.theme.accent, tint: document.theme.tint },
        bigPicture: document.bigPicture,
        library: document.library,
        updates: document.updates,
      })
      .catch(() => {
        /* the mirror is best effort; the next sync retries it */
      });
  }, []);

  const cacheDocument = useCallback((document: AccountPreferences, revision: number, pending: boolean) => {
    const userId = userIdRef.current;
    if (userId === null) return;
    writeCachedPreferences(userId, { document, revision, pending });
  }, []);

  /** Marks the document as synced only when nothing newer is queued. */
  const settle = useCallback(
    (document: AccountPreferences, savedRevision: number, savedAt: string | null, nextSource: PreferencesSource) => {
      revisionRef.current = savedRevision;
      setRevision(savedRevision);
      setLastSyncedAt(savedAt);
      setSource(nextSource);
      conflictRef.current = null;
      setConflictRevision(null);
      if (preferencesEqual(documentRef.current, document)) {
        pendingRef.current = false;
        setStatus("synced");
        cacheDocument(document, savedRevision, false);
      } else {
        pendingRef.current = true;
        setStatus("pending");
        cacheDocument(documentRef.current, savedRevision, true);
        scheduleRef.current();
      }
    },
    [cacheDocument],
  );

  const flush = useCallback(async () => {
    const userId = userIdRef.current;
    if (userId === null || !pendingRef.current || inFlightRef.current) return;
    if (isOffline()) {
      setStatus("offline");
      return;
    }
    inFlightRef.current = true;
    const document = documentRef.current;
    const baseRevision = revisionRef.current;
    try {
      const saved = await api.putAccountPreferences(document, baseRevision);
      if (userIdRef.current !== userId) return;
      settle(document, saved.revision, saved.updatedAt, "server");
      mirrorDesktop(document);
    } catch (cause) {
      if (userIdRef.current !== userId) return;
      if (cause instanceof ApiError && cause.status === 409) {
        // Another device wrote first. Keep the local values pending and let
        // the user choose; retrying automatically would overwrite them.
        const remote = await api.accountPreferences().catch(() => null);
        conflictRef.current = remote?.revision ?? null;
        setConflictRevision(conflictRef.current);
        setStatus("conflict");
        return;
      }
      setStatus(isOffline() || (cause instanceof ApiError && cause.status === 0) ? "offline" : "error");
      // Keep the pending flag: the next online/focus/visibility event retries.
      scheduleRef.current();
    } finally {
      inFlightRef.current = false;
    }
  }, [cacheDocument, mirrorDesktop, settle]);

  const schedule = useCallback(
    (delay = DEBOUNCE_MS) => {
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      timerRef.current = window.setTimeout(() => {
        timerRef.current = null;
        void flush();
      }, delay);
    },
    [flush],
  );
  scheduleRef.current = schedule;

  const queue = useCallback(
    (document: AccountPreferences) => {
      if (preferencesEqual(document, documentRef.current) && !pendingRef.current) return;
      documentRef.current = document;
      setPreferences(document);
      pendingRef.current = true;
      setStatus((current) => (current === "conflict" ? current : "pending"));
      cacheDocument(document, revisionRef.current, true);
      scheduleRef.current();
    },
    [cacheDocument],
  );

  const update = useCallback(
    (patch: PreferencesPatch) => {
      const next = mergePreferences(documentRef.current, patch);
      queue(next);
    },
    [queue],
  );

  /* ---------------------------------- boot --------------------------------- */

  useEffect(() => {
    const userId = user?.id ?? null;
    userIdRef.current = userId;
    hydratedRef.current = false;
    pendingRef.current = false;
    inFlightRef.current = false;
    conflictRef.current = null;
    setConflictRevision(null);
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
      timerRef.current = null;
    }

    if (userId === null) {
      // Signed out: stop syncing and keep the cached document for this account
      // so the next sign-in does not lose it. The account-scoped local values
      // go back to defaults, so the next account on this device cannot inherit
      // the previous one's theme or layout.
      if (signedInRef.current) {
        const defaults = defaultPreferences();
        documentRef.current = defaults;
        setPreferences(defaults);
        applyRef.current(defaults);
        setSource("default");
      }
      signedInRef.current = false;
      setStatus("idle");
      setRevision(0);
      setLastSyncedAt(null);
      return;
    }
    signedInRef.current = true;

    let cancelled = false;
    const cached = readCachedPreferences(userId);
    if (cached) {
      documentRef.current = cached.document;
      revisionRef.current = cached.revision;
      pendingRef.current = cached.pending;
      setPreferences(cached.document);
      setRevision(cached.revision);
      setSource("cache");
    }

    setStatus("loading");
    void (async () => {
      // The desktop mirrors the document into `settings.json`; it is the only
      // cache when the webview profile was reset but the app data survived.
      if (!cached && isNative()) {
        try {
          const mirror = preferencesFromDesktop(await native.getSettings());
          if (mirror && !cancelled) {
            documentRef.current = mirror;
            setPreferences(mirror);
            setSource("device");
          }
        } catch {
          /* the server read below is authoritative */
        }
      }
      try {
        const remote = await api.accountPreferences();
        if (cancelled || userIdRef.current !== userId) return;
        if (remote.document === null) {
          // First sign-in: seed the account with this device's values instead
          // of resetting the user's theme and layout.
          const local = readLocalPreferences();
          const seeded = await api.putAccountPreferences(local, 0);
          if (cancelled || userIdRef.current !== userId) return;
          documentRef.current = local;
          revisionRef.current = seeded.revision;
          pendingRef.current = false;
          setPreferences(local);
          setRevision(seeded.revision);
          setLastSyncedAt(seeded.updatedAt);
          setStatus("synced");
          setSource("server");
          cacheDocument(local, seeded.revision, false);
          mirrorDesktop(local);
        } else {
          const serverDocument = sanitizePreferences(remote.document);
          // Capture a queued local change before the server document rewrites
          // the same storage keys; it wins and is flushed with the new revision.
          const pendingLocal = cached?.pending === true ? readLocalPreferences() : null;
          documentRef.current = serverDocument;
          revisionRef.current = remote.revision;
          pendingRef.current = pendingLocal !== null;
          setRevision(remote.revision);
          setLastSyncedAt(remote.updatedAt);
          setSource("server");
          cacheDocument(serverDocument, remote.revision, pendingRef.current);
          applyRef.current(serverDocument);
          if (pendingLocal) {
            // The queued local values are what the user last chose; put them
            // back on screen and flush them with the fresh revision.
            documentRef.current = pendingLocal;
            setPreferences(pendingLocal);
            applyRef.current(pendingLocal);
            setStatus("pending");
            scheduleRef.current();
          } else {
            setStatus("synced");
          }
        }
        hydratedRef.current = true;
      } catch (cause) {
        if (cancelled || userIdRef.current !== userId) return;
        hydratedRef.current = true;
        if (cause instanceof ApiError && cause.status === 409) {
          // Another device created the document between the read and the seed.
          const remote = await api.accountPreferences().catch(() => null);
          conflictRef.current = remote?.revision ?? null;
          setConflictRevision(conflictRef.current);
          setStatus("conflict");
          return;
        }
        setStatus(isOffline() || (cause instanceof ApiError && cause.status === 0) ? "offline" : "error");
        if (pendingRef.current) scheduleRef.current(1500);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [user?.id, cacheDocument, mirrorDesktop]);

  /* ------------------------- local changes -> queue ------------------------ */

  useEffect(() => {
    const onChange = () => {
      if (!hydratedRef.current || applyingRef.current || userIdRef.current === null) return;
      const local = readLocalPreferences();
      if (preferencesEqual(local, documentRef.current)) return;
      queue(local);
    };
    window.addEventListener(PREFERENCES_CHANGED_EVENT, onChange);
    return () => window.removeEventListener(PREFERENCES_CHANGED_EVENT, onChange);
  }, [queue]);

  /* ------------------------------ reconnection ----------------------------- */

  useEffect(() => {
    const onOnline = () => {
      if (pendingRef.current && conflictRef.current === null) void flush();
    };
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      if (userIdRef.current === null) return;
      if (pendingRef.current) {
        if (conflictRef.current === null) void flush();
        return;
      }
      // Poll on focus: another device may have written while this one was idle.
      void (async () => {
        const userId = userIdRef.current;
        try {
          const remote = await api.accountPreferences();
          if (userIdRef.current !== userId || pendingRef.current || remote.document === null) return;
          if (remote.revision <= revisionRef.current) return;
          const document = sanitizePreferences(remote.document);
          documentRef.current = document;
          revisionRef.current = remote.revision;
          setRevision(remote.revision);
          setLastSyncedAt(remote.updatedAt);
          setStatus("synced");
          setSource("server");
          cacheDocument(document, remote.revision, false);
          applyRef.current(document);
        } catch {
          /* the next visibility change retries */
        }
      })();
    };
    window.addEventListener("online", onOnline);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.removeEventListener("online", onOnline);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [cacheDocument, flush]);

  /* -------------------------------- actions -------------------------------- */

  const resetToServer = useCallback(async () => {
    const userId = userIdRef.current;
    if (userId === null) return;
    try {
      const cleared = await api.resetAccountPreferences();
      if (userIdRef.current !== userId) return;
      const defaults = defaultPreferences();
      documentRef.current = defaults;
      revisionRef.current = cleared.revision;
      pendingRef.current = false;
      conflictRef.current = null;
      setConflictRevision(null);
      setPreferences(defaults);
      setRevision(cleared.revision);
      setLastSyncedAt(cleared.updatedAt);
      setStatus("synced");
      setSource("default");
      cacheDocument(defaults, cleared.revision, false);
      applyRef.current(defaults);
      mirrorDesktop(defaults);
    } catch (cause) {
      setStatus(isOffline() || (cause instanceof ApiError && cause.status === 0) ? "offline" : "error");
    }
  }, [cacheDocument, mirrorDesktop]);

  const useDeviceSettings = useCallback(async () => {
    const userId = userIdRef.current;
    if (userId === null) return;
    const local = readLocalPreferences();
    documentRef.current = local;
    setPreferences(local);
    pendingRef.current = true;
    setStatus("pending");
    cacheDocument(local, revisionRef.current, true);
    try {
      const saved = await api.putAccountPreferences(local, "*");
      if (userIdRef.current !== userId) return;
      settle(local, saved.revision, saved.updatedAt, "device");
      mirrorDesktop(local);
    } catch (cause) {
      setStatus(isOffline() || (cause instanceof ApiError && cause.status === 0) ? "offline" : "error");
    }
  }, [cacheDocument, mirrorDesktop, settle]);

  const resolveConflict = useCallback(
    async (choice: "device" | "server") => {
      const userId = userIdRef.current;
      if (userId === null) return;
      if (choice === "server") {
        try {
          const remote = await api.accountPreferences();
          if (userIdRef.current !== userId) return;
          const document = sanitizePreferences(remote.document);
          documentRef.current = document;
          revisionRef.current = remote.revision;
          pendingRef.current = false;
          conflictRef.current = null;
          setConflictRevision(null);
          setPreferences(document);
          setRevision(remote.revision);
          setLastSyncedAt(remote.updatedAt);
          setStatus("synced");
          setSource("server");
          cacheDocument(document, remote.revision, false);
          applyRef.current(document);
        } catch {
          setStatus("error");
        }
        return;
      }
      let base = conflictRef.current;
      if (base === null) {
        const remote = await api.accountPreferences().catch(() => null);
        base = remote?.revision ?? null;
      }
      if (base === null) {
        setStatus("error");
        return;
      }
      try {
        const document = documentRef.current;
        const saved = await api.putAccountPreferences(document, base);
        if (userIdRef.current !== userId) return;
        settle(document, saved.revision, saved.updatedAt, "device");
        mirrorDesktop(document);
      } catch {
        setStatus("error");
      }
    },
    [cacheDocument, mirrorDesktop, settle],
  );

  /* ------------------------ provider values to document ------------------- */

  useEffect(() => {
    // The theme setters and the layout store are the source of truth for the
    // settings UI; mirror them into the document when the user changes one.
    if (!hydratedRef.current || applyingRef.current || userIdRef.current === null) return;
    const current = documentRef.current;
    const next = sanitizePreferences({
      theme: { selection: theme, accent, tint },
      bigPicture: {
        layout,
        sound: readSoundPreference(),
        haptics: readControllerPrefs(localStorage, scopedStorageKey("ludex.bigPicture.controller")).haptics,
      },
      library: { view },
      updates: { prompt: updatePrompt, notifications: updateNotifications },
    });
    if (preferencesEqual(next, current)) return;
    // The bus covers writes made outside this effect (Big Picture, palette);
    // this effect catches React state that changed without a storage write.
    queue(next);
  }, [theme, accent, tint, view, layout, updatePrompt, updateNotifications, queue]);

  const value: PreferencesValue = {
    preferences,
    status,
    revision,
    lastSyncedAt,
    conflictRevision,
    source,
    update,
    resetToServer,
    useDeviceSettings,
    resolveConflict,
  };

  return <PreferencesContext.Provider value={value}>{children}</PreferencesContext.Provider>;
}

export function usePreferences(): PreferencesValue {
  const value = useContext(PreferencesContext);
  if (!value) throw new Error("usePreferences must be used inside PreferencesProvider");
  return value;
}

/** Account-level update reminder choice, applied by the update notifier. */
export function useUpdatePrompt(): UpdatePromptPreference {
  return usePreferences().preferences.updates.prompt;
}

/** Account-level update notification channels, applied by the update notifier (#213). */
export function useUpdateNotifications(): UpdateNotificationPreference {
  return usePreferences().preferences.updates.notifications;
}
