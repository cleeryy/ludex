import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { t } from "@/lib/i18n";
import {
  UploadController,
  type SelectResult,
  type StartResult,
  type UploadSnapshot,
  type UploadStorage,
} from "@/lib/upload";
import { useAuth, useIsEditor } from "@/state/auth";
import { useToast } from "@/state/toasts";

interface UploadValue extends UploadSnapshot {
  selectFile: (file: File) => SelectResult;
  start: () => Promise<StartResult>;
  /** Pick the archive for a restored session and continue immediately. */
  resume: (file: File) => Promise<SelectResult | StartResult>;
  cancel: () => Promise<void>;
  dismissResult: () => void;
}

const UploadContext = createContext<UploadValue | null>(null);

function browserStorage(): UploadStorage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    // Safari private mode and embedded webviews can throw on the getter.
    return null;
  }
}

/**
 * App-wide upload owner (#185). It sits above the router so the chunk loop and
 * the live progress survive route changes, and it is the only place that
 * toasts a finished upload or invalidates the library queries.
 */
export function UploadProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const { ready: authReady } = useAuth();
  const isEditor = useIsEditor();

  // The controller outlives renders; callbacks read the latest providers.
  const handlers = useRef({ push, queryClient });
  handlers.current = { push, queryClient };

  const controllerRef = useRef<UploadController | null>(null);
  if (controllerRef.current === null) {
    controllerRef.current = new UploadController({
      transport: api,
      storage: browserStorage(),
      onComplete: (result) => {
        const { push: notify, queryClient: client } = handlers.current;
        void client.invalidateQueries({ queryKey: ["games"] });
        void client.invalidateQueries({ queryKey: ["stats"] });
        void client.invalidateQueries({ queryKey: ["scan"] });
        notify({
          title: t("Upload complete"),
          description: t("{file} is now in the library.", { file: result.fileName }),
          tone: "success",
        });
      },
      onError: (message) => {
        handlers.current.push({ title: t("Upload failed"), description: message, tone: "danger" });
      },
    });
  }
  const controller = controllerRef.current;

  useEffect(() => {
    if (!authReady) return;
    // Only editors and admins may open an upload; a member with a stale id in
    // localStorage gets it dropped instead of a 403 loop.
    if (isEditor) void controller.restore();
    else void controller.forget();
  }, [authReady, isEditor, controller]);

  const snapshot = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );

  const value = useMemo<UploadValue>(
    () => ({
      ...snapshot,
      selectFile: (file: File) => controller.select(file),
      start: () => controller.start(),
      resume: async (file: File) => {
        const picked = controller.select(file);
        if (picked !== "selected") return picked;
        return controller.start();
      },
      cancel: () => controller.cancel(),
      dismissResult: () => controller.dismissResult(),
    }),
    [snapshot, controller],
  );

  return <UploadContext.Provider value={value}>{children}</UploadContext.Provider>;
}

export function useUpload(): UploadValue {
  const value = useContext(UploadContext);
  if (!value) throw new Error("useUpload must be used inside UploadProvider");
  return value;
}
