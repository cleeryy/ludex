import { cn } from "@/lib/cn";

/**
 * LUDEX wordmark and monogram. Drawn, not typeset: a compressed geometric
 * alphabet with a squared U, a chamfered D and a signal full stop. The same
 * paths are the app icon and the favicon.
 *
 * #268 adds the Console cut (`cut="console"`): the same drawing with a heavier
 * stroke and a larger dot, because the 22-unit stroke reads thin at three
 * metres. The wordmark only appears at boot, in About and on empty/error walls —
 * the console bar carries the monogram.
 */

const LETTERS = [
  "M0 0H22V78H66V100H0Z",
  "M80 0H102V78H136V0H158V100H80Z",
  "M172 0H218L246 26V74L218 100H172ZM194 22V78H212L224 66V34L212 22Z",
  "M260 0H326V22H282V39H310V61H282V78H326V100H260Z",
  "M340 0H362L418 100H396Z",
  "M396 0H418L362 100H340Z",
];

const FULL_WIDTH = 452;
const NO_DOT_WIDTH = 418;

export function Wordmark({
  height = 18,
  withDot = true,
  cut = "default",
  className,
  label = "Ludex",
}: {
  height?: number;
  withDot?: boolean;
  /** `console` is the 10-foot cut: heavier stroke, larger dot. */
  cut?: "default" | "console";
  className?: string;
  label?: string;
}) {
  const width = withDot ? FULL_WIDTH : NO_DOT_WIDTH;
  const consoleCut = cut === "console";
  return (
    <svg
      viewBox={`0 0 ${width} 100`}
      height={height}
      width={(height * width) / 100}
      role="img"
      aria-label={label}
      className={cn("overflow-visible", className)}
    >
      <g
        fill="currentColor"
        stroke={consoleCut ? "currentColor" : undefined}
        strokeWidth={consoleCut ? 7 : undefined}
        strokeLinejoin={consoleCut ? "round" : undefined}
        paintOrder={consoleCut ? "stroke" : undefined}
      >
        {LETTERS.map((path) => (
          <path key={path} d={path} fillRule={path.includes("ZM") ? "evenodd" : undefined} />
        ))}
      </g>
      {withDot && (
        <rect
          x={consoleCut ? 430 : 434}
          y={consoleCut ? 74 : 78}
          width={consoleCut ? 22 : 18}
          height={consoleCut ? 22 : 18}
          fill="var(--signal)"
        />
      )}
    </svg>
  );
}

/**
 * The Console monogram: the chamfered D and the dot, nothing else. Used in the
 * bar at 28u, where a 17px wordmark used to be a grey smear at three metres.
 */
export function Monogram({
  size = 28,
  className,
  label = "Ludex",
}: {
  size?: number;
  className?: string;
  label?: string;
}) {
  return (
    <svg
      viewBox="0 0 56 64"
      width={size}
      height={size}
      role="img"
      aria-label={label}
      className={className}
    >
      <path
        d="M4 4H28L42 17V47L28 60H4ZM16 16V48H26L32 42V22L26 16Z"
        fill="currentColor"
        fillRule="evenodd"
      />
      <rect x="42" y="46" width="12" height="12" fill="var(--signal)" />
    </svg>
  );
}

/** Square mark used as the app icon: the chamfered D on the deep background. */
export function Mark({ size = 28, className }: { size?: number; className?: string }) {
  return (
    <svg viewBox="0 0 64 64" width={size} height={size} role="img" aria-label="Ludex" className={className}>
      <rect width="64" height="64" rx="14" fill="var(--bg-deep)" />
      <path
        d="M15 12H32L42 22V42L32 52H15ZM24 22V42H31L34 39V25L31 22Z"
        fill="var(--ink)"
        fillRule="evenodd"
      />
      <rect x="42" y="42" width="10" height="10" fill="var(--signal)" />
    </svg>
  );
}
