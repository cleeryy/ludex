import { useCallback, useEffect, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { isNative, native } from "@/lib/native";
import { isOverlayShortcut } from "@/lib/overlay";
import { OverlayScreen } from "@/components/overlay/overlay-screen";

/**
 * In-app overlay host (#144 phase 1).
 *
 * Mounted inside the router of the main window. Shift+Tab (and the desktop's
 * `overlay://open` event) draws the same panel above the app without a second
 * window, which works on every platform and when the native listener is
 * unavailable. The backend decides the presentation: it emits `overlay://open`
 * only when the main window is focused, and otherwise uses the floating
 * window.
 */
export function OverlayHost() {
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
  const location = useLocation();

  const close = useCallback(() => {
    setOpen(false);
    if (isNative()) void native.closeOverlay().catch(() => undefined);
  }, []);

  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    const offs: Array<() => void> = [];
    const add = (pending: Promise<() => void>) => {
      void pending.then((off) => {
        if (disposed) off();
        else offs.push(off);
      });
    };
    add(native.onOverlayOpen(() => setOpen(true)));
    add(native.onOverlayClose(() => setOpen(false)));
    add(
      native.onOverlayNavigate((path) => {
        setOpen(false);
        navigate(path);
      }),
    );
    return () => {
      disposed = true;
      for (const off of offs) off();
    };
  }, [navigate]);

  // Shift+Tab is the keyboard trigger. While the overlaid panel is open it
  // closes it; otherwise the desktop command opens the in-app overlay when the
  // main window is focused. A browser preview opens the panel locally so the
  // layout can be reviewed without the desktop shell.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isOverlayShortcut(event)) return;
      // While the panel is open its own focus trap owns Tab; the global
      // shortcut only opens and closes it from the app underneath.
      if (event.target instanceof HTMLElement && event.target.closest("[data-overlay-mode]")) return;
      event.preventDefault();
      if (open) {
        close();
        return;
      }
      if (!isNative()) {
        setOpen(true);
        return;
      }
      // Ask the backend first: a floating overlay opened by the controller
      // listener must close on Shift+Tab too, even when this window never saw
      // the `overlay://open` event.
      void native
        .overlayStatus()
        .then((status) => {
          if (status?.open) return native.closeOverlay().then(() => setOpen(false));
          return native.openOverlay().then((opened) => {
            if (opened.presentation === "inApp") setOpen(true);
          });
        })
        // A rejected open (disabled overlay, game opt-out) must stay closed;
        // the settings panel explains how to enable it.
        .catch(() => undefined);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [close, open]);

  // A route change while the overlay is open means the game launched from the
  // Library tab; the overlay closes before the new screen takes focus.
  useEffect(() => {
    setOpen(false);
  }, [location.pathname]);

  if (!open) return null;
  return <OverlayScreen mode="in-app" onClose={() => setOpen(false)} />;
}
