import type { ComponentProps } from "react";
import { cn } from "@/lib/cn";

/**
 * Unified shadcn/Swiss table primitives (#333): `bg-bg-deep` header, 1px
 * `border-line` row dividers, `hover:bg-surface-2`, UI font on headers (never
 * monospace), and `.num` tabular numerals on numeric columns.
 */
export function TableContainer({ className, ...rest }: ComponentProps<"div">) {
  return (
    <div
      className={cn(
        "w-full overflow-x-auto rounded-[var(--r-md)] border border-line bg-surface",
        className,
      )}
      {...rest}
    />
  );
}

export function Table({ className, ...rest }: ComponentProps<"table">) {
  return <table className={cn("w-full border-collapse text-left text-body text-ink", className)} {...rest} />;
}

export function TableHead({ className, ...rest }: ComponentProps<"thead">) {
  return <thead className={cn("border-b border-line bg-bg-deep", className)} {...rest} />;
}

export function TableBody({ className, ...rest }: ComponentProps<"tbody">) {
  return <tbody className={cn("divide-y divide-line", className)} {...rest} />;
}

export function TableRow({ className, ...rest }: ComponentProps<"tr">) {
  return <tr className={cn("transition-colors duration-[100ms] hover:bg-surface-2", className)} {...rest} />;
}

export function TableHeaderCell({
  className,
  numeric,
  ...rest
}: ComponentProps<"th"> & { numeric?: boolean }) {
  return (
    <th
      scope="col"
      className={cn(
        "h-10 px-4 text-meta font-medium text-ink-3",
        numeric && "text-right",
        className,
      )}
      {...rest}
    />
  );
}

export function TableCell({
  className,
  numeric,
  ...rest
}: ComponentProps<"td"> & { numeric?: boolean }) {
  return (
    <td
      className={cn(
        "px-4 py-3 text-body text-ink-2",
        numeric && "num text-right text-ink",
        className,
      )}
      {...rest}
    />
  );
}
