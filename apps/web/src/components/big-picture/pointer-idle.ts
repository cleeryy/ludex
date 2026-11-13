import { useEffect, type RefObject } from "react";

/* --------------------------------------------------------------------------
   Pointer idle (#218).

   Big Picture is driven from a couch. A mouse cursor parked in the middle of
   the artwork is the one desktop leftover on the TV screen, so the screen
   hides it after a short pause and restores it the moment the pointer moves.
   The state is one attribute on the screen root: a mouse move must never
   re-render the library, and the stylesheet owns the visual (see the
   `[data-pointer-idle]` rules in `styles/index.css`).
   -------------------------------------------------------------------------- */

/** Long enough to not blink between two clicks, short enough to feel deliberate. */
export const POINTER_IDLE_MS = 1600;

/** The attribute the stylesheet reads. */
export const POINTER_IDLE_ATTRIBUTE = "data-pointer-idle";

/** Events that prove the pointer is back in the user's hand. */
export const POINTER_ACTIVITY_EVENTS = ["pointermove", "pointerdown", "wheel"] as const;

/**
 * Whether the pointer has been still long enough to hide. Pure so the timing is
 * testable without a DOM; a clock that jumps backwards never hides it.
 */
export function pointerIdleExpired(
  now: number,
  lastActivityAt: number,
  delayMs = POINTER_IDLE_MS,
): boolean {
  return now - lastActivityAt >= delayMs;
}

/**
 * Hides the cursor on `target` after `delayMs` without pointer movement and
 * restores it on activity and on unmount. `cursor: none` only hides the
 * pointer: clicks, menus and inputs keep working, and the first movement of
 * the mouse brings it back.
 */
export function usePointerIdle(target: RefObject<HTMLElement | null>, delayMs = POINTER_IDLE_MS): void {
  useEffect(() => {
    const root = target.current;
    if (!root || typeof window === "undefined") return;
    let timer = 0;
    let lastActivityAt = performance.now();

    const hideIfIdle = () => {
      const now = performance.now();
      if (!pointerIdleExpired(now, lastActivityAt, delayMs)) {
        // A throttled timer can fire early after a tab switch; wait the rest
        // instead of hiding a cursor that just moved.
        timer = window.setTimeout(hideIfIdle, Math.max(16, delayMs - (now - lastActivityAt)));
        return;
      }
      target.current?.setAttribute(POINTER_IDLE_ATTRIBUTE, "true");
    };

    const wake = () => {
      lastActivityAt = performance.now();
      target.current?.removeAttribute(POINTER_IDLE_ATTRIBUTE);
      window.clearTimeout(timer);
      timer = window.setTimeout(hideIfIdle, delayMs);
    };

    wake();
    for (const event of POINTER_ACTIVITY_EVENTS) {
      window.addEventListener(event, wake, { passive: true });
    }

    return () => {
      window.clearTimeout(timer);
      for (const event of POINTER_ACTIVITY_EVENTS) {
        window.removeEventListener(event, wake);
      }
      target.current?.removeAttribute(POINTER_IDLE_ATTRIBUTE);
    };
  }, [target, delayMs]);
}
