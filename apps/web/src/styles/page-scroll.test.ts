import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { PUBLIC_PAGE_BASE_CLASSES, PUBLIC_PAGE_CLASS } from "@/components/public-page";

/**
 * Public-route scrolling guard (#176).
 *
 * The authenticated shell scrolls inside AppShell, so `styles/index.css` locks
 * the viewport on `body`. Account-free routes have no internal scroller: they
 * must opt into `body:has(.public-page)` or their footer is clipped below the
 * fold and the mouse wheel does nothing. This test pins both halves of that
 * contract so a new public page cannot regress one route at a time.
 */

const webRoot = new URL("../../", import.meta.url).pathname;
const css = fs.readFileSync(path.join(webRoot, "src/styles/index.css"), "utf8");

/** Body of the first `selector { … }` block, without nested braces. */
function ruleBody(source: string, selector: string): string {
  const start = source.indexOf(`${selector} {`);
  expect(start, `missing CSS rule for ${selector}`).toBeGreaterThan(-1);
  const end = source.indexOf("}", start);
  return source.slice(start, end);
}

const PUBLIC_ROUTE_FILES = [
  "src/screens/download.tsx",
  "src/screens/share.tsx",
  "src/screens/collection-share.tsx",
  "src/components/auth-layout.tsx",
];

describe("public page scroll contract", () => {
  it("exports one class name for the CSS hook and the wrapper", () => {
    expect(PUBLIC_PAGE_CLASS).toBe("public-page");
    expect(PUBLIC_PAGE_BASE_CLASSES.split(" ")).toContain(PUBLIC_PAGE_CLASS);
    expect(PUBLIC_PAGE_BASE_CLASSES).toContain("min-h-dvh");
  });

  it("keeps the authenticated shell viewport locked", () => {
    const body = ruleBody(css, "body");
    expect(body).toMatch(/overflow:\s*hidden/);
    expect(body).not.toMatch(/overflow-y:\s*(auto|scroll)/);
  });

  it("hands the viewport back to the browser while a public page is mounted", () => {
    const unlock = ruleBody(css, `body:has(.${PUBLIC_PAGE_CLASS})`);
    expect(unlock).toMatch(/overflow-y:\s*auto/);
  });

  it.each(PUBLIC_ROUTE_FILES)("renders %s inside the shared public shell", (file) => {
    const source = fs.readFileSync(path.join(webRoot, file), "utf8");
    expect(source).toContain('from "@/components/public-page"');
    expect(source).toContain("<PublicPage");
  });
});
