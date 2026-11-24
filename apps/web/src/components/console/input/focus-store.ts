/**
 * Focus bookkeeping for the input scope stack (#267).
 *
 * A scope that opens on top of another surface (the command menu over the
 * library, the tour over a screen) must not guess where focus goes when it
 * closes: the exact element that owned the ring before is restored. The
 * snapshot is taken when the scope is pushed and applied when it is popped,
 * which is what makes B land on the game the player left.
 *
 * Pure DOM helpers, no React: the provider calls them, the tests call them.
 */

export interface FocusSnapshot {
  /** The element that had DOM focus or wore the gamepad ring. */
  target: HTMLElement | null;
  /** Whether the target also wore the `data-gamepad-focus` mark. */
  marked: boolean;
  /** `true` when the snapshot came from an explicit mark, not `activeElement`. */
  fromMark: boolean;
}

function asElement(node: Element | null): HTMLElement | null {
  return node instanceof HTMLElement ? node : null;
}

/**
 * Capture the current focus owner. The ring mark wins over `activeElement`:
 * a mouse click on a list row can leave focus on `body` while the ring still
 * sits on the item the pad selected.
 */
export function captureFocus(doc: Document | null = typeof document === "undefined" ? null : document): FocusSnapshot {
  if (!doc) return { target: null, marked: false, fromMark: false };
  const marked = asElement(doc.querySelector("[data-gamepad-focus='true']"));
  if (marked) return { target: marked, marked: true, fromMark: true };
  const active = asElement(doc.activeElement);
  if (!active || active === doc.body) return { target: null, marked: false, fromMark: false };
  return { target: active, marked: active.hasAttribute("data-gamepad-focus"), fromMark: false };
}

/** Is the element still in the document (an overlay may have unmounted it)? */
function isConnected(element: HTMLElement | null): element is HTMLElement {
  return Boolean(element && element.isConnected);
}

/**
 * Put focus and the ring back on the captured target. A surface that no longer
 * exists (the card was filtered out, the overlay's own row was removed) is a
 * no-op: the caller keeps whatever the browser considers focused rather than
 * moving the ring to an arbitrary element.
 *
 * A restore draws the ring it captured and nothing else (#270). The surface
 * underneath may have re-focused while the scope was on top — closing a sheet
 * re-centres the rail for the game the player left — and sweeping every mark
 * here would leave the player with no ring at all on the frame the sheet
 * disappears. Focus still moves back; a ring the snapshot never owned is not
 * the restore's to erase.
 */
export function restoreFocus(
  snapshot: FocusSnapshot,
  doc: Document | null = typeof document === "undefined" ? null : document,
): boolean {
  if (!doc) return false;
  const target = snapshot.target;
  if (!isConnected(target)) return false;
  if (snapshot.marked || snapshot.fromMark) {
    doc.querySelectorAll("[data-gamepad-focus]").forEach((element) => element.removeAttribute("data-gamepad-focus"));
    target.setAttribute("data-gamepad-focus", "true");
  }
  try {
    target.focus({ preventScroll: true });
  } catch {
    return false;
  }
  return true;
}

/**
 * Single focus-mark owner (#267): move the gamepad focus ring to `target`,
 * clearing any stale mark across the document first. Surfaces never run
 * ad-hoc `querySelectorAll("[data-gamepad-focus]")` loops themselves.
 */
export function setFocusMark(
  target: HTMLElement | null,
  options: { focus?: boolean } = {},
  doc: Document | null = typeof document === "undefined" ? null : document,
): boolean {
  if (!doc || !isConnected(target)) return false;
  clearFocusMarks("[data-gamepad-focus]", doc);
  target.setAttribute("data-gamepad-focus", "true");
  if (options.focus !== false) {
    try {
      target.focus({ preventScroll: true });
    } catch {
      return false;
    }
  }
  return true;
}

/** Remove gamepad focus marks matching `selector` (default: all marks). */
export function clearFocusMarks(
  selector = "[data-gamepad-focus]",
  doc: Document | null = typeof document === "undefined" ? null : document,
): void {
  if (!doc) return;
  doc.querySelectorAll(selector).forEach((element) => element.removeAttribute("data-gamepad-focus"));
}
