/**
 * WCAG contrast helpers. Pure functions, no DOM: the boot path stores resolved
 * values so this module never has to run before the first paint.
 */

export const AA_TEXT = 4.5;
export const AA_FOCUS = 3;
export const TV_TEXT = 7;
export const TV_FOCUS = 4.5;

const HEX_PATTERN = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;

/** Accepts #rgb and #rrggbb; returns lowercase `#rrggbb`, or null. */
export function normalizeHex(input: string): string | null {
  if (!HEX_PATTERN.test(input.trim())) return null;
  let value = input.trim().toLowerCase().slice(1);
  if (value.length === 3) value = value.split("").map((char) => char + char).join("");
  return `#${value}`;
}

export function isHexColor(input: string): boolean {
  return normalizeHex(input) !== null;
}

export function hexToRgb(hex: string): [number, number, number] {
  const normalized = normalizeHex(hex);
  if (!normalized) throw new Error(`not a hex colour: ${hex}`);
  return [1, 3, 5].map((offset) => Number.parseInt(normalized.slice(offset, offset + 2), 16)) as [
    number,
    number,
    number,
  ];
}

export function rgbToHex([r, g, b]: [number, number, number]): string {
  const channel = (value: number) => Math.round(Math.min(255, Math.max(0, value))).toString(16).padStart(2, "0");
  return `#${channel(r)}${channel(g)}${channel(b)}`;
}

export function luminance(hex: string): number {
  const [r, g, b] = hexToRgb(hex).map((channel) => {
    const value = channel / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrastRatio(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light + 0.05) / (dark + 0.05);
}

export function roundRatio(ratio: number): number {
  return Math.round(ratio * 100) / 100;
}

export function mixHex(a: string, b: string, amount: number): string {
  const from = hexToRgb(a);
  const to = hexToRgb(b);
  const ratio = Math.min(1, Math.max(0, amount));
  return rgbToHex([
    from[0] + (to[0] - from[0]) * ratio,
    from[1] + (to[1] - from[1]) * ratio,
    from[2] + (to[2] - from[2]) * ratio,
  ]);
}

/**
 * Smallest mix towards white (or black) that clears `min` on every background.
 * Used by the TV variant and by accent fallbacks, never to accept a user colour
 * that failed validation: the picker rejects those instead.
 */
export function ensureContrast(color: string, backgrounds: readonly string[], min: number): string {
  if (backgrounds.every((background) => contrastRatio(color, background) >= min)) return color;
  for (const target of ["#ffffff", "#000000"]) {
    for (let step = 1; step <= 100; step += 1) {
      const candidate = mixHex(color, target, step / 100);
      if (backgrounds.every((background) => contrastRatio(candidate, background) >= min)) return candidate;
    }
  }
  return color;
}

/** Best foreground for text placed on `background`, preferring the theme's own inks. */
export function bestForeground(background: string, candidates: readonly string[]): string {
  const ranked = candidates
    .map((candidate) => ({ candidate, ratio: contrastRatio(candidate, background) }))
    .sort((a, b) => b.ratio - a.ratio);
  return ranked[0]?.candidate ?? "#ffffff";
}
