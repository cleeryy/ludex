// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InputProvider, useInputScope } from "@/components/console/input/input-provider";
import { usePadBack } from "@/components/console/input/use-pad-back";

/**
 * The back-only screen scope (#237, #308).
 *
 * `/console/game/:id` and `/console/remote` are desktop screens a couch player
 * can reach; without a pad scope, B did nothing and the player was stuck. This
 * pins the three rules that make the escape hatch safe:
 *
 * - B runs the callback exactly once (a double dispatch is a double navigation);
 * - the console scope underneath never sees the same press;
 * - a disabled scope does not swallow B — it falls through, so the screen's own
 *   owner can handle it.
 *
 * The pad is the mutable `navigator.getGamepads` object and the real polling
 * loop; no input hook is mocked. Breaking any of the three rules fails this
 * file, which is the regression net #308 asks the console to keep.
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

function tap(button: number) {
  act(() => {
    press(button, true);
    step();
  });
  act(() => {
    press(button, false);
    step();
  });
}

function Screen({ onBack, enabled }: { onBack: () => void; enabled?: boolean }) {
  usePadBack(onBack, { enabled });
  return null;
}

function ConsoleScope({ log }: { log: string[] }) {
  useInputScope({
    id: "console",
    onAction: (action) => {
      log.push(action);
    },
    onKey: (event) => {
      log.push(`key:${event.key}`);
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

describe("usePadBack", () => {
  it("runs B once and keeps the press away from the console", () => {
    const log: string[] = [];
    const onBack = vi.fn();
    harness(
      <>
        <ConsoleScope log={log} />
        <Screen onBack={onBack} />
      </>,
    );

    tap(1);
    expect(onBack).toHaveBeenCalledTimes(1);
    // One press, one owner: the scope below must not navigate too.
    expect(log).toEqual([]);

    // A is not a back action and the scope below still stays out of it.
    tap(0);
    expect(onBack).toHaveBeenCalledTimes(1);
    expect(log).toEqual([]);
  });

  it("lets a disabled scope fall through instead of swallowing B", () => {
    const log: string[] = [];
    const onBack = vi.fn();
    harness(
      <>
        <ConsoleScope log={log} />
        <Screen onBack={onBack} enabled={false} />
      </>,
    );

    tap(1);
    expect(onBack).not.toHaveBeenCalled();
    expect(log).toEqual(["back"]);
  });
});
