import { useEffect, useState } from "react";
import { isNative, native } from "@/lib/native";
import { useConsoleShellFlag } from "@/lib/console-flag";
import { CONSOLE_ROOT } from "@/components/console/routes/surface";

/**
 * Which shell the device opens on `/` (#267).
 *
 * The device flag is synchronous (`localStorage`). Console mode is a native
 * probe; it is cached for the process so the root route does not flash the
 * desktop home while waiting for the first answer. `pending` lets the gate keep
 * the boot screen up instead of painting a desktop it is about to leave.
 */
let nativeActive: boolean | null = null;
let nativeProbe: Promise<boolean> | null = null;

function probeNativeConsole(): Promise<boolean> {
  if (nativeActive !== null) return Promise.resolve(nativeActive);
  if (!nativeProbe) {
    nativeProbe = native
      .consoleMode()
      .then((status) => {
        nativeActive = status.active;
        return status.active;
      })
      .catch(() => {
        nativeActive = false;
        return false;
      });
  }
  return nativeProbe;
}

export interface ConsoleEntryState {
  /** The root route belongs to the Console. */
  active: boolean;
  /** The native answer is still in flight; do not paint a desktop home yet. */
  pending: boolean;
}

/**
 * The root/login redirect, as a pure rule: a console device never shows the
 * desktop login, and its home is `/console`. `/holdLogin` is handled by Gate.
 */
export function consoleEntryRedirect(pathname: string, state: ConsoleEntryState): string | null {
  if (state.active && (pathname === "/" || pathname === "/login")) return CONSOLE_ROOT;
  return null;
}

export function useConsoleEntry(): ConsoleEntryState {
  const flag = useConsoleShellFlag();
  const [nativeState, setNativeState] = useState<boolean | null>(nativeActive);

  useEffect(() => {
    if (!isNative() || nativeState !== null) return;
    let cancelled = false;
    void probeNativeConsole().then((active) => {
      if (!cancelled) setNativeState(active);
    });
    return () => {
      cancelled = true;
    };
  }, [nativeState]);

  return {
    active: flag || nativeState === true,
    pending: !flag && isNative() && nativeState === null,
  };
}
