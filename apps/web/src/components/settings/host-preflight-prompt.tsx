import { useState, useSyncExternalStore } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Cpu, X } from "lucide-react";
import {
  dismissHostPrompt,
  hostBridge,
  hostGaps,
  hostPromptDismissed,
} from "@/lib/host";
import { isNative } from "@/lib/native";
import { isWizardActive, subscribeWizard } from "@/lib/onboarding";
import { t, tn } from "@/lib/i18n";
import { Button } from "@/components/ui/controls";
import { qk } from "@/lib/query-keys";

/**
 * First-run host prompt (#285).
 *
 * A small, dismissible card on the desktop shell when the stored preflight
 * report has gaps. It never blocks, never installs: it points at Settings →
 * Host where the consent screen lists the exact packages. The report itself is
 * persisted by the desktop, so this costs no probe.
 */
export function HostPreflightPrompt() {
  const native = isNative();
  const wizardActive = useSyncExternalStore(subscribeWizard, isWizardActive);
  const [dismissedFor, setDismissedFor] = useState<string | null>(null);
  const report = useQuery({
    queryKey: qk.hostPreflight,
    queryFn: () => hostBridge.report(),
    enabled: native,
    staleTime: 5 * 60_000,
    retry: false,
  });

  if (!native || wizardActive || !report.data) return null;
  const gaps = hostGaps(report.data);
  if (gaps.length === 0) return null;
  if (dismissedFor === report.data.checkedAt || hostPromptDismissed(report.data.checkedAt)) {
    return null;
  }

  const dismiss = () => {
    dismissHostPrompt(report.data?.checkedAt ?? "");
    setDismissedFor(report.data?.checkedAt ?? null);
  };

  return (
    <aside
      role="status"
      aria-live="polite"
      aria-label={t("Host checks")}
      className="fixed inset-x-3 bottom-[calc(56px+env(safe-area-inset-bottom)+10px)] z-40 flex items-start gap-3 border border-line-2 bg-surface p-3.5 text-ink shadow-panel lg:inset-x-auto lg:bottom-5 lg:end-5 lg:w-[380px]"
    >
      <Cpu className="mt-0.5 size-4 shrink-0 text-signal-text" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <p className="text-body font-medium">
          {tn(gaps.length, "1 runtime piece is missing", "{count} runtime pieces are missing")}
        </p>
        <p className="mt-1 text-meta leading-relaxed text-ink-3">
          {t("Some games will not start until Vulkan, UMU or the other runtimes are installed.")}
        </p>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Link
            to="/settings?section=host&install=1"
            onClick={dismiss}
            className="inline-flex h-8 select-none items-center justify-center rounded-[var(--r-sm)] border border-signal bg-signal px-3 text-body font-semibold text-on-signal transition-[filter] duration-[120ms] hover:brightness-[1.07]"
          >
            {t("Review host checks")}
          </Link>
          <Button size="sm" variant="ghost" onClick={dismiss}>
            {t("Not now")}
          </Button>
        </div>
      </div>
      <button
        type="button"
        onClick={dismiss}
        aria-label={t("Dismiss")}
        className="grid size-7 shrink-0 place-items-center text-ink-3 transition-colors hover:text-ink"
      >
        <X className="size-3.5" aria-hidden="true" />
      </button>
    </aside>
  );
}
