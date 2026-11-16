// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleBackdrop } from "@/components/big-picture/console-backdrop";

/**
 * The wall must never be a live full-screen filter (#270). These tests hold the
 * two properties the bake depends on: the layer falls back to the live plate
 * when the engine cannot blur a canvas, and the plate follows the *settled*
 * selection rather than every repeat of a held direction.
 */

beforeEach(() => {
  // jsdom has no 2D context: the bake has to degrade to the live plate, which
  // is exactly the path an engine without canvas filters takes.
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function wall(container: HTMLElement): HTMLDivElement {
  const element = container.querySelector<HTMLDivElement>(".bp-backdrop-art");
  if (!element) throw new Error("the console wall did not render");
  return element;
}

describe("console wall", () => {
  it("keeps the live plate when the canvas cannot be painted", () => {
    const { container } = render(
      <ConsoleBackdrop wall="artwork" room="night" title="Chrono Trigger" artUrl={null} />,
    );
    const layer = wall(container);
    expect(layer.getAttribute("data-bp-wall")).toBe("live");
    expect(layer.querySelector("canvas[data-bp-wall-plate='true']")).not.toBeNull();
    // The artwork the player can see is the live plate, still in the tree.
    expect(layer.querySelector("[data-bp-art='true']")).not.toBeNull();
    expect(container.querySelector(".bp-backdrop-wash")).not.toBeNull();
    expect(container.querySelector(".bp-backdrop")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("paints no plate for the walls that never decode an image", () => {
    const { container } = render(<ConsoleBackdrop wall="gradient" room="arcade" title="Any" artUrl={null} />);
    expect(container.querySelector(".bp-backdrop-art")).toBeNull();
    expect(container.querySelector(".bp-backdrop")?.getAttribute("data-console-backdrop")).toBe("gradient");
  });

  it("follows the selection when it settles, not on every repeat", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(
      <ConsoleBackdrop wall="artwork" room="night" title="Chrono Trigger" artUrl={null} />,
    );
    const before = wall(container).querySelector(".bp-wall-source")?.innerHTML ?? "";
    expect(before).not.toBe("");

    rerender(<ConsoleBackdrop wall="artwork" room="night" title="Outer Wilds" artUrl={null} />);
    // The rail re-centres on every repeat; the wall waits for the player to stop.
    expect(wall(container).querySelector(".bp-wall-source")?.innerHTML).toBe(before);

    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(wall(container).querySelector(".bp-wall-source")?.innerHTML).not.toBe(before);
  });
});
