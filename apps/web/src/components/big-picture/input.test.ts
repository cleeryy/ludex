import { describe, expect, it } from "vitest";
import { hintContextFor, routeGamepadAction, routeKey } from "./input";

/* --------------------------------------------------------------------------
   #138: the input → command table. One assertion per promise the screen makes:
   overlay ownership, the physical button behind `prevTab`/`menu`, and keyboard
   parity.
   -------------------------------------------------------------------------- */

const button = (index: number) => ({ kind: "button" as const, index });

describe("library routing", () => {
  it("moves with every direction and activates with A", () => {
    expect(routeGamepadAction(null, "left")).toEqual({ type: "move", direction: "left" });
    expect(routeGamepadAction(null, "right")).toEqual({ type: "move", direction: "right" });
    expect(routeGamepadAction(null, "up")).toEqual({ type: "move", direction: "up" });
    expect(routeGamepadAction(null, "down")).toEqual({ type: "move", direction: "down" });
    expect(routeGamepadAction(null, "confirm")).toEqual({ type: "activate" });
  });

  it("opens the exit confirmation with B", () => {
    expect(routeGamepadAction(null, "back")).toEqual({ type: "open", overlay: "exit" });
  });

  it("opens search with X and the menu with Start or Guide", () => {
    expect(routeGamepadAction(null, "menu", button(2))).toEqual({ type: "open", overlay: "search" });
    expect(routeGamepadAction(null, "menu", button(9))).toEqual({ type: "open", overlay: "menu" });
    expect(routeGamepadAction(null, "menu", button(16))).toEqual({ type: "open", overlay: "menu" });
    expect(routeGamepadAction(null, "menu", button(17))).toEqual({ type: "open", overlay: "menu" });
    // A remapped `menu` input still opens search rather than doing nothing.
    expect(routeGamepadAction(null, "menu", { kind: "axis", index: 1, direction: 1 })).toEqual({
      type: "open",
      overlay: "search",
    });
  });

  it("opens the queue with Y", () => {
    expect(routeGamepadAction(null, "downloads")).toEqual({ type: "open", overlay: "downloads" });
  });

  it("uses shoulders for shelves and triggers for pages", () => {
    expect(routeGamepadAction(null, "prevTab", button(4))).toEqual({ type: "shelf", delta: -1 });
    expect(routeGamepadAction(null, "nextTab", button(5))).toEqual({ type: "shelf", delta: 1 });
    expect(routeGamepadAction(null, "prevTab", button(6))).toEqual({ type: "page", delta: -1 });
    expect(routeGamepadAction(null, "nextTab", button(7))).toEqual({ type: "page", delta: 1 });
    expect(routeGamepadAction(null, "nextTab", { kind: "axis", index: 5, direction: 1 })).toEqual({
      type: "page",
      delta: 1,
    });
  });
});

describe("overlay routing", () => {
  it("gives the pad to the overlay and never navigates behind it", () => {
    for (const overlay of ["menu", "profiles", "downloads", "exit"] as const) {
      expect(routeGamepadAction(overlay, "up")).toEqual({ type: "highlight", direction: "up" });
      expect(routeGamepadAction(overlay, "left")).toEqual({ type: "highlight", direction: "up" });
      expect(routeGamepadAction(overlay, "down")).toEqual({ type: "highlight", direction: "down" });
      expect(routeGamepadAction(overlay, "right")).toEqual({ type: "highlight", direction: "down" });
      expect(routeGamepadAction(overlay, "confirm")).toEqual({ type: "select" });
      expect(routeGamepadAction(overlay, "back")).toEqual({ type: "close" });
      expect(routeGamepadAction(overlay, "menu")).toEqual({ type: "close" });
      expect(routeGamepadAction(overlay, "downloads")).toEqual({ type: "none" });
      expect(routeGamepadAction(overlay, "prevTab")).toEqual({ type: "none" });
      expect(routeGamepadAction(overlay, "nextTab")).toEqual({ type: "none" });
    }
  });

  it("teaches the on-screen keyboard by context", () => {
    expect(routeGamepadAction("search", "left")).toEqual({ type: "highlight", direction: "left" });
    expect(routeGamepadAction("search", "down")).toEqual({ type: "highlight", direction: "down" });
    expect(routeGamepadAction("search", "confirm")).toEqual({ type: "select" });
    expect(routeGamepadAction("search", "back")).toEqual({ type: "close" });
    expect(routeGamepadAction("search", "menu", button(2))).toEqual({ type: "search", step: "backspace" });
    expect(routeGamepadAction("search", "menu", button(9))).toEqual({ type: "close" });
    expect(routeGamepadAction("search", "downloads")).toEqual({ type: "search", step: "space" });
    expect(routeGamepadAction("search", "prevTab")).toEqual({ type: "search", step: "clear" });
    expect(routeGamepadAction("search", "nextTab")).toEqual({ type: "search", step: "submit" });
  });

  it("maps each overlay to its hint context", () => {
    expect(hintContextFor("menu")).toBe("menu");
    expect(hintContextFor("downloads")).toBe("downloads");
  });
});

describe("keyboard parity", () => {
  it("routes the library keys to the same commands", () => {
    expect(routeKey(null, "ArrowLeft")).toEqual({ type: "move", direction: "left" });
    expect(routeKey(null, "ArrowDown")).toEqual({ type: "move", direction: "down" });
    expect(routeKey(null, "Enter")).toEqual({ type: "activate" });
    expect(routeKey(null, "Escape")).toEqual({ type: "exit" });
    expect(routeKey(null, "Backspace")).toEqual({ type: "open", overlay: "exit" });
    expect(routeKey(null, "[")).toEqual({ type: "shelf", delta: -1 });
    expect(routeKey(null, "]")).toEqual({ type: "shelf", delta: 1 });
    expect(routeKey(null, "PageUp")).toEqual({ type: "page", delta: -1 });
    expect(routeKey(null, "PageDown")).toEqual({ type: "page", delta: 1 });
    expect(routeKey(null, "/")).toEqual({ type: "open", overlay: "search" });
    expect(routeKey(null, "m")).toEqual({ type: "open", overlay: "menu" });
    expect(routeKey(null, "p")).toEqual({ type: "open", overlay: "profiles" });
    expect(routeKey(null, "d")).toEqual({ type: "open", overlay: "downloads" });
    expect(routeKey(null, "l")).toEqual({ type: "layout" });
    expect(routeKey(null, "g")).toEqual({ type: "game" });
  });

  it("leaves unrelated keys to the DOM", () => {
    expect(routeKey(null, "a")).toBeNull();
    expect(routeKey(null, "Tab")).toBeNull();
  });

  it("moves the overlay highlight with the arrows and selects with Enter", () => {
    expect(routeKey("menu", "ArrowUp")).toEqual({ type: "highlight", direction: "up" });
    expect(routeKey("menu", "ArrowLeft")).toEqual({ type: "highlight", direction: "up" });
    expect(routeKey("menu", "ArrowDown")).toEqual({ type: "highlight", direction: "down" });
    expect(routeKey("menu", "ArrowRight")).toEqual({ type: "highlight", direction: "down" });
    expect(routeKey("menu", "Enter")).toEqual({ type: "select" });
    expect(routeKey("menu", "Escape")).toEqual({ type: "close" });
  });

  it("keeps printable characters native inside the search field", () => {
    expect(routeKey("search", "h")).toBeNull();
    expect(routeKey("search", "Backspace")).toBeNull();
    expect(routeKey("search", "Escape")).toEqual({ type: "close" });
    expect(routeKey("search", "Enter")).toEqual({ type: "search", step: "submit" });
    expect(routeKey("search", "ArrowUp")).toEqual({ type: "highlight", direction: "up" });
    expect(routeKey("search", "ArrowRight")).toEqual({ type: "highlight", direction: "right" });
  });
});
