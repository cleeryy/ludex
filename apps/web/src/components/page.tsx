import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { Button } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

export function PageHeader({
  title,
  meta,
  description,
  actions,
  className,
}: {
  title: ReactNode;
  meta?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <header className={cn("flex flex-col gap-4 border-b border-line pb-6", className)}>
      <div className="flex flex-wrap items-end justify-between gap-4">
        <h1 className="text-[clamp(2rem,3.4vw,3.25rem)] font-semibold leading-[0.98] tracking-[-0.03em]">{title}</h1>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {meta && <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[12.5px] text-ink-3">{meta}</div>}
      {description && <p className="measure text-[14.5px] leading-relaxed text-ink-2">{description}</p>}
    </header>
  );
}

export function EmptyState({
  title,
  description,
  children,
  className,
  compact,
}: {
  title: string;
  description: string;
  children?: ReactNode;
  className?: string;
  compact?: boolean;
}) {
  return (
    <div className={cn("flex items-stretch gap-8 border border-line bg-surface/50", compact ? "p-6" : "p-8 sm:p-10", className)}>
      <div className="hatch hidden w-24 shrink-0 border border-line sm:block" aria-hidden="true">
        <div className="h-full w-full bg-[linear-gradient(180deg,transparent,var(--surface))]" />
      </div>
      <div className="flex flex-col justify-center gap-3">
        <h2 className="text-[24px] font-semibold leading-tight tracking-[-0.02em]">{title}</h2>
        <p className="measure text-[14px] leading-relaxed text-ink-2">{description}</p>
        {children && <div className="mt-2 flex flex-wrap items-center gap-2">{children}</div>}
      </div>
    </div>
  );
}

export function ErrorState({
  title = "That didn’t load",
  message,
  onRetry,
  action,
}: {
  title?: string;
  message: string;
  onRetry?: () => void;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-start gap-3 border border-line bg-surface/50 p-8" role="alert">
      <span className="h-px w-10 bg-danger" aria-hidden="true" />
      <h2 className="text-[20px] font-semibold tracking-[-0.02em]">{title}</h2>
      <p className="measure text-[14px] leading-relaxed text-ink-2">{message}</p>
      <div className="mt-1 flex gap-2">
        {onRetry && (
          <Button variant="default" onClick={onRetry}>
            {t("Try again")}
          </Button>
        )}
        {action}
      </div>
    </div>
  );
}

export function SectionHeader({
  title,
  description,
  actions,
  count,
  id,
  level = 2,
}: {
  title: string;
  description?: string;
  actions?: ReactNode;
  count?: number;
  /** Anchor for `aria-labelledby` on the section this header names. */
  id?: string;
  /**
   * Heading depth only: an `h3` keeps the section look so nested panels do not
   * invent a fourth heading treatment (#174).
   */
  level?: 2 | 3;
}) {
  const Heading = level === 3 ? "h3" : "h2";
  return (
    <div className="flex flex-wrap items-baseline justify-between gap-3 pb-3">
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
        <Heading
          id={id}
          className={
            level === 3
              ? "text-[13px] font-medium tracking-[-0.01em]"
              : "text-[17px] font-semibold tracking-[-0.015em]"
          }
        >
          {title}
        </Heading>
        {typeof count === "number" && <span className="num text-[12px] text-ink-3">{count}</span>}
        {description && (
          <span className={level === 3 ? "text-[11.5px] text-ink-3" : "text-[13px] text-ink-3"}>{description}</span>
        )}
      </div>
      {actions}
    </div>
  );
}
