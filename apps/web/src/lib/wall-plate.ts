/**
 * The Console wall plate (#270).
 *
 * The wall is one still: the selected game's artwork, blurred and veiled,
 * parked behind every surface. Painting that blur as a live CSS filter over a
 * full-screen layer is the one thing the rail cannot afford — the compositor
 * re-rasterizes the filter whenever anything above it moves, which cost
 * 50–67 ms p95 with the wallpaper visible and 16.8 ms with it hidden
 * (`docs/console/PERFORMANCE.md`).
 *
 * So the blur is baked once, off the rail's path, into a fixed 16:9 texture.
 * The layer stretches that texture to the TV, and a scroll composites a
 * 256×144 bitmap instead of filtering 1920×1080 every frame.
 *
 * Everything here is pure except the canvas support probe, so the geometry and
 * the stylesheet parsing are unit-tested without a renderer.
 */

export interface WallSize {
  width: number;
  height: number;
}

/**
 * 256×144 is enough for a 26u blur and 1/56 of the pixels of a 1080p plate.
 * The texture is scaled by CSS, so the same plate is correct at 720p and 4K.
 */
export const WALL_PLATE: WallSize = { width: 256, height: 144 };

/**
 * The texture is painted a little larger than the box it fills, so the blur
 * samples artwork at the edges instead of fading to transparent.
 */
const BLEED = 1.08;

/** Below this a texture-space blur stops being worth a bake (and NaN guards). */
const MIN_BLUR = 0.2;

export interface WallFilter {
  /** CSS blur radius in px, on the layer's own size. */
  blur: number;
  /** `saturate()` from the same declaration; 1 when the wall does not ask for it. */
  saturate: number;
}

/**
 * Reads the wall's own stylesheet filter, so the baked plate is the same blur
 * the live layer would have painted. A room that spends no blur (`none` or
 * `blur(0px)`) returns zero: a sharp artwork must stay a sharp artwork, not a
 * stretched low-resolution texture.
 */
export function parseWallFilter(filter: string): WallFilter {
  const blur = /blur\((\d+(?:\.\d+)?)px\)/.exec(filter);
  const saturate = /saturate\((\d+(?:\.\d+)?)\)/.exec(filter);
  return { blur: blur ? Number(blur[1]) : 0, saturate: saturate ? Number(saturate[1]) : 1 };
}

/**
 * `object-fit: cover` maths: the source is cropped and centred, never
 * stretched — the same mapping `preserveAspectRatio="xMidYMid slice"` and
 * `object-cover` give the live plate.
 */
export function coverRect(
  source: WallSize,
  box: WallSize,
): { x: number; y: number; width: number; height: number } {
  const scale = Math.max(box.width / source.width, box.height / source.height);
  const width = source.width * scale;
  const height = source.height * scale;
  return { x: (box.width - width) / 2, y: (box.height - height) / 2, width, height };
}

/**
 * The CSS blur radius expressed in texture pixels. A CSS px on the 1920-wide
 * layer and a texture px on the 256-wide plate differ by the same ratio at
 * every resolution, so the wall looks identical at 720p, 1080p and 4K.
 */
export function plateBlur(blurPx: number, viewportWidth: number, plate: WallSize = WALL_PLATE): number {
  if (viewportWidth <= 0 || blurPx <= 0) return 0;
  return Math.round(((blurPx * plate.width) / viewportWidth) * 100) / 100;
}

let filterSupport: boolean | null = null;

/**
 * Does this engine actually blur a canvas? WebKit ignores `ctx.filter`, and a
 * silently sharp plate reads worse than the live blur the stylesheet already
 * paints, so the check is behavioural: half brightness on white must read back
 * grey. The answer is cached — it cannot change inside one document.
 */
export function canvasFilterSupported(doc: Document | null = typeof document === "undefined" ? null : document): boolean {
  if (filterSupport !== null) return filterSupport;
  filterSupport = false;
  if (!doc) return filterSupport;
  try {
    const probe = doc.createElement("canvas");
    probe.width = 2;
    probe.height = 2;
    const ctx = probe.getContext("2d");
    if (!ctx) return filterSupport;
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, 2, 2);
    ctx.filter = "brightness(0.5)";
    ctx.fillRect(0, 0, 2, 2);
    ctx.filter = "none";
    const red = ctx.getImageData(0, 0, 1, 1).data[0];
    filterSupport = typeof red === "number" && red < 250;
  } catch {
    filterSupport = false;
  }
  return filterSupport;
}

/** Test seam: the probe is cached for the life of the document. */
export function resetCanvasFilterSupport(): void {
  filterSupport = null;
}

export interface PaintWallOptions extends WallFilter {
  /** The layer's CSS width; the blur is mapped into texture pixels with it. */
  viewportWidth: number;
  box?: WallSize;
}

/**
 * Paints the plate, blur included. Returns `false` when the engine cannot blur
 * a canvas or the context is unusable, so the caller keeps the live CSS filter
 * rather than shipping a sharp picture behind the wash.
 */
export function paintWallPlate(
  ctx: CanvasRenderingContext2D | null,
  image: CanvasImageSource,
  source: WallSize,
  options: PaintWallOptions,
  doc: Document | null = typeof document === "undefined" ? null : document,
): boolean {
  if (!ctx || source.width <= 0 || source.height <= 0) return false;
  const box = options.box ?? WALL_PLATE;
  const blur = plateBlur(options.blur, options.viewportWidth, box);
  if (blur < MIN_BLUR) return false;
  if (!canvasFilterSupported(doc)) return false;

  const grown: WallSize = { width: box.width * BLEED, height: box.height * BLEED };
  const rect = coverRect(source, grown);
  const offsetX = (grown.width - box.width) / 2;
  const offsetY = (grown.height - box.height) / 2;

  ctx.clearRect(0, 0, box.width, box.height);
  ctx.filter = `blur(${blur}px) saturate(${options.saturate})`;
  ctx.drawImage(image, rect.x - offsetX, rect.y - offsetY, rect.width, rect.height);
  ctx.filter = "none";
  return true;
}
