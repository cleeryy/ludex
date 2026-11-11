import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { motion, useReducedMotion } from "motion/react";
import { ArrowLeft, ArrowRight, Download, FolderOpen, HardDrive, Palette, Server, ShieldCheck, Sparkles, X } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { formatRelative } from "@/lib/format";
import { formatNumber, LOCALES, t, tn, useI18n } from "@/lib/i18n";
import { installBridge, installsAvailable } from "@/lib/install";
import { isNative } from "@/lib/native";
import {
  clampOnboardingStep,
  clearOnboardingSkip,
  hasSkippedOnboarding,
  markOnboardingSkipped,
  nearestVisibleOnboardingStep,
  ONBOARDING_STEP_IDS,
  OPEN_ONBOARDING_EVENT,
  setWizardActive,
  visibleOnboardingSteps,
  type OnboardingStepId,
} from "@/lib/onboarding";
import type { LocalSettings } from "@/lib/types";
import { useAuth, useIsAdmin } from "@/state/auth";
import { useSettings } from "@/state/settings";
import type { ThemeSelection } from "@/theme/types";
import { Badge, Button, Kbd } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";

/**
 * Optional first-run tour (#119). It is mounted by the app shell, opens once
 * per account, and never gates the UI: the library stays usable behind it.
 * Every mutating action is an explicit click; the only writes are the two
 * onboarding preference fields.
 */
export function OnboardingWizard() {
  const { user, setup, updateOnboarding } = useAuth();
  const isAdmin = useIsAdmin();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [step, setStep] = useState(0);
  const panelRef = useRef<HTMLDivElement>(null);
  const restoreFocus = useRef<HTMLElement | null>(null);
  const autoOpenedFor = useRef<number | null>(null);
  const userRef = useRef(user);
  useEffect(() => {
    userRef.current = user;
  }, [user]);
  const titleId = useId();
  const reduce = useReducedMotion();

  const visible = visibleOnboardingSteps(isAdmin);
  const position = Math.max(0, visible.indexOf(step));
  const last = position >= visible.length - 1;

  const folders = useQuery({ queryKey: ["folders"], queryFn: () => api.folders(), enabled: open, staleTime: 60_000 });
  const stats = useQuery({ queryKey: ["stats"], queryFn: () => api.stats(), enabled: open, staleTime: 30_000 });
  const scan = useQuery({
    queryKey: ["scan"],
    queryFn: () => api.scanStatus(),
    enabled: open,
    refetchInterval: (query) => (query.state.data?.running ? 4_000 : false),
  });
  const desktopAvailable = installsAvailable();
  const desktop = useQuery({
    queryKey: ["desktop-settings"],
    queryFn: () => installBridge.settings(),
    enabled: open && desktopAvailable,
  });

  const skip = useCallback(() => {
    const current = userRef.current;
    if (current) markOnboardingSkipped(current.id);
    setOpen(false);
  }, []);

  const goTo = useCallback(
    (next: number) => {
      const clamped = clampOnboardingStep(next);
      setStep(clamped);
      // Optimistic: the panel never waits on the network to advance.
      void updateOnboarding({ step: clamped });
    },
    [updateOnboarding],
  );

  const complete = useCallback(() => {
    void updateOnboarding({ completed: true, step });
    setOpen(false);
  }, [step, updateOnboarding]);

  // Open once per account after the first successful login/setup. A skipped
  // session, a completed account or a returning user never sees a popup.
  useEffect(() => {
    if (open || !user) return;
    if (user.onboardingCompletedAt) return;
    if (hasSkippedOnboarding(user.id)) return;
    if (autoOpenedFor.current === user.id) return;
    autoOpenedFor.current = user.id;
    setStep(nearestVisibleOnboardingStep(user.onboardingStep, isAdmin));
    setOpen(true);
  }, [isAdmin, open, user]);

  // Focus management and Escape handling. The panel is non-modal, so Tab can
  // leave it into the app; only the step content itself is announced.
  useEffect(() => {
    if (!open) return;
    restoreFocus.current = document.activeElement as HTMLElement | null;
    setWizardActive(true);
    const frame = requestAnimationFrame(() => {
      panelRef.current?.focus({ preventScroll: true });
    });
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      skip();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("keydown", onKeyDown);
      setWizardActive(false);
      restoreFocus.current?.focus?.({ preventScroll: true });
    };
  }, [open, skip]);

  // Settings → Help and the getting-started card ask the shell to reopen it.
  useEffect(() => {
    const onOpen = (event: Event) => {
      const detail = (event as CustomEvent<{ restart?: boolean }>).detail;
      clearOnboardingSkip();
      autoOpenedFor.current = user?.id ?? null;
      if (detail?.restart) {
        setStep(0);
        void updateOnboarding({ completed: false, step: 0 });
      } else {
        setStep(nearestVisibleOnboardingStep(user?.onboardingStep ?? 0, isAdmin));
      }
      setOpen(true);
    };
    window.addEventListener(OPEN_ONBOARDING_EVENT, onOpen);
    return () => window.removeEventListener(OPEN_ONBOARDING_EVENT, onOpen);
  }, [isAdmin, updateOnboarding, user?.id, user?.onboardingStep]);

  if (!open) return null;

  const stepId = ONBOARDING_STEP_IDS[step];
  const settings = desktop.data ?? null;
  const next = () => {
    if (last) complete();
    else goTo(visible[position + 1]);
  };
  const back = () => {
    if (position > 0) goTo(visible[position - 1]);
  };
  const openLibrary = () => {
    complete();
    navigate("/library");
  };

  let content: ReactNode;
  switch (stepId) {
    case "welcome":
      content = (
        <WelcomeStep
          libraryPath={folders.data?.gamesDir ?? null}
          serverName={setup?.serverName ?? "Ludex"}
          local={isNative() && settings?.mode === "local"}
        />
      );
      break;
    case "library":
      content = (
        <LibraryStep
          games={stats.data?.games ?? null}
          finishedAt={scan.data?.finishedAt ?? null}
          gamesDir={folders.data?.gamesDir ?? null}
          isEditor={isAdmin || user?.role === "editor"}
        />
      );
      break;
    case "downloads":
      content = <DownloadsStep settings={settings} desktopAvailable={desktopAvailable} />;
      break;
    case "saves":
      content = <SavesStep settings={settings} desktopAvailable={desktopAvailable} />;
      break;
    case "appearance":
      content = <AppearanceStep />;
      break;
    case "admin":
      content = <AdminStep />;
      break;
    default:
      content = <FinishStep onOpenLibrary={openLibrary} />;
      break;
  }

  return (
    <motion.div
      initial={reduce ? false : { opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: reduce ? 0 : 0.18, ease: [0.16, 1, 0.3, 1] }}
      className={cn(
        "fixed inset-x-3 bottom-[calc(56px+env(safe-area-inset-bottom)+10px)] z-[70] flex max-h-[min(78vh,640px)] flex-col overflow-hidden border border-line-2 bg-surface text-ink shadow-panel",
        "lg:inset-x-auto lg:bottom-5 lg:end-5 lg:w-[430px]",
      )}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="false"
        aria-labelledby={titleId}
        tabIndex={-1}
        className="flex min-h-0 flex-1 flex-col outline-none"
      >
        <div className="flex items-start justify-between gap-4 border-b border-line px-5 pb-4 pt-4">
          <div className="min-w-0">
            <p className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.14em] text-ink-3">
              <Sparkles className="size-3 text-signal-text" aria-hidden="true" />
              {t("Getting started")}
            </p>
            <h2 id={titleId} className="mt-1.5 text-[19px] font-semibold leading-tight tracking-[-0.02em]">
              {stepTitle(stepId)}
            </h2>
          </div>
          <button
            type="button"
            onClick={skip}
            aria-label={t("Close onboarding")}
            className="grid size-8 shrink-0 place-items-center text-ink-3 transition-colors hover:text-ink"
          >
            <X className="size-4" aria-hidden="true" />
          </button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4" aria-live="polite">
          {content}
        </div>

        <div className="border-t border-line px-5 py-3">
          <div className="flex items-center justify-between gap-3">
            <button type="button" onClick={skip} className="text-[12px] text-ink-3 transition-colors hover:text-ink">
              {t("Skip for now")}
            </button>
            <button type="button" onClick={complete} className="text-[12px] text-ink-3 transition-colors hover:text-ink">
              {t("Don’t show again")}
            </button>
          </div>
          <div className="mt-2.5 flex items-center justify-between gap-3">
            <div
              role="group"
              aria-label={t("Step {current} of {total}", { current: position + 1, total: visible.length })}
              className="flex items-center gap-1.5"
            >
              {visible.map((index, dot) => (
                <span
                  key={index}
                  aria-hidden="true"
                  className={cn(
                    "h-1.5 rounded-full transition-[width,background-color] duration-200",
                    dot === position ? "w-4 bg-signal" : "w-1.5 bg-line-2",
                    dot < position && "bg-ink-3",
                  )}
                />
              ))}
            </div>
            <div className="flex items-center gap-2">
              <Button
                variant="ghost"
                size="sm"
                onClick={back}
                disabled={position === 0}
                icon={<ArrowLeft className="rtl:-scale-x-100 size-3.5" />}
              >
                {t("Back")}
              </Button>
              <Button
                variant="primary"
                size="sm"
                onClick={next}
                iconRight={<ArrowRight className="rtl:-scale-x-100 size-3.5" />}
              >
                {last ? t("Finish") : t("Next")}
              </Button>
            </div>
          </div>
        </div>
      </div>
    </motion.div>
  );
}

function stepTitle(id: OnboardingStepId): string {
  switch (id) {
    case "welcome":
      return t("Welcome to Ludex");
    case "library":
      return t("Your library");
    case "downloads":
      return t("Downloads");
    case "saves":
      return t("Saves and integrations");
    case "appearance":
      return t("Appearance");
    case "admin":
      return t("Server administration");
    default:
      return t("You’re set");
  }
}

/* --------------------------------- pieces -------------------------------- */

function Fact({ label, value, mono }: { label: string; value: ReactNode; mono?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-line py-2 last:border-0">
      <dt className="shrink-0 text-[12px] text-ink-3">{label}</dt>
      <dd className={cn("min-w-0 truncate text-right text-[12.5px] text-ink", mono && "font-mono text-[11.5px]")}>{value}</dd>
    </div>
  );
}

function DesktopOnly() {
  return <Badge tone="outline">{t("Desktop only")}</Badge>;
}

function StepLink({ to, children, badge }: { to: string; children: ReactNode; badge?: boolean }) {
  return (
    <Link
      to={to}
      className="flex min-h-10 items-center justify-between gap-3 border-b border-line py-2 text-[13px] text-ink-2 transition-colors last:border-0 hover:text-ink"
    >
      <span className="flex items-center gap-2">
        <ArrowRight className="rtl:-scale-x-100 size-3.5 text-ink-3" aria-hidden="true" />
        {children}
      </span>
      {badge && <DesktopOnly />}
    </Link>
  );
}

/* --------------------------------- steps --------------------------------- */

function WelcomeStep({ libraryPath, serverName, local }: { libraryPath: string | null; serverName: string; local: boolean }) {
  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        {t("Ludex indexes the archives on your disk and keeps progress, collections and saves per account. It never uploads your games to a third party.")}
      </p>
      <dl className="border-t border-line">
        <Fact label={t("Library location")} value={local ? t("On this computer") : t("On the server")} />
        <Fact label={t("Connection")} value={local ? t("Local library") : t("Remote server")} />
        <Fact label={t("Server")} value={serverName} />
        {libraryPath && <Fact label={t("Games folder")} value={libraryPath} mono />}
      </dl>
      <p className="text-[11.5px] leading-relaxed text-ink-3">
        {t("You can change all of this later. Nothing in this tour changes a setting for you.")}
      </p>
    </div>
  );
}

function LibraryStep({
  games,
  finishedAt,
  gamesDir,
  isEditor,
}: {
  games: number | null;
  finishedAt: string | null;
  gamesDir: string | null;
  isEditor: boolean;
}) {
  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        {t("Ludex walks the games folder three levels deep and indexes archives and portable executables. Counts come from the server.")}
      </p>
      <dl className="border-t border-line">
        <Fact label={t("Games indexed")} value={games === null ? "—" : tn(games, "1 game", "{count} games")} />
        <Fact label={t("Last scan")} value={finishedAt ? formatRelative(finishedAt) : t("Never")} />
        {gamesDir && <Fact label={t("Games folder")} value={gamesDir} mono />}
      </dl>
      <div>
        {isEditor ? (
          <>
            <StepLink to="/admin/scan">{t("Scan the library")}</StepLink>
            <StepLink to="/admin/ingest">{t("Add games")}</StepLink>
          </>
        ) : (
          <p className="text-[11.5px] leading-relaxed text-ink-3">
            {t("An administrator adds games from Admin → Library scan.")}
          </p>
        )}
        <StepLink to="/settings" badge={!isNative()}>
          {t("Import a Steam library")}
        </StepLink>
        <StepLink to="/settings" badge={!isNative()}>
          {t("Launcher discovery")}
        </StepLink>
      </div>
    </div>
  );
}

function DownloadsStep({ settings, desktopAvailable }: { settings: LocalSettings | null; desktopAvailable: boolean }) {
  const native = isNative();
  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        {t("Install folders, bandwidth and stream installs are desktop settings. In the browser you can download archives without installing them.")}
      </p>
      {native && settings ? (
        <dl className="border-t border-line">
          <Fact label={t("Install folder")} value={settings.installDir} mono />
          <Fact label={t("Download folder")} value={settings.downloadDir} mono />
          <Fact
            label={t("Bandwidth limit")}
            value={settings.bandwidthLimitKib > 0 ? `${formatNumber(settings.bandwidthLimitKib)} ${t("KiB/s")}` : t("Unlimited")}
          />
          <Fact label={t("Stream installs")} value={settings.streamInstall ? t("On") : t("Off")} />
        </dl>
      ) : (
        <div className="flex items-center justify-between gap-3 border-y border-line py-3">
          <p className="text-[12.5px] text-ink-2">{t("Install and download folders live in the desktop app.")}</p>
          <DesktopOnly />
        </div>
      )}
      <p className="border-l-2 border-line-2 pl-3 text-[11.5px] leading-relaxed text-ink-3">
        {t("Stream installs start faster but cannot resume, and they use more bandwidth. Normal downloads resume after a dropped connection.")}
      </p>
      <div>
        <StepLink to="/settings" badge={!desktopAvailable}>
          {t("Open download settings")}
        </StepLink>
      </div>
    </div>
  );
}

function SavesStep({ settings, desktopAvailable }: { settings: LocalSettings | null; desktopAvailable: boolean }) {
  const native = isNative();
  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        {t("Back up save files after each session, show what you are playing on Discord, and track achievements.")}
      </p>
      <dl className="border-t border-line">
        <Fact
          label={t("Automatic cloud saves")}
          value={native && settings ? (settings.cloudAutoSync ? t("On") : t("Off")) : <DesktopOnly />}
        />
        <Fact label={t("Save folder detection")} value={<DesktopOnly />} />
        <Fact
          label={t("Discord presence")}
          value={native && settings ? (settings.discordPresence ? t("On") : t("Off")) : <DesktopOnly />}
        />
        <Fact label={t("Achievements")} value={t("Per account")} />
      </dl>
      <div>
        <StepLink to="/settings" badge={!desktopAvailable}>
          {t("Open integration settings")}
        </StepLink>
        <StepLink to="/community">{t("See achievements")}</StepLink>
      </div>
      <p className="text-[11.5px] leading-relaxed text-ink-3">
        {t("Ludex never turns these on for you; every toggle is one click in Settings.")}
      </p>
    </div>
  );
}

function AppearanceStep() {
  const { theme, setTheme } = useSettings();
  const { locale, setLocale } = useI18n();
  const options: { value: ThemeSelection; label: string }[] = [
    { value: "midnight", label: t("Dark") },
    { value: "light", label: t("Light") },
    { value: "system", label: t("System") },
  ];
  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        {t("Theme and language apply to this browser or app window, not to the account.")}
      </p>
      <div>
        <p className="mb-2 text-[12px] text-ink-3">{t("Theme")}</p>
        <div className="grid grid-cols-3 gap-2">
          {options.map((option) => (
            <button
              key={option.value}
              type="button"
              onClick={() => setTheme(option.value)}
              aria-pressed={theme === option.value}
              className={cn(
                "flex min-h-11 items-center gap-2 border px-2.5 text-left text-[12.5px] transition-colors",
                theme === option.value ? "border-signal bg-surface-2 text-ink" : "border-line text-ink-2 hover:border-line-2",
              )}
            >
              <Palette className={cn("size-3.5", theme === option.value ? "text-signal-text" : "text-ink-3")} aria-hidden="true" />
              {option.label}
            </button>
          ))}
        </div>
      </div>
      <div>
        <p className="mb-2 text-[12px] text-ink-3">{t("Language")}</p>
        <Select
          label={t("Language")}
          value={locale}
          onChange={(value) => void setLocale(value)}
          options={LOCALES.map((entry) => ({ value: entry.code, label: entry.label, hint: entry.englishLabel }))}
        />
        <p className="mt-2 text-[11.5px] leading-relaxed text-ink-3">
          {t("Community translations fall back to English for missing strings.")}
        </p>
      </div>
    </div>
  );
}

function AdminStep() {
  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        {t("Optional administrator setup. The library works without any of it.")}
      </p>
      <div className="flex flex-col gap-2">
        <AdminLink to="/admin/authentication" icon={<ShieldCheck className="size-4" />} title={t("Single sign-on (OIDC)")} />
        <AdminLink to="/admin/users" icon={<Server className="size-4" />} title={t("Accounts and roles")} />
        <AdminLink to="/admin/server" icon={<HardDrive className="size-4" />} title={t("Server and storage")} />
      </div>
      <p className="text-[11.5px] leading-relaxed text-ink-3">
        {t("Normal accounts skip this step. Open any page from the Admin section of the sidebar.")}
      </p>
    </div>
  );
}

function AdminLink({ to, icon, title }: { to: string; icon: ReactNode; title: string }) {
  return (
    <Link
      to={to}
      className="flex items-center gap-3 border border-line px-3 py-2.5 text-[13px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
    >
      <span className="text-ink-3">{icon}</span>
      <span className="flex-1">{title}</span>
      <ArrowRight className="rtl:-scale-x-100 size-3.5 text-ink-3" aria-hidden="true" />
    </Link>
  );
}

function FinishStep({ onOpenLibrary }: { onOpenLibrary: () => void }) {
  const shortcuts: { keys: ReactNode; label: string }[] = [
    { keys: <Kbd>⌘K</Kbd>, label: t("Search anything") },
    { keys: <span className="font-mono text-[11px]">g h</span>, label: t("Home") },
    { keys: <span className="font-mono text-[11px]">g l</span>, label: t("Library") },
    { keys: <span className="font-mono text-[11px]">g d</span>, label: t("Downloads") },
  ];
  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        {t("The library is ready. These shortcuts work anywhere in the app.")}
      </p>
      <dl className="border-t border-line">
        {shortcuts.map((shortcut, index) => (
          <div key={index} className="flex items-center justify-between gap-4 border-b border-line py-2 last:border-0">
            <dt className="text-[12.5px] text-ink-2">{shortcut.label}</dt>
            <dd>{shortcut.keys}</dd>
          </div>
        ))}
      </dl>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={onOpenLibrary} icon={<FolderOpen className="size-4" />}>
          {t("Open the library")}
        </Button>
        <Link to="/settings" className="inline-flex h-8 items-center gap-1.5 px-2 text-[12.5px] text-ink-3 hover:text-ink">
          <Download className="size-3.5" aria-hidden="true" />
          {t("Open settings")}
        </Link>
      </div>
      <p className="text-[11.5px] leading-relaxed text-ink-3">
        {t("You can replay this tour from Settings → Help.")}
      </p>
    </div>
  );
}
