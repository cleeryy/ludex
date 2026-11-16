import { describe, expect, it } from "vitest";
import {
  hasBlockingModal,
  hasModalDialog,
  moveModalFocus,
  activateFocusedControl,
  type FocusTarget,
} from "./gamepad-core";

/* --------------------------------------------------------------------------
   Controller escape and dialog walking (#237).

   Console's Install primary opens a desktop modal, and a couch player used to
   be able to open it and then only close it. A pad now walks the dialog's focus
   stops and presses them, while a "pad layer" (the onboarding tour) blocks the
   screen behind it without blocking itself.
   -------------------------------------------------------------------------- */

function target(name: string, options: { hidden?: boolean } = {}): FocusTarget & {
  name: string;
  focused: boolean;
  clicks: number;
} {
  return {
    name,
    focused: false,
    clicks: 0,
    offsetParent: options.hidden ? null : {},
    focus() {
      this.focused = true;
    },
    setAttribute() {
      /* the ring attribute is not what this test asserts */
    },
    removeAttribute() {
      /* idem */
    },
    scrollIntoView() {
      /* jsdom has no layout */
    },
    click() {
      this.clicks += 1;
    },
  };
}

function host(targets: FocusTarget[]): {
  querySelector(selector: string): unknown;
  querySelectorAll(): ArrayLike<FocusTarget>;
} {
  return {
    querySelector: () => null,
    querySelectorAll: () => targets,
  };
}

describe("blocking surfaces", () => {
  it("treats a real modal and a declared pad layer as blocking", () => {
    const seen: string[] = [];
    const query = (selector: string) => {
      seen.push(selector);
      return selector.includes("data-pad-layer") ? {} : null;
    };
    expect(hasBlockingModal({ querySelector: query })).toBe(true);
    expect(seen[0]).toContain('[role="dialog"][aria-modal="true"]');
    expect(seen[0]).toContain('[data-pad-layer="true"]');
  });

  it("does not let a pad layer block the loop that owns it", () => {
    const layerOnly = { querySelector: (selector: string) => (selector.includes("data-pad-layer") ? {} : null) };
    expect(hasBlockingModal(layerOnly)).toBe(true);
    expect(hasModalDialog(layerOnly)).toBe(false);
  });

  it("survives a host that throws", () => {
    expect(
      hasBlockingModal({
        querySelector: () => {
          throw new Error("detached");
        },
      }),
    ).toBe(false);
  });
});

describe("moveModalFocus", () => {
  it("starts at the first stop when nothing inside has focus", () => {
    const first = target("first");
    const second = target("second");
    const surface = host([first, second]);
    const moved = moveModalFocus(surface, null, 1);
    expect(moved).toBe(first);
    expect(first.focused).toBe(true);
  });

  it("walks forward and wraps", () => {
    const first = target("first");
    const second = target("second");
    const surface = host([first, second]);
    expect(moveModalFocus(surface, first, 1)).toBe(second);
    expect(moveModalFocus(surface, second, 1)).toBe(first);
  });

  it("walks backward and wraps to the last stop", () => {
    const first = target("first");
    const last = target("last");
    const surface = host([first, last]);
    expect(moveModalFocus(surface, first, -1)).toBe(last);
    expect(moveModalFocus(surface, last, -1)).toBe(first);
  });

  it("skips hidden stops and refuses an empty dialog", () => {
    const hidden = target("hidden", { hidden: true });
    const visible = target("visible");
    expect(moveModalFocus(host([hidden, visible]), null, 1)).toBe(visible);
    expect(moveModalFocus(host([]), null, 1)).toBeNull();
    expect(moveModalFocus(null, null, 1)).toBeNull();
  });
});

describe("activateFocusedControl", () => {
  it("clicks the focused control", () => {
    const button = target("ok");
    expect(activateFocusedControl(button)).toBe(true);
    expect(button.clicks).toBe(1);
  });

  it("ignores focus that is not a control", () => {
    expect(activateFocusedControl({ focus() {} })).toBe(false);
    expect(activateFocusedControl(null)).toBe(false);
    expect(activateFocusedControl(undefined)).toBe(false);
  });
});
