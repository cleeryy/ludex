import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowUpRight,
  Check,
  CircleAlert,
  Download,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
} from "lucide-react";
import { api } from "@/lib/api";
import { formatBytes, formatDate, formatSpeed } from "@/lib/format";
import { formatRelativeTime, t } from "@/lib/i18n";
import { isNative } from "@/lib/native";
import {
  channelNeedsConfirmation,
  isVersionSkipped,
  RELEASES_URL,
  type UpdateChannel,
} from "@/lib/updates";
import { useUpdater } from "@/components/updates/update-provider";
import { ExternalAnchor, ReleaseNotes } from "@/components/updates/parts";
import { Badge, Button, Progress, Segmented, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";

/**
 * Update center (#163).
 *
 * One page answers "what changed, on which channel, how do I install it":
 * current version, channel switch with an explicit early warning, manual check,
 * available version with sanitised release notes, download progress, restart
 * path, specific errors and a local skip. The browser build explains that
 * updates are desktop-only instead of pretending to check.
 */
export function UpdateCenter() {
  return isNative() ? <DesktopUpdateCenter /> : <BrowserUpdateNotes />;
}

function DesktopUpdateCenter() {
  const {
    status,
    loading,
    checking,
    installing,
    installed,
    progress,
    error,
    check,
    install,
    restart,
    setChannel,
    skip,
    remindLater,
    clearError,
  } = useUpdater();
  const [confirmEarly, setConfirmEarly] = useState(false);
  const [pendingChannel, setPendingChannel] = useState<UpdateChannel | null>(null);
  const initialCheck = useRef(false);

  // Opening the update center is a request to know where the install stands.
  // The first visit runs one check when no check has ever completed; later
  // visits respect the manual button and the 24 h startup throttle.
  useEffect(() => {
    if (loading || !status || status.lastCheckedAt || error || initialCheck.current) return;
    initialCheck.current = true;
    void check();
  }, [loading, status, error, check]);

  const available = status?.available ?? null;
  const skipped = isVersionSkipped(available?.version, status?.skippedVersion);
  const configured = status?.configured ?? true;

  const chooseChannel = (next: UpdateChannel) => {
    if (!status || next === status.channel) return;
    if (channelNeedsConfirmation(status.channel, next)) {
      setPendingChannel(next);
      setConfirmEarly(true);
      return;
    }
    void setChannel(next);
  };

  if (loading) {
    return (
      <div className="flex flex-col gap-6">
        <Skeleton className="h-4 w-56" />
        <Skeleton className="h-24 w-full" />
        <Skeleton className="h-40 w-full" />
      </div>
    );
  }

  if (!status) {
    return (
      <div className="flex flex-col items-start gap-3 border border-line bg-surface/50 p-6" role="alert">
        <p className="text-[13.5px] text-ink-2">{t("Ludex could not read its updater state.")}</p>
        <Button variant="default" size="sm" loading={checking} onClick={() => void check()}>
          {t("Try again")}
        </Button>
      </div>
    );
  }

  return (
    <div className="grid gap-12 lg:grid-cols-[minmax(0,1fr)_300px] lg:gap-14">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line pb-4">
          <p className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3">
            {status?.lastCheckedAt
              ? t("Last check {when}", { when: formatRelativeTime(status.lastCheckedAt) })
              : t("Not checked yet")}
          </p>
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            loading={checking}
            onClick={() => void check()}
          >
            {t("Check now")}
          </Button>
        </div>

        {error && (
          <div className="mt-5 flex flex-wrap items-start gap-3 border border-danger/45 bg-danger/10 p-4" role="alert">
            <CircleAlert className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden="true" />
            <div className="min-w-0 flex-1">
              <p className="text-[12.5px] font-medium text-danger">{t("The update check could not finish")}</p>
              <p className="measure mt-1 text-[12px] leading-relaxed text-ink-2">{error}</p>
            </div>
            <div className="flex gap-2">
              <Button variant="ghost" size="sm" onClick={clearError}>
                {t("Dismiss")}
              </Button>
              <Button variant="default" size="sm" loading={checking} onClick={() => void check()}>
                {t("Try again")}
              </Button>
            </div>
          </div>
        )}

        {!configured && (
          <div className="mt-6 border border-line-2 bg-surface/60 p-5">
            <Badge tone="danger">{t("Signing key missing")}</Badge>
            <p className="measure mt-3 text-[13px] leading-relaxed text-ink-2">
              {status?.message ??
                t("This build cannot install updates because it has no release signing key.")}
            </p>
            <ExternalAnchor href={`${RELEASES_URL}/latest`} className="mt-3 text-[12.5px]">
              {t("Download a signed release")}
              <ArrowUpRight className="size-3.5" aria-hidden="true" />
            </ExternalAnchor>
          </div>
        )}

        {installed && (
          <div className="mt-6 flex flex-wrap items-center gap-3 border border-ok/40 bg-ok/10 p-4">
            <Check className="size-4 shrink-0 text-ok" aria-hidden="true" />
            <p className="min-w-0 flex-1 text-[13px] text-ink-2">
              {installed.restarted
                ? t("Ludex {version} is installed. The installer restarts the app.", { version: installed.version })
                : t("Ludex {version} is installed. Restart Ludex to use it.", { version: installed.version })}
            </p>
            {!installed.restarted && (
              <Button
                variant="primary"
                size="sm"
                icon={<RotateCcw className="size-3.5" />}
                onClick={restart}
              >
                {t("Restart Ludex")}
              </Button>
            )}
          </div>
        )}

        {available && configured ? (
          <section className="pt-8">
            <div className="flex flex-wrap items-end justify-between gap-x-8 gap-y-4">
              <div>
                <p className="font-mono text-[10.5px] uppercase tracking-[0.16em] text-signal-text">
                  {t("Available now")}
                </p>
                <div className="mt-3 flex flex-wrap items-baseline gap-3">
                  <h2 className="font-display text-[clamp(2.75rem,6vw,4.5rem)] leading-[0.85] tracking-[-0.045em]">
                    {available.version}
                  </h2>
                  {available.isPrerelease && <Badge tone="signal">{t("Edge build")}</Badge>}
                </div>
                <p className="mt-3 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12.5px] text-ink-3">
                  <span>{t("Installed {version}", { version: available.currentVersion })}</span>
                  {available.publishedAt && (
                    <>
                      <span aria-hidden="true">·</span>
                      <span>{t("Published {date}", { date: formatDate(available.publishedAt) })}</span>
                    </>
                  )}
                </p>
              </div>
              <ExternalAnchor href={available.releaseUrl} className="text-[12.5px]">
                {t("Release page")}
                <ArrowUpRight className="size-3.5" aria-hidden="true" />
              </ExternalAnchor>
            </div>

            {skipped && (
              <div className="mt-6 flex flex-wrap items-center justify-between gap-3 border border-line-2 bg-surface/60 p-3.5">
                <p className="text-[12px] leading-relaxed text-ink-2">
                  {t("You skipped this version, so Ludex will not prompt you about it again. Installing still works.")}
                </p>
                <Button variant="ghost" size="sm" onClick={() => void skip(null)}>
                  {t("Stop skipping")}
                </Button>
              </div>
            )}

            <div className="mt-7 border-t border-line pt-6">
              <h3 className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3">
                {t("What changed")}
              </h3>
              <ReleaseNotes notes={available.notes} version={available.version} className="mt-4" />
            </div>

            <div className="mt-8 flex flex-wrap items-center gap-2.5">
              <Button
                variant="primary"
                loading={installing}
                icon={<Download className="size-4" />}
                onClick={() => void install()}
              >
                {installed ? t("Install again") : t("Update now")}
              </Button>
              <Button variant="ghost" onClick={remindLater}>
                {t("Remind me later")}
              </Button>
              {!skipped && (
                <Button variant="ghost" onClick={() => void skip(available.version)}>
                  {t("Skip this version")}
                </Button>
              )}
            </div>
            <p className="mt-2.5 flex items-center gap-1.5 text-[11px] text-ink-3">
              <ShieldCheck className="size-3.5 shrink-0" aria-hidden="true" />
              {t("Every release is signature-checked before it replaces Ludex. Game files and saves are never touched.")}
            </p>

            {installing && (
              <div className="mt-6 border border-line bg-surface/60 p-4">
                <div className="flex flex-wrap items-baseline justify-between gap-2">
                  <p className="text-[12.5px] text-ink-2">{t("Downloading and installing")}</p>
                  <p className="num text-[11px] text-ink-3">
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
          </section>
        ) : (
          configured && (
            <section className="pt-10">
              <p className="font-mono text-[10.5px] uppercase tracking-[0.16em] text-ok">
                {t("Up to date")}
              </p>
              <h2 className="mt-3 max-w-[18ch] font-display text-[clamp(2.5rem,5vw,4rem)] leading-[0.92] tracking-[-0.04em]">
                {t("You’re on the newest version.")}
              </h2>
              <p className="measure mt-4 text-[13.5px] leading-relaxed text-ink-3">
                {t("No newer build is published on this channel right now. The next automatic check runs at most once a day.")}
              </p>
              <div className="mt-6 flex flex-wrap items-center gap-2.5">
                <Button
                  variant="default"
                  size="sm"
                  icon={<RefreshCw className="size-3.5" />}
                  loading={checking}
                  onClick={() => void check()}
                >
                  {t("Check again")}
                </Button>
                <ExternalAnchor href={`${RELEASES_URL}/latest`} className="text-[12.5px]">
                  {t("Browse releases")}
                  <ArrowUpRight className="size-3.5" aria-hidden="true" />
                </ExternalAnchor>
              </div>
            </section>
          )
        )}
      </div>

      <aside className="flex flex-col gap-8 lg:border-s lg:border-line lg:ps-8">
        <section>
          <h2 className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3">
            {t("Channel")}
          </h2>
          <div className="mt-3">
            <Segmented
              value={status?.channel ?? "stable"}
              onChange={chooseChannel}
              label={t("Update channel")}
              options={[
                { value: "stable", label: t("Stable") },
                { value: "early", label: t("Early access") },
              ]}
            />
          </div>
          <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
            {status?.channel === "early"
              ? t("Early access follows the edge prerelease, built from every passing commit on main.")
              : t("Stable follows tagged releases. This is the right channel for playing, not testing.")}
          </p>
          {status?.channel === "early" && (
            <p className="mt-2 border-s-2 border-danger/60 ps-2.5 text-[11.5px] leading-relaxed text-danger">
              {t("Edge builds can break launches, saves and provider integrations. Switch back to stable if something feels wrong.")}
            </p>
          )}
          <ExternalAnchor href={RELEASES_URL} className="mt-3 text-[11.5px]">
            {t("Release history")}
            <ArrowUpRight className="size-3.5" aria-hidden="true" />
          </ExternalAnchor>
        </section>

        <section className="border-t border-line pt-6">
          <h2 className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3">
            {t("This install")}
          </h2>
          <dl className="mt-3 flex flex-col gap-2 text-[12px]">
            <div className="flex items-baseline justify-between gap-3">
              <dt className="text-ink-3">{t("Version")}</dt>
              <dd className="num text-ink-2">{status?.currentVersion ?? "—"}</dd>
            </div>
            <div className="flex items-baseline justify-between gap-3">
              <dt className="text-ink-3">{t("Channel")}</dt>
              <dd className="num text-ink-2">{status?.channel ?? "stable"}</dd>
            </div>
            <div className="flex items-baseline justify-between gap-3">
              <dt className="text-ink-3">{t("Last check")}</dt>
              <dd className="text-end text-ink-2">
                {status?.lastCheckedAt ? formatDate(status.lastCheckedAt) : t("Never")}
              </dd>
            </div>
            <div className="flex items-baseline justify-between gap-3">
              <dt className="text-ink-3">{t("Signing")}</dt>
              <dd>
                <Badge tone={configured ? "ok" : "danger"}>
                  {configured ? t("Verified releases") : t("Key missing")}
                </Badge>
              </dd>
            </div>
          </dl>
        </section>

        <section className="border-t border-line pt-6">
          <h2 className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3">
            {t("Source")}
          </h2>
          <p className="mt-3 break-all font-mono text-[10.5px] leading-relaxed text-ink-3">
            {status?.endpoint ?? "—"}
          </p>
          {status?.endpoint && (
            <ExternalAnchor href={status.endpoint} className="mt-2 text-[11.5px]">
              {t("Open latest.json")}
              <ArrowUpRight className="size-3.5" aria-hidden="true" />
            </ExternalAnchor>
          )}
          <p className="mt-3 text-[11px] leading-relaxed text-ink-3">
            {t("GitHub serves the channel manifest. Ludex verifies the signature of every download before installing it.")}
          </p>
        </section>
      </aside>

      <ConfirmDialog
        open={confirmEarly}
        onOpenChange={(open) => {
          setConfirmEarly(open);
          if (!open) setPendingChannel(null);
        }}
        title={t("Switch to early access?")}
        description={t("Early builds come from the edge prerelease and can break launches, saves or provider integrations. You can switch back to stable at any time. Ludex re-checks for updates as soon as you switch.")}
        confirmLabel={t("Switch to early access")}
        tone="primary"
        onConfirm={() => {
          const next = pendingChannel ?? "early";
          setConfirmEarly(false);
          setPendingChannel(null);
          void setChannel(next);
        }}
      />
    </div>
  );
}

function BrowserUpdateNotes() {
  const health = useQuery({
    queryKey: ["health"],
    queryFn: () => api.health(),
    staleTime: 60_000,
    retry: false,
  });

  return (
    <div className="grid gap-10 lg:grid-cols-[minmax(0,1fr)_300px] lg:gap-14">
      <section>
        <Badge tone="outline">{t("Desktop app")}</Badge>
        <h2 className="mt-4 max-w-[20ch] font-display text-[clamp(2.25rem,4.5vw,3.5rem)] leading-[0.95] tracking-[-0.04em]">
          {t("Ludex updates itself in the desktop app.")}
        </h2>
        <p className="measure mt-4 text-[14px] leading-relaxed text-ink-2">
          {t("A browser cannot replace the Ludex application or its signature check. Install the desktop build once, then every new version, its notes and the channel switch live in this same update center.")}
        </p>
        <div className="mt-7 flex flex-wrap items-center gap-2.5">
          <ExternalAnchor
            href={`${RELEASES_URL}/latest`}
            className="inline-flex h-11 items-center gap-2 border border-signal bg-signal px-4 text-[14px] font-semibold text-on-signal no-underline transition-[filter] hover:brightness-[1.07]"
          >
            <Download className="size-4" aria-hidden="true" />
            {t("Download Ludex")}
          </ExternalAnchor>
          <ExternalAnchor href={RELEASES_URL} className="text-[12.5px]">
            {t("Release notes and downloads")}
            <ArrowUpRight className="size-3.5" aria-hidden="true" />
          </ExternalAnchor>
        </div>

        <div className="mt-10 border-t border-line pt-6">
          <h3 className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3">
            {t("This browser session")}
          </h3>
          <dl className="mt-3 flex max-w-[420px] flex-col gap-2 text-[12.5px]">
            <div className="flex items-baseline justify-between gap-6">
              <dt className="text-ink-3">{t("Current version")}</dt>
              <dd className="num text-ink-2">{health.data?.version ?? "—"}</dd>
            </div>
            <div className="flex items-baseline justify-between gap-6">
              <dt className="text-ink-3">{t("Updates")}</dt>
              <dd className="text-ink-2">{t("Desktop app only")}</dd>
            </div>
          </dl>
          <p className="measure mt-3 text-[11px] leading-relaxed text-ink-3">
            {t("The browser always runs the build your server serves; a new page build arrives on the next reload. This is separate from the desktop app's version.")}
          </p>
        </div>
      </section>

      <aside className="flex flex-col gap-6 lg:border-s lg:border-line lg:ps-8">
        <section>
          <h2 className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3">
            {t("How desktop updates work")}
          </h2>
          <ol className="mt-3 flex flex-col gap-3 text-[12px] leading-relaxed text-ink-2">
            {[
              t("Ludex checks the stable or early channel at most once a day."),
              t("A new version shows its notes and date before anything is downloaded."),
              t("The download is checked against the release signature, then Ludex restarts."),
              t("Game files and saves are never part of an app update."),
            ].map((step, index) => (
              <li key={step} className="flex gap-3">
                <span className="num mt-[0.15em] shrink-0 text-[10.5px] text-ink-3">
                  {String(index + 1).padStart(2, "0")}
                </span>
                <span>{step}</span>
              </li>
            ))}
          </ol>
        </section>
        <p className="border-t border-line pt-5 text-[11px] leading-relaxed text-ink-3">
          {t("Releases are published from GitHub and signed. Issues and screenshots are welcome on the release page.")}
        </p>
      </aside>
    </div>
  );
}
