import type { Page } from "@playwright/test";

/**
 * Fake standard-mapping gamepad for the Console tests (#270).
 *
 * The harness installs one `page.addInitScript` that replaces
 * `navigator.getGamepads` with a real pad-shaped object: `mapping: "standard"`,
 * 17 buttons, 4 axes, a `timestamp`, and hot-plug events. Nothing here mocks a
 * React hook: the application's own `useGamepad` polling loop reads the fake pad
 * through the same Gamepad API it uses for hardware, so a test exercises the
 * repeat tracker, focus routing, profile resolution and cue haptics.
 *
 * ```ts
 * const pad = await installFakePad(page);
 * await page.goto("/#/bigpicture");
 * await pad.tap("a");            // confirm, pressed for two frames then released
 * await pad.hold("right", 700);  // crosses the 380 ms repeat delay
 * await pad.chord(["lb", "rb"]); // both shoulder buttons in one frame
 * await pad.disconnect();        // hot-unplug, `gamepaddisconnected` fires
 * await pad.connect();           // hot-plug, `gamepadconnected` fires
 * await pad.setStick("left", 1, 0);
 * await pad.setABSwap(true);     // Nintendo pad: physical A reports as button 1
 * ```
 *
 * Frame discipline: every helper that changes state waits for two animation
 * frames before returning, so the next assertion cannot race the rAF poll. The
 * atomic helpers (`tap`, `hold`, `chord`) do the press, the hold and the
 * release in the page, which keeps the press visible for at least two polls
 * even when the machine is busy.
 */

/** Gamepad "standard" mapping button indices. */
export const PAD_BUTTONS = {
  a: 0,
  b: 1,
  x: 2,
  y: 3,
  lb: 4,
  rb: 5,
  lt: 6,
  rt: 7,
  back: 8,
  start: 9,
  ls: 10,
  rs: 11,
  up: 12,
  down: 13,
  left: 14,
  right: 15,
  home: 16,
} as const;

export type PadButton = keyof typeof PAD_BUTTONS;

/** A named standard button or a raw button index. */
export type PadInput = PadButton | number;

export interface FakePadOptions {
  /** Controller slot; connected pads keep their index across hot-plug. */
  index?: number;
  id?: string;
  mapping?: string;
  connected?: boolean;
  /** Emulates a Nintendo pad: `a` reports on button 1, `b` on button 0. */
  swapAB?: boolean;
  /** Left stick x/y, right stick x/y. */
  axes?: [number, number, number, number];
}

export interface FakePadSnapshot {
  connected: boolean;
  id: string;
  mapping: string;
  swapAB: boolean;
  timestamp: number;
  axes: number[];
  buttons: Array<{ pressed: boolean; value: number; touched: boolean }>;
}

interface FakePadConfig extends Required<FakePadOptions> {
  buttons: Record<string, number>;
  buttonCount: number;
}

interface FakePadBridge {
  down(input: PadInput | PadInput[]): void;
  up(input: PadInput | PadInput[]): void;
  pulse(input: PadInput | PadInput[], holdMs: number): Promise<void>;
  setButton(input: PadInput, value: number): void;
  setAxes(values: number[]): void;
  setStick(side: "left" | "right", x: number, y: number): void;
  connect(): void;
  disconnect(): void;
  setABSwap(enabled: boolean): void;
  bumpTimestamp(): void;
  frames(count: number): Promise<void>;
  state(): FakePadSnapshot;
  events(): string[];
  reset(): void;
}

type PadWindow = Window & { __ludexPad: FakePadBridge };

/**
 * Runs in the page before the application bundle. It is serialized by
 * Playwright, so it may only use its argument and page globals.
 */
function installFakePadScript(config: FakePadConfig): void {
  const names: Record<string, number> = config.buttons;
  let swapAB = config.swapAB;
  const events: string[] = [];

  const pad = {
    index: config.index,
    id: config.id,
    mapping: config.mapping,
    connected: config.connected,
    axes: config.axes.slice(),
    buttons: Array.from({ length: config.buttonCount }, () => ({
      pressed: false,
      value: 0,
      touched: false,
    })),
    timestamp: 0,
  };

  const bump = () => {
    pad.timestamp = performance.now();
  };

  const resolve = (input: PadInput): number => {
    if (typeof input === "number") return input;
    if (input === "a" && swapAB) return 1;
    if (input === "b" && swapAB) return 0;
    const index = names[input];
    if (index === undefined) throw new Error(`unknown pad button: ${String(input)}`);
    return index;
  };

  const resolveAll = (input: PadInput | PadInput[]): number[] =>
    (Array.isArray(input) ? input : [input]).map(resolve);

  const down = (input: PadInput | PadInput[]) => {
    for (const index of resolveAll(input)) {
      const button = pad.buttons[index];
      if (!button) continue;
      button.pressed = true;
      button.value = 1;
      button.touched = true;
    }
    bump();
  };

  const up = (input: PadInput | PadInput[]) => {
    for (const index of resolveAll(input)) {
      const button = pad.buttons[index];
      if (!button) continue;
      button.pressed = false;
      button.value = 0;
      button.touched = false;
    }
    bump();
  };

  const frames = (count: number): Promise<void> =>
    new Promise((resolveFrames) => {
      let left = Math.max(1, Math.floor(count));
      const tick = () => {
        left -= 1;
        if (left <= 0) resolveFrames();
        else requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });

  const pulse = async (input: PadInput | PadInput[], holdMs: number) => {
    down(input);
    // Two polls: one frame to read the press, one to settle any React update.
    await frames(2);
    if (holdMs > 0) await new Promise((resolveHold) => setTimeout(resolveHold, holdMs));
    up(input);
    await frames(2);
  };

  const dispatch = (type: "gamepadconnected" | "gamepaddisconnected") => {
    const event = new Event(type);
    // Real GamepadEvent carries the pad; the app only rescans, but keep shape.
    Object.defineProperty(event, "gamepad", {
      configurable: true,
      value: pad.connected ? pad : null,
    });
    window.dispatchEvent(event);
  };

  const connect = () => {
    if (pad.connected) return;
    pad.connected = true;
    events.push("connected");
    bump();
    dispatch("gamepadconnected");
  };

  const disconnect = () => {
    if (!pad.connected) return;
    pad.connected = false;
    events.push("disconnected");
    bump();
    dispatch("gamepaddisconnected");
  };

  const setAxes = (values: number[]) => {
    pad.axes = [values[0] ?? 0, values[1] ?? 0, values[2] ?? 0, values[3] ?? 0].map((value) =>
      Number.isFinite(value) ? value : 0,
    );
    bump();
  };

  const bridge: FakePadBridge = {
    down,
    up,
    pulse,
    setButton(input, value) {
      const index = resolve(input);
      const button = pad.buttons[index];
      if (!button) return;
      const clamped = Math.min(1, Math.max(0, Number.isFinite(value) ? value : 0));
      button.value = clamped;
      button.pressed = clamped > 0.5;
      button.touched = clamped > 0;
      bump();
    },
    setAxes,
    setStick(side, x, y) {
      const offset = side === "left" ? 0 : 2;
      const next = pad.axes.slice();
      next[offset] = Number.isFinite(x) ? x : 0;
      next[offset + 1] = Number.isFinite(y) ? y : 0;
      setAxes(next);
    },
    connect,
    disconnect,
    setABSwap(enabled) {
      swapAB = Boolean(enabled);
      events.push(`swapAB:${swapAB}`);
    },
    bumpTimestamp: bump,
    frames,
    state() {
      return {
        connected: pad.connected,
        id: pad.id,
        mapping: pad.mapping,
        swapAB,
        timestamp: pad.timestamp,
        axes: pad.axes.slice(),
        buttons: pad.buttons.map((button) => ({ ...button })),
      };
    },
    events: () => events.slice(),
    reset() {
      for (const button of pad.buttons) {
        button.pressed = false;
        button.value = 0;
        button.touched = false;
      }
      setAxes([0, 0, 0, 0]);
    },
  };

  // The API returns a fresh array-like every call, with null holes for the
  // free slots; some WebViews return a plain array, so match that shape.
  Object.defineProperty(navigator, "getGamepads", {
    configurable: true,
    writable: true,
    value: () =>
      Array.from({ length: 4 }, (_unused, index) =>
        index === pad.index && pad.connected ? pad : null,
      ),
  });

  (window as unknown as { __ludexPad: FakePadBridge }).__ludexPad = bridge;
}

const DEFAULT_CONFIG: FakePadConfig = {
  index: 0,
  id: "Xbox Wireless Controller (STANDARD GAMEPAD)",
  mapping: "standard",
  connected: true,
  swapAB: false,
  axes: [0, 0, 0, 0],
  buttons: PAD_BUTTONS as unknown as Record<string, number>,
  buttonCount: 17,
};

/**
 * Installs the stub and returns the controller. Call it before `page.goto`:
 * `addInitScript` only applies to documents created after it is registered.
 */
export async function installFakePad(page: Page, options: FakePadOptions = {}): Promise<FakePad> {
  const config: FakePadConfig = {
    ...DEFAULT_CONFIG,
    ...options,
    axes: options.axes ?? DEFAULT_CONFIG.axes,
  };
  await page.addInitScript(installFakePadScript, config);
  return new FakePad(page);
}

export class FakePad {
  constructor(private readonly page: Page) {}

  /** Presses the inputs and waits two frames so the poll sees them. */
  async press(input: PadInput | PadInput[]): Promise<void> {
    await this.page.evaluate((value) => (window as unknown as PadWindow).__ludexPad.down(value), input);
    await this.waitFrames(2);
  }

  /** Releases the inputs and waits two frames so the repeat tracker clears. */
  async release(input: PadInput | PadInput[]): Promise<void> {
    await this.page.evaluate((value) => (window as unknown as PadWindow).__ludexPad.up(value), input);
    await this.waitFrames(2);
  }

  /** One press and release inside the page; `holdMs` < 380 avoids a repeat. */
  async tap(input: PadInput | PadInput[], holdMs = 40): Promise<void> {
    await this.page.evaluate(
      async ({ value, hold }) => (window as unknown as PadWindow).__ludexPad.pulse(value, hold),
      { value: input, hold: holdMs },
    );
  }

  /** Keeps the inputs pressed for `ms` (crosses the 380 ms repeat delay). */
  async hold(input: PadInput | PadInput[], ms = 250): Promise<void> {
    await this.tap(input, ms);
  }

  /** Presses several inputs in the same frame, then releases them together. */
  async chord(inputs: PadInput[], holdMs = 60): Promise<void> {
    await this.tap(inputs, holdMs);
  }

  /** Sets an analog button value (trigger travel, 0..1) without a release. */
  async setButton(input: PadInput, value: number): Promise<void> {
    await this.page.evaluate(
      ({ target, next }) => (window as unknown as PadWindow).__ludexPad.setButton(target, next),
      { target: input, next: value },
    );
    await this.waitFrames(2);
  }

  /** Sets left stick x/y, right stick x/y. */
  async setAxes(values: number[]): Promise<void> {
    await this.page.evaluate((next) => (window as unknown as PadWindow).__ludexPad.setAxes(next), values);
    await this.waitFrames(2);
  }

  /** Moves one stick, leaving the other alone. */
  async setStick(side: "left" | "right", x: number, y: number): Promise<void> {
    await this.page.evaluate(
      ({ stick, x: nextX, y: nextY }) =>
        (window as unknown as PadWindow).__ludexPad.setStick(stick, nextX, nextY),
      { stick: side, x, y },
    );
    await this.waitFrames(2);
  }

  /** Emulates a Nintendo pad for the next presses (`a` becomes button 1). */
  async setABSwap(enabled: boolean): Promise<void> {
    await this.page.evaluate((value) => (window as unknown as PadWindow).__ludexPad.setABSwap(value), enabled);
  }

  /** Hot-plug: marks the pad connected and fires `gamepadconnected`. */
  async connect(): Promise<void> {
    await this.page.evaluate(() => (window as unknown as PadWindow).__ludexPad.connect());
    await this.waitFrames(2);
  }

  /** Hot-unplug: fires `gamepaddisconnected`; `getGamepads` returns a null slot. */
  async disconnect(): Promise<void> {
    await this.page.evaluate(() => (window as unknown as PadWindow).__ludexPad.disconnect());
    await this.waitFrames(2);
  }

  /** Advances the pad timestamp without changing input (poll/timeout probes). */
  async bumpTimestamp(): Promise<void> {
    await this.page.evaluate(() => (window as unknown as PadWindow).__ludexPad.bumpTimestamp());
  }

  /** Waits for `count` animation frames; the default covers two polls. */
  async waitFrames(count = 2): Promise<void> {
    await this.page.evaluate((value) => (window as unknown as PadWindow).__ludexPad.frames(value), count);
  }

  /** Reads the pad the page sees, not the harness's own bookkeeping. */
  async state(): Promise<FakePadSnapshot> {
    return this.page.evaluate(() => (window as unknown as PadWindow).__ludexPad.state());
  }

  /** Hot-plug history, e.g. ["connected", "swapAB:true", "disconnected"]. */
  async events(): Promise<string[]> {
    return this.page.evaluate(() => (window as unknown as PadWindow).__ludexPad.events());
  }

  /** Releases every button and centres both sticks; keeps the pad connected. */
  async reset(): Promise<void> {
    await this.page.evaluate(() => (window as unknown as PadWindow).__ludexPad.reset());
    await this.waitFrames(2);
  }
}
