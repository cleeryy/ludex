import { beforeEach, describe, expect, it } from "vitest";
import {
  APP_TITLE,
  composeDocumentTitle,
  formatDocumentTitle,
  registerDocumentTitle,
  resetDocumentTitle,
  setRouteDocumentTitle,
} from "./document-title";

/* --------------------------------------------------------------------------
   Document title (#237). The tab said "Ludex" on every screen. The precedence
   rule is pure so it can be pinned here: the route label is the floor, the
   screen with real content (a game, a share) wins, and closing it restores the
   destination name instead of leaving the last one behind.
   -------------------------------------------------------------------------- */

describe("document title", () => {
  beforeEach(() => resetDocumentTitle());

  it("formats a label with the product name and falls back to the bare name", () => {
    expect(formatDocumentTitle("Library")).toBe("Library · Ludex");
    expect(formatDocumentTitle(`  ${APP_TITLE}  `)).toBe("Ludex · Ludex");
    expect(formatDocumentTitle("")).toBe("Ludex");
    expect(formatDocumentTitle(null)).toBe("Ludex");
  });

  it("uses the route label as the floor", () => {
    setRouteDocumentTitle("Library");
    expect(composeDocumentTitle()).toBe("Library · Ludex");
    setRouteDocumentTitle(null);
    expect(composeDocumentTitle()).toBe("Ludex");
  });

  it("lets the deepest screen win and restores on unmount", () => {
    setRouteDocumentTitle("Library");
    const releaseGame = registerDocumentTitle("Hades");
    expect(composeDocumentTitle()).toBe("Hades · Ludex");
    const releaseDialog = registerDocumentTitle("Install Hades");
    expect(composeDocumentTitle()).toBe("Install Hades · Ludex");
    releaseDialog();
    expect(composeDocumentTitle()).toBe("Hades · Ludex");
    releaseGame();
    expect(composeDocumentTitle()).toBe("Library · Ludex");
  });

  it("ignores a double release", () => {
    const release = registerDocumentTitle("Hades");
    release();
    release();
    expect(composeDocumentTitle()).toBe("Ludex");
  });

  it("survives a route change while a screen title is registered", () => {
    const release = registerDocumentTitle("Hades");
    setRouteDocumentTitle("Downloads");
    expect(composeDocumentTitle()).toBe("Hades · Ludex");
    release();
    expect(composeDocumentTitle()).toBe("Downloads · Ludex");
  });
});
