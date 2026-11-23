import type { ComponentProps, ReactNode } from "react";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/cn";

/* --------------------------------- button -------------------------------- */

export type ButtonVariant = "primary" | "ink" | "outline" | "default" | "ghost" | "quiet" | "danger";
export type ButtonSize = "sm" | "md" | "lg" | "icon";

const buttonVariants: Record<ButtonVariant, string> = {
  primary:
    "bg-signal text-on-signal border border-signal hover:brightness-[1.06] active:brightness-95 font-semibold",
  ink: "bg-ink text-bg border border-ink hover:opacity-90 font-semibold",
  outline: "border border-line-2 bg-surface text-ink hover:border-ink-3 hover:bg-surface-2",
  default: "border border-line-2 bg-surface text-ink hover:border-ink-3 hover:bg-surface-2",
  ghost: "border border-transparent text-ink-2 hover:text-ink hover:bg-surface",
  quiet: "border border-transparent text-ink-2 hover:text-ink",
  danger: "border border-danger/45 text-danger hover:bg-danger/10 hover:border-danger/70",
};

const buttonSizes: Record<ButtonSize, string> = {
  sm: "h-[30px] px-2.5 text-label gap-1.5",
  md: "h-9 px-3.5 text-body gap-2",
  lg: "h-10 px-4 text-lead gap-2",
  icon: "size-9 shrink-0",
};

export type ButtonProps = ComponentProps<"button"> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  icon?: ReactNode;
  iconRight?: ReactNode;
};

export function Button({
  variant = "outline",
  size = "md",
  loading = false,
  icon,
  iconRight,
  className,
  children,
  disabled,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled || loading}
      className={cn(
        "inline-flex select-none items-center justify-center rounded-[var(--r-sm)] font-medium",
        "transition-[background-color,border-color,color,filter,opacity,transform] duration-[120ms]",
        "active:scale-[0.98] motion-reduce:active:scale-100",
        "disabled:pointer-events-none disabled:opacity-40",
        buttonVariants[variant],
        buttonSizes[size],
        className,
      )}
      {...rest}
    >
      {loading ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : icon}
      {children}
      {!loading && iconRight}
    </button>
  );
}

export function IconButton({ label, ...props }: ButtonProps & { label: string }) {
  return <Button size="icon" aria-label={label} title={label} {...props} />;
}

/* --------------------------------- inputs -------------------------------- */

export function Input({ className, ...rest }: ComponentProps<"input">) {
  return (
    <input
      className={cn(
        "h-9 w-full rounded-[var(--r-sm)] border border-line-2 bg-bg-deep px-3 text-body text-ink",
        "placeholder:text-ink-3 transition-colors duration-[120ms] hover:border-ink-3",
        "disabled:cursor-not-allowed disabled:opacity-50",
        className,
      )}
      {...rest}
    />
  );
}

export function Textarea({ className, ...rest }: ComponentProps<"textarea">) {
  return (
    <textarea
      className={cn(
        "w-full resize-y rounded-[var(--r-sm)] border border-line-2 bg-bg-deep px-3 py-2 text-body leading-relaxed text-ink",
        "placeholder:text-ink-3 transition-colors duration-[120ms] hover:border-ink-3",
        "min-h-24 disabled:cursor-not-allowed disabled:opacity-50",
        className,
      )}
      {...rest}
    />
  );
}

export function Field({
  label,
  hint,
  error,
  htmlFor,
  children,
  className,
  aside,
}: {
  label: string;
  hint?: ReactNode;
  error?: string | null;
  htmlFor?: string;
  children: ReactNode;
  className?: string;
  aside?: ReactNode;
}) {
  return (
    <div className={cn("flex flex-col gap-1.5", className)}>
      <div className="flex items-baseline justify-between gap-3">
        <label htmlFor={htmlFor} className="text-label font-medium text-ink">
          {label}
        </label>
        {aside}
      </div>
      {children}
      {error ? (
        <p className="text-meta text-danger" role="alert">
          {error}
        </p>
      ) : hint ? (
        <p className="text-meta text-ink-3">{hint}</p>
      ) : null}
    </div>
  );
}

/* -------------------------------- controls ------------------------------- */

export function Switch({
  checked,
  onCheckedChange,
  label,
  disabled,
}: {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onCheckedChange(!checked)}
      className={cn(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors duration-[160ms]",
        checked ? "border-signal bg-signal" : "border-line-2 bg-bg-deep hover:border-ink-3",
        disabled && "cursor-not-allowed opacity-40",
      )}
    >
      <span
        className={cn(
          "pointer-events-none block size-3 rounded-full transition-transform duration-[160ms] ease-out",
          checked
            ? "translate-x-[19px] bg-on-signal rtl:translate-x-[-19px]"
            : "translate-x-[3px] bg-ink-3 rtl:translate-x-[-3px]",
        )}
      />
    </button>
  );
}

export function Checkbox({
  checked,
  onCheckedChange,
  label,
}: {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  label: string;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onCheckedChange(!checked)}
      className={cn(
        "grid size-5 place-items-center rounded-[var(--r-xs)] border transition-colors duration-[120ms]",
        checked ? "border-signal bg-signal text-on-signal" : "border-line-2 bg-bg-deep hover:border-ink-3",
      )}
    >
      {checked && (
        <svg viewBox="0 0 12 12" className="size-3" aria-hidden="true">
          <path d="M1.5 6.2 4.4 9 10.5 2.6" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="square" />
        </svg>
      )}
    </button>
  );
}

/* ---------------------------------- leds --------------------------------- */

export type LedTone = "signal" | "info" | "ok" | "warn" | "danger" | "muted";

const ledTones: Record<LedTone, string> = {
  signal: "bg-signal",
  info: "bg-info",
  ok: "bg-ok",
  warn: "bg-warn",
  danger: "bg-danger",
  muted: "bg-ink-3",
};

/** 7×7px square hardware LED for active and live states (#333). */
export function Led({
  tone = "signal",
  pulse = false,
  className,
}: {
  tone?: LedTone;
  pulse?: boolean;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-block size-[7px] shrink-0 rounded-[1px]",
        ledTones[tone],
        pulse && "animate-pulse motion-reduce:animate-none",
        className,
      )}
      aria-hidden="true"
    />
  );
}

/* --------------------------------- badges -------------------------------- */

export type BadgeTone = "default" | "signal" | "info" | "warn" | "danger" | "ok" | "solid" | "outline";

const badgeTones: Record<BadgeTone, string> = {
  default: "border-line-2 bg-surface-2 text-ink-2",
  signal: "border-signal/45 bg-signal/12 text-signal-text",
  info: "border-info/45 bg-info/12 text-info",
  warn: "border-warn/45 bg-warn/12 text-warn",
  danger: "border-danger/45 bg-danger/10 text-danger",
  ok: "border-ok/40 bg-ok/10 text-ok",
  solid: "border-transparent bg-ink text-bg",
  outline: "border-line-2 bg-transparent text-ink-3",
};

const badgeLedTone: Partial<Record<BadgeTone, LedTone>> = {
  signal: "signal",
  info: "info",
  warn: "warn",
  danger: "danger",
  ok: "ok",
  default: "muted",
  outline: "muted",
};

export function Badge({
  children,
  tone = "default",
  led = false,
  className,
}: {
  children: ReactNode;
  tone?: BadgeTone;
  led?: boolean;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex h-[21px] items-center gap-1.5 rounded-[var(--r-xs)] border px-1.5 text-meta font-medium leading-none",
        badgeTones[tone],
        className,
      )}
    >
      {led && <Led tone={badgeLedTone[tone] ?? "signal"} />}
      {children}
    </span>
  );
}

/* --------------------------------- cards --------------------------------- */

export function Card({ className, ...rest }: ComponentProps<"div">) {
  return (
    <div
      className={cn("rounded-[var(--r-md)] border border-line bg-surface p-5 sm:p-6", className)}
      {...rest}
    />
  );
}

export const Panel = Card;

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

/* ------------------------------- progress -------------------------------- */

export function Progress({
  value,
  max = 100,
  className,
  tone = "signal",
  label,
}: {
  value: number | null;
  max?: number;
  className?: string;
  tone?: "signal" | "info" | "warn" | "danger" | "ok";
  label?: string;
}) {
  const pct = value === null ? null : Math.min(100, Math.max(0, (value / max) * 100));
  const color =
    tone === "danger"
      ? "bg-danger"
      : tone === "ok"
        ? "bg-ok"
        : tone === "info"
          ? "bg-info"
          : tone === "warn"
            ? "bg-warn"
            : "bg-signal";
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={max}
      aria-valuenow={value === null ? undefined : Math.round(value)}
      className={cn("relative h-1.5 w-full overflow-hidden rounded-[var(--r-xs)] bg-line", className)}
    >
      {pct === null ? (
        <div className={cn("absolute inset-y-0 start-0 w-1/3 animate-[slide_1.4s_ease-in-out_infinite]", color)} />
      ) : (
        <div className={cn("h-full transition-[width] duration-500 ease-out", color)} style={{ width: `${pct}%` }} />
      )}
    </div>
  );
}

/**
 * 16-segment dual-color progress gauge (#333): completed progress renders in
 * `--signal`, active network/stream head or secondary progress renders in
 * `--info`, and remaining capacity sits in `bg-line`.
 */
export function SegmentedGauge({
  value,
  secondary,
  segments = 16,
  active = false,
  size = "md",
  label,
  className,
}: {
  value: number | null;
  /** Optional secondary fill (0..100) painted in `--info` (e.g. network stream/verify). */
  secondary?: number | null;
  segments?: number;
  /** Highlights the leading frontier segment(s) in `--info` while a transfer is live. */
  active?: boolean;
  size?: "sm" | "md";
  label?: string;
  className?: string;
}) {
  const clamped = value === null ? null : Math.min(100, Math.max(0, value));
  const filled = clamped === null ? 0 : Math.round((clamped / 100) * segments);
  const secondaryFilled =
    typeof secondary === "number"
      ? Math.min(segments, Math.max(filled, Math.round((Math.min(100, Math.max(0, secondary)) / 100) * segments)))
      : active && clamped !== null && filled < segments
        ? Math.min(segments, filled + (clamped > 0 ? 2 : 1))
        : filled;

  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={clamped === null ? undefined : Math.round(clamped)}
      className={cn("grid w-full gap-[2px]", size === "sm" ? "h-1.5" : "h-2.5", className)}
      style={{ gridTemplateColumns: `repeat(${segments}, minmax(0, 1fr))` }}
    >
      {Array.from({ length: segments }, (_, index) => {
        const isPrimary = clamped !== null && index < filled;
        const isInfo =
          clamped === null
            ? active && index < 4
            : !isPrimary && index < secondaryFilled;
        return (
          <span
            key={index}
            className={cn(
              "h-full rounded-[1px] transition-colors duration-[160ms]",
              isPrimary ? "bg-signal" : isInfo ? "bg-info" : "bg-line",
              clamped === null && isInfo && "animate-pulse motion-reduce:animate-none",
            )}
          />
        );
      })}
    </div>
  );
}

/* ------------------------------- telemetry ------------------------------- */

export interface TelemetryCell {
  label: string;
  value: ReactNode;
  detail?: ReactNode;
  tone?: "default" | "signal" | "info" | "ok" | "warn" | "danger";
}

const telemetryTone: Record<NonNullable<TelemetryCell["tone"]>, string> = {
  default: "text-ink",
  signal: "text-signal-text",
  info: "text-info",
  ok: "text-ok",
  warn: "text-warn",
  danger: "text-danger",
};

/** 4-column ARC/Swiss telemetry strip (#333). Labels use UI sans; values use `.num`. */
export function TelemetryStrip({
  items,
  className,
  label,
}: {
  items: TelemetryCell[];
  className?: string;
  label?: string;
}) {
  return (
    <dl
      aria-label={label}
      className={cn(
        "grid grid-cols-2 divide-y divide-line overflow-hidden rounded-[var(--r-md)] border border-line bg-surface sm:grid-cols-4 sm:divide-x sm:divide-y-0",
        className,
      )}
    >
      {items.map((item) => (
        <div key={item.label} className="flex flex-col gap-1 px-4 py-3.5">
          <dt className="text-meta font-medium text-ink-3">{item.label}</dt>
          <dd className={cn("num text-title font-semibold leading-tight tracking-[-0.02em]", telemetryTone[item.tone ?? "default"])}>
            {item.value}
          </dd>
          {item.detail && <dd className="text-meta text-ink-3">{item.detail}</dd>}
        </div>
      ))}
    </dl>
  );
}

/* --------------------------------- misc ---------------------------------- */

export function Skeleton({ className }: { className?: string }) {
  return <div className={cn("shimmer rounded-[var(--r-sm)]", className)} aria-hidden="true" />;
}

export function Segmented<T extends string>({
  value,
  onChange,
  options,
  label,
  size = "md",
}: {
  value: T;
  onChange: (value: T) => void;
  options: { value: T; label: string; icon?: ReactNode }[];
  label: string;
  size?: "sm" | "md";
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className="inline-flex items-stretch rounded-[var(--r-sm)] border border-line-2 bg-bg-deep p-0.5"
    >
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={value === option.value}
          onClick={() => onChange(option.value)}
          className={cn(
            "inline-flex items-center justify-center gap-1.5 rounded-[var(--r-xs)] px-2.5 font-medium transition-colors duration-[120ms]",
            size === "sm" ? "h-7 text-meta" : "h-8 text-label",
            value === option.value ? "bg-surface-2 text-ink" : "text-ink-3 hover:text-ink-2",
          )}
        >
          {option.icon}
          <span className={size === "sm" ? "sr-only" : undefined}>{option.label}</span>
        </button>
      ))}
    </div>
  );
}
