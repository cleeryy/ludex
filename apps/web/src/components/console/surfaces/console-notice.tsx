import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { AlertTriangle, Check, Info } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";

/**
 * The Console notice surface (#267).
 *
 * A 10-foot screen has no toast viewport: a desktop toast is small, transient
 * and paints over the rail while a player is reading a title. Console errors
 * and confirmations therefore render here, as one card in the shell's own
 * language, anchored where the footer hints already live. Nothing on this
 * surface is a modal: it never takes the ring, and the next press still lands
 * on the focused game.
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
  const reduceMotion = useReducedMotion();

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
      <div className="pointer-events-none fixed inset-x-0 bottom-4 z-40 flex justify-center bp-pad-x">
        <AnimatePresence initial={false}>
          {notice && (
            <motion.div
              key={`${notice.title}\u0000${notice.description ?? ""}`}
              role="status"
              aria-live="polite"
              className={cn(
                "pointer-events-auto flex max-w-[560px] items-start gap-3 px-4 py-3 bp-panel",
                notice.tone === "danger" && "border-danger/60",
                notice.tone === "success" && "border-ok/60",
              )}
              initial={reduceMotion ? false : { opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 6 }}
              transition={{ duration: reduceMotion ? 0 : 0.18, ease: [0.22, 1, 0.36, 1] }}
            >
              {notice.tone === "danger" ? (
                <AlertTriangle className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden="true" />
              ) : notice.tone === "success" ? (
                <Check className="mt-0.5 size-4 shrink-0 text-ok" aria-hidden="true" />
              ) : (
                <Info className="mt-0.5 size-4 shrink-0 text-ink-3" aria-hidden="true" />
              )}
              <div className="min-w-0">
                <p className="text-bp-body leading-snug">{notice.title}</p>
                {notice.description && (
                  <p className="mt-1 text-bp-meta leading-relaxed text-ink-2">{notice.description}</p>
                )}
              </div>
              <button
                type="button"
                className="ml-auto shrink-0 rounded-bp-control px-1.5 py-0.5 font-mono text-bp-micro text-ink-3 hover:text-ink"
                onClick={dismiss}
                aria-label={t("Dismiss")}
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
