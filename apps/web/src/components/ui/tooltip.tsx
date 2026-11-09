import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

export function Tooltip({ label, children, side = "top" }: { label: string; children: ReactNode; side?: "top" | "bottom" }) {
  return (
    <span className="group/tooltip relative inline-flex">
      {children}
      <span
        role="tooltip"
        className={cn(
          "pointer-events-none absolute left-1/2 z-50 -translate-x-1/2 whitespace-nowrap rounded-[2px] border border-line-2 bg-surface-2 px-2 py-1 text-[11.5px] text-ink",
          "opacity-0 transition-opacity delay-0 duration-100 group-hover/tooltip:opacity-100 group-hover/tooltip:delay-500 group-focus-within/tooltip:opacity-100",
          side === "top" ? "bottom-[calc(100%+6px)]" : "top-[calc(100%+6px)]",
        )}
      >
        {label}
      </span>
    </span>
  );
}
