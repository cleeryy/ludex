// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InputProvider, useInputScope } from "@/components/console/input/input-provider";
import { ConsoleOverlayInputScope } from "@/components/console/input/overlay-scope";

/**
 * The pad is a real fake: a mutable `navigator.getGamepads` object, the real
 * `useGamepad` polling loop, and a manual `requestAnimationFrame` queue so a
 * press is one deterministic frame. No hook is mocked.
 */
const pad = {
  connected: true,
  id: "fake-standard-pad",
  index: 0,
  mapping: "standard",
  axes: [0, 0, 0, 0],
  buttons: Array.from({ length: 16 }, () => ({ pressed: false, value: 0 })),
  timestamp: 0,
};

let frames: FrameRequestCallback[] = [];

function step(framesToRun = 2) {
  for (let index = 0; index < framesToRun; index += 1) {
    const queue = frames;
    frames = [];
    for (const callback of queue) callback(16 + index * 16);
  }
}

function press(button: number, down: boolean) {
  pad.timestamp += 1;
  pad.buttons[button] = { pressed: down, value: down ? 1 : 0 };
}

beforeEach(() => {
  pad.buttons = Array.from({ length: 16 }, () => ({ pressed: false, value: 0 }));
  frames = [];
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.push(callback);
    return frames.length;
  });
  vi.stubGlobal("cancelAnimationFrame", () => undefined);
  Object.defineProperty(navigator, "getGamepads", {
    configurable: true,
    value: () => [pad],
  });
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function event(message: string) {
  return message;
}

function Scope({
  id,
  log,
  enabled = true,
}: {
  id: string;
  log: string[];
  enabled?: boolean;
}) {
  useInputScope({
    id,
    enabled,
    onAction: (action) => log.push(event(`${id}:${action}`)),
    onKey: (keyEvent) => {
      log.push(event(`${id}:key:${keyEvent.key}`));
    },
  });
  return null;
}

function harness(children: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <InputProvider>{children}</InputProvider>
    </QueryClientProvider>,
  );
}

describe("one input owner", () => {
  it("runs one action for one press with the console overlay scope open", () => {
    const log: string[] = [];
    function ConsoleLike() {
      useInputScope({
        id: "console",
        onAction: (action) => log.push(`console:${action}`),
        onKey: () => undefined,
      });
      return null;
    }
    harness(
      <>
        <ConsoleLike />
        {/* The real component the Console mounts while a menu or plate is up. */}
        <ConsoleOverlayInputScope
          onAction={(action) => log.push(`overlay:${action}`)}
          onKey={() => undefined}
          options={{}}
        />
      </>,
    );

    act(() => {
      press(0, true);
      step();
    });
    act(() => {
      press(0, false);
      step();
    });

    expect(log).toEqual(["overlay:confirm"]);
  });

  it("sends a fake-pad press to the top scope exactly once", () => {
    const log: string[] = [];
    harness(
      <>
        <Scope id="library" log={log} />
        <Scope id="overlay" log={log} />
      </>,
    );

    act(() => {
      press(0, true);
      step();
    });
    act(() => {
      press(0, false);
      step();
    });

    expect(log).toEqual(["overlay:confirm"]);
  });

  it("restores the previous scope and its exact focus target when the overlay closes", () => {
    const log: string[] = [];
    const { rerender } = harness(
      <>
        <button type="button" data-testid="card">
          Chrono Trigger
        </button>
        <Scope id="library" log={log} />
      </>,
    );
    const card = screen.getByTestId("card");
    act(() => card.focus());

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const tree = (overlay: boolean) => (
      <QueryClientProvider client={client}>
        <InputProvider>
          <button type="button" data-testid="card">
            Chrono Trigger
          </button>
          <Scope id="library" log={log} />
          {overlay && <Scope id="overlay" log={log} />}
        </InputProvider>
      </QueryClientProvider>
    );

    rerender(tree(true));
    // The overlay owns the ring now.
    act(() => card.focus());
    act(() => {
      press(0, true);
      step();
    });
    act(() => {
      press(0, false);
      step();
    });
    expect(log).toEqual(["overlay:confirm"]);

    rerender(tree(false));
    expect(document.activeElement).toBe(card);

    act(() => {
      press(0, true);
      step();
    });
    act(() => {
      press(0, false);
      step();
    });
    expect(log).toEqual(["overlay:confirm", "library:confirm"]);
  });

  it("routes keyboard and remote keys through the same top scope", () => {
    const log: string[] = [];
    harness(
      <>
        <Scope id="library" log={log} />
        <Scope id="overlay" log={log} />
      </>,
    );

    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });

    expect(log).toEqual(["overlay:key:Escape"]);
  });

  it("skips a disabled scope instead of blocking the surface below it", () => {
    const log: string[] = [];
    harness(
      <>
        <Scope id="library" log={log} />
        <Scope id="onboarding" log={log} enabled={false} />
      </>,
    );

    act(() => {
      fireEvent.keyDown(window, { key: "Enter" });
    });

    expect(log).toEqual(["library:key:Enter"]);
  });
});
