/* --------------------------------------------------------------------------
   Horizontal-only scrolling (#237).

   `Element.scrollIntoView({ block: "nearest" })` also scrolls the vertical
   ancestor, so a chip row that is already visible horizontally could still
   drag `main[data-scroll-root]` 796 px down on a phone. This module computes
   the horizontal offset directly and touches nothing else.
   -------------------------------------------------------------------------- */

export interface CenteredScrollInput {
  /** The scroller's viewport and content widths. */
  viewportWidth: number;
  contentWidth: number;
  /** Position of the scroller and of the target inside the same coordinates. */
  viewportLeft: number;
  targetLeft: number;
  targetWidth: number;
  /** The scroller's current offset. */
  scrollLeft: number;
}

/** Offset that centres `target` in `scroller`, clamped to the scrollable range. */
export function centeredScrollLeft({
  viewportWidth,
  contentWidth,
  viewportLeft,
  targetLeft,
  targetWidth,
  scrollLeft,
}: CenteredScrollInput): number {
  const target =
    scrollLeft + (targetLeft - viewportLeft) - Math.max(0, (viewportWidth - targetWidth) / 2);
  const max = Math.max(0, contentWidth - viewportWidth);
  return Math.min(max, Math.max(0, target));
}
