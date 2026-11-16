import { describe, expect, it } from "vitest";
import { isSafeUrl, originOf, safeRedirectUrl, toSafeUrl } from "@/lib/safe-url";

/**
 * Evidence for the `js/xss-through-dom` triage (#266): every `src`, `href` and
 * redirect in the web app goes through `toSafeUrl`/`assetUrl`, and the tests
 * below pin what that boundary accepts.
 */

describe("toSafeUrl", () => {
  it("accepts the schemes the app produces", () => {
    expect(toSafeUrl("https://cdn.example.test/cover.png")).toBe(
      "https://cdn.example.test/cover.png",
    );
    expect(toSafeUrl("http://10.0.0.5:8080/api/media/1")).toBe("http://10.0.0.5:8080/api/media/1");
    expect(toSafeUrl("data:image/png;base64,AAAA")).toBe("data:image/png;base64,AAAA");
    expect(toSafeUrl("blob:https://app.test/8d1f")).toBe("blob:https://app.test/8d1f");
    expect(toSafeUrl("demo://cover/1")).toBe("demo://cover/1");
  });

  it("accepts origin-relative paths", () => {
    expect(toSafeUrl("/api/media/42")).toBe("/api/media/42");
    expect(toSafeUrl("/#/login")).toBe("/#/login");
  });

  it("rejects script and protocol-relative URLs", () => {
    for (const hostile of [
      "javascript:alert(1)",
      "JavaScript:alert(1)",
      " javaScript:alert(1)",
      "vbscript:msgbox(1)",
      "file:///etc/passwd",
      "mailto:someone@example.test",
      "//evil.test/x",
      "/\\evil.test/x",
      "\\\\evil.test\\x",
      "https:",
      "",
      "   ",
    ]) {
      expect(toSafeUrl(hostile), hostile).toBeNull();
    }
  });

  it("rejects control characters", () => {
    expect(toSafeUrl("/api/media/1\nSet-Cookie: x")).toBeNull();
    expect(toSafeUrl("https://x.test/\u0000")).toBeNull();
  });

  it("brands only values that pass isSafeUrl", () => {
    const safe = toSafeUrl("/api/media/1");
    expect(safe).not.toBeNull();
    expect(isSafeUrl(safe)).toBe(true);
    expect(isSafeUrl("javascript:alert(1)")).toBe(false);
    expect(isSafeUrl(null)).toBe(false);
  });
});

describe("originOf", () => {
  it("resolves relative and absolute inputs", () => {
    expect(originOf("https://api.test:8080/base")).toBe("https://api.test:8080");
    expect(originOf("/api", "https://app.test/x")).toBe("https://app.test");
    expect(originOf("")).toBeNull();
    expect(originOf(null)).toBeNull();
  });
});

describe("safeRedirectUrl", () => {
  const base = "https://app.test/#/login";

  it("allows the configured server and the page origin", () => {
    expect(safeRedirectUrl("https://api.test/#/login?desktop_state=1", ["https://api.test"], base)).toBe(
      "https://api.test/#/login?desktop_state=1",
    );
    expect(safeRedirectUrl("/#/login", ["https://app.test"], base)).toBe("https://app.test/#/login");
  });

  it("refuses an off-origin redirect", () => {
    expect(safeRedirectUrl("https://evil.test/#/login", ["https://api.test"], base)).toBeNull();
    expect(safeRedirectUrl("//evil.test/#/login", ["https://api.test"], base)).toBeNull();
    expect(safeRedirectUrl("javascript:alert(1)", ["https://api.test"], base)).toBeNull();
    expect(safeRedirectUrl("https://user:pass@api.test/", ["https://api.test"], base)).toBeNull();
    expect(safeRedirectUrl(null, ["https://api.test"], base)).toBeNull();
  });
});
