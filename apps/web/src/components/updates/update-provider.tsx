import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import type { UpdateInstalled, UpdateProgress, UpdateStatus } from "@/lib/types";
import {
  clearRemindLater,
  readRemindAfter,
  setRemindLater,
  shouldNotify,
  REMIND_LATER_MS,
} from "@/lib/updates";
import { t } from "@/lib/i18n";
import { useToast } from "@/state/toasts";

/**
 * One updater state for the whole app (#163).
 *
 * The update center, the Settings status line and the launch prompt all read
 * the same status, so a check started in one place is immediately visible in
 * the others. Only the desktop build has an updater: in a browser every value
 * stays empty and the pages explain that updates are desktop-only.
 */

interface UpdaterValue {
  status: UpdateStatus | null;
  /** True until the first status (or the browser fallback) is known. */
  loading: boolean;
  checking: boolean;
  installing: boolean;
  progress: UpdateProgress | null;
  installed: UpdateInstalled | null;
  /** Last check or action failure, already human-readable. */
  error: string | null;
  /** End of an active "remind me later" window, if any. */
  remindAfter: number | null;
  check: (manual?: boolean) => Promise<UpdateStatus | null>;
  install: () => Promise<void>;
  restart: () => void;
  setChannel: (channel: UpdateStatus["channel"]) => Promise<void>;
  skip: (version: string | null) => Promise<void>;
  remindLater: () => void;
  clearError: () => void;
}

const UpdaterContext = createContext<UpdaterValue | null>(null);

export function UpdateProvider({ children }: { children: ReactNode }) {
  const { push } = useToast();
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [loading, setLoading] = useState(isNative());
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [installed, setInstalled] = useState<UpdateInstalled | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [remindAfter, setRemindAfterState] = useState<number | null>(() => readRemindAfter());

  useEffect(() => {
    if (!isNative()) {
      setLoading(false);
      return;
    }
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    void native
      .updateStatus()
      .then((next) => {
        if (disposed) return;
        // Keep a status already received from a startup event: the command may
        // resolve after the check and never carries `available`.
        if (next) setStatus((current) => current ?? next);
        setLoading(false);
      })
      .catch(() => {
        if (!disposed) setLoading(false);
      });

    void (async () => {
      const offAvailable = await native.onUpdateAvailable((next) => {
        setStatus((current) => (current ? { ...current, ...next } : next));
      });
      if (disposed) offAvailable();
      else unlisteners.push(offAvailable);

      const offProgress = await native.onUpdateProgress((next) => {
        setProgress(next);
      });
      if (disposed) offProgress();
      else unlisteners.push(offProgress);

      const offInstalled = await native.onUpdateInstalled((payload) => {
        setInstalled(payload);
        setProgress(null);
      });
      if (disposed) offInstalled();
      else unlisteners.push(offInstalled);
    })();

    return () => {
      disposed = true;
      for (const off of unlisteners) off();
    };
  }, []);

  const check = useCallback(
    async (manual = true): Promise<UpdateStatus | null> => {
      if (!isNative()) return null;
      setChecking(true);
      setError(null);
      try {
        const next = await native.checkForUpdates(manual);
        setStatus(next);
        return next;
      } catch (cause) {
        setError(errorMessage(cause));
        return null;
      } finally {
        setChecking(false);
      }
    },
    [],
  );

  const install = useCallback(async () => {
    if (!isNative()) return;
    setInstalling(true);
    setProgress(null);
    setInstalled(null);
    setError(null);
    try {
      const next = await native.installUpdate();
      // The command keeps the installed version in `available` for backwards
      // compatibility; the center shows it from `installed` instead, so clear
      // it and never re-offer the same install.
      setStatus({ ...next, available: null, notify: false });
      const version = next.available?.version ?? "";
      setInstalled((current) =>
        current ?? (version ? { version, restarted: /automatically/i.test(next.message ?? "") } : null),
      );
      push({
        title: t("Update installed"),
        description: next.message ?? undefined,
        tone: "success",
      });
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not install the update"), description: message, tone: "danger" });
    } finally {
      setInstalling(false);
      setProgress(null);
    }
  }, [push]);

  const restart = useCallback(() => {
    void native.restartApp().catch((cause: Error) => {
      push({ title: t("Could not restart Ludex"), description: errorMessage(cause), tone: "danger" });
    });
  }, [push]);

  const setChannel = useCallback(
    async (channel: UpdateStatus["channel"]) => {
      if (!isNative()) return;
      setChecking(true);
      setError(null);
      try {
        const next = await native.setUpdateChannel(channel);
        setStatus(next);
        push({
          title: t("Update channel changed"),
          description:
            channel === "early"
              ? t("New checks follow the edge prerelease.")
              : t("New checks follow the latest stable release."),
        });
      } catch (cause) {
        const message = errorMessage(cause);
        setError(message);
        push({ title: t("Could not change the update channel"), description: message, tone: "danger" });
        const fresh = await native.updateStatus();
        if (fresh) setStatus(fresh);
      } finally {
        setChecking(false);
      }
    },
    [push],
  );

  const skip = useCallback(
    async (version: string | null) => {
      if (!isNative()) return;
      try {
        const next = await native.setUpdateSkip(version);
        // `set_update_skip` answers without a network check, so keep the
        // available version already on screen and recompute the prompt flag.
        // A skipped update stays installable from the center.
        setStatus((current) => {
          const available = current?.available ?? null;
          return {
            ...next,
            available,
            notify: available ? shouldNotify(available.version, next.skippedVersion) : false,
          };
        });
        // A skip replaces the reminder window: one silence mechanism is enough.
        clearRemindLater();
        setRemindAfterState(null);
        push({
          title: version
            ? t("Ludex {version} will not prompt again", { version })
            : t("Update prompts are back on"),
        });
      } catch (cause) {
        push({
          title: t("Could not save the update preference"),
          description: errorMessage(cause),
          tone: "danger",
        });
      }
    },
    [push],
  );

  const remindLater = useCallback(() => {
    const until = setRemindLater(Date.now(), REMIND_LATER_MS);
    setRemindAfterState(until);
    push({ title: t("We will remind you next time") });
  }, [push]);

  const clearError = useCallback(() => setError(null), []);

  const value = useMemo<UpdaterValue>(
    () => ({
      status,
      loading,
      checking,
      installing,
      progress,
      installed,
      error,
      remindAfter,
      check,
      install,
      restart,
      setChannel,
      skip,
      remindLater,
      clearError,
    }),
    [
      status,
      loading,
      checking,
      installing,
      progress,
      installed,
      error,
      remindAfter,
      check,
      install,
      restart,
      setChannel,
      skip,
      remindLater,
      clearError,
    ],
  );

  return <UpdaterContext.Provider value={value}>{children}</UpdaterContext.Provider>;
}

export function useUpdater(): UpdaterValue {
  const value = useContext(UpdaterContext);
  if (!value) throw new Error("useUpdater must be used inside UpdateProvider");
  return value;
}
