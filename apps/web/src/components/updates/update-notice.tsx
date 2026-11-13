import { ArrowRight, Download, X } from "lucide-react";
import { Link } from "react-router-dom";
import { formatBytes } from "@/lib/format";
import { t } from "@/lib/i18n";

/**
 * Persistent in-app notice for a new build (#213).
 *
 * It sits where the badge sits and replaces it while visible: title, current
 * version and download size, then three explicit choices. "Open update center"
 * and the close button only mark the notice read — the version is never skipped
 * and the update center stays the source of truth. The notifier owns the
 * dedupe/skip/later decisions; this component only renders them.
 */
export function UpdateNotice({
  version,
  currentVersion,
  sizeBytes,
  onOpen,
  onLater,
  onSkip,
  onDismiss,
  disabled = false,
}: {
  version: string;
  currentVersion: string;
  sizeBytes: number | null;
  onOpen: () => void;
  onLater: () => void;
  onSkip: () => void;
  onDismiss: () => void;
  disabled?: boolean;
}) {
  return (
    <section
      role="status"
      aria-live="polite"
      aria-label={t("Update available: Ludex {version}", { version })}
      className="fade-in fixed bottom-[calc(40px+16px)] end-5 z-40 hidden w-[340px] overflow-hidden rounded-[2px] border border-line-2 border-s-2 border-s-signal bg-surface-2 shadow-toast md:block"
    >
      <div className="flex items-start gap-3 ps-4 pe-2 py-3">
        <span className="mt-0.5 grid size-7 shrink-0 place-items-center rounded-[2px] border border-signal/50 bg-signal/12 text-signal-text">
          <Download className="size-3.5" aria-hidden="true" />
        </span>
        <div className="min-w-0 flex-1">
          <p className="text-[13px] font-medium leading-snug text-ink">
            {t("Ludex {version} is available", { version })}
          </p>
          <p className="num mt-0.5 text-[11.5px] leading-snug text-ink-2">
            {sizeBytes
              ? t("You are on {current} · {size}", {
                  current: currentVersion,
                  size: formatBytes(sizeBytes),
                })
              : t("You are on {current}", { current: currentVersion })}
          </p>
        </div>
        <button
          type="button"
          onClick={onDismiss}
          aria-label={t("Dismiss notification")}
          className="-mr-1 grid size-7 shrink-0 place-items-center rounded-[2px] text-ink-3 hover:bg-surface hover:text-ink"
        >
          <X className="size-3.5" />
        </button>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 border-t border-line px-4 py-2">
        <Link
          to="/updates"
          onClick={onOpen}
          className="inline-flex items-center gap-1 text-[11.5px] font-medium text-ink underline-offset-4 hover:underline"
        >
          {t("Open update center")}
          <ArrowRight className="size-3" aria-hidden="true" />
        </Link>
        <button
          type="button"
          onClick={onLater}
          disabled={disabled}
          className="text-[11.5px] text-ink-3 underline-offset-4 transition-colors hover:text-ink hover:underline disabled:opacity-40"
        >
          {t("Later")}
        </button>
        <button
          type="button"
          onClick={onSkip}
          disabled={disabled}
          className="text-[11.5px] text-ink-3 underline-offset-4 transition-colors hover:text-ink hover:underline disabled:opacity-40"
        >
          {t("Skip this version")}
        </button>
      </div>
    </section>
  );
}
