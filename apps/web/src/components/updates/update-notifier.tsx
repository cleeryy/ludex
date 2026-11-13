import { useEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { ArrowRight, Download, RotateCcw } from "lucide-react";
import { formatBytes, formatDate, formatSpeed } from "@/lib/format";
import { t } from "@/lib/i18n";
import { isNative, native } from "@/lib/native";
import {
  markPrompted,
  markUpdateNoticeLater,
  markUpdateNoticeShown,
  readPromptedVersion,
  readUpdateNotice,
  shouldPrompt,
  shouldSendOsNotification,
  shouldShowBadge,
  shouldShowUpdateNotice,
  type UpdateNoticeRecord,
} from "@/lib/updates";
import { useUpdater } from "@/components/updates/update-provider";
import { UpdateNotice } from "@/components/updates/update-notice";
import { useToast } from "@/state/toasts";
import { useUpdateNotifications, useUpdatePrompt } from "@/state/preferences";
import { ExternalAnchor, ReleaseNotes } from "@/components/updates/parts";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";

/** Body shared by the in-app card and the OS banner (#213). */
function noticeBody(currentVersion: string, sizeBytes: number | null): string {
  return sizeBytes
    ? t("You are on {current} · {size}", {
        current: currentVersion,
        size: formatBytes(sizeBytes),
      })
    : t("You are on {current}", { current: currentVersion });
}

/**
 * Launch prompt, persistent badge and update notice for app updates (#163, #213).
 *
 * One dialog per version at launch — never twice, never while Big Picture is
 * open, never on the update center itself. "Later" silences it for a day,
 * "Skip this version" persists the skip so only a newer version prompts again.
 * Closing the dialog never cancels a running install.
 *
 * The in-app notice (#213) is a second, persistent surface: one card per
 * version with the current version and the download size, "Open update center"
 * / "Later" / "Skip this version". Marking it read never skips the version and
 * the badge/update center stay the source of truth. When the desktop window is
 * not in front and the account allows it, the same text becomes an OS
 * notification; a running game and the console shell never get a popup.
 */
export function UpdateNotifier() {
  const {
    status,
    installing,
    installed,
    progress,
    error,
    remindAfter,
    install,
    restart,
    skip,
    remindLater,
  } = useUpdater();
  const location = useLocation();
  const { push } = useToast();
  const promptPreference = useUpdatePrompt();
  const noticePreference = useUpdateNotifications();
  const [open, setOpen] = useState(false);
  const [prompted, setPrompted] = useState<string | null>(() => readPromptedVersion());
  const [notice, setNotice] = useState<UpdateNoticeRecord | null>(() => readUpdateNotice());
  const [noticeVisible, setNoticeVisible] = useState(false);
  // Bumped when a "Later" window ends so the notice can come back without a
  // new update event.
  const [noticeNow, setNoticeNow] = useState(() => Date.now());
  const osRequested = useRef<Set<string>>(new Set());

  const available = status?.available ?? null;
  const version = available?.version ?? "";
  const at = location.pathname;
  const onCenter = at.startsWith("/updates");
  const onBigPicture = at.startsWith("/bigpicture");
  const quietRoute = onCenter || onBigPicture || at === "/setup" || at === "/login";

  useEffect(() => {
    if (!available || quietRoute || open) return;
    // #201: the account preference can silence the launch prompt entirely or
    // keep only the badge. The update center itself is never hidden.
    if (promptPreference !== "always") return;
    if (
      !shouldPrompt({
        status,
        skippedVersion: status?.skippedVersion,
        remindAfter,
        promptedVersion: prompted,
      })
    ) {
      return;
    }
    setOpen(true);
  }, [available, quietRoute, open, remindAfter, prompted, status, promptPreference]);

  /* --------------------------- in-app notice (#213) -------------------------- */

  useEffect(() => {
    if (!available || open) return;
    if (noticeVisible) return;
    if (quietRoute) return;
    const due = shouldShowUpdateNotice({
      status,
      preference: noticePreference,
      notice,
      remindAfter,
      onUpdateCenter: onCenter,
      now: noticeNow,
    });
    if (!due) return;
    // Persist the dedupe before rendering: a later check, a relaunch or a
    // second window must not raise the same notice again.
    markUpdateNoticeShown(available.version);
    setNotice({ version: available.version, state: "shown", until: null });
    setNoticeVisible(true);
  }, [
    available,
    open,
    noticeVisible,
    quietRoute,
    notice,
    remindAfter,
    noticePreference,
    onCenter,
    status,
    noticeNow,
  ]);

  // Let a "Later" window expire without waiting for the next check.
  useEffect(() => {
    if (notice?.state !== "later" || notice.until === null) return;
    const delay = notice.until - Date.now();
    if (delay <= 0) return;
    const timer = window.setTimeout(() => setNoticeNow(Date.now()), delay + 1_000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  // Big Picture is a console surface: the existing in-shell toast carries the
  // news, once per version, and never an OS popup.
  useEffect(() => {
    if (!onBigPicture || !available || open) return;
    if (
      !shouldShowUpdateNotice({
        status,
        preference: noticePreference,
        notice,
        remindAfter,
        onUpdateCenter: onCenter,
        now: noticeNow,
      })
    ) {
      return;
    }
    markUpdateNoticeShown(available.version);
    setNotice({ version: available.version, state: "shown", until: null });
    push({
      title: t("Ludex {version} is available", { version: available.version }),
      description: noticeBody(available.currentVersion, available.sizeBytes),
    });
  }, [
    onBigPicture,
    available,
    open,
    notice,
    remindAfter,
    noticePreference,
    onCenter,
    status,
    push,
    noticeNow,
  ]);

  /* ---------------------------- OS notification (#213) ---------------------- */

  useEffect(() => {
    // The launch dialog may be open behind a window that is in the tray, so the
    // OS decision does not wait for the in-app card: it only needs the notice
    // to be due and the window to be in the background.
    if (!available || !isNative()) return;
    if (
      !shouldShowUpdateNotice({
        status,
        preference: noticePreference,
        notice,
        remindAfter,
        onUpdateCenter: onCenter,
        now: Date.now(),
      })
    ) {
      return;
    }
    const focused = typeof document === "undefined" ? true : document.hasFocus();
    if (
      !shouldSendOsNotification({
        preference: noticePreference,
        focused,
        minimized: false,
      })
    ) {
      return;
    }
    const key = `available:${available.version}`;
    if (osRequested.current.has(key)) return;
    osRequested.current.add(key);
    void native.notifyUpdateAvailable({
      version: available.version,
      title: t("Ludex {version} is available", { version: available.version }),
      body: noticeBody(available.currentVersion, available.sizeBytes),
    });
  }, [available, status, notice, remindAfter, noticePreference, onCenter]);

  // #213: "Ludex {version} installed, restart to use it" gets the same OS
  // treatment when the window is not in front. The in-app toast comes from the
  // provider; the update center holds the restart action the click opens.
  useEffect(() => {
    if (!installed || installed.restarted || !isNative()) return;
    const focused = typeof document === "undefined" ? true : document.hasFocus();
    if (!shouldSendOsNotification({ preference: noticePreference, focused, minimized: false })) {
      return;
    }
    const key = `installed:${installed.version}`;
    if (osRequested.current.has(key)) return;
    osRequested.current.add(key);
    void native.notifyUpdateAvailable({
      version: installed.version,
      kind: "installed",
      title: t("Ludex {version} is installed", { version: installed.version }),
      body: t("Restart Ludex to use it."),
    });
  }, [installed, noticePreference]);

  const dismiss = () => {
    if (installing) return;
    const seen = version || installed?.version || "";
    if (seen) {
      markPrompted(seen);
      setPrompted(seen);
    }
    setOpen(false);
  };

  const later = () => {
    remindLater();
    if (version) {
      const until = markUpdateNoticeLater(version);
      setNotice({ version, state: "later", until });
      setNoticeVisible(false);
    }
    setOpen(false);
  };

  const doSkip = async () => {
    setNoticeVisible(false);
    await skip(version || null);
    setOpen(false);
  };

  const showBadge =
    promptPreference !== "off" &&
    shouldShowBadge(status) &&
    !open &&
    !noticeVisible &&
    !quietRoute &&
    (remindAfter === null || Date.now() >= remindAfter);

  if (!available && !installed) return null;

  const installedVersion = installed?.version ?? available?.version ?? "";

  return (
    <>
      {showBadge && (
        <Link
          to="/updates"
          className="fade-in fixed bottom-[calc(40px+16px)] end-5 z-40 hidden items-center gap-3 rounded-[2px] border border-line-2 bg-surface px-3.5 py-2.5 text-ink shadow-panel md:flex"
          aria-label={t("Update available: Ludex {version}", { version })}
        >
          <span className="grid size-7 shrink-0 place-items-center rounded-[2px] border border-signal/50 bg-signal/12 text-signal-text">
            <Download className="size-3.5" aria-hidden="true" />
          </span>
          <span className="min-w-0">
            <span className="block text-[12px] font-medium leading-tight">{t("Update available")}</span>
            <span className="num block text-[10.5px] leading-tight text-ink-3">{version}</span>
          </span>
          <ArrowRight className="size-3.5 shrink-0 text-ink-3" aria-hidden="true" />
        </Link>
      )}

      {noticeVisible && available && !installed && status?.notify !== false && (
        <UpdateNotice
          version={available.version}
          currentVersion={available.currentVersion}
          sizeBytes={available.sizeBytes}
          disabled={installing}
          onOpen={() => setNoticeVisible(false)}
          onDismiss={() => setNoticeVisible(false)}
          onLater={later}
          onSkip={() => void doSkip()}
        />
      )}

      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!next) dismiss();
        }}
        dismissible={!installing}
        size="lg"
        title={t("Ludex {version} is ready", { version: available?.version ?? installedVersion })}
        description={
          installed
            ? installed.restarted
              ? t("Ludex {version} is installed. The installer restarts the app.", {
                  version: installedVersion,
                })
              : t("Ludex {version} is installed. Restart Ludex to use it.", {
                  version: installedVersion,
                })
            : t("You are on {current}. Nothing changes until you install.", {
                current: available?.currentVersion ?? "",
              })
        }
        footer={
          installed ? (
            installed.restarted ? (
              <Button variant="primary" onClick={dismiss}>
                {t("Done")}
              </Button>
            ) : (
              <>
                <Button variant="ghost" onClick={dismiss}>
                  {t("Later")}
                </Button>
                <Button
                  variant="primary"
                  icon={<RotateCcw className="size-3.5" />}
                  onClick={restart}
                >
                  {t("Restart Ludex")}
                </Button>
              </>
            )
          ) : (
            <>
              <Button variant="ghost" onClick={later} disabled={installing}>
                {t("Later")}
              </Button>
              <Button variant="ghost" onClick={() => void doSkip()} disabled={installing}>
                {t("Skip this version")}
              </Button>
              <Button
                variant="primary"
                loading={installing}
                icon={<Download className="size-4" />}
                onClick={() => void install()}
              >
                {error ? t("Try again") : t("Update now")}
              </Button>
            </>
          )
        }
      >
        {available && (
          <>
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5 font-mono text-[10.5px] uppercase tracking-[0.12em] text-ink-3">
              <span>{t("Installed {version}", { version: available.currentVersion })}</span>
              {available.publishedAt && (
                <>
                  <span aria-hidden="true">·</span>
                  <span>{formatDate(available.publishedAt)}</span>
                </>
              )}
              {available.sizeBytes ? (
                <>
                  <span aria-hidden="true">·</span>
                  <span>{formatBytes(available.sizeBytes)}</span>
                </>
              ) : null}
              {available.isPrerelease && <Badge tone="signal">{t("Edge build")}</Badge>}
            </div>

            <div className="mt-5 max-h-[36vh] overflow-y-auto pe-1">
              <ReleaseNotes notes={available.notes} version={available.version} />
            </div>

            {installing && (
              <div className="mt-5 border border-line bg-surface/60 p-3.5">
                <div className="flex flex-wrap items-baseline justify-between gap-2">
                  <p className="text-[12px] text-ink-2">{t("Downloading and installing")}</p>
                  <p className="num text-[10.5px] text-ink-3">
                    {progress?.percent !== null && progress?.percent !== undefined
                      ? `${Math.round(progress.percent)}%`
                      : t("Starting")}
                  </p>
                </div>
                <Progress value={progress?.percent ?? null} label={t("Update download")} className="mt-3" />
                <p className="num mt-2 text-[10.5px] text-ink-3">
                  {formatBytes(progress?.downloaded ?? 0)}
                  {progress?.total ? ` / ${formatBytes(progress.total)}` : ""}
                  {progress?.speedBps ? ` · ${formatSpeed(progress.speedBps)}` : ""}
                </p>
              </div>
            )}

            {error && !installing && (
              <p className="mt-4 border border-danger/45 bg-danger/10 p-3 text-[12px] leading-relaxed text-danger" role="alert">
                {error}
              </p>
            )}

            <div className="mt-5 flex flex-wrap items-center justify-between gap-3 border-t border-line pt-4">
              <ExternalAnchor href={available.releaseUrl} className="text-[12px]">
                {t("Release page")}
                <ArrowRight className="size-3.5" aria-hidden="true" />
              </ExternalAnchor>
              <span className="text-[11px] text-ink-3">
                {t("Signed build. Game files are never part of an app update.")}
              </span>
            </div>
          </>
        )}
      </Dialog>
    </>
  );
}
