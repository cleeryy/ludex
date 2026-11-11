import { cn } from "@/lib/cn";

/**
 * LUDEX wordmark. Drawn, not typeset: a compressed geometric alphabet with a
 * squared U, a chamfered D and a signal-yellow full stop. The same paths are
 * the app icon and the favicon.
 */
export function Wordmark({
  height = 18,
  withDot = true,
  className,
  label = "Ludex",
}: {
  height?: number;
  withDot?: boolean;
  className?: string;
  label?: string;
}) {
  const width = withDot ? 452 : 418;
  return (
    <svg
      viewBox={`0 0 ${width} 100`}
      height={height}
      width={(height * width) / 100}
      role="img"
      aria-label={label}
      className={cn("overflow-visible", className)}
    >
      <g fill="currentColor">
        <path d="M0 0H22V78H66V100H0Z" />
        <path d="M80 0H102V78H136V0H158V100H80Z" />
        <path d="M172 0H218L246 26V74L218 100H172ZM194 22V78H212L224 66V34L212 22Z" fillRule="evenodd" />
        <path d="M260 0H326V22H282V39H310V61H282V78H326V100H260Z" />
        <path d="M340 0H362L418 100H396Z" />
        <path d="M396 0H418L362 100H340Z" />
      </g>
      {withDot && <rect x="434" y="78" width="18" height="18" fill="var(--signal)" />}
    </svg>
  );
}

/** Square monogram used as the app icon and in tight spaces. */
export function Mark({ size = 28, className }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} role="img" aria-label="Ludex" className={className}>
      <rect width="64" height="64" fill="var(--bg-deep)" />
      <g transform="translate(14 12) scale(0.4)" fill="var(--ink)">
        <path d="M0 0H22V78H66V100H0Z" />
      </g>
      <rect x="44" y="40" width="12" height="12" fill="var(--signal)" />
    </svg>
  );
}
