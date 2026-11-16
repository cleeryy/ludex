import { ArtPlate } from "@/components/art/cover";
import type { ConsoleWall } from "@/theme/console-room";

/* --------------------------------------------------------------------------
   The console wallpaper (#268).

   One fixed layer behind every chrome surface — and the only blurred layer in
   the product. `artwork` paints the selected game's backdrop as a still,
   blurred plate under an 80 % wash; `solid` and `gradient` paint a token-only
   canvas and never decode an image. The layer is decorative, never focusable,
   and never animates: switching games swaps the plate, which the browser paints
   once. Projector and Arcade rooms set the opacity and blur to zero.
   -------------------------------------------------------------------------- */

export function ConsoleBackdrop({
  wall,
  title,
  artUrl,
}: {
  wall: ConsoleWall;
  title: string;
  artUrl: string | null;
}) {
  return (
    <div className="bp-backdrop" data-console-backdrop={wall} aria-hidden="true">
      {wall === "artwork" && (
        <div className="bp-backdrop-art">
          <ArtPlate title={title} artUrl={artUrl} className="h-full w-full" />
        </div>
      )}
      <div className="bp-backdrop-wash" />
    </div>
  );
}
