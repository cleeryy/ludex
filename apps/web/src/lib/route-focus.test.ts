import { describe, expect, it } from "vitest";
import {
  routeDestination,
  routePathname,
  routeScreenKey,
  routeTitleId,
  shouldMoveRouteFocus,
} from "./route-focus";

/* --------------------------------------------------------------------------
   Route focus and titles (#237). Two behaviours that are easy to get subtly
   wrong and impossible to test through the router without a DOM:
   an in-page section link must not steal focus, and every destination must
   announce itself in the tab.
   -------------------------------------------------------------------------- */

describe("route screen key", () => {
  it("collapses an in-page section onto the page that owns it", () => {
    expect(routeScreenKey("/game/12")).toBe("/game/12");
    expect(routeScreenKey("/game/12/rom")).toBe("/game/12");
    expect(routeScreenKey("/game/12/files")).toBe("/game/12");
  });

  it("treats another game as another screen", () => {
    expect(routeScreenKey("/game/13/rom")).not.toBe(routeScreenKey("/game/12/rom"));
  });

  it("keeps nested screens distinct", () => {
    expect(routeScreenKey("/")).toBe("/");
    expect(routeScreenKey("/library")).toBe("/library");
    expect(routeScreenKey("/admin/users/7")).toBe("/admin/users/7");
  });

  it("strips the query and the hash", () => {
    expect(routePathname("/settings?section=appearance")).toBe("/settings");
    expect(routePathname("/library#top")).toBe("/library");
    expect(routePathname("")).toBe("/");
  });
});

describe("route titles", () => {
  it("names every top-level destination from the navigation table", () => {
    expect(routeTitleId("/")).toBe("Home");
    expect(routeTitleId("/library")).toBe("Library");
    expect(routeTitleId("/downloads")).toBe("Downloads");
    expect(routeTitleId("/settings?section=appearance")).toBe("Settings");
    expect(routeTitleId("/news")).toBe("News");
    expect(routeTitleId("/admin/users")).toBe("Users");
  });

  it("leaves the game page to the game, which knows its own name", () => {
    expect(routeTitleId("/game/12")).toBeNull();
    expect(routeTitleId("/game/12/rom")).toBeNull();
  });

  it("follows a prefix destination into its children", () => {
    expect(routeDestination("/collections/5")?.labelKey).toBe("Collections");
    expect(routeDestination("/community/u/3")?.labelKey).toBe("Community");
    expect(routeDestination("/library?sort=rating")?.labelKey).toBe("Library");
  });

  it("has no title for an unknown path", () => {
    expect(routeTitleId("/nope")).toBeNull();
    expect(routeTitleId("/library/extra")).toBeNull();
  });
});

describe("route focus", () => {
  it("moves focus when the screen changes and stays put otherwise", () => {
    expect(shouldMoveRouteFocus(null, "/library")).toBe(false);
    expect(shouldMoveRouteFocus("/library", "/library")).toBe(false);
    expect(shouldMoveRouteFocus("/library", "/downloads")).toBe(true);
    expect(shouldMoveRouteFocus("/game/12", "/game/13")).toBe(true);
  });
});
