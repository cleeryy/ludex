import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import type { BigPictureLayout } from "./layout";

/* --------------------------------------------------------------------------
   Layout thumbnails (#141). Drawn with the live theme tokens, so the preview
   in Settings and in the quick switcher always looks like the real screen —
   the same rule the theme cards follow (#140). Decorative only.
   -------------------------------------------------------------------------- */

function Bar({ className }: { className?: string }) {
  return <span className={cn("block rounded-[1px] bg-line-2", className)} />;
}

function Plate({ className }: { className?: string }) {
  return <span className={cn("block aspect-[3/4] rounded-[1px] bg-line-2", className)} />;
}

function ShelfBody() {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-0 flex-1 items-center gap-1.5 p-1.5">
        <Plate className="h-full w-auto" />
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <Bar className="h-[3px] w-10 bg-ink" />
          <Bar className="h-[2px] w-7" />
          <Bar className="h-[2px] w-5" />
        </div>
      </div>
      <div className="flex gap-1 border-t border-line p-1">
        <Plate className="w-3" />
        <Plate className="w-3 outline outline-1 outline-signal" />
        <Plate className="w-3" />
        <Plate className="w-3" />
        <Plate className="w-3" />
      </div>
    </div>
  );
}

function HeroBody() {
  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-end gap-1 bg-surface-2 p-1.5">
      <Bar className="h-[4px] w-12 bg-ink" />
      <Bar className="h-[2px] w-8" />
      <div className="mt-0.5 flex items-end gap-1">
        <Plate className="w-2.5 opacity-60" />
        <Plate className="w-3.5 outline outline-1 outline-signal" />
        <Plate className="w-2.5 opacity-60" />
      </div>
    </div>
  );
}

function GridBody() {
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1 p-1.5">
      <div className="flex items-center gap-1">
        <span className="block aspect-[3/4] w-2.5 rounded-[1px] bg-line-2" />
        <div className="flex flex-col gap-1">
          <Bar className="h-[3px] w-9 bg-ink" />
          <Bar className="h-[2px] w-6" />
        </div>
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-5 gap-1">
        {Array.from({ length: 10 }).map((_, index) => (
          <Plate key={index} className={index === 6 ? "outline outline-1 outline-signal" : "opacity-80"} />
        ))}
      </div>
    </div>
  );
}

function CompactBody() {
  return (
    <div className="flex min-h-0 flex-1 flex-col justify-center gap-1 p-1.5">
      {Array.from({ length: 4 }).map((_, index) => (
        <div
          key={index}
          className={cn(
            "flex items-center gap-1 rounded-[1px] py-0.5",
            index === 1 ? "bg-surface-2 ps-1 outline outline-1 outline-signal" : "ps-1",
          )}
        >
          <span className="block aspect-[3/4] w-1.5 rounded-[1px] bg-line-2" />
          <Bar className={cn("h-[2px]", index === 1 ? "w-10 bg-ink" : "w-8")} />
          <Bar className="ms-auto me-1 h-[2px] w-2" />
        </div>
      ))}
    </div>
  );
}

function DockBody() {
  return (
    <div className="flex min-h-0 flex-1 flex-col justify-end gap-1 p-1.5">
      <div className="flex items-center justify-between">
        <Bar className="h-[2px] w-9" />
        <Bar className="h-[2px] w-3" />
      </div>
      <Bar className="h-[4px] w-14 bg-ink" />
      <Bar className="h-[2px] w-10" />
      <div className="mt-0.5 flex items-end gap-1">
        <Plate className="w-2.5 opacity-50" />
        <Plate className="w-4 outline outline-1 outline-signal" />
        <Plate className="w-2.5 opacity-50" />
      </div>
      <div className="mt-0.5 flex gap-1 border-t border-line pt-1">
        {Array.from({ length: 4 }).map((_, index) => (
          <span key={index} className="h-[3px] flex-1 rounded-[1px] bg-line-2" />
        ))}
      </div>
    </div>
  );
}

function SpotlightBody() {
  return (
    <div className="flex min-h-0 flex-1 flex-col justify-between gap-1 bg-surface-2 p-1.5">
      <div className="flex justify-end gap-1">
        {Array.from({ length: 3 }).map((_, index) => (
          <span key={index} className="h-[4px] w-4 rounded-[1px] bg-line-2" />
        ))}
      </div>
      <div className="flex flex-col gap-1">
        <Bar className="h-[2px] w-10" />
        <Bar className="h-[4px] w-16 bg-ink" />
        <Bar className="h-[2px] w-12" />
      </div>
      <div className="flex items-end gap-1">
        <Plate className="w-2.5 opacity-50" />
        <Plate className="w-3.5 outline outline-1 outline-signal" />
        <Plate className="w-2.5 opacity-50" />
      </div>
    </div>
  );
}

const BODIES: Record<BigPictureLayout, () => ReactNode> = {
  shelf: ShelfBody,
  dock: DockBody,
  spotlight: SpotlightBody,
  hero: HeroBody,
  grid: GridBody,
  compact: CompactBody,
};

export function LayoutPreview({ layout, className }: { layout: BigPictureLayout; className?: string }) {
  const Body = BODIES[layout];
  return (
    <div className={cn("flex h-[74px] w-full flex-col overflow-hidden border border-line-2 bg-bg", className)} aria-hidden="true">
      <div className="flex h-2.5 shrink-0 items-center gap-1 border-b border-line bg-surface px-1.5">
        <span className="h-[3px] w-3.5 rounded-[1px] bg-ink" />
        <span className="ms-auto h-[3px] w-2.5 rounded-[1px] bg-ink-3" />
      </div>
      <Body />
    </div>
  );
}
