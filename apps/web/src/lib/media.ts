/**
 * Bounded media rendering helpers (#159).
 *
 * The game page mounts the gallery only after the details render, and the strip
 * never mounts every screenshot at once: a large gallery is revealed a page at
 * a time as the sentinel scrolls into view. Keeping the arithmetic pure makes
 * the cap testable without a DOM.
 */

/** Tiles mounted per page of the media strip. */
export const MEDIA_PAGE_SIZE = 8;

/**
 * Clamp a requested window to the item count, never below one page (unless
 * there are fewer items). A non-finite request falls back to one page.
 */
export function mediaWindow(
  total: number,
  requested: number,
  pageSize: number = MEDIA_PAGE_SIZE,
): number {
  if (!Number.isFinite(total) || total <= 0) return 0;
  const page = Number.isFinite(pageSize) && pageSize >= 1 ? Math.floor(pageSize) : MEDIA_PAGE_SIZE;
  const wanted = Number.isFinite(requested) && requested > 0 ? Math.floor(requested) : page;
  return Math.min(Math.max(page, wanted), Math.ceil(total));
}

/** True while a sentinel should reveal another page. */
export function hasMoreMedia(total: number, shown: number): boolean {
  if (!Number.isFinite(total) || total <= 0) return false;
  return Math.floor(Number.isFinite(shown) ? shown : 0) < Math.ceil(total);
}
