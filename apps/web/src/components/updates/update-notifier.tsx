import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { ArrowRight, Download, RotateCcw } from "lucide-react";
import { formatBytes, formatDate, formatSpeed } from "@/lib/format";
import { t } from "@/lib/i18n";
import {
  markPrompted,
  readPromptedVersion,
  shouldPrompt,
  shouldShowBadge,
} from "@/lib/updates";
import { useUpdater } from "@/components/updates/update-provider";
import { ExternalAnchor, ReleaseNotes } from "@/components/updates/parts";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";

/**
 * Launch prompt and persistent badge for app updates (#163).
 *
 * One dialog per version at launch — never twice, never while Big Picture is
 * open, never on the update center itself. "Later" silences it for a day,
 * "Skip this version" persists the skip so only a newer version prompts again.
 * Closing the dialog never cancels a running install.
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
  const [open, setOpen] = useState(false);
  const [prompted, setPrompted] = useState<string | null>(() => readPromptedVersion());

  const available = status?.available ?? null;
  const version = available?.version ?? "";
  const at = location.pathname;
  const onCenter = at.startsWith("/updates");
  const quietRoute = onCenter || at.startsWith("/bigpicture") || at === "/setup" || at === "/login";

  useEffect(() => {
    if (!available || quietRoute || open) return;
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
  }, [available, version, quietRoute, open, remindAfter, prompted, status]);

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
    setOpen(false);
  };

  const doSkip = async () => {
    await skip(version || null);
    setOpen(false);
  };

  const showBadge =
    shouldShowBadge(status) &&
    !open &&
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
