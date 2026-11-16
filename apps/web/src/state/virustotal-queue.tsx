import {
  createContext,
  useContext,
  useMemo,
  useRef,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import {
  createVirusTotalQueue,
  type VirusTotalQueue,
  type VirusTotalQueueStart,
  type VirusTotalQueueSnapshot,
} from "@/lib/virustotal-queue";
import { useToast } from "@/state/toasts";

export interface VirusTotalQueueValue extends VirusTotalQueueSnapshot {
  /** Requests still queued, the in-flight one included. */
  remaining: number;
  /** Second starts append; see `VirusTotalQueue.start`. */
  start: (ids: number[]) => VirusTotalQueueStart;
  cancel: () => void;
}

const VirusTotalQueueContext = createContext<VirusTotalQueueValue | null>(null);

/**
 * App-wide owner of the VirusTotal bulk-scan queue (#238). It sits next to
 * `ToastProvider`, above `HashRouter`, so navigating to Library, Console or a
 * game page never unmounts the scheduler; only the queue's own `cancel` stops
 * it. The Games screen is a consumer and re-renders from the same snapshot.
 *
 * Completion (done/failed) toasts and the drained invalidation live here, not
 * in the screen, so the result is visible from any route.
 */
export function VirusTotalQueueProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const { push } = useToast();

  // The controller outlives renders; callbacks read the latest providers.
  const handlers = useRef({ push, queryClient });
  handlers.current = { push, queryClient };

  const controllerRef = useRef<VirusTotalQueue | null>(null);
  if (controllerRef.current === null) {
    controllerRef.current = createVirusTotalQueue({
      scan: (gameId) => api.scanVirusTotal({ gameId }),
      invalidate: (queryKey) => handlers.current.queryClient.invalidateQueries({ queryKey }),
      toast: (toast) => handlers.current.push(toast),
    });
  }
  const controller = controllerRef.current;

  const snapshot = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );

  const value = useMemo<VirusTotalQueueValue>(
    () => ({
      ...snapshot,
      remaining: controller.remaining,
      start: (ids: number[]) => controller.start(ids),
      cancel: () => controller.cancel(),
    }),
    [snapshot, controller],
  );

  return (
    <VirusTotalQueueContext.Provider value={value}>{children}</VirusTotalQueueContext.Provider>
  );
}

export function useVirusTotalQueue(): VirusTotalQueueValue {
  const value = useContext(VirusTotalQueueContext);
  if (!value) throw new Error("useVirusTotalQueue must be used inside VirusTotalQueueProvider");
  return value;
}
