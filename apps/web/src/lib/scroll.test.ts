import { describe, expect, it } from "vitest";
import { centeredScrollLeft } from "./scroll";

/* --------------------------------------------------------------------------
   Horizontal-only scroll (#237).

   `/game/:id` opened 796 px down on a 390x844 phone because the chip row used
   `scrollIntoView({ block: "nearest" })`, which also scrolls the vertical
   ancestor. The replacement only ever computes a scrollLeft, so the page
   offset cannot move. These cases pin the arithmetic, including the 796 px
   regression: a chip that is already visible must produce a no-op.
   -------------------------------------------------------------------------- */

describe("centeredScrollLeft", () => {
  it("returns 0 when the target is already centred and the content fits", () => {
    expect(
      centeredScrollLeft({
        viewportWidth: 390,
        contentWidth: 390,
        viewportLeft: 0,
        targetLeft: 120,
        targetWidth: 150,
        scrollLeft: 0,
      }),
    ).toBe(0);
  });

  it("centres a chip that is off to the right", () => {
    // 390 px viewport, 900 px of chips, chip at 600..710.
    expect(
      centeredScrollLeft({
        viewportWidth: 390,
        contentWidth: 900,
        viewportLeft: 0,
        targetLeft: 600,
        targetWidth: 110,
        scrollLeft: 0,
      }),
    ).toBe(460);
  });

  it("accounts for an already scrolled list", () => {
    // The chip is 200 px from the left edge of the viewport at scrollLeft 400.
    expect(
      centeredScrollLeft({
        viewportWidth: 390,
        contentWidth: 900,
        viewportLeft: 0,
        targetLeft: 200,
        targetWidth: 90,
        scrollLeft: 400,
      }),
    ).toBe(450);
  });

  it("clamps to the scrollable range instead of overscrolling", () => {
    expect(
      centeredScrollLeft({
        viewportWidth: 390,
        contentWidth: 900,
        viewportLeft: 0,
        targetLeft: 860,
        targetWidth: 100,
        scrollLeft: 0,
      }),
    ).toBe(510);
    expect(
      centeredScrollLeft({
        viewportWidth: 390,
        contentWidth: 900,
        viewportLeft: 0,
        targetLeft: 0,
        targetWidth: 100,
        scrollLeft: 200,
      }),
    ).toBe(55);
  });

  it("never returns a negative offset", () => {
    expect(
      centeredScrollLeft({
        viewportWidth: 390,
        contentWidth: 900,
        viewportLeft: 0,
        targetLeft: 10,
        targetWidth: 40,
        scrollLeft: 0,
      }),
    ).toBe(0);
  });
});
