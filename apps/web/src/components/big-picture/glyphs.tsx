import type { ReactNode } from "react";

/**
 * Console glyphs (#268).
 *
 * Drawn for the product: 24-unit grid, 2-unit stroke, round caps, one solid
 * variant for the active state where it helps. Lucide is not used inside the
 * console chrome: a 1.5 px hairline icon next to 28u type reads as a mistake at
 * three metres, and two destinations must never share a glyph.
 */

export type GlyphName =
  | "search"
  | "library"
  | "downloads"
  | "settings"
  | "power"
  | "play"
  | "install"
  | "back"
  | "menu"
  | "check"
  | "pad"
  | "vibrate"
  | "volume"
  | "volumeOff"
  | "sleep"
  | "restart"
  | "signout"
  | "bundle";

function shape(name: GlyphName): ReactNode {
  switch (name) {
    case "search":
      return (
        <>
          <circle cx="10.5" cy="10.5" r="6.5" />
          <path d="M15.5 15.5 21 21" />
        </>
      );
    case "library":
      return (
        <>
          <path d="M4 5v14M9 5v14" />
          <path d="M14.5 5.8 19 6.6l2.2 12-4.5.8z" />
        </>
      );
    case "downloads":
      return (
        <>
          <path d="M12 3v10" />
          <path d="M8 9.5 12 13.5 16 9.5" />
          <path d="M4 16v3.5h16V16" />
        </>
      );
    case "settings":
      return (
        <>
          <path d="M3 8h7M14 8h7" />
          <path d="M3 16h4M11 16h10" />
          <circle cx="12" cy="8" r="2.5" />
          <circle cx="9" cy="16" r="2.5" />
        </>
      );
    case "power":
      return (
        <>
          <path d="M12 3v8" />
          <path d="M7.2 6.2a7 7 0 1 0 9.6 0" />
        </>
      );
    case "play":
      return <path d="M8 4.5 20 12 8 19.5z" fill="currentColor" stroke="none" />;
    case "install":
      return (
        <>
          <path d="M12 4v9" />
          <path d="M8.5 9.5 12 13l3.5-3.5" />
          <path d="M4 17h16" />
        </>
      );
    case "back":
      return (
        <>
          <path d="M14 5 7 12l7 7" />
        </>
      );
    case "menu":
      return (
        <>
          <path d="M5 8h14M5 12h14M5 16h14" />
        </>
      );
    case "check":
      return <path d="M5 12.5 10 17.5 19.5 7" />;
    case "bundle":
      return (
        <>
          <path d="M4 8h16v11H4z" />
          <path d="M4 8l2-3h12l2 3" />
          <path d="M10 12h4" />
        </>
      );
    case "pad":
      return (
        <>
          <path d="M7 8h10a4 4 0 0 1 3.9 4.9l-.7 3.2A2.4 2.4 0 0 1 17.6 18l-2.1-2H8.5l-2.1 2a2.4 2.4 0 0 1-2.6-1.9l-.7-3.2A4 4 0 0 1 7 8z" />
          <path d="M8 11v3M6.5 12.5h3" />
          <circle cx="16" cy="12" r="0.9" fill="currentColor" stroke="none" />
          <circle cx="18" cy="14" r="0.9" fill="currentColor" stroke="none" />
        </>
      );
    case "vibrate":
      return (
        <>
          <rect x="8" y="6" width="8" height="12" rx="2" />
          <path d="M4 9v6M20 9v6" />
        </>
      );
    case "volume":
      return (
        <>
          <path d="M4 10v4h3l4 3.5v-11L7 10z" />
          <path d="M15 9.5a3.5 3.5 0 0 1 0 5M18 7a7 7 0 0 1 0 10" />
        </>
      );
    case "volumeOff":
      return (
        <>
          <path d="M4 10v4h3l4 3.5v-11L7 10z" />
          <path d="M15 9.5 20 14.5M20 9.5 15 14.5" />
        </>
      );
    case "sleep":
      return <path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z" />;
    case "restart":
      return (
        <>
          <path d="M4 12a8 8 0 1 0 2.4-5.7" />
          <path d="M4 4v4.5h4.5" />
        </>
      );
    case "signout":
      return (
        <>
          <path d="M14 4H5v16h9" />
          <path d="M12 12h9M17.5 7.5 22 12l-4.5 4.5" />
        </>
      );
  }
}

export function Glyph({
  name,
  className,
  size = 24,
  label,
}: {
  name: GlyphName;
  className?: string;
  /** Unit size; the console passes 24–28u. */
  size?: number;
  /** Accessible name; omit when the glyph sits next to its own label. */
  label?: string;
}) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      className={className}
    >
      {shape(name)}
    </svg>
  );
}
