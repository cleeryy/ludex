import { useState } from "react";
import { Plug, X } from "lucide-react";
import { keepAwakeSentence, keepAwakeLabels, useInhibitStatus } from "@/lib/keep-awake";
import { isNative } from "@/lib/native";
import { t } from "@/lib/i18n";

/**
 * Suspend inhibitor banner (#288).
 *
 * A strip at the top of the desktop shell while Ludex blocks suspend and
 * display blanking. Dismissing it hides it for the current set of jobs only: a
 * new job, a game or a stream shows it again, and the reason it exists (the
 * machine must not sleep) stays visible in Settings → Host.
 */
export function KeepAwakeBanner() {
  const status = useInhibitStatus().data;
  const [dismissedFor, setDismissedFor] = useState<string | null>(null);

  if (!isNative() || !status?.supported) return null;
  const sentence = keepAwakeSentence(status);
  if (!sentence) return null;

  const signature = `${status.jobs}:${status.count}:${keepAwakeLabels(status)}`;
  if (dismissedFor === signature) return null;

  return (
    <div
      role="status"
      aria-live="polite"
      className="flex items-start gap-3 border-b border-line bg-surface px-4 py-2.5 lg:px-6"
    >
      <Plug className="mt-0.5 size-4 shrink-0 text-signal-text" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <p className="text-[12.5px] leading-snug text-ink">{sentence}</p>
        <p className="mt-0.5 text-[11.5px] leading-snug text-ink-3">
          {t("Ludex blocks suspend and display blanking until the last job ends, then releases the inhibitor.")}
        </p>
      </div>
      <button
        type="button"
        onClick={() => setDismissedFor(signature)}
        aria-label={t("Dismiss")}
        className="grid size-7 shrink-0 place-items-center text-ink-3 transition-colors hover:text-ink"
      >
        <X className="size-3.5" aria-hidden="true" />
      </button>
    </div>
  );
}
