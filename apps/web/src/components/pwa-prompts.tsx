import { Download, RefreshCw, X } from "lucide-react";
import { t } from "@/lib/i18n";
import { applyUpdate, dismissInstall, dismissUpdate, promptInstall, usePwa } from "@/lib/pwa";
import { Button } from "@/components/ui/controls";

/**
 * Two deferred prompts in one strip: install the PWA, or reload into the
 * build the service worker just finished caching. Both stay out of the way of
 * the mobile tab bar and the grain overlay.
 */
export function PwaPrompts() {
  const { canInstall, updateReady } = usePwa();
  if (!canInstall && !updateReady) return null;

  return (
    <div
      className="fixed inset-x-3 bottom-[calc(56px+env(safe-area-inset-bottom)+10px)] z-50 lg:inset-x-auto lg:bottom-5 lg:end-5 lg:w-[360px]"
      role="status"
      aria-live="polite"
    >
      {updateReady ? (
        <div className="flex items-start gap-3 border border-line-2 bg-surface p-3.5 text-ink shadow-panel">
          <RefreshCw className="mt-0.5 size-4 shrink-0 text-signal-text" aria-hidden="true" />
          <div className="min-w-0 flex-1">
            <p className="text-[13px] font-medium">{t("A new build is ready")}</p>
            <p className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
              {t("Reload to finish the update. The library stays indexed on the server.")}
            </p>
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <Button size="sm" variant="primary" onClick={applyUpdate} icon={<RefreshCw className="size-3.5" />}>
                {t("Reload now")}
              </Button>
              <Button size="sm" variant="ghost" onClick={dismissUpdate}>
                {t("Later")}
              </Button>
            </div>
          </div>
          <button
            type="button"
            onClick={dismissUpdate}
            aria-label={t("Dismiss")}
            className="grid size-7 shrink-0 place-items-center text-ink-3 hover:text-ink"
          >
            <X className="size-3.5" aria-hidden="true" />
          </button>
        </div>
      ) : (
        <div className="flex items-start gap-3 border border-line-2 bg-surface p-3.5 text-ink shadow-panel">
          <Download className="mt-0.5 size-4 shrink-0 text-signal-text" aria-hidden="true" />
          <div className="min-w-0 flex-1">
            <p className="text-[13px] font-medium">{t("Install Ludex")}</p>
            <p className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
              {t("Keep the shell on your home screen and open the library full screen, even offline.")}
            </p>
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <Button size="sm" variant="primary" onClick={() => void promptInstall()} icon={<Download className="size-3.5" />}>
                {t("Install")}
              </Button>
              <Button size="sm" variant="ghost" onClick={dismissInstall}>
                {t("Not now")}
              </Button>
            </div>
          </div>
          <button
            type="button"
            onClick={dismissInstall}
            aria-label={t("Dismiss")}
            className="grid size-7 shrink-0 place-items-center text-ink-3 hover:text-ink"
          >
            <X className="size-3.5" aria-hidden="true" />
          </button>
        </div>
      )}
    </div>
  );
}
