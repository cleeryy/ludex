import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  ArrowRight,
  Cloud,
  Download as DownloadIcon,
  ExternalLink,
  Gamepad2,
  HardDrive,
  KeyRound,
  Monitor,
  ShieldCheck,
  Smartphone,
} from "lucide-react";
import { api } from "@/lib/api";
import { formatBytes, formatDate } from "@/lib/format";
import { useAuth } from "@/state/auth";
import { Wordmark } from "@/components/brand";
import { SectionHeader } from "@/components/page";
import { Badge, Skeleton } from "@/components/ui/controls";
import { t } from "@/lib/i18n";
import {
  assetKindLabel,
  assetsForPlatform,
  availablePlatforms,
  detectDesktopPlatform,
  isMobileBrowser,
  platformName,
  preferredAsset,
  primaryDownload,
  shortSha256,
} from "@/lib/desktop-download";
import type { DesktopAsset, DesktopPlatform } from "@/lib/types";

const PRIMARY_LINK =
  "inline-flex h-12 select-none items-center justify-center gap-2 rounded-[2px] border border-signal bg-signal px-5 text-[15px] font-semibold text-on-signal transition-[filter] duration-[120ms] hover:brightness-[1.07] active:brightness-95";
const SECONDARY_LINK =
  "inline-flex h-12 select-none items-center justify-center gap-2 rounded-[2px] border border-line-2 bg-surface px-5 text-sm text-ink transition-colors duration-[120ms] hover:border-ink-3 hover:bg-surface-2";

function serverAddress(): string {
  if (typeof window === "undefined") return "";
  try {
    const url = new URL(window.location.href);
    url.hash = "";
    url.search = "";
    return url.toString();
  } catch {
    return "";
  }
}

function ExternalLinkNote({ children }: { children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1">
      {children}
      <ExternalLink className="size-3.5 shrink-0" aria-hidden="true" />
      <span className="sr-only">{t("Opens in a new tab")}</span>
    </span>
  );
}

/**
 * Public download page (#164), reachable logged out at `#/download`.
 *
 * It lists the installers the latest stable release publishes, highlights the
 * one matching the visitor's OS, and always offers the release page so an
 * offline or rate-limited server never leaves a dead end. Nothing here needs an
 * account: installs, cloud saves and controller support are what the desktop
 * app adds, so the page explains that before asking anyone to sign in.
 */
export function DownloadScreen() {
  const { user } = useAuth();
  const [userAgent] = useState(() => (typeof navigator === "undefined" ? "" : navigator.userAgent));
  const platform = useMemo(() => detectDesktopPlatform(userAgent), [userAgent]);
  const mobile = useMemo(() => isMobileBrowser(userAgent), [userAgent]);
  const address = useMemo(serverAddress, []);

  const release = useQuery({
    queryKey: desktopDownloadQueryKey(platform),
    queryFn: () => api.desktopDownload(platform ?? undefined),
    staleTime: 5 * 60_000,
    retry: 1,
  });

  const data = release.data;
  const primary = primaryDownload(data, platform);
  const platforms = data ? availablePlatforms(data.assets) : [];
  const offline = data?.source === "fallback";

  return (
    <div className="min-h-dvh bg-bg text-ink">
      <header className="page flex flex-wrap items-center justify-between gap-3 py-5">
        <Link to="/" aria-label={t("Ludex home")} className="py-1">
          <Wordmark height={18} />
        </Link>
        <nav aria-label={t("Account")} className="flex items-center gap-4 text-[12.5px] text-ink-2">
          <Link to={user ? "/" : "/login"} className="underline decoration-line-2 underline-offset-4 hover:text-ink">
            {user ? t("Open the web app") : t("Sign in")}
          </Link>
        </nav>
      </header>

      <main className="page pb-16">
        <section aria-labelledby="download-heading" className="border-b border-line pb-10 pt-6">
          <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
            {t("Ludex desktop · stable channel")}
          </p>
          <h1
            id="download-heading"
            className="mt-4 max-w-[22ch] text-[clamp(2.4rem,5vw,4.5rem)] font-semibold leading-[0.94] tracking-[-0.035em]"
          >
            {t("Play your library on your PC.")}
          </h1>
          <p className="measure mt-5 text-[15px] leading-relaxed text-ink-2">
            {t(
              "The browser shows your shelf. The desktop app is what installs, launches and updates your games, keeps cloud saves in sync and talks to your controller.",
            )}
          </p>

          <div className="mt-7 flex flex-wrap items-center gap-3">
            {primary ? (
              <a href={primary.href} rel="noreferrer" className={PRIMARY_LINK}>
                <DownloadIcon className="size-4" aria-hidden="true" />
                {primary.asset
                  ? platform
                    ? t("Download for {platform}", { platform: platformName(platform) })
                    : t("Download {platform}", { platform: platformName(primary.asset.platform) })
                  : t("Open the release page")}
              </a>
            ) : (
              <Skeleton className="h-12 w-56" />
            )}
            <Link to={user ? "/" : "/login"} className={SECONDARY_LINK}>
              {user ? t("Use the web app instead") : t("Sign in to the server")}
            </Link>
          </div>

          <div className="mt-5 flex flex-wrap items-center gap-x-4 gap-y-2 font-mono text-[11px] uppercase tracking-[0.06em] text-ink-3">
            {data?.version && <span>{`v${data.version}`}</span>}
            {data?.releasedAt && <span>{t("Released {date}", { date: formatDate(data.releasedAt) })}</span>}
            <span>{t("Stable channel")}</span>
            {data?.stale && <Badge tone="outline">{t("Cached release data")}</Badge>}
          </div>

          {offline && (
            <p role="status" className="mt-5 max-w-[68ch] border-l-2 border-line-2 pl-3 text-[12.5px] leading-relaxed text-ink-2">
              {t("The release feed is unavailable right now. Download from the release page instead; the links below will fill in once the server can reach it.")}
            </p>
          )}
        </section>

        {mobile && (
          <section aria-labelledby="mobile-heading" className="border-b border-line py-8">
            <div className="flex items-start gap-4 border border-line bg-surface p-5">
              <Smartphone className="mt-0.5 size-5 shrink-0 text-ink-3" aria-hidden="true" />
              <div className="min-w-0">
                <h2 id="mobile-heading" className="text-[15px] font-medium">
                  {t("Installs need a PC")}
                </h2>
                <p className="mt-1 max-w-[65ch] text-[12.5px] leading-relaxed text-ink-3">
                  {t("Open this address on your computer to install games: the desktop app unpacks archives and launches them, which a phone browser cannot do.")}
                </p>
                <code className="mt-3 block overflow-x-auto whitespace-nowrap border border-line bg-bg px-3 py-2 font-mono text-[12px] text-ink-2">
                  {address}
                </code>
              </div>
            </div>
          </section>
        )}

        <section aria-labelledby="features-heading" className="border-b border-line py-10">
          <SectionHeader
            id="features-heading"
            title={t("What the desktop app adds")}
            description={t("Everything the browser cannot do on its own.")}
          />
          <ul className="grid gap-px border border-line bg-line sm:grid-cols-2">
            {[
              {
                icon: <HardDrive className="size-4" aria-hidden="true" />,
                title: t("Install and launch games"),
                body: t("Unpacks archives, runs installers and launches each game with your settings."),
              },
              {
                icon: <Cloud className="size-4" aria-hidden="true" />,
                title: t("Cloud saves"),
                body: t("Snapshots your save folders and keeps them with your account on the server."),
              },
              {
                icon: <Gamepad2 className="size-4" aria-hidden="true" />,
                title: t("Controllers and Big Picture"),
                body: t("Full-screen library navigation and controller profiles for the couch."),
              },
              {
                icon: <ShieldCheck className="size-4" aria-hidden="true" />,
                title: t("Signed updates"),
                body: t("Updates itself from the signed release feed, without reinstalling."),
              },
            ].map((feature) => (
              <li key={feature.title} className="bg-bg p-5">
                <div className="flex items-center gap-2.5">
                  <span className="text-ink-3">{feature.icon}</span>
                  <h3 className="text-[14px] font-medium">{feature.title}</h3>
                </div>
                <p className="mt-2 text-[12.5px] leading-relaxed text-ink-3">{feature.body}</p>
              </li>
            ))}
          </ul>
        </section>

        <section aria-labelledby="installers-heading" className="border-b border-line py-10">
          <SectionHeader
            id="installers-heading"
            title={t("All installers")}
            description={
              data?.version
                ? t("Version {version}, published {date}.", {
                    version: data.version,
                    date: formatDate(data.releasedAt),
                  })
                : t("Every platform published by the latest stable release.")
            }
            actions={
              data?.checksumsUrl ? (
                <a
                  href={data.checksumsUrl}
                  rel="noreferrer"
                  className="text-[12.5px] text-ink-2 underline decoration-line-2 underline-offset-4 hover:text-ink"
                >
                  <ExternalLinkNote>{t("SHA256 checksums")}</ExternalLinkNote>
                </a>
              ) : undefined
            }
          />

          {release.isPending && (
            <div className="grid gap-4 sm:grid-cols-2">
              <Skeleton className="h-40 w-full" />
              <Skeleton className="h-40 w-full" />
            </div>
          )}

          {release.isError && (
            <div className="border border-line bg-surface p-5">
              <p className="text-[14px] font-medium">{t("The download service did not answer.")}</p>
              <p className="mt-1 max-w-[65ch] text-[12.5px] leading-relaxed text-ink-3">
                {t("Check that the server is reachable, then try again. Downloads only ever come from the release feed, never from this page.")}
              </p>
              <button
                type="button"
                onClick={() => void release.refetch()}
                className={`${SECONDARY_LINK} mt-4 h-9 px-3 text-[13px]`}
              >
                {t("Try again")}
              </button>
            </div>
          )}

          {data && platforms.length > 0 && (
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              {platforms.map((item) => (
                <PlatformCard
                  key={item}
                  platform={item}
                  assets={assetsForPlatform(data.assets, item)}
                  recommended={preferredAsset(data.assets, item)}
                  highlight={item === platform}
                />
              ))}
            </div>
          )}

          {data && platforms.length > 0 && !platforms.includes("macos") && (
            <p className="mt-4 text-[11.5px] leading-relaxed text-ink-3">
              {t(
                "macOS builds are experimental and unsigned; building from source is the supported path for now.",
              )}
            </p>
          )}

          {data && platforms.length === 0 && !release.isError && (
            <div className="border border-line bg-surface p-5">
              <p className="text-[14px] font-medium">{t("No installer is published for this release yet.")}</p>
              <p className="mt-1 max-w-[65ch] text-[12.5px] leading-relaxed text-ink-3">
                {t("The release page is the source of truth: it lists every asset as soon as the build finishes.")}
              </p>
              <a href={data.releaseUrl} rel="noreferrer" className={`${SECONDARY_LINK} mt-4 h-9 px-3 text-[13px]`}>
                <ExternalLinkNote>{t("Open the release page")}</ExternalLinkNote>
              </a>
            </div>
          )}

          {data && (
            <p className="mt-4 text-[11.5px] leading-relaxed text-ink-3">
              {t("Links point at the published release assets; this server never proxies the binaries.")}{" "}
              <a
                href={data.releaseUrl}
                rel="noreferrer"
                className="underline decoration-line-2 underline-offset-2 hover:text-ink-2"
              >
                {data.tag ?? t("Release page")}
              </a>
            </p>
          )}
        </section>

        {data?.notes && (
          <section aria-labelledby="notes-heading" className="border-b border-line py-10">
            <SectionHeader id="notes-heading" title={t("Release notes")} />
            <p className="measure text-[13.5px] leading-relaxed text-ink-2">{data.notes}</p>
            <a
              href={data.releaseUrl}
              rel="noreferrer"
              className="mt-3 inline-flex items-center gap-1 text-[12.5px] text-ink-2 underline decoration-line-2 underline-offset-4 hover:text-ink"
            >
              <ExternalLinkNote>{t("Read the full release notes")}</ExternalLinkNote>
            </a>
          </section>
        )}

        <section aria-labelledby="signin-heading" className="border-b border-line py-10">
          <div className="grid gap-6 sm:grid-cols-2">
            <div>
              <h2 id="signin-heading" className="flex items-center gap-2 text-[17px] font-semibold tracking-[-0.015em]">
                <KeyRound className="size-4 text-ink-3" aria-hidden="true" />
                {t("Already have the desktop app?")}
              </h2>
              <p className="mt-2 text-[13px] leading-relaxed text-ink-2">
                {t("Point it at this server and sign in with the same account; the library shows up as soon as it connects.")}
              </p>
              <Link
                to={user ? "/" : "/login"}
                className="mt-4 inline-flex items-center gap-1 text-[13px] text-ink underline decoration-line-2 underline-offset-4 hover:text-ink-2"
              >
                {user ? t("Open the web app") : t("Sign in")}
                <ArrowRight className="rtl:-scale-x-100 size-3.5" aria-hidden="true" />
              </Link>
            </div>
            <div>
              <h2 className="flex items-center gap-2 text-[17px] font-semibold tracking-[-0.015em]">
                <Monitor className="size-4 text-ink-3" aria-hidden="true" />
                {t("Just browsing?")}
              </h2>
              <p className="mt-2 text-[13px] leading-relaxed text-ink-2">
                {t("The web app opens the same library, metadata and progress. No install, no local storage.")}
              </p>
              <Link
                to={user ? "/" : "/login"}
                className="mt-4 inline-flex items-center gap-1 text-[13px] text-ink underline decoration-line-2 underline-offset-4 hover:text-ink-2"
              >
                {user ? t("Back to the library") : t("Sign in to browse")}
                <ArrowRight className="rtl:-scale-x-100 size-3.5" aria-hidden="true" />
              </Link>
            </div>
          </div>
        </section>

        <section aria-labelledby="requirements-heading" className="pt-10">
          <SectionHeader id="requirements-heading" title={t("System requirements")} />
          <dl className="border-t border-line text-[13px]">
            {[
              [t("Windows"), t("Windows 10 or 11, 64-bit")],
              [t("Linux"), t("glibc distribution, AppImage or Debian package, 64-bit")],
              [t("macOS"), t("macOS 11 or newer, Apple Silicon and Intel")],
              [t("Server"), t("A Ludex server this computer can reach, on the LAN or over HTTPS")],
            ].map(([label, value]) => (
              <div key={label} className="flex flex-wrap items-baseline justify-between gap-2 border-b border-line py-2.5">
                <dt className="text-ink-3">{label}</dt>
                <dd className="text-ink-2">{value}</dd>
              </div>
            ))}
          </dl>
        </section>
      </main>

      <footer className="page flex flex-wrap items-center justify-between gap-3 border-t border-line py-5 text-[11.5px] text-ink-3">
        <span>
          {data?.version ? `Ludex v${data.version}` : "Ludex"} · AGPL-3.0
        </span>
        <nav aria-label={t("Downloads")} className="flex items-center gap-4">
          <Link to="/" className="hover:text-ink">
            {t("Library")}
          </Link>
          <Link to="/login" className="hover:text-ink">
            {t("Sign in")}
          </Link>
        </nav>
      </footer>
    </div>
  );
}

function PlatformCard({
  platform,
  assets,
  recommended,
  highlight,
}: {
  platform: DesktopPlatform;
  assets: DesktopAsset[];
  recommended: DesktopAsset | null;
  highlight: boolean;
}) {
  return (
    <article
      aria-labelledby={`platform-${platform}`}
      className={`border bg-bg p-5 ${highlight ? "border-signal/60" : "border-line"}`}
    >
      <div className="flex items-center justify-between gap-3">
        <h3 id={`platform-${platform}`} className="text-[15px] font-medium">
          {platformName(platform)}
        </h3>
        {highlight && <Badge tone="signal">{t("Your system")}</Badge>}
      </div>
      <ul className="mt-3 divide-y divide-line">
        {assets.map((asset) => (
          <li key={asset.name} className="py-2.5 first:pt-0 last:pb-0">
            <div className="flex items-baseline justify-between gap-3">
              <a
                href={asset.url}
                rel="noreferrer"
                className="min-w-0 truncate text-[13px] text-ink underline decoration-line-2 underline-offset-4 hover:text-ink-2"
                title={asset.name}
              >
                {assetKindLabel(asset.kind)}
                {asset === recommended ? ` · ${t("recommended")}` : ""}
              </a>
              <span className="num shrink-0 text-[11.5px] text-ink-3">{formatBytes(asset.sizeBytes)}</span>
            </div>
            <code className="mt-1 block truncate font-mono text-[10.5px] text-ink-3" title={asset.name}>
              {asset.name}
            </code>
            {asset.sha256 && (
              <code
                className="mt-1 block truncate font-mono text-[10.5px] text-ink-3"
                title={`SHA-256 ${asset.sha256}`}
              >
                sha256 {shortSha256(asset.sha256)}
              </code>
            )}
          </li>
        ))}
      </ul>
    </article>
  );
}

/** Query key shared with any other surface that reads the download metadata. */
export function desktopDownloadQueryKey(platform: DesktopPlatform | null): [string, string] {
  return ["desktop-download", platform ?? "all"];
}
