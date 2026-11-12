import fs from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import { initPwa } from "@/lib/pwa";

vi.mock("@/lib/native", () => ({ isNative: () => true }));

const publicDir = new URL("../../public/", import.meta.url);

describe("PWA assets", () => {
  const manifest = JSON.parse(fs.readFileSync(new URL("manifest.webmanifest", publicDir), "utf8"));

  it("declares an installable standalone app", () => {
    expect(manifest.name).toBe("Ludex");
    expect(manifest.start_url).toBeTruthy();
    expect(manifest.scope).toBeTruthy();
    expect(manifest.display).toBe("standalone");
    expect(manifest.theme_color).toMatch(/^#[0-9a-f]{6}$/i);
    expect(manifest.background_color).toMatch(/^#[0-9a-f]{6}$/i);
  });

  it("ships 192/512 icons and a maskable icon", () => {
    const sizes = manifest.icons.map((icon: { sizes: string }) => icon.sizes);
    expect(sizes).toContain("192x192");
    expect(sizes).toContain("512x512");
    expect(manifest.icons.some((icon: { purpose?: string }) => icon.purpose === "maskable")).toBe(true);
  });

  it("has real PNG icons at the declared sizes", () => {
    for (const [file, size] of [
      ["icons/icon-192.png", 192],
      ["icons/icon-512.png", 512],
      ["icons/maskable-512.png", 512],
      ["icons/apple-touch-icon.png", 180],
    ] as const) {
      const buffer = fs.readFileSync(new URL(file, publicDir));
      expect(buffer.subarray(1, 4).toString("ascii"), file).toBe("PNG");
      expect(buffer.readUInt32BE(16), `${file} width`).toBe(size);
      expect(buffer.readUInt32BE(20), `${file} height`).toBe(size);
    }
  });

  it("exposes the remote install queue as a shortcut (#167)", () => {
    const urls = manifest.shortcuts.map((shortcut: { url: string }) => shortcut.url);
    expect(urls).toContain("./#/remote");
    expect(manifest.shortcuts.some((shortcut: { name: string }) => shortcut.name === "Remote installs")).toBe(true);
  });

  it("precaches the shell but never API/auth responses", () => {
    const worker = fs.readFileSync(new URL("sw.js", publicDir), "utf8");
    expect(worker).toContain("const VERSION = \"__CACHE_VERSION__\";");
    expect(worker).toContain("const PRECACHE = __PRECACHE__;");
    expect(worker).toContain('url.pathname.includes("/api/")');
    expect(worker).toContain('url.pathname.includes("/api/media/")');
    expect(worker).toContain("SKIP_WAITING");
    expect(worker).toContain("offline.html");
    // Only the media route may call cache.put for API traffic.
    const apiBlock = worker.slice(worker.indexOf("if (url.pathname.includes(\"/api/\"))"), worker.indexOf("if (request.mode === \"navigate\")"));
    expect(apiBlock).not.toContain("cache.put");
  });

  it("ships the offline fallback", () => {
    const offline = fs.readFileSync(new URL("offline.html", publicDir), "utf8");
    expect(offline).toContain("Offline");
    expect(offline).toContain('onclick="location.reload()"');
  });
});

describe("desktop webview (#177)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("drops an old worker and the ludex caches, and registers nothing", async () => {
    const unregister = vi.fn().mockResolvedValue(true);
    const register = vi.fn();
    const addEventListener = vi.fn();
    const dropCache = vi.fn().mockResolvedValue(true);
    const cachesApi = {
      keys: async () => ["ludex-shell-abc123", "ludex-media-v1", "another-app"],
      delete: dropCache,
    };
    vi.stubGlobal("window", { addEventListener, caches: cachesApi });
    vi.stubGlobal("caches", cachesApi);
    vi.stubGlobal("navigator", {
      serviceWorker: { getRegistrations: async () => [{ unregister }], register },
    });

    initPwa();

    await vi.waitFor(() => expect(unregister).toHaveBeenCalled());
    await vi.waitFor(() => expect(dropCache).toHaveBeenCalledWith("ludex-media-v1"));
    expect(dropCache).toHaveBeenCalledWith("ludex-shell-abc123");
    expect(dropCache).not.toHaveBeenCalledWith("another-app");
    expect(register).not.toHaveBeenCalled();
    expect(addEventListener).not.toHaveBeenCalled();
  });
});
