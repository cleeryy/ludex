import fs from "node:fs";
import { describe, expect, it } from "vitest";

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
