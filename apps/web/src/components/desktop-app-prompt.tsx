import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { Link, useLocation } from "react-router-dom";
import { Download, X } from "lucide-react";
import { isNative } from "@/lib/native";
import { usePwa } from "@/lib/pwa";
import { useAuth } from "@/state/auth";
import { isWizardActive, subscribeWizard } from "@/lib/onboarding";
import { Button } from "@/components/ui/controls";
import { t } from "@/lib/i18n";
import {
  detectDesktopPlatform,
  hideDesktopPrompt,
  platformName,
  readDesktopPromptDismissal,
  shouldOfferDesktopApp,
  snoozeDesktopPrompt,
} from "@/lib/desktop-download";

/** How long a visitor browses before the prompt appears. */
const PROMPT_DELAY_MS = 6_000;
/** Library routes: the prompt only shows once the user has seen the shelf. */
const LIBRARY_ROUTES = new Set(["/", "/library", "/recent", "/bookmarks", "/all"]);

function standaloneDisplay(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return window.matchMedia("(display-mode: standalone)").matches;
}

/**
 * One small, dismissible "Get the desktop app" prompt (#164).
 *
 * It only exists in a browser tab: the Tauri shell already *is* the desktop
 * app, and an installed PWA would nag someone who deliberately chose the web
 * app. "Not now" lasts for the session, "Don't show again" is permanent, and
 * the PWA install/reload strip in the same corner always wins so the two never
 * stack.
 */
export function DesktopAppPrompt() {
  const { user } = useAuth();
  const location = useLocation();
  const pwa = usePwa();
  const [environment, setEnvironment] = useState(() => ({
    native: isNative(),
    standalone: standaloneDisplay(),
    ...readDesktopPromptDismissal(),
  }));
  const [visible, setVisible] = useState(false);
  // The first-run tour owns the screen while it is open; a user who skipped it
  // is still eligible for the prompt later.
  const wizardActive = useSyncExternalStore(subscribeWizard, isWizardActive);
  const platform = useMemo(
    () => detectDesktopPlatform(typeof navigator === "undefined" ? "" : navigator.userAgent),
    [],
  );

  const eligible =
    Boolean(user) &&
    LIBRARY_ROUTES.has(location.pathname) &&
    !wizardActive &&
    shouldOfferDesktopApp(environment);

  useEffect(() => {
    if (!eligible || pwa.canInstall || pwa.updateReady) return;
    const timer = window.setTimeout(() => setVisible(true), PROMPT_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [eligible, pwa.canInstall, pwa.updateReady]);

  if (!visible || !eligible) return null;

  const dismiss = (permanent: boolean) => {
    if (permanent) {
      hideDesktopPrompt();
      setEnvironment((current) => ({ ...current, hidden: true }));
    } else {
      snoozeDesktopPrompt();
      setEnvironment((current) => ({ ...current, snoozed: true }));
    }
    setVisible(false);
  };

  return (
    <aside
      role="status"
      aria-live="polite"
      aria-label={t("Desktop app")}
      className="fixed inset-x-3 bottom-[calc(56px+env(safe-area-inset-bottom)+10px)] z-40 flex items-start gap-3 border border-line-2 bg-surface p-3.5 text-ink shadow-panel lg:inset-x-auto lg:bottom-5 lg:start-5 lg:w-[380px]"
    >
      <Download className="mt-0.5 size-4 shrink-0 text-signal-text" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium">{t("Get the desktop app")}</p>
        <p className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
          {t("Install and launch games, sync cloud saves and play with a controller.")}
        </p>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Link
            to="/download"
            onClick={() => setVisible(false)}
            className="inline-flex h-8 select-none items-center justify-center gap-1.5 rounded-[2px] border border-signal bg-signal px-3 text-[13px] font-semibold text-on-signal transition-[filter] duration-[120ms] hover:brightness-[1.07]"
          >
            {platform
              ? t("Download for {platform}", { platform: platformName(platform) })
              : t("See download options")}
          </Link>
          <Button size="sm" variant="ghost" onClick={() => dismiss(false)}>
            {t("Use the web app")}
          </Button>
        </div>
        <button
          type="button"
          onClick={() => dismiss(true)}
          className="mt-2 text-[11px] text-ink-3 underline decoration-line-2 underline-offset-2 hover:text-ink-2"
        >
          {t("Don't show again")}
        </button>
      </div>
      <button
        type="button"
        onClick={() => dismiss(false)}
        aria-label={t("Dismiss")}
        className="grid size-7 shrink-0 place-items-center text-ink-3 hover:text-ink"
      >
        <X className="size-3.5" aria-hidden="true" />
      </button>
    </aside>
  );
}
