import { Skeleton } from "@/components/ui/controls";
import type { BigPictureLayout } from "./layout";

/* --------------------------------------------------------------------------
   Big Picture loading (#141). Each layout gets the skeleton of its own
   composition, so the first paint already tells the user which layout is
   coming instead of flashing one generic rail.
   -------------------------------------------------------------------------- */

function Rail({ count }: { count: number }) {
  return (
    <div className="flex gap-3 border-t border-line p-5">
      {Array.from({ length: count }).map((_, index) => (
        <Skeleton key={index} className="aspect-[3/4] w-[120px] shrink-0" />
      ))}
    </div>
  );
}

export function LayoutSkeleton({ layout, columns }: { layout: BigPictureLayout; columns: number }) {
  if (layout === "grid") {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex items-center gap-6 border-b border-line px-6 py-6">
          <Skeleton className="aspect-[3/4] w-[96px] shrink-0" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-7 w-2/5" />
            <Skeleton className="h-2.5 w-1/3" />
            <Skeleton className="h-9 w-40" />
          </div>
        </div>
        <div
          className="grid min-h-0 flex-1 gap-3 overflow-hidden p-6"
          style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
        >
          {Array.from({ length: columns * 2 }).map((_, index) => (
            <Skeleton key={index} className="aspect-[3/4] w-full" />
          ))}
        </div>
      </div>
    );
  }

  if (layout === "compact") {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <Skeleton className="m-4 h-9 w-64" />
        {Array.from({ length: 8 }).map((_, index) => (
          <div key={index} className="flex items-center gap-4 border-b border-line px-5 py-2.5">
            <Skeleton className="h-3 w-5" />
            <Skeleton className="aspect-[3/4] w-[34px]" />
            <div className="flex flex-1 flex-col gap-1.5">
              <Skeleton className="h-3 w-1/3" />
              <Skeleton className="h-2 w-1/5" />
            </div>
          </div>
        ))}
      </div>
    );
  }

  if (layout === "hero") {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <div className="flex flex-1 flex-col items-center justify-end gap-3 pb-8">
          <Skeleton className="h-2.5 w-28" />
          <Skeleton className="h-12 w-1/2" />
          <Skeleton className="h-2.5 w-64" />
          <Skeleton className="mt-2 h-10 w-36" />
        </div>
        <Rail count={6} />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-1 items-center gap-10 px-12">
        <Skeleton className="aspect-[3/4] w-[220px] shrink-0" />
        <div className="flex flex-1 flex-col gap-3">
          <Skeleton className="h-2.5 w-24" />
          <Skeleton className="h-16 w-1/2" />
          <Skeleton className="h-2.5 w-2/5" />
          <Skeleton className="h-10 w-40" />
        </div>
      </div>
      <Rail count={7} />
    </div>
  );
}
