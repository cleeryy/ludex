import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { AnimatePresence, motion } from "motion/react";
import { t } from "@/lib/i18n";

/**
 * The Console notice surface (#267, #268).
 *
 * A 10-foot screen has no toast viewport: a desktop toast is small, transient
 * and paints over the rail while a player is reading a title. Console errors
 * and confirmations therefore render here, as one card in the Cabinet's own
 * language — bottom-leading, above the band, one at a time, with the dot as its
 * only ornament. Nothing on this surface is a modal: it never takes the ring,
 * and the next press still lands on the focused game.
 */

export type ConsoleNoticeTone = "default" | "success" | "danger";

export interface ConsoleNotice {
  title: string;
  description?: string;
  tone?: ConsoleNoticeTone;
  /** Milliseconds before it clears itself; `0` keeps it until replaced. */
  durationMs?: number;
}

interface ConsoleNoticeApi {
  notify: (notice: ConsoleNotice) => void;
  dismiss: () => void;
}

const ConsoleNoticeContext = createContext<ConsoleNoticeApi | null>(null);

const DEFAULT_DURATION_MS = 6_000;

export function ConsoleNoticeProvider({ children }: { children: ReactNode }) {
  const [notice, setNotice] = useState<ConsoleNotice | null>(null);
  const timer = useRef<number | undefined>(undefined);

  const clearTimer = () => {
    if (timer.current !== undefined) {
      window.clearTimeout(timer.current);
      timer.current = undefined;
    }
  };

  const dismiss = useCallback(() => {
    clearTimer();
    setNotice(null);
  }, []);

  const notify = useCallback((next: ConsoleNotice) => {
    clearTimer();
    setNotice(next);
    const duration = next.durationMs ?? DEFAULT_DURATION_MS;
    if (duration > 0) timer.current = window.setTimeout(() => setNotice(null), duration);
  }, []);

  useEffect(() => clearTimer, []);

  const api = useMemo<ConsoleNoticeApi>(() => ({ notify, dismiss }), [notify, dismiss]);

  return (
    <ConsoleNoticeContext.Provider value={api}>
      {children}
      <div className="pointer-events-none fixed inset-x-0 bottom-[calc(var(--bp-band-h)+var(--bp-safe-y))] z-40 flex justify-start bp-pad-x">
        <AnimatePresence initial={false}>
          {notice && (
            <motion.div
              key={`${notice.title}\u0000${notice.description ?? ""}`}
              role="status"
              aria-live="polite"
              data-tone={notice.tone === "danger" ? "danger" : undefined}
              className="bp-notice pointer-events-auto flex max-w-[min(52ch,46vw)] items-start gap-3 px-4 py-3"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.12 }}
            >
              <span
                className={notice.tone === "danger" ? "bp-dot mt-2 shrink-0 bg-danger" : "bp-dot mt-2 shrink-0"}
                aria-hidden="true"
              />
              <div className="min-w-0 flex-1">
                <p className="text-bp-body font-medium leading-tight text-ink">{notice.title}</p>
                {notice.description && (
                  <p className="mt-1 line-clamp-2 text-bp-meta leading-relaxed text-ink-3">
                    {notice.description}
                  </p>
                )}
              </div>
              <button
                type="button"
                className="shrink-0 text-bp-meta text-ink-3 hover:text-ink"
                onClick={dismiss}
              >
                {t("Dismiss")}
              </button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </ConsoleNoticeContext.Provider>
  );
}

export function useConsoleNotice(): ConsoleNoticeApi {
  const value = useContext(ConsoleNoticeContext);
  if (!value) throw new Error("useConsoleNotice must be used inside ConsoleNoticeProvider");
  return value;
}
