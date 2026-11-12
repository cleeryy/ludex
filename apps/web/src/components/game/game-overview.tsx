import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { ArrowRight, Download, HardDrive, Link2 } from "lucide-react";
import { cn } from "@/lib/cn";
import { formatBytes, formatDate, formatMinutes, formatRelative, formatSeconds } from "@/lib/format";
import { gameSectionPath } from "@/lib/game-sections";
import { isNative } from "@/lib/native";
import type { GameDetail, GamePlaytime, GameUpdate, GameVersion, InstalledGame } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { Badge, Button } from "@/components/ui/controls";
import { SettingsGroup } from "@/components/settings/settings-section";
import { t } from "@/lib/i18n";

/**
 * Overview group (#189): the facts that used to live in the 320px aside, the
 * install state and the progress. No second state editor — the hero picker owns
 * the progress state — and no second CTA when the game is installed.
 */
export function GameOverview({
  game,
  installed,
  update,
  preferredVersion,
  playtime,
  onInstall,
  onDownload,
  onLinkExisting,
  remoteInstall,
}: {
  game: GameDetail;
  installed?: InstalledGame;
  update?: GameUpdate;
  preferredVersion: GameVersion | null;
  playtime?: GamePlaytime;
  /** Desktop: open the install dialog. */
  onInstall: () => void;
  /** Browser: start the archive download. */
  onDownload: () => void;
  onLinkExisting: () => void;
  /** Browser install-on-the-desktop action, when the account has one. */
  remoteInstall?: ReactNode;
}) {
  const { user } = useAuth();
  const native = isNative();
  const totalPlayed = formatSeconds(playtime?.totalSeconds) ?? formatMinutes(game.minutesPlayed);

  return (
    <div className="flex flex-col gap-8">
      <SettingsGroup title={t("Install status")}>
        {installed ? (
          <dl className="border-t border-line">
            <Fact label={t("Status")}>
              <span className="flex items-center gap-1.5">
                <span
                  className={cn("size-1.5 rounded-full", installed.source === "adopted" ? "bg-signal" : "bg-ok")}
                  aria-hidden="true"
                />
                {t("Installed")}
                <span className="num text-ink-3">{formatBytes(installed.sizeBytes)}</span>
              </span>
            </Fact>
            <Fact label={t("Version")}>
              <span className="flex flex-wrap items-center justify-end gap-2">
                <span className="num">{installed.version ? `v${installed.version}` : t("this build")}</span>
                {update && (
                  <Badge tone="signal">
                    {update.latestVersionLabel ? t("Update to {version}", { version: `v${update.latestVersionLabel}` }) : t("Update available")}
                  </Badge>
                )}
              </span>
            </Fact>
            <Fact label={t("Source")}>
              {installed.source === "adopted" ? (
                <Badge tone="signal">{t("Adopted")}</Badge>
              ) : installed.source === "ludex" || !installed.source ? (
                <Badge tone="outline">{t("Ludex")}</Badge>
              ) : (
                <span className="truncate">{installed.source}</span>
              )}
            </Fact>
            <Fact label={t("Location")}>
              <span className="truncate font-mono text-[11px]" title={installed.installPath}>
                {installed.installPath}
              </span>
            </Fact>
          </dl>
        ) : (
          <div className="border-t border-line pt-4">
            <p className="flex items-center gap-2 text-[13.5px] text-ink-2">
              <HardDrive className="size-4 text-ink-3" aria-hidden="true" />
              {t("Not installed on this device")}
            </p>
            <p className="mt-1 max-w-[62ch] text-[11.5px] leading-relaxed text-ink-3">
              {preferredVersion
                ? t("Installing uses {version} ({size}). Your progress, saves and achievements stay on the server.", {
                    version: preferredVersion.version ? `v${preferredVersion.version}` : preferredVersion.fileName,
                    size: formatBytes(preferredVersion.size),
                  })
                : t("There is no archive on the server yet. An administrator can add one in the ingest screen.")}
            </p>
            <div className="mt-3 flex flex-wrap items-center gap-2">
              {native ? (
                <Button variant="primary" icon={<Download className="size-4" />} onClick={onInstall}>
                  {t("Install")}
                </Button>
              ) : (
                <Button variant="primary" icon={<Download className="size-4" />} onClick={onDownload}>
                  {t("Download")}
                </Button>
              )}
              {native && (
                <Button variant="ghost" icon={<Link2 className="size-4" />} onClick={onLinkExisting}>
                  {t("Link existing installation")}
                </Button>
              )}
            </div>
            {remoteInstall && <div className="mt-3">{remoteInstall}</div>}
          </div>
        )}
        {game.kind !== "rom" && (
          <p className="mt-3">
            <Link
              to={gameSectionPath(game.id, "files")}
              className="inline-flex items-center gap-1.5 text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
            >
              {t("Manage files")}
              <ArrowRight className="rtl:-scale-x-100 size-3" aria-hidden="true" />
            </Link>
          </p>
        )}
      </SettingsGroup>

      <SettingsGroup title={t("Details")}>
        <dl className="border-t border-line">
          <Fact label={t("Developer")}>{game.developers.join(", ") || "—"}</Fact>
          <Fact label={t("Publisher")}>{game.publishers.join(", ") || "—"}</Fact>
          <Fact label={t("Released")}>{formatDate(game.releaseDate)}</Fact>
          <Fact label={t("Genres")}>{game.genres.join(", ") || "—"}</Fact>
          <Fact label={t("Platform")}>{game.platform ?? "—"}</Fact>
          <Fact label={t("Age rating")}>{game.ageRating ? `${game.ageRating}+` : t("Not rated")}</Fact>
        </dl>
        {game.tags.length > 0 && (
          <div className="mt-3 flex flex-wrap gap-1.5">
            {game.tags.map((tag) => (
              <span key={tag} className="border border-line px-2 py-1 text-[11px] text-ink-3">
                {tag}
              </span>
            ))}
          </div>
        )}
      </SettingsGroup>

      <SettingsGroup title={t("Your progress")}>
        <dl className="border-t border-line">
          <Fact label={t("Playtime")}>
            <span className="num">{totalPlayed ?? t("Not played")}</span>
          </Fact>
          <Fact label={t("Last session")}>{formatRelative(game.lastPlayedAt)}</Fact>
          {native && playtime && playtime.sessions > 0 && (
            <Fact label={t("Sessions on this device")}>
              <span className="num">{playtime.sessions}</span>
            </Fact>
          )}
        </dl>
        {native && playtime && playtime.sessions > 0 && (
          <ul className="border-t border-line">
            {playtime.recentSessions.slice(0, 5).map((session) => (
              <li
                key={session.id}
                className="flex items-baseline justify-between gap-3 border-b border-line py-1.5 font-mono text-[10.5px] text-ink-3"
              >
                <span>{formatDate(session.startedAt)}</span>
                <span className="text-ink-2">{formatSeconds(session.seconds) ?? `${session.minutes}m`}</span>
              </li>
            ))}
          </ul>
        )}
        {user && (
          <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
            {t("Progress is stored per account on the server.")}
          </p>
        )}
      </SettingsGroup>
    </div>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
      <dt className="shrink-0 text-[12px] text-ink-3">{label}</dt>
      <dd className="min-w-0 text-right text-[12.5px] text-ink-2">{children}</dd>
    </div>
  );
}
