import { Skeleton } from "@/components/ui/controls";
import type { BigPictureLayout } from "./layout";

/* --------------------------------------------------------------------------
   Console loading (#141, #268). Each composition gets the skeleton of its own
   shape — the stage block plus five plates, the wall's header and grid, the
   list's rows — so the first paint already tells the player what is coming.
   No shimmer: a moving gradient is decoration at ten feet; the plate is the
   shape of the content and it stops moving.
   -------------------------------------------------------------------------- */

function assertNever(value: never): never {
  throw new Error(`Unhandled console layout: ${String(value)}`);
}

function Rail({ count }: { count: number }) {
  return (
    <div className="flex gap-[var(--bp-rail-gap)] px-[var(--bp-safe-x)] py-4">
      {Array.from({ length: count }).map((_, index) => (
        <Skeleton key={index} className="aspect-[3/4] w-[var(--bp-rail-card)] shrink-0" />
      ))}
    </div>
  );
}

function StageCopy() {
  return (
    <div className="flex flex-1 flex-col justify-end gap-3 px-[var(--bp-safe-x)] pb-8">
      <Skeleton className="h-[calc(var(--bp-u)*24)] w-[calc(var(--bp-u)*180)]" />
      <Skeleton className="h-[calc(var(--bp-u)*56)] w-1/2" />
      <Skeleton className="h-[calc(var(--bp-u)*24)] w-[calc(var(--bp-u)*320)]" />
      <Skeleton className="h-[calc(var(--bp-u)*64)] w-[calc(var(--bp-u)*160)]" />
    </div>
  );
}

export function LayoutSkeleton({ layout, columns }: { layout: BigPictureLayout; columns: number }) {
  switch (layout) {
    case "shelf":
      return (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1 flex-col bg-surface">
            <StageCopy />
          </div>
          <Rail count={6} />
        </div>
      );

    case "grid":
      return (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex items-end gap-6 border-b border-line px-[var(--bp-safe-x)] py-5">
            <Skeleton className="aspect-[3/4] w-[calc(var(--bp-u)*92)] shrink-0" />
            <div className="flex flex-1 flex-col gap-2">
              <Skeleton className="h-[calc(var(--bp-u)*24)] w-1/4" />
              <Skeleton className="h-[calc(var(--bp-u)*34)] w-2/5" />
              <Skeleton className="h-[calc(var(--bp-u)*24)] w-1/3" />
            </div>
          </div>
          <div className="flex gap-2 px-[var(--bp-safe-x)] py-3">
            {Array.from({ length: 3 }).map((_, index) => (
              <Skeleton key={index} className="h-[calc(var(--bp-u)*40)] w-[calc(var(--bp-u)*110)]" />
            ))}
          </div>
          <div
            className="grid min-h-0 flex-1 gap-4 overflow-hidden px-[var(--bp-safe-x)] pb-6"
            style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
          >
            {Array.from({ length: columns * 2 }).map((_, index) => (
              <Skeleton key={index} className="aspect-[3/4] w-full" />
            ))}
          </div>
        </div>
      );

    case "compact":
      return (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex flex-wrap items-end gap-4 border-b border-line px-[var(--bp-safe-x)] py-4">
            <div className="flex flex-1 flex-col gap-2">
              <Skeleton className="h-[calc(var(--bp-u)*24)] w-[calc(var(--bp-u)*180)]" />
              <Skeleton className="h-[calc(var(--bp-u)*34)] w-1/3" />
              <Skeleton className="h-[calc(var(--bp-u)*24)] w-[calc(var(--bp-u)*260)]" />
            </div>
            <Skeleton className="h-[calc(var(--bp-u)*64)] w-[calc(var(--bp-u)*150)]" />
          </div>
          <div className="flex gap-2 px-[var(--bp-safe-x)] py-3">
            {Array.from({ length: 3 }).map((_, index) => (
              <Skeleton key={index} className="h-[calc(var(--bp-u)*40)] w-[calc(var(--bp-u)*110)]" />
            ))}
          </div>
          {Array.from({ length: 7 }).map((_, index) => (
            <div key={index} className="flex items-center gap-4 border-b border-line px-[var(--bp-safe-x)] py-3">
              <Skeleton className="aspect-[3/4] w-[calc(var(--bp-u)*39)]" />
              <div className="flex flex-1 flex-col gap-2">
                <Skeleton className="h-[calc(var(--bp-u)*28)] w-1/3" />
                <Skeleton className="h-[calc(var(--bp-u)*24)] w-1/5" />
              </div>
            </div>
          ))}
        </div>
      );

    default:
      return assertNever(layout);
  }
}
