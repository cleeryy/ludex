import { normalizeHex } from "./contrast";

/**
 * "From artwork" accent (#268).
 *
 * The replacement for five pastel colours is one algorithmic idea: read the
 * colour a cover is actually made of, keep a little of it, and refuse anything
 * that cannot carry text. Three rules, in order:
 *
 *   1. pick the dominant *chromatic* cluster, not the average: averaging box
 *      art over a black border always yields brown;
 *   2. clamp the chroma in OKLCH (≤ 0.16) so a neon key art cannot become the
 *      interface's only event;
 *   3. hand the colour to `consoleAccent`, which lifts and validates it at 7:1.
 *
 * Reading pixels needs a canvas, which is exactly what a unit test must not
 * need: the pixel buffer is the seam. `extractArtworkAccent` takes an injected
 * reader, caches per game id, and never throws — a tainted canvas, a missing
 * cover or an offline wall all resolve to `null`, and the caller keeps the
 * theme signal.
 */

/** Max OKLCH chroma an artwork colour may keep. */
export const ARTWORK_MAX_CHROMA = 0.16;
/** Pixels below this saturation (max − min channel) are grey and never an accent. */
const MIN_SATURATION = 0.16;
/** A colour must own at least this share of chromatic pixels to be the answer. */
const MIN_SHARE = 0.12;

export interface Oklch {
  /** Lightness, 0–1. */
  l: number;
  /** Chroma, 0–0.4. */
  c: number;
  /** Hue angle in degrees. */
  h: number;
}

function srgbChannelToLinear(value: number): number {
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

function linearChannelToSrgb(value: number): number {
  return value <= 0.0031308 ? value * 12.92 : 1.055 * value ** (1 / 2.4) - 0.055;
}

/** sRGB hex → OKLCH. The inverse of {@link oklchToHex}. */
export function hexToOklch(hex: string): Oklch | null {
  const normalized = normalizeHex(hex);
  if (!normalized) return null;
  const [r, g, b] = [1, 3, 5].map((offset) =>
    srgbChannelToLinear(Number.parseInt(normalized.slice(offset, offset + 2), 16) / 255),
  ) as [number, number, number];

  const l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
  const m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
  const s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

  const lRoot = Math.cbrt(l);
  const mRoot = Math.cbrt(m);
  const sRoot = Math.cbrt(s);

  const lightness = 0.2104542553 * lRoot + 0.793617785 * mRoot - 0.0040720468 * sRoot;
  const a = 1.9779984951 * lRoot - 2.428592205 * mRoot + 0.4505937099 * sRoot;
  const bb = 0.0259040371 * lRoot + 0.7827717662 * mRoot - 0.808675766 * sRoot;

  const chroma = Math.sqrt(a * a + bb * bb);
  const hue = (Math.atan2(bb, a) * 180) / Math.PI;
  return { l: lightness, c: chroma, h: hue < 0 ? hue + 360 : hue };
}

function oklchToRgb({ l, c, h }: Oklch): [number, number, number] {
  const radians = (h * Math.PI) / 180;
  const a = c * Math.cos(radians);
  const b = c * Math.sin(radians);

  const lRoot = l + 0.3963377774 * a + 0.2158037573 * b;
  const mRoot = l - 0.1055613458 * a - 0.0638541728 * b;
  const sRoot = l - 0.0894841775 * a - 1.291485548 * b;

  const ll = lRoot ** 3;
  const mm = mRoot ** 3;
  const ss = sRoot ** 3;

  return [
    4.0767416621 * ll - 3.3077115913 * mm + 0.2309699292 * ss,
    -1.2684380046 * ll + 2.6097574011 * mm - 0.3413193965 * ss,
    -0.0041960863 * ll - 0.7034186147 * mm + 1.707614701 * ss,
  ];
}

function inGamut([r, g, b]: [number, number, number]): boolean {
  return [r, g, b].every((channel) => channel >= -0.001 && channel <= 1.001);
}

/** OKLCH → hex, reducing chroma until the colour fits sRGB. */
export function oklchToHex(color: Oklch): string | null {
  const lightness = Math.min(1, Math.max(0, color.l));
  let chroma = Math.max(0, Math.min(0.4, color.c));
  let rgb = oklchToRgb({ ...color, l: lightness, c: chroma });
  for (let step = 0; step < 24 && !inGamut(rgb); step += 1) {
    chroma *= 0.92;
    rgb = oklchToRgb({ ...color, l: lightness, c: chroma });
  }
  if (!inGamut(rgb)) return null;
  const channel = (value: number) =>
    Math.round(Math.min(1, Math.max(0, linearChannelToSrgb(value))) * 255)
      .toString(16)
      .padStart(2, "0");
  return `#${rgb.map(channel).join("")}`;
}

/**
 * A cover colour the Cabinet can carry: same hue, lightness kept away from both
 * ends, chroma clamped. Returns null for a grey pixel, which has no hue to keep.
 */
export function clampArtworkColor(hex: string, maxChroma = ARTWORK_MAX_CHROMA): string | null {
  const oklch = hexToOklch(hex);
  if (!oklch) return null;
  if (oklch.c < 0.02) return null;
  return oklchToHex({
    l: Math.min(0.82, Math.max(0.45, oklch.l)),
    c: Math.min(maxChroma, oklch.c),
    h: oklch.h,
  });
}

interface Bucket {
  r: number;
  g: number;
  b: number;
  count: number;
  saturation: number;
}

/**
 * Dominant chromatic colour of a cover, from an RGBA buffer.
 *
 * Pixels are bucketed on the top five bits per channel, which is coarse enough
 * that a gradient votes as one colour and fine enough that two boxes do not
 * merge. Greys, near-black and near-white are dropped: a cover's border is not
 * its colour. The winner is the bucket with the highest `count × saturation`
 * share; a tie is broken by saturation so a saturated minority beats the muddy
 * majority.
 */
export function dominantArtworkColor(
  pixels: ArrayLike<number>,
  { stride = 4, alphaThreshold = 128 } = {},
): string | null {
  const buckets = new Map<number, Bucket>();
  let chromatic = 0;
  const total = Math.floor(pixels.length / stride);
  for (let index = 0; index < total; index += 1) {
    const offset = index * stride;
    const alpha = stride === 4 ? (pixels[offset + 3] ?? 255) : 255;
    if (alpha < alphaThreshold) continue;
    const r = pixels[offset] ?? 0;
    const g = pixels[offset + 1] ?? 0;
    const b = pixels[offset + 2] ?? 0;
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    if (max < 24 || min > 232) continue;
    const saturation = (max - min) / 255;
    if (saturation < MIN_SATURATION) continue;
    chromatic += 1;
    const key = ((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3);
    const current = buckets.get(key);
    if (current) {
      current.count += 1;
      current.r += r;
      current.g += g;
      current.b += b;
    } else {
      buckets.set(key, { r, g, b, count: 1, saturation });
    }
  }
  if (chromatic === 0) return null;
  const ranked = [...buckets.values()]
    .map((bucket) => ({
      ...bucket,
      share: bucket.count / chromatic,
      score: (bucket.count / chromatic) * bucket.saturation + bucket.saturation * 0.15,
    }))
    .sort((a, b) => b.score - a.score);
  const winner = ranked[0];
  if (!winner || winner.count / chromatic < MIN_SHARE * 0.5) return null;
  const hex = `#${[winner.r, winner.g, winner.b]
    .map((sum) => Math.round(sum / winner.count).toString(16).padStart(2, "0"))
    .join("")}`;
  return clampArtworkColor(hex);
}

/** Reads a decoded image into an RGBA buffer; injected so tests need no canvas. */
export type PixelReader = (image: CanvasImageSource) => ArrayLike<number> | null;

const MAX_SAMPLE = 64;

/** Canvas reader: draws the cover at 64×64 and returns its pixels, or null. */
export function canvasPixelReader(): PixelReader {
  return (image) => {
    try {
      const canvas = document.createElement("canvas");
      canvas.width = MAX_SAMPLE;
      canvas.height = MAX_SAMPLE;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) return null;
      context.drawImage(image, 0, 0, MAX_SAMPLE, MAX_SAMPLE);
      return context.getImageData(0, 0, MAX_SAMPLE, MAX_SAMPLE).data;
    } catch {
      // A cross-origin cover taints the canvas; the caller keeps the theme signal.
      return null;
    }
  };
}

const cache = new Map<number, string | null>();
const pending = new Map<number, Promise<string | null>>();

/** Test hook: forget every cached extraction. */
export function resetArtworkAccentCache(): void {
  cache.clear();
  pending.clear();
}

export function cachedArtworkAccent(gameId: number): string | null | undefined {
  return cache.get(gameId);
}

export interface ExtractOptions {
  url: string | null;
  gameId: number;
  readPixels?: PixelReader;
  loadImage?: (url: string) => Promise<CanvasImageSource>;
}

function loadImageElement(url: string): Promise<CanvasImageSource> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.crossOrigin = "anonymous";
    image.decoding = "async";
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("cover could not be decoded"));
    image.src = url;
  });
}

/**
 * The accent for one game, cached by id. Never rejects: null means "this cover
 * has no colour the Cabinet can carry", and the settings row says so once.
 */
export function extractArtworkAccent({
  url,
  gameId,
  readPixels,
  loadImage,
}: ExtractOptions): Promise<string | null> {
  if (!url) return Promise.resolve(null);
  if (cache.has(gameId)) return Promise.resolve(cache.get(gameId) ?? null);
  const inFlight = pending.get(gameId);
  if (inFlight) return inFlight;
  const task = (async () => {
    try {
      const image = await (loadImage ?? loadImageElement)(url);
      const pixels = (readPixels ?? canvasPixelReader())(image);
      const color = pixels ? dominantArtworkColor(pixels) : null;
      cache.set(gameId, color);
      return color;
    } catch {
      cache.set(gameId, null);
      return null;
    } finally {
      pending.delete(gameId);
    }
  })();
  pending.set(gameId, task);
  return task;
}
