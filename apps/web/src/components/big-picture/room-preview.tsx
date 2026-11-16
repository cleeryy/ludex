import { ArtPlate } from "@/components/art/cover";
import { t } from "@/lib/i18n";
import type { BigPictureLayout } from "./layout";
import type { ConsoleRoomSettings } from "@/theme/console-room";

/* --------------------------------------------------------------------------
   The Appearance preview (#268).

   A small, token-driven home: the stage plate, the selected title, the rail and
   the band, drawn with the live room. It is not a screenshot and not a second
   renderer — it reads the same custom properties as the shell, so what a player
   sees here is what the screen already looks like behind the sheet. Changes
   apply as they are made; there is nothing to confirm. Hidden from assistive
   technology: the rows carry the values.
   -------------------------------------------------------------------------- */

export function RoomPreview({
  settings,
  layout,
  artUrl,
}: {
  settings: ConsoleRoomSettings;
  layout: BigPictureLayout;
  artUrl: string | null;
}) {
  return (
    <figure
      aria-hidden="true"
      className="bp-panel relative flex w-full max-w-[calc(var(--bp-u)*520)] flex-col overflow-hidden"
      data-preview-room={settings.room}
      data-preview-layout={layout}
    >
      <div className="relative flex h-[calc(var(--bp-u)*104)] items-end overflow-hidden bg-surface">
        <ArtPlate title={t("Preview")} artUrl={artUrl} className="absolute inset-0 h-full w-full" />
        <div className="absolute inset-0 bp-stage-wash" />
        <div className="relative flex w-full flex-col gap-[calc(var(--bp-u)*6)] p-[calc(var(--bp-u)*12)]">
          <span className="h-[calc(var(--bp-u)*8)] w-[42%] rounded-full bg-ink/80" />
          <span className="h-[calc(var(--bp-u)*6)] w-[26%] rounded-full bg-ink-2/70" />
          <div className="mt-[calc(var(--bp-u)*4)] flex gap-[calc(var(--bp-u)*6)]">
            <span className="h-[calc(var(--bp-u)*18)] w-[calc(var(--bp-u)*56)] rounded-[var(--bp-radius-cover)] bg-signal" />
            <span className="h-[calc(var(--bp-u)*18)] w-[calc(var(--bp-u)*44)] rounded-[var(--bp-radius-cover)] border border-line-2" />
          </div>
        </div>
      </div>
      <div className="flex gap-[var(--bp-rail-gap)] px-[calc(var(--bp-u)*12)] py-[calc(var(--bp-u)*8)]">
        {Array.from({ length: 5 }).map((_, index) => (
          <span
            key={index}
            className="aspect-[3/4] w-[calc(var(--bp-u)*34)] shrink-0 rounded-[var(--bp-radius-cover)] bg-surface-2"
          />
        ))}
      </div>
      <div className="flex items-center gap-[calc(var(--bp-u)*12)] border-t border-line px-[calc(var(--bp-u)*12)] py-[calc(var(--bp-u)*8)]">
        {Array.from({ length: 5 }).map((_, index) => (
          <span
            key={index}
            className="h-[calc(var(--bp-u)*6)] w-[calc(var(--bp-u)*26)] rounded-full bg-ink-3/60"
          />
        ))}
      </div>
      <figcaption className="sr-only">
        {t("Preview of the room. It updates as you change a knob.")}
      </figcaption>
    </figure>
  );
}
