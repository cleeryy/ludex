import type { ComponentProps, ReactNode } from "react";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/cn";

/* --------------------------------- button -------------------------------- */

type ButtonVariant = "primary" | "default" | "ghost" | "quiet" | "danger";
type ButtonSize = "sm" | "md" | "lg" | "icon";

const buttonVariants: Record<ButtonVariant, string> = {
  primary:
    "bg-signal text-on-signal border border-signal hover:brightness-[1.07] active:brightness-95 font-semibold",
  default: "border border-line-2 bg-surface text-ink hover:border-ink-3 hover:bg-surface-2",
  ghost: "border border-transparent text-ink-2 hover:text-ink hover:bg-surface",
  quiet: "border border-transparent text-ink-2 hover:text-ink",
  danger: "border border-danger/45 text-danger hover:bg-danger/10 hover:border-danger/70",
};

const buttonSizes: Record<ButtonSize, string> = {
  sm: "h-8 px-3 text-[13px] gap-1.5",
  md: "h-11 px-4 text-sm gap-2",
  lg: "h-12 px-5 text-[15px] gap-2",
  icon: "h-11 w-11 shrink-0",
};

export type ButtonProps = ComponentProps<"button"> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  icon?: ReactNode;
  iconRight?: ReactNode;
};

export function Button({
  variant = "default",
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
        "inline-flex select-none items-center justify-center rounded-[2px] transition-[background-color,border-color,color,filter,opacity] duration-[120ms]",
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
        "h-11 w-full rounded-[2px] border border-line bg-surface px-3 text-[15px] text-ink",
        "placeholder:text-ink-3 transition-colors duration-[120ms] hover:border-line-2",
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
        "w-full resize-y rounded-[2px] border border-line bg-surface px-3 py-2.5 text-[15px] leading-relaxed text-ink",
        "placeholder:text-ink-3 transition-colors duration-[120ms] hover:border-line-2",
        "min-h-28",
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
        <label htmlFor={htmlFor} className="text-[13px] font-medium text-ink">
          {label}
        </label>
        {aside}
      </div>
      {children}
      {error ? (
        <p className="text-[12.5px] text-danger" role="alert">
          {error}
        </p>
      ) : hint ? (
        <p className="text-[12.5px] text-ink-3">{hint}</p>
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
        checked ? "border-signal/70 bg-signal/20" : "border-line-2 bg-surface-2",
        disabled && "cursor-not-allowed opacity-40",
      )}
    >
      <span
        className={cn(
          "pointer-events-none block size-3 rounded-full transition-transform duration-[160ms] ease-out",
          checked ? "translate-x-[19px] bg-signal rtl:translate-x-[-19px]" : "translate-x-[3px] bg-ink-3 rtl:translate-x-[-3px]",
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
        "grid size-5 place-items-center rounded-[2px] border transition-colors duration-[120ms]",
        checked ? "border-signal bg-signal text-on-signal" : "border-line-2 bg-surface hover:border-ink-3",
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

export function Badge({
  children,
  tone = "default",
  className,
}: {
  children: ReactNode;
  tone?: "default" | "signal" | "danger" | "ok" | "solid" | "outline";
  className?: string;
}) {
  const tones: Record<string, string> = {
    default: "border-line-2 bg-surface text-ink-2",
    signal: "border-signal/50 bg-signal/12 text-signal-text",
    danger: "border-danger/45 bg-danger/10 text-danger",
    ok: "border-ok/40 bg-ok/10 text-ok",
    solid: "border-transparent bg-ink text-bg",
    outline: "border-line-2 text-ink-3",
  };
  return (
    <span
      className={cn(
        "inline-flex h-5 items-center rounded-[2px] border px-1.5 text-[11px] font-medium leading-none",
        tones[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

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
  tone?: "signal" | "danger" | "ok";
  label?: string;
}) {
  const pct = value === null ? null : Math.min(100, Math.max(0, (value / max) * 100));
  const color = tone === "danger" ? "bg-danger" : tone === "ok" ? "bg-ok" : "bg-signal";
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={max}
      aria-valuenow={value === null ? undefined : Math.round(value)}
      className={cn("relative h-1 w-full overflow-hidden rounded-full bg-line", className)}
    >
      {pct === null ? (
        <div className={cn("absolute inset-y-0 start-0 w-1/3 animate-[slide_1.4s_ease-in-out_infinite]", color)} />
      ) : (
        <div className={cn("h-full transition-[width] duration-500 ease-out", color)} style={{ width: `${pct}%` }} />
      )}
    </div>
  );
}

export function Skeleton({ className }: { className?: string }) {
  return <div className={cn("shimmer rounded-[2px]", className)} aria-hidden="true" />;
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
      className="inline-flex items-stretch rounded-[2px] border border-line bg-surface p-0.5"
    >
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={value === option.value}
          onClick={() => onChange(option.value)}
          className={cn(
            "inline-flex items-center justify-center gap-1.5 rounded-[1px] px-2.5 transition-colors duration-[120ms]",
            size === "sm" ? "h-7 text-[12px]" : "h-9 text-[13px]",
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
