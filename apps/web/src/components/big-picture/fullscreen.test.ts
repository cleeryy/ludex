import { describe, expect, it } from "vitest";
import {
  bigPictureFullscreenPlan,
  fullscreenRestoreOnExit,
  type FullscreenPlanInput,
} from "./fullscreen";

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

/* --------------------------------------------------------------------------
   #269: the native window policy replaces the console-mode inference. The
   truth table covers Windows/Linux/macOS through the emitted policy; the only
   platform difference is decorations, which Big Picture does not touch.
   -------------------------------------------------------------------------- */

const base: FullscreenPlanInput = {
  native: true,
  policy: { fullscreen: false, tray: true },
  legacyConsoleReady: true,
  legacyConsoleActive: false,
  legacyConsoleFullscreen: false,
};

describe("bigPictureFullscreenPlan", () => {
  it("takes the desktop window and hands it back on exit", () => {
    expect(bigPictureFullscreenPlan(base)).toBe("take-over");
  });

  it("never touches a console window, fullscreen or --windowed", () => {
    // tray: false is the policy's console session (it never has a tray).
    expect(bigPictureFullscreenPlan({ ...base, policy: { fullscreen: true, tray: false } })).toBe(
      "native",
    );
    expect(bigPictureFullscreenPlan({ ...base, policy: { fullscreen: false, tray: false } })).toBe(
      "native",
    );
  });

  it("leaves a policy-fullscreen desktop window to Rust", () => {
    expect(bigPictureFullscreenPlan({ ...base, policy: { fullscreen: true, tray: true } })).toBe(
      "native",
    );
  });

  it("does nothing in the browser build", () => {
    expect(bigPictureFullscreenPlan({ ...base, native: false })).toBe("none");
  });

  it("waits while the policy is loading", () => {
    expect(bigPictureFullscreenPlan({ ...base, policy: undefined })).toBe("wait");
  });

  it("keeps the pre-269 behaviour when the desktop has no policy", () => {
    // Older desktop: fall back to console_mode(), the way the screen used to.
    expect(bigPictureFullscreenPlan({ ...base, policy: null })).toBe("take-over");
    expect(
      bigPictureFullscreenPlan({
        ...base,
        policy: null,
        legacyConsoleReady: false,
      }),
    ).toBe("wait");
    expect(
      bigPictureFullscreenPlan({
        ...base,
        policy: null,
        legacyConsoleActive: true,
        legacyConsoleFullscreen: false,
      }),
    ).toBe("native");
    expect(
      bigPictureFullscreenPlan({
        ...base,
        policy: null,
        legacyConsoleActive: true,
        legacyConsoleFullscreen: true,
      }),
    ).toBe("take-over");
  });
});
