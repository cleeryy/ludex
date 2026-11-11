import { describe, expect, it } from "vitest";
import {
  assetKindLabel,
  assetKindRank,
  assetsForPlatform,
  availablePlatforms,
  detectDesktopPlatform,
  hideDesktopPrompt,
  isMobileBrowser,
  isStandaloneDisplay,
  platformName,
  preferredAsset,
  primaryDownload,
  shortSha256,
  shouldOfferDesktopApp,
  snoozeDesktopPrompt,
} from "@/lib/desktop-download";
import type { DesktopAsset, DesktopDownload } from "@/lib/types";

const WINDOWS_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36";
const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15";
const LINUX_UA =
  "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0";
const IPHONE_UA =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
const ANDROID_UA =
  "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Mobile Safari/537.36";
const CHROMEBOOK_UA =
  "Mozilla/5.0 (X11; CrOS x86_64 15917.71.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36";

const assets: DesktopAsset[] = [
  {
    platform: "windows",
    kind: "installer",
    name: "Ludex_0.3.2_x64-setup.exe",
    url: "https://example.test/Ludex_0.3.2_x64-setup.exe",
    sizeBytes: 18_400_000,
    sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  },
  {
    platform: "linux",
    kind: "deb",
    name: "ludex_0.3.2_amd64.deb",
    url: "https://example.test/ludex_0.3.2_amd64.deb",
    sizeBytes: 21_700_000,
    sha256: null,
  },
  {
    platform: "linux",
    kind: "appimage",
    name: "ludex_0.3.2_amd64.AppImage",
    url: "https://example.test/ludex_0.3.2_amd64.AppImage",
    sizeBytes: 72_100_000,
    sha256: null,
  },
  {
    platform: "macos",
    kind: "dmg",
    name: "Ludex_0.3.2_universal.dmg",
    url: "https://example.test/Ludex_0.3.2_universal.dmg",
    sizeBytes: 24_900_000,
    sha256: null,
  },
];

const release: DesktopDownload = {
  channel: "stable",
  source: "github",
  stale: false,
  version: "0.3.2",
  tag: "v0.3.2",
  releasedAt: "2026-11-10T09:30:00Z",
  releaseUrl: "https://example.test/releases/tag/v0.3.2",
  notes: "Cloud saves.",
  checksumsUrl: null,
  platform: null,
  recommended: null,
  assets,
};

describe("platform detection", () => {
  it("detects the three desktop platforms", () => {
    expect(detectDesktopPlatform(WINDOWS_UA)).toBe("windows");
    expect(detectDesktopPlatform(MAC_UA)).toBe("macos");
    expect(detectDesktopPlatform(LINUX_UA)).toBe("linux");
  });

  it("keeps phones, tablets and Chromebooks on the all-platforms layout", () => {
    expect(detectDesktopPlatform(IPHONE_UA)).toBeNull();
    expect(detectDesktopPlatform(ANDROID_UA)).toBeNull();
    expect(detectDesktopPlatform(CHROMEBOOK_UA)).toBeNull();
    expect(detectDesktopPlatform("")).toBeNull();
    expect(detectDesktopPlatform(null)).toBeNull();
  });

  it("flags mobile browsers for the 'open this on your PC' card", () => {
    expect(isMobileBrowser(IPHONE_UA)).toBe(true);
    expect(isMobileBrowser(ANDROID_UA)).toBe(true);
    expect(isMobileBrowser(WINDOWS_UA)).toBe(false);
    expect(isMobileBrowser(MAC_UA)).toBe(false);
    expect(isMobileBrowser("")).toBe(false);
  });
});

describe("asset selection", () => {
  it("ranks one-file installers before distro packages", () => {
    expect(assetKindRank("installer")).toBeLessThan(assetKindRank("deb"));
    expect(assetKindRank("appimage")).toBeLessThan(assetKindRank("rpm"));

    const linux = assetsForPlatform(assets, "linux");
    expect(linux.map((asset) => asset.kind)).toEqual(["appimage", "deb"]);
  });

  it("picks the AppImage, dmg or setup.exe for the primary button", () => {
    expect(preferredAsset(assets, "windows")?.name).toBe("Ludex_0.3.2_x64-setup.exe");
    expect(preferredAsset(assets, "linux")?.name).toBe("ludex_0.3.2_amd64.AppImage");
    expect(preferredAsset(assets, "macos")?.name).toBe("Ludex_0.3.2_universal.dmg");
    expect(preferredAsset(assets, null)).toBeNull();
  });

  it("lifts the API recommendation over local ranking", () => {
    const recommended = assets[1];
    const href = primaryDownload({ ...release, recommended }, "linux");
    expect(href).toEqual({ href: "https://example.test/ludex_0.3.2_amd64.deb", asset: recommended });
  });

  it("falls back to the release page when no asset exists", () => {
    const fallback: DesktopDownload = {
      ...release,
      source: "fallback",
      stale: true,
      version: null,
      assets: [],
    };
    expect(primaryDownload(fallback, "windows")).toEqual({
      href: "https://example.test/releases/tag/v0.3.2",
      asset: null,
    });
    expect(primaryDownload(null, "windows")).toBeNull();
  });

  it("lists only platforms that have an asset", () => {
    expect(availablePlatforms(assets)).toEqual(["windows", "linux", "macos"]);
    expect(availablePlatforms(assets.filter((asset) => asset.platform === "linux"))).toEqual(["linux"]);
  });

  it("labels platforms and asset kinds", () => {
    expect(platformName("windows")).toBe("Windows");
    expect(platformName("linux")).toBe("Linux");
    expect(platformName("macos")).toBe("macOS");
    expect(assetKindLabel("installer")).toBe("Installer");
    expect(assetKindLabel("dmg")).toBe("Disk image");
  });

  it("shortens hashes without hiding the full value", () => {
    const sha = "0123456789abcdef0123456789abcdef";
    expect(shortSha256(sha)).toBe("0123456789abcdef…");
    expect(shortSha256("short")).toBe("short");
  });
});

describe("in-app prompt", () => {
  it("never prompts inside the desktop app or an installed PWA", () => {
    expect(
      shouldOfferDesktopApp({ native: true, standalone: false, hidden: false, snoozed: false }),
    ).toBe(false);
    expect(
      shouldOfferDesktopApp({ native: false, standalone: true, hidden: false, snoozed: false }),
    ).toBe(false);
  });

  it("respects both dismissals", () => {
    expect(
      shouldOfferDesktopApp({ native: false, standalone: false, hidden: true, snoozed: false }),
    ).toBe(false);
    expect(
      shouldOfferDesktopApp({ native: false, standalone: false, hidden: false, snoozed: true }),
    ).toBe(false);
    expect(
      shouldOfferDesktopApp({ native: false, standalone: false, hidden: false, snoozed: false }),
    ).toBe(true);
  });

  it("recognises standalone display modes", () => {
    expect(isStandaloneDisplay("standalone")).toBe(true);
    expect(isStandaloneDisplay("fullscreen")).toBe(true);
    expect(isStandaloneDisplay("minimal-ui")).toBe(true);
    expect(isStandaloneDisplay("browser")).toBe(false);
    expect(isStandaloneDisplay(null)).toBe(false);
  });

  it("exposes dismissal helpers without throwing outside a browser", () => {
    // These are no-ops in the node test environment (no localStorage).
    expect(() => snoozeDesktopPrompt()).not.toThrow();
    expect(() => hideDesktopPrompt()).not.toThrow();
  });
});
