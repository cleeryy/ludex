import { describe, expect, it } from "vitest";
import { fullscreenRestoreOnExit, shouldEnterFullscreen } from "./fullscreen";

describe("fullscreenRestoreOnExit", () => {
  it("hands the window back when Big Picture entered fullscreen itself", () => {
    expect(fullscreenRestoreOnExit({ wasFullscreen: false, entered: true })).toBe("exit");
  });

  it("hands the window back when it was already fullscreen on mount", () => {
    // The trapped case from #233: the window was fullscreen before this screen
    // existed, so nothing inside it ever offered a way out.
    expect(fullscreenRestoreOnExit({ wasFullscreen: true, entered: false })).toBe("exit");
  });

  it("leaves a windowed window alone", () => {
    expect(fullscreenRestoreOnExit({ wasFullscreen: false, entered: false })).toBe("none");
  });

  it("hands the window back even if it was fullscreen and the enter was redundant", () => {
    expect(fullscreenRestoreOnExit({ wasFullscreen: true, entered: true })).toBe("exit");
  });
});

describe("shouldEnterFullscreen", () => {
  const base = {
    isNative: true,
    consoleReady: true,
    consoleActive: false,
    consoleFullscreen: false,
    alreadyFullscreen: false,
  };

  it("enters on a plain manual visit", () => {
    expect(shouldEnterFullscreen(base)).toBe(true);
  });

  it("leaves an already-fullscreen window to the restore path", () => {
    expect(shouldEnterFullscreen({ ...base, alreadyFullscreen: true })).toBe(false);
  });

  it("does nothing in the browser build", () => {
    expect(shouldEnterFullscreen({ ...base, isNative: false })).toBe(false);
  });

  it("waits for the console status", () => {
    expect(shouldEnterFullscreen({ ...base, consoleReady: false })).toBe(false);
  });

  it("does not fight a console session that started windowed", () => {
    expect(
      shouldEnterFullscreen({ ...base, consoleActive: true, consoleFullscreen: false }),
    ).toBe(false);
  });

  it("enters for a console session that runs fullscreen", () => {
    expect(
      shouldEnterFullscreen({ ...base, consoleActive: true, consoleFullscreen: true }),
    ).toBe(true);
  });
});
