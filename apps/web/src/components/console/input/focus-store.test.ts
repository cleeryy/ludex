// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";

import { captureFocus, restoreFocus } from "@/components/console/input/focus-store";
import { applyGamepadFocus } from "@/lib/gamepad-core";

/**
 * The ring's two writers (#270). A scope pop restores the snapshot it captured;
 * the surface underneath may have drawn its own ring while the scope was on
 * top. These tests pin which of the two owns the mark.
 */

function button(label: string): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.textContent = label;
  document.body.appendChild(element);
  return element;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("focus restore", () => {
  it("puts the mark back on the element the snapshot owned", () => {
    const card = button("Chrono Trigger");
    const other = button("Elsewhere");
    applyGamepadFocus([card], 0);
    const snapshot = captureFocus();

    // The sheet took the ring for itself while it was up.
    applyGamepadFocus([other], 0);
    expect(other.hasAttribute("data-gamepad-focus")).toBe(true);

    expect(restoreFocus(snapshot)).toBe(true);
    expect(card.hasAttribute("data-gamepad-focus")).toBe(true);
    expect(other.hasAttribute("data-gamepad-focus")).toBe(false);
    expect(document.activeElement).toBe(card);
  });

  it("keeps a ring the snapshot never owned", () => {
    // A pointer left DOM focus on the card without the pad ring, so the sheet
    // was pushed with an unmarked snapshot...
    const card = button("Chrono Trigger");
    card.focus();
    const snapshot = captureFocus();
    expect(snapshot.marked).toBe(false);

    // ...and the surface drew the ring while the sheet was up (the shell
    // re-centres the rail as it closes).
    const intent = button("Intent");
    intent.setAttribute("data-gamepad-focus", "true");

    // Restoring focus must not sweep a mark it is not going to redraw: that
    // left the player with no ring at all on the frame the sheet disappeared.
    expect(restoreFocus(snapshot)).toBe(true);
    expect(document.activeElement).toBe(card);
    expect(intent.hasAttribute("data-gamepad-focus")).toBe(true);
  });

  it("leaves focus alone when the snapshot target is gone", () => {
    const card = button("Gone");
    applyGamepadFocus([card], 0);
    const snapshot = captureFocus();
    card.remove();

    const intent = button("Intent");
    intent.setAttribute("data-gamepad-focus", "true");
    expect(restoreFocus(snapshot)).toBe(false);
    expect(intent.hasAttribute("data-gamepad-focus")).toBe(true);
  });
});
