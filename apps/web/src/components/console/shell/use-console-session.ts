import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { isNative, native } from "@/lib/native";
import { t } from "@/lib/i18n";
import { useWindowPolicy } from "@/lib/window-policy";
import type { ConsoleStatus } from "@/lib/types";
import { bigPictureFullscreenPlan, fullscreenRestoreOnExit } from "@/components/big-picture/fullscreen";
import { useConsoleNotice } from "@/components/console/surfaces/console-notice";

/**
 * Device session of the Console shell (#267): the native console status, OS
 * fullscreen, pad battery, the running game and the online flag.
 *
 * These used to be eleven effects inside the 2,748-line screen. They have
 * nothing to do with the library, the focus model or the overlays, and they are
 * the part a second window would share, so they live in one hook that answers
 * "what does this machine allow right now".
 */
export interface ConsoleSession {
  online: boolean;
  padBattery: { level: number; charging: boolean } | null;
  consoleMode: ConsoleStatus | null;
  consoleActive: boolean;
  runningGame: { sessionId: string; gameId: number; title: string } | null;
  setRunningGame: React.Dispatch<React.SetStateAction<{ sessionId: string; gameId: number; title: string } | null>>;
  /** Console session: quit the app; desktop: leave for the library room. */
  exit: () => void;
}

export function useConsoleSession(): ConsoleSession {
  const navigate = useNavigate();
  const { notify } = useConsoleNotice();
  const [online, setOnline] = useState(() => (typeof navigator === "undefined" ? true : navigator.onLine));
  const [padBattery, setPadBattery] = useState<{ level: number; charging: boolean } | null>(null);
  const [runningGame, setRunningGame] = useState<{ sessionId: string; gameId: number; title: string } | null>(null);
  const [consoleMode, setConsoleMode] = useState<ConsoleStatus | null>(null);
  const consoleActive = consoleMode?.active === true;

  useEffect(() => {
    const update = () => setOnline(navigator.onLine);
    window.addEventListener("online", update);
    window.addEventListener("offline", update);
    return () => {
      window.removeEventListener("online", update);
      window.removeEventListener("offline", update);
    };
  }, []);

  // Pad battery only exists through the native listener; the Gamepad API does
  // not report it. Absent (or a pad with no battery) hides the slot instead of
  // showing a fabricated value.
  useEffect(() => {
    if (!isNative()) return;
    let cancelled = false;
    const load = () => {
      void native
        .overlayStatus()
        .then((status) => {
          if (cancelled) return;
          const controllers = status?.listener.controllers ?? [];
          const active = controllers.find((controller) => controller.active) ?? controllers[0];
          setPadBattery(
            active && active.battery !== null
              ? { level: Math.round(active.battery), charging: active.charging }
              : null,
          );
        })
        .catch(() => {
          /* the listener is optional; the bar simply omits the battery */
        });
    };
    load();
    const timer = window.setInterval(load, 60_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  // A game launched from the Console is the only one this shell can stop, and
  // the exit event clears it even when the player leaves through the game.
  useEffect(() => {
    if (!isNative()) return;
    let cancelled = false;
    let off: (() => void) | undefined;
    void native
      .onGameExited(({ gameId }) => {
        setRunningGame((current) => (current && current.gameId === gameId ? null : current));
      })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else off = unlisten;
      })
      .catch(() => {
        /* no session tracking without the desktop event bridge */
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  useEffect(() => {
    if (!isNative()) return;
    let cancelled = false;
    void native
      .consoleMode()
      .then((status) => {
        if (!cancelled) setConsoleMode(status);
      })
      .catch(() => {
        /* the desktop build always answers; a failure stays in normal mode */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // The console owns the OS window in the desktop app: enter real fullscreen on
  // mount and hand the window back on the way out.
  //
  // #269: Rust resolves the window policy once (`window://state`); this reads it
  // instead of re-deriving fullscreen from `console_mode()` plus
  // `isFullscreen()` (#233). A console session — fullscreen or `--windowed` —
  // owns its window and is never touched from here. With no policy (older
  // desktop) the plan falls back to the pre-#269 console inference.
  const windowPolicy = useWindowPolicy();
  // A primitive: an unchanged plan must not re-run the effect when a settings
  // save emits a new policy object.
  const fullscreenPlan = bigPictureFullscreenPlan({
    native: isNative(),
    policy: windowPolicy,
    legacyConsoleReady: consoleMode !== null,
    legacyConsoleActive: consoleActive,
    legacyConsoleFullscreen: consoleMode?.fullscreen === true,
  });
  useEffect(() => {
    if (fullscreenPlan !== "take-over") return;
    let unmounted = false;
    let wasFullscreen = false;
    let entered = false;

    const enter = async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        wasFullscreen = await win.isFullscreen();
        if (unmounted || wasFullscreen) return;
        await win.setFullscreen(true);
        entered = true;
      } catch (cause) {
        console.warn("Console could not enter fullscreen", cause);
        if (!unmounted) {
          notify({
            title: "Fullscreen unavailable",
            description: t("Staying in the in-app view."),
          });
        }
      }
    };

    const start = enter();

    // Cleanup waits for a still-running enter so it knows what the window
    // looked like before the Console touched it, and hands it back in that
    // state (#233).
    return () => {
      unmounted = true;
      void start.then(async () => {
        if (fullscreenRestoreOnExit({ wasFullscreen, entered }) !== "exit") return;
        try {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          await getCurrentWindow().setFullscreen(false);
        } catch (cause) {
          console.warn("Console could not leave fullscreen", cause);
        }
      });
    };
  }, [notify, fullscreenPlan]);

  const exit = useCallback(() => {
    // A console session (#172) has one exit: quit back to the host desktop.
    if (consoleActive) {
      void native.exitConsole().catch((cause) => {
        console.warn("Console mode could not quit", cause);
        navigate("/library", { replace: true });
      });
      return;
    }
    // #267: never navigate to `/` here — with the device flag on, the root
    // redirects straight back into the Console and the exit would loop. The
    // desktop library is the honest "leave the console" target.
    navigate("/library");
  }, [consoleActive, navigate]);

  return {
    online,
    padBattery,
    consoleMode,
    consoleActive,
    runningGame,
    setRunningGame,
    exit,
  };
}
