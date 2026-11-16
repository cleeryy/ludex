import { describe, expect, it } from "vitest";
import {
  ARTWORK_MAX_CHROMA,
  clampArtworkColor,
  dominantArtworkColor,
  extractArtworkAccent,
  hexToOklch,
  oklchToHex,
  resetArtworkAccentCache,
} from "./artwork-accent";

/* --------------------------------------------------------------------------
   "From artwork" accents (#268).

   The replacement for five pastel colours: read the cover, keep a little of it,
   clamp the chroma in OKLCH, and let `consoleAccent` lift it to 7:1. The pixel
   buffer is the seam, so none of this needs a canvas — and a tainted or missing
   cover resolves to null, never to a broken colour.
   -------------------------------------------------------------------------- */

function pixels(colors: Array<[number, number, number, number?]>, repeat = 10): number[] {
  return colors.flatMap(([r, g, b, a = 255]) => Array.from({ length: repeat }, () => [r, g, b, a]).flat());
}

describe("OKLCH round trip", () => {
  it("reads and writes a hex colour", () => {
    const oklch = hexToOklch("#3f7fbf");
    expect(oklch).not.toBeNull();
    expect(oklch?.c).toBeGreaterThan(0.05);
    expect(oklchToHex(oklch!)).toMatch(/^#[0-9a-f]{6}$/);
    expect(hexToOklch("nope")).toBeNull();
  });

  it("reduces chroma until the colour fits sRGB instead of clipping", () => {
    const hex = oklchToHex({ l: 0.6, c: 0.4, h: 150 });
    expect(hex).not.toBeNull();
    expect(hexToOklch(hex!)!.c).toBeLessThanOrEqual(0.4);
  });
});

describe("artwork colour clamp", () => {
  it("keeps the hue and clamps the chroma", () => {
    const clamped = clampArtworkColor("#00ff66");
    expect(clamped).not.toBeNull();
    const oklch = hexToOklch(clamped!)!;
    expect(oklch.c).toBeLessThanOrEqual(ARTWORK_MAX_CHROMA + 0.001);
    expect(oklch.l).toBeGreaterThanOrEqual(0.44);
    expect(oklch.l).toBeLessThanOrEqual(0.83);
  });

  it("refuses a grey, which has no hue to keep", () => {
    expect(clampArtworkColor("#7a7a7a")).toBeNull();
    expect(clampArtworkColor("#000000")).toBeNull();
  });
});

describe("dominant cover colour", () => {
  it("picks the saturated minority over a muddy majority", () => {
    const buffer = pixels(
      [
        [20, 18, 16], // near-black border
        [120, 118, 112], // grey wash
        [24, 90, 190], // the box's blue
      ],
      6,
    );
    const color = dominantArtworkColor(buffer);
    expect(color).not.toBeNull();
    const oklch = hexToOklch(color!)!;
    // Blue: hue around 250–270.
    expect(oklch.h).toBeGreaterThan(220);
    expect(oklch.h).toBeLessThan(300);
  });

  it("returns null for a grey cover", () => {
    expect(dominantArtworkColor(pixels([[90, 90, 90]], 20))).toBeNull();
    expect(dominantArtworkColor([])).toBeNull();
  });

  it("ignores transparent pixels", () => {
    const transparent = pixels([[255, 0, 0, 0]], 4);
    const opaque = pixels([[30, 120, 60, 255]], 4);
    const color = dominantArtworkColor([...transparent, ...opaque]);
    expect(color).not.toBeNull();
  });
});

describe("extraction cache", () => {
  it("reads a cover once per game and never throws", async () => {
    resetArtworkAccentCache();
    const blue = new Uint8ClampedArray(pixels([[24, 90, 190]], 12));
    let reads = 0;
    const readPixels = () => {
      reads += 1;
      return blue;
    };
    const loadImage = async () => ({}) as CanvasImageSource;

    const first = await extractArtworkAccent({ url: "demo://cover", gameId: 7, readPixels, loadImage });
    const second = await extractArtworkAccent({ url: "demo://cover", gameId: 7, readPixels, loadImage });
    expect(first).toBe(second);
    expect(first).not.toBeNull();
    expect(reads).toBe(1);

    const failed = await extractArtworkAccent({
      url: "https://example.invalid/cover.png",
      gameId: 8,
      readPixels,
      loadImage: async () => {
        throw new Error("blocked");
      },
    });
    expect(failed).toBeNull();
  });

  it("returns null without a cover url", async () => {
    resetArtworkAccentCache();
    await expect(extractArtworkAccent({ url: null, gameId: 1 })).resolves.toBeNull();
  });
});
