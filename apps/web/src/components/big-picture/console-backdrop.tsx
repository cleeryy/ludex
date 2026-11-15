import { ArtPlate } from "@/components/art/cover";
import type { ConsoleBackground } from "@/theme/console-appearance";

/* --------------------------------------------------------------------------
   The console wallpaper (#229).

   One fixed layer behind every chrome surface. `artwork` paints the selected
   game's backdrop as a still, blurred plate; `solid` and `gradient` paint a
   token-only canvas and never decode an image. The layer is decorative, never
   focusable, and never animates: switching games swaps the plate, which the
   browser paints once.

   Budget: this is the only full-screen blur in the console. It is static, it is
   disabled by `prefers-reduced-transparency` and `prefers-contrast`, and the
   chrome above it uses `backdrop-filter` on the same still frame.
   -------------------------------------------------------------------------- */

export function ConsoleBackdrop({
  background,
  title,
  artUrl,
}: {
  background: ConsoleBackground;
  title: string;
  artUrl: string | null;
}) {
  return (
    <div className="bp-backdrop" data-console-backdrop={background} aria-hidden="true">
      {background === "artwork" && (
        <div className="bp-backdrop-art">
          <ArtPlate title={title} artUrl={artUrl} className="h-full w-full" />
        </div>
      )}
      <div className="bp-backdrop-wash" />
    </div>
  );
}
