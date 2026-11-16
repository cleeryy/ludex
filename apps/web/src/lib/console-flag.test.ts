// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  CONSOLE_SHELL_STORAGE_KEY,
  consoleShellEnabled,
  setConsoleShellEnabled,
  subscribeConsoleShell,
} from "@/lib/console-flag";

describe("console_v2 device flag (#267)", () => {
  beforeEach(() => {
    window.localStorage.clear();
    setConsoleShellEnabled(false);
  });

  it("defaults off and stays out of the account preference document", () => {
    expect(CONSOLE_SHELL_STORAGE_KEY).toBe("ludex.console.shell");
    expect(consoleShellEnabled()).toBe(false);
    // The flag is device-local: turning it on writes only the console key, so
    // no account preference (theme, language, layout) is touched.
    const before = { ...window.localStorage };
    setConsoleShellEnabled(true);
    expect(consoleShellEnabled()).toBe(true);
    expect(window.localStorage.getItem(CONSOLE_SHELL_STORAGE_KEY)).toBe("1");
    const added = Object.keys({ ...window.localStorage }).filter((key) => !(key in before));
    expect(added).toEqual([CONSOLE_SHELL_STORAGE_KEY]);
  });

  it("notifies subscribers once per real change", () => {
    const listener = vi.fn();
    const off = subscribeConsoleShell(listener);
    setConsoleShellEnabled(true);
    setConsoleShellEnabled(true);
    expect(listener).toHaveBeenCalledTimes(1);
    setConsoleShellEnabled(false);
    expect(listener).toHaveBeenCalledTimes(2);
    off();
    setConsoleShellEnabled(true);
    expect(listener).toHaveBeenCalledTimes(2);
  });

  it("treats a storage that refuses writes as off instead of throwing", () => {
    const spy = vi.spyOn(window.localStorage, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(consoleShellEnabled()).toBe(false);
    spy.mockRestore();
  });
});
