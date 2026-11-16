import { describe, expect, it } from "vitest";

import {
  canvasFilterSupported,
  coverRect,
  paintWallPlate,
  parseWallFilter,
  plateBlur,
  resetCanvasFilterSupport,
  WALL_PLATE,
} from "@/lib/wall-plate";

/** A 2D context that records the two calls the plate makes. */
function fakeContext(readback: number) {
  const calls: unknown[][] = [];
  const context = {
    filter: "none",
    fillStyle: "",
    clearRect: (...args: unknown[]) => calls.push(["clearRect", ...args]),
    fillRect: () => undefined,
    getImageData: () => ({ data: [readback, readback, readback, 255] }),
    drawImage: (...args: unknown[]) => calls.push(["drawImage", context.filter, ...args]),
  };
  return { context: context as unknown as CanvasRenderingContext2D, calls };
}

/** A document whose canvas reports `readback` after the brightness probe. */
function fakeDocument(readback: number) {
  const { context, calls } = fakeContext(readback);
  const canvas = { width: 0, height: 0, getContext: () => context };
  return { doc: { createElement: () => canvas } as unknown as Document, context, calls };
}

const image = {} as CanvasImageSource;
const source = { width: 300, height: 400 };
const paint = { blur: 26, saturate: 1.12, viewportWidth: 2150 };

describe("wall plate geometry", () => {
  it("reads the blur and saturation the stylesheet asked for", () => {
    expect(parseWallFilter("blur(26px) saturate(1.12)")).toEqual({ blur: 26, saturate: 1.12 });
    expect(parseWallFilter("blur(18.72px)")).toEqual({ blur: 18.72, saturate: 1 });
    expect(parseWallFilter("none")).toEqual({ blur: 0, saturate: 1 });
    expect(parseWallFilter("blur(0px) saturate(1.12)")).toEqual({ blur: 0, saturate: 1.12 });
  });

  it("crops the artwork like object-fit: cover", () => {
    const portrait = coverRect({ width: 300, height: 400 }, WALL_PLATE);
    expect(portrait.width).toBeCloseTo(256, 5);
    expect(portrait.height).toBeCloseTo(341.33, 1);
    expect(portrait.x).toBeCloseTo(0, 5);
    expect(portrait.y).toBeCloseTo(-98.67, 1);

    const landscape = coverRect({ width: 1920, height: 1080 }, WALL_PLATE);
    expect(landscape).toEqual({ x: 0, y: 0, width: 256, height: 144 });
  });

  it("keeps the blur the same fraction of the plate at every resolution", () => {
    // 26u at 1920 (u = 1px) and at 4K (u = 2px), over a layer 12 % wider than
    // the viewport: the texture-space radius is the same.
    expect(plateBlur(26, 2150)).toBeCloseTo(3.1, 2);
    expect(plateBlur(52, 4300)).toBeCloseTo(3.1, 2);
    expect(plateBlur(26, 0)).toBe(0);
    expect(plateBlur(0, 1920)).toBe(0);
  });
});

describe("wall plate paint", () => {
  it("refuses to paint without a context or a real blur", () => {
    resetCanvasFilterSupport();
    expect(paintWallPlate(null, image, source, paint)).toBe(false);

    const { doc, context } = fakeDocument(64);
    expect(paintWallPlate(context, image, source, { ...paint, blur: 0 }, doc)).toBe(false);
    expect(paintWallPlate(context, image, { width: 0, height: 0 }, paint, doc)).toBe(false);
  });

  it("refuses to paint when the engine ignores canvas filters", () => {
    resetCanvasFilterSupport();
    // A probe that reads back white ignored `brightness(0.5)`: the caller keeps
    // the live CSS filter instead of shipping a sharp plate.
    const { doc } = fakeDocument(255);
    expect(canvasFilterSupported(doc)).toBe(false);
    expect(paintWallPlate(fakeContext(64).context, image, source, paint, doc)).toBe(false);
  });

  it("draws the blurred plate over the whole texture, with bleed for the edges", () => {
    resetCanvasFilterSupport();
    const { doc, context, calls } = fakeDocument(64);
    expect(paintWallPlate(context, image, source, paint, doc)).toBe(true);

    const draw = calls.find((call) => call[0] === "drawImage");
    expect(draw).toBeDefined();
    const [, filter, , x, y, width, height] = draw as [string, string, unknown, number, number, number, number];
    // 26px on a 2150px layer is 3.1px on the 256px plate; saturation travels
    // with it so the baked still matches the live layer exactly.
    expect(filter).toBe("blur(3.1px) saturate(1.12)");
    expect(context.filter).toBe("none");
    // 8 % larger than the plate on both axes, centred: the blur samples
    // artwork at the edges instead of fading to transparent.
    expect(x).toBeCloseTo(-10.24, 2);
    expect(y).toBeCloseTo(-112.32, 1);
    expect(width).toBeCloseTo(276.48, 2);
    expect(height).toBeCloseTo(368.64, 1);
  });
});
