import { useState, useSyncExternalStore } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, Check, Circle, X } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { installBridge, installsAvailable } from "@/lib/install";
import { t } from "@/lib/i18n";
import {
  dismissGettingStarted,
  gettingStartedDismissed,
  isWizardActive,
  openOnboarding,
  subscribeWizard,
} from "@/lib/onboarding";
import { useAuth } from "@/state/auth";

/**
 * Compact checklist shown after the wizard is skipped or closed (#119). It
 * stops appearing once the applicable recommended steps are done or the user
 * dismisses it; "Resume" reopens the tour at the persisted step.
 */
export function GettingStartedCard({ className }: { className?: string }) {
  const { user } = useAuth();
  const wizardActive = useSyncExternalStore(subscribeWizard, isWizardActive);
  const [dismissed, setDismissed] = useState(() => gettingStartedDismissed());
  const desktop = installsAvailable();

  const stats = useQuery({ queryKey: ["stats"], queryFn: () => api.stats(), enabled: Boolean(user), staleTime: 30_000 });
  const settings = useQuery({
    queryKey: ["desktop-settings"],
    queryFn: () => installBridge.settings(),
    enabled: Boolean(user) && desktop,
  });

  if (!user || user.onboardingCompletedAt || wizardActive || dismissed) return null;

  const games = stats.data?.games ?? 0;
  const current = settings.data ?? null;
  const rows = [
    {
      id: "library",
      label: t("Index your library"),
      done: games > 0,
      to: user.role === "user" ? "/library" : "/admin/scan",
    },
    {
      id: "install",
      label: t("Choose install folders"),
      done: !desktop || Boolean(current?.installDir),
      to: "/settings",
    },
    {
      id: "saves",
      label: t("Turn on automatic cloud saves"),
      done: !desktop || Boolean(current?.cloudAutoSync),
      to: "/settings",
    },
  ];
  const remaining = rows.filter((row) => !row.done);

  const hide = () => {
    dismissGettingStarted();
    setDismissed(true);
  };

  return (
    <section className={cn("border border-line bg-surface/40", className)} aria-labelledby="getting-started-heading">
      <div className="flex items-start justify-between gap-4 border-b border-line px-4 py-3">
        <div>
          <h2 id="getting-started-heading" className="text-[13.5px] font-medium text-ink">
            {t("Getting started")}
          </h2>
          <p className="mt-0.5 font-mono text-[10px] text-ink-3">
            {remaining.length === 0
              ? t("All recommended steps are done.")
              : t("{done} of {total} done", { done: rows.length - remaining.length, total: rows.length })}
          </p>
        </div>
        <button
          type="button"
          onClick={hide}
          aria-label={t("Dismiss")}
          className="grid size-7 shrink-0 place-items-center text-ink-3 transition-colors hover:text-ink"
        >
          <X className="size-3.5" aria-hidden="true" />
        </button>
      </div>
      <ul className="px-4">
        {rows.map((row) => (
          <li key={row.id} className="border-b border-line py-2">
            <Link to={row.to} className="flex items-center gap-2.5 text-[12.5px] text-ink-2 transition-colors hover:text-ink">
              {row.done ? (
                <Check className="size-3.5 shrink-0 text-ok" aria-label={t("Done")} />
              ) : (
                <Circle className="size-3.5 shrink-0 text-ink-3" aria-label={t("Not done")} />
              )}
              <span className={cn("flex-1", row.done && "text-ink-3 line-through")}>{row.label}</span>
              <ArrowRight className="rtl:-scale-x-100 size-3.5 text-ink-3" aria-hidden="true" />
            </Link>
          </li>
        ))}
      </ul>
      <div className="flex items-center justify-between gap-3 px-4 py-3">
        <button type="button" onClick={hide} className="text-[12px] text-ink-3 transition-colors hover:text-ink">
          {t("Dismiss")}
        </button>
        <button
          type="button"
          onClick={() => openOnboarding()}
          className="inline-flex items-center gap-1.5 text-[12.5px] font-medium text-ink transition-colors hover:text-signal-text"
        >
          {t("Resume")}
          <ArrowRight className="rtl:-scale-x-100 size-3.5" aria-hidden="true" />
        </button>
      </div>
    </section>
  );
}
