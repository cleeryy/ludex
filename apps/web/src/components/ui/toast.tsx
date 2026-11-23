import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { AlertTriangle, Check, X } from "lucide-react";
import { useToast } from "@/state/toasts";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";

export function ToastViewport() {
  const { toasts, dismiss } = useToast();
  const reduce = useReducedMotion();

  return (
    <div
      className="pointer-events-none fixed bottom-[calc(56px+env(safe-area-inset-bottom)+12px)] end-4 z-[100] lg:bottom-4 flex w-[min(360px,calc(100vw-32px))] flex-col gap-2"
      role="region"
      aria-label={t("Notifications")}
    >
      <AnimatePresence initial={false}>
        {toasts.map((toast) => (
          <motion.div
            key={toast.id}
            layout={!reduce}
            initial={reduce ? false : { opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: 6 }}
            transition={{ duration: reduce ? 0 : 0.18, ease: [0.16, 1, 0.3, 1] }}
            className={cn(
              "pointer-events-auto relative overflow-hidden rounded-[var(--r-lg)] border border-line-2 bg-elevated pl-4 pr-2 py-3",
              "shadow-toast",
            )}
            role="status"
          >
            <span
              className={cn(
                "absolute inset-y-0 start-0 w-[3px]",
                toast.tone === "danger" ? "bg-danger" : toast.tone === "success" ? "bg-ok" : "bg-signal",
              )}
              aria-hidden="true"
            />
            <div className="flex items-start gap-2.5">
              <span className="mt-0.5 text-ink-3" aria-hidden="true">
                {toast.tone === "danger" ? <AlertTriangle className="size-3.5 text-danger" /> : toast.tone === "success" ? <Check className="size-3.5 text-ok" /> : null}
              </span>
              <div className="min-w-0 flex-1">
                <p className="text-body font-medium leading-snug text-ink">{toast.title}</p>
                {toast.description && <p className="mt-0.5 text-label leading-snug text-ink-2">{toast.description}</p>}
              </div>
              <button
                type="button"
                onClick={() => dismiss(toast.id)}
                aria-label={t("Dismiss notification")}
                className="-mr-1 grid size-7 shrink-0 place-items-center rounded-[var(--r-xs)] text-ink-3 hover:bg-surface-2 hover:text-ink"
              >
                <X className="size-3.5" />
              </button>
            </div>
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
