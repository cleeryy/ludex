import { Link } from "react-router-dom";
import { ArrowRight, RefreshCw } from "lucide-react";
import { formatRelativeTime, t } from "@/lib/i18n";
import { isNative } from "@/lib/native";
import { isVersionSkipped } from "@/lib/updates";
import { useUpdater } from "@/components/updates/update-provider";
import { Badge, Button } from "@/components/ui/controls";

/**
 * Compact update status in Settings (#163).
 *
 * All the explaining, note reading, channel switching and installing happens on
 * `/updates`; Settings keeps one honest line and a way in, so the page does not
 * grow a second updater UI.
 */
export function DesktopUpdatePanel() {
  const { status, checking, error, check } = useUpdater();
  const desktop = isNative();
  const available = status?.available ?? null;
  const skipped = isVersionSkipped(available?.version, status?.skippedVersion);
  const configured = status?.configured ?? true;
  // A fresh install has never checked: "up to date" would be a lie.
  const unchecked = desktop && (!status || (!status.lastCheckedAt && !available));

  const badge = !desktop ? (
    <Badge tone="outline">{t("Desktop only")}</Badge>
  ) : !configured ? (
    <Badge tone="danger">{t("Signing key missing")}</Badge>
  ) : skipped && available ? (
    <Badge tone="outline">{t("Skipped")}</Badge>
  ) : available ? (
    <Badge tone="signal">{t("Update available")}</Badge>
  ) : unchecked ? (
    <Badge tone="outline">{t("Not checked yet")}</Badge>
  ) : (
    <Badge tone="ok">{t("Up to date")}</Badge>
  );

  return (
    <div className="border-b border-line py-4">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-[13px] font-medium text-ink">{t("App updates")}</p>
            {badge}
          </div>
          <p className="mt-1 max-w-[56ch] text-[11.5px] leading-relaxed text-ink-3">
            {desktop
              ? t("Version {version} on the {channel} channel. Release notes, the channel switch, skipping and installing live in the update center.", {
                  version: status?.currentVersion ?? "—",
                  channel: status?.channel === "early" ? t("early access") : t("stable"),
                })
              : t("App updates need the desktop build: the browser always runs the build your server serves. Release notes and downloads are on GitHub.")}
          </p>
          {desktop && (
            <p className="mt-1 font-mono text-[10.5px] text-ink-3">
              {status?.lastCheckedAt
                ? t("Last check {when}", { when: formatRelativeTime(status.lastCheckedAt) })
                : t("Not checked yet")}
              {available ? ` · ${available.version}` : ""}
            </p>
          )}
          {error && <p className="mt-1 text-[11.5px] text-danger">{error}</p>}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {desktop && (
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw className="size-3.5" />}
              loading={checking}
              onClick={() => void check()}
            >
              {t("Check now")}
            </Button>
          )}
          <Link to="/updates">
            <Button variant="default" size="sm" iconRight={<ArrowRight className="size-3.5" />}>
              {t("Open update center")}
            </Button>
          </Link>
        </div>
      </div>
    </div>
  );
}
