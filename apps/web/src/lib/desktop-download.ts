import { t } from "@/lib/i18n";
import type { DesktopAsset, DesktopAssetKind, DesktopDownload, DesktopPlatform } from "@/lib/types";

/**
 * Public download page helpers (#164).
 *
 * The decisions live outside React so they can be unit-tested and so the same
 * rules drive the page and the in-app prompt:
 * - which platform to offer first (user-agent detection is only a hint);
 * - which asset is the primary installer for that platform;
 * - when the browser may see the "get the desktop app" prompt at all.
 */

/** Platform whose installer the primary button should offer, if any. */
export function detectDesktopPlatform(userAgent: string | null | undefined): DesktopPlatform | null {
  const ua = (userAgent ?? "").trim();
  if (!ua) return null;
  // Mobile browsers often claim Linux or macOS. They cannot run the desktop
  // app, so the page keeps the "all platforms" layout and points at a PC.
  if (isMobileBrowser(ua)) return null;
  // ChromeOS reports Linux too but cannot run the AppImage.
  if (/CrOS/i.test(ua)) return null;
  if (/Windows NT|Win64|WOW64|Win32/i.test(ua)) return "windows";
  if (/Macintosh|Mac OS X/i.test(ua)) return "macos";
  if (/Linux|X11/i.test(ua)) return "linux";
  return null;
}

/** Whether the visitor is on a phone or tablet. */
export function isMobileBrowser(userAgent: string | null | undefined): boolean {
  return /Android|iPhone|iPad|iPod|Mobile|IEMobile|Opera Mini|webOS|BlackBerry/i.test(userAgent ?? "");
}

/** Display label for a platform. */
export function platformName(platform: DesktopPlatform): string {
  switch (platform) {
    case "windows":
      return t("Windows");
    case "linux":
      return t("Linux");
    case "macos":
      return t("macOS");
  }
}

/** Display label for an asset kind. */
export function assetKindLabel(kind: DesktopAssetKind): string {
  switch (kind) {
    case "installer":
      return t("Installer");
    case "appimage":
      return t("AppImage");
    case "deb":
      return t("Debian package");
    case "rpm":
      return t("RPM package");
    case "dmg":
      return t("Disk image");
  }
}

/** Smaller wins: the primary button prefers a one-file installer. */
export function assetKindRank(kind: DesktopAssetKind): number {
  switch (kind) {
    case "installer":
    case "appimage":
    case "dmg":
      return 0;
    case "deb":
      return 1;
    case "rpm":
      return 2;
  }
}

/** Best installer for a platform, independent of the API's recommendation. */
export function preferredAsset(
  assets: readonly DesktopAsset[],
  platform: DesktopPlatform | null | undefined,
): DesktopAsset | null {
  if (!platform) return null;
  let best: DesktopAsset | null = null;
  for (const asset of assets) {
    if (asset.platform !== platform) continue;
    if (!best || assetKindRank(asset.kind) < assetKindRank(best.kind)) best = asset;
  }
  return best;
}

/** Assets offered for a platform, primary installer first. */
export function assetsForPlatform(
  assets: readonly DesktopAsset[],
  platform: DesktopPlatform,
): DesktopAsset[] {
  return assets
    .filter((asset) => asset.platform === platform)
    .sort((left, right) => assetKindRank(left.kind) - assetKindRank(right.kind));
}

/** Platforms that actually have an asset, in menu order. */
export function availablePlatforms(assets: readonly DesktopAsset[]): DesktopPlatform[] {
  const order: DesktopPlatform[] = ["windows", "linux", "macos"];
  return order.filter((platform) => assets.some((asset) => asset.platform === platform));
}

/**
 * The page falls back to the release page whenever the API could not resolve
 * an installer (offline, rate-limited, or a release without that platform).
 */
export function primaryDownload(
  download: DesktopDownload | null | undefined,
  platform: DesktopPlatform | null,
): { href: string; asset: DesktopAsset | null } | null {
  if (!download) return null;
  const asset = download.recommended ?? preferredAsset(download.assets, platform);
  if (asset) return { href: asset.url, asset };
  return download.releaseUrl ? { href: download.releaseUrl, asset: null } : null;
}

/** Short hash shown next to an asset; the full value stays in `title`. */
export function shortSha256(sha256: string): string {
  const value = sha256.trim();
  return value.length > 16 ? `${value.slice(0, 16)}…` : value;
}

/** `0.3.2` for `v0.3.2`, unchanged otherwise. */
export function versionLabel(download: Pick<DesktopDownload, "version">): string | null {
  return download.version?.trim() || null;
}

/* --------------------------- in-app prompt state ------------------------- */

/** localStorage: "Don't show again" is permanent for this browser. */
export const DESKTOP_PROMPT_HIDE_KEY = "ludex.desktopAppPrompt.hidden";
/** sessionStorage: "Not now" hides the prompt until the tab is closed. */
export const DESKTOP_PROMPT_SNOOZE_KEY = "ludex.desktopAppPrompt.snoozed";

/** A browser tab with the app installed as a PWA or opened full screen. */
export function isStandaloneDisplay(displayMode: string | null | undefined): boolean {
  return displayMode === "standalone" || displayMode === "fullscreen" || displayMode === "minimal-ui";
}

export interface DesktopPromptEnvironment {
  /** Running inside the Tauri shell. */
  native: boolean;
  /** `window.matchMedia("(display-mode: standalone)").matches`. */
  standalone: boolean;
  /** Permanent dismissal stored by the browser. */
  hidden: boolean;
  /** Session dismissal stored by the tab. */
  snoozed: boolean;
}

/**
 * The prompt only exists for a browser session that has not dismissed it and
 * is not already the desktop app or an installed PWA.
 */
export function shouldOfferDesktopApp(environment: DesktopPromptEnvironment): boolean {
  return (
    !environment.native && !environment.standalone && !environment.hidden && !environment.snoozed
  );
}

function readFlag(storage: "local" | "session", key: string): boolean {
  try {
    const store = storage === "local" ? window.localStorage : window.sessionStorage;
    return store.getItem(key) === "1";
  } catch {
    return false;
  }
}

function writeFlag(storage: "local" | "session", key: string): void {
  try {
    const store = storage === "local" ? window.localStorage : window.sessionStorage;
    store.setItem(key, "1");
  } catch {
    /* private mode: the prompt simply stays hidden for this render */
  }
}

/** Reads both dismissal flags without touching the environment. */
export function readDesktopPromptDismissal(): { hidden: boolean; snoozed: boolean } {
  return {
    hidden: readFlag("local", DESKTOP_PROMPT_HIDE_KEY),
    snoozed: readFlag("session", DESKTOP_PROMPT_SNOOZE_KEY),
  };
}

/** "Not now": hidden for the rest of the browser session. */
export function snoozeDesktopPrompt(): void {
  writeFlag("session", DESKTOP_PROMPT_SNOOZE_KEY);
}

/** "Don't show again": hidden for this browser from now on. */
export function hideDesktopPrompt(): void {
  writeFlag("local", DESKTOP_PROMPT_HIDE_KEY);
}
