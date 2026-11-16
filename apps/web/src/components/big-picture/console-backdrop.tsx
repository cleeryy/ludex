import { useEffect, useRef, useState } from "react";
import { ArtPlate } from "@/components/art/cover";
import { paintWallPlate, parseWallFilter, WALL_PLATE, type WallSize } from "@/lib/wall-plate";
import type { ConsoleRoom, ConsoleWall } from "@/theme/console-room";

/* --------------------------------------------------------------------------
   The console wallpaper (#268), painted once (#270).

   One fixed layer behind every chrome surface — and the only blurred layer in
   the product. `artwork` paints the selected game's backdrop, blurred and
   veiled; `solid` and `gradient` paint a token-only canvas and never decode an
   image. The layer is decorative, never focusable, and never animates.

   The blur is *baked*: the live plate is drawn once into a 256×144 canvas with
   the stylesheet's own blur and saturation, and the layer stretches that
   texture to the TV. A live `filter: blur()` over a full-screen layer is
   re-rasterized every time anything moves above it, which is the whole rail
   budget on a software compositor (docs/console/PERFORMANCE.md); a baked plate
   is a static bitmap the compositor blits. Engines that cannot blur a canvas
   keep the live filter, which is exactly what shipped before.
   -------------------------------------------------------------------------- */

/**
 * The wall follows the selection when it settles, never during a held press.
 * A repeat re-centres the rail faster than a bake can finish, and the wall is
 * the last thing that should compete with the cover the player is on.
 */
const SETTLE_MS = 160;

interface WallArt {
  title: string;
  artUrl: string | null;
}

interface WallSource {
  node: CanvasImageSource;
  size: WallSize;
}

/** The artwork the layer actually renders, as an image a canvas can draw. */
async function loadWallSource(container: HTMLElement): Promise<WallSource | null> {
  const image = container.querySelector("img");
  if (image) {
    // A real backdrop is already decoded by the live plate; reuse that decode.
    if (image.complete && image.naturalWidth === 0) return null;
    if (!image.complete) {
      const loaded = await new Promise<boolean>((resolve) => {
        image.addEventListener("load", () => resolve(true), { once: true });
        image.addEventListener("error", () => resolve(false), { once: true });
      });
      if (!loaded) return null;
    }
    const size = { width: image.naturalWidth, height: image.naturalHeight };
    return size.width > 0 && size.height > 0 ? { node: image, size } : null;
  }

  const svg = container.querySelector("svg");
  if (!svg) return null;
  // The generated plates are inline SVG. As an image they need a size of their
  // own, so the viewBox becomes width/height before serializing; the markup
  // carries every colour as an attribute, which is why it survives the trip.
  const clone = svg.cloneNode(true) as SVGSVGElement;
  const box = svg.viewBox.baseVal;
  const size = { width: Math.round(box.width) || 300, height: Math.round(box.height) || 400 };
  clone.setAttribute("width", String(size.width));
  clone.setAttribute("height", String(size.height));
  const markup = new XMLSerializer().serializeToString(clone);
  const url = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(markup)}`;

  const loaded = await new Promise<HTMLImageElement | null>((resolve) => {
    const element = new Image();
    element.onload = () => resolve(element);
    element.onerror = () => resolve(null);
    element.src = url;
  });
  return loaded ? { node: loaded, size } : null;
}

export function ConsoleBackdrop({
  wall,
  room,
  title,
  artUrl,
}: {
  wall: ConsoleWall;
  room: ConsoleRoom;
  title: string;
  artUrl: string | null;
}) {
  // What the layer renders. It lags the selection by one settle.
  const [settled, setSettled] = useState<WallArt>({ title, artUrl });
  const [bakedKey, setBakedKey] = useState<string | null>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const sourceRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    if (settled.title === title && settled.artUrl === artUrl) return;
    const timer = window.setTimeout(() => setSettled({ title, artUrl }), SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [title, artUrl, settled]);

  // Keyed by everything the plate depends on: a new game, a new room or a new
  // wall invalidates the bake, and the derived `baked` drops back to the live
  // plate for the frames the new texture takes to paint.
  const key = `${settled.title}\u0000${settled.artUrl ?? ""}\u0000${room}\u0000${wall}`;
  const baked = bakedKey === key;

  useEffect(() => {
    const layer = layerRef.current;
    const canvas = canvasRef.current;
    const source = sourceRef.current;
    if (!layer || !canvas || !source) return;

    // Read the blur the stylesheet decided on (the room owns it) instead of
    // duplicating the rule here: `none` means this wall is meant to be sharp.
    const filter = parseWallFilter(getComputedStyle(layer).filter);
    if (filter.blur <= 0) return;

    let cancelled = false;
    void (async () => {
      const plate = await loadWallSource(source);
      if (cancelled || !plate) return;
      const ok = paintWallPlate(canvas.getContext("2d"), plate.node, plate.size, {
        blur: filter.blur,
        saturate: filter.saturate,
        viewportWidth: layer.clientWidth || window.innerWidth,
      });
      if (!cancelled && ok) setBakedKey(key);
    })();
    return () => {
      cancelled = true;
    };
  }, [key]);

  return (
    <div className="bp-backdrop" data-console-backdrop={wall} aria-hidden="true">
      {wall === "artwork" && (
        <div
          ref={layerRef}
          className="bp-backdrop-art"
          data-bp-wall={baked ? "baked" : "live"}
        >
          <canvas
            ref={canvasRef}
            className="bp-wall-plate"
            width={WALL_PLATE.width}
            height={WALL_PLATE.height}
            data-bp-wall-plate="true"
          />
          <div ref={sourceRef} className="bp-wall-source">
            <ArtPlate title={settled.title} artUrl={settled.artUrl} className="h-full w-full" />
          </div>
        </div>
      )}
      <div className="bp-backdrop-wash" />
    </div>
  );
}
