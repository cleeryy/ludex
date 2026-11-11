import fs from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * Desktop trailer CSP guard (#152).
 *
 * The packaged app serves the web build from a custom scheme, so the trailer
 * iframes are only allowed when `frame-src` names the providers. A missing
 * frame-src silently falls back to `default-src 'self'` and the trailer shows
 * a blocked-content icon. This asserts the committed config, not the runtime.
 */

const configUrl = new URL("../../../desktop/src-tauri/tauri.conf.json", import.meta.url);
const TRAILER_ORIGINS = [
  "'self'",
  "https://www.youtube-nocookie.com",
  "https://www.youtube.com",
  "https://player.vimeo.com",
];

interface TauriConfig {
  app?: { security?: { csp?: string | null; devCsp?: string | null } };
}

function directives(csp: string): Map<string, string[]> {
  const map = new Map<string, string[]>();
  for (const part of csp.split(";")) {
    const [name, ...values] = part.trim().split(/\s+/);
    if (name) map.set(name.toLowerCase(), values);
  }
  return map;
}

describe("desktop CSP for trailer iframes (#152)", () => {
  const config = JSON.parse(fs.readFileSync(configUrl, "utf8")) as TauriConfig;
  const csp = config.app?.security?.csp ?? "";
  const devCsp = config.app?.security?.devCsp ?? "";

  it("keeps a CSP configured", () => {
    expect(csp.trim()).not.toBe("");
  });

  it("allows the trailer frame origins", () => {
    const frameSrc = directives(csp).get("frame-src");
    expect(frameSrc).toBeDefined();
    for (const origin of TRAILER_ORIGINS) {
      expect(frameSrc).toContain(origin);
    }
  });

  it("still blocks objects and unknown frames", () => {
    const parsed = directives(csp);
    expect(parsed.get("default-src")).toEqual(["'self'"]);
    expect(parsed.get("object-src")).toEqual(["'none'"]);
    expect(parsed.get("frame-src")).not.toContain("*");
  });

  it("uses the same frame origins in devCsp when a separate policy exists", () => {
    if (!devCsp.trim()) return;
    const frameSrc = directives(devCsp).get("frame-src");
    expect(frameSrc).toBeDefined();
    for (const origin of TRAILER_ORIGINS) {
      expect(frameSrc).toContain(origin);
    }
  });
});
