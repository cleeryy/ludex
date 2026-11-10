import fs from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * Theme contrast guard (#23). Parses the token blocks from index.css and
 * requires WCAG AA (4.5:1) for every text/surface pair the UI uses, in both
 * themes and inside the dark `force-dark` stage on the game page.
 */

const css = fs.readFileSync(new URL("./index.css", import.meta.url), "utf8");

function tokensFor(selector: string): Record<string, string> {
  const start = css.indexOf(selector);
  if (start === -1) throw new Error(`Missing token block ${selector}`);
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  const block = css.slice(open + 1, close);
  const tokens: Record<string, string> = {};
  for (const match of block.matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{3,8})/g)) {
    tokens[match[1]] = match[2];
  }
  return tokens;
}

function luminance(hex: string): number {
  let value = hex.replace("#", "");
  if (value.length === 3) value = value.split("").map((char) => char + char).join("");
  const [r, g, b] = [0, 2, 4].map((offset) => {
    const channel = Number.parseInt(value.slice(offset, offset + 2), 16) / 255;
    return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(foreground: string, background: string): number {
  const [light, dark] = [luminance(foreground), luminance(background)].sort((a, b) => b - a);
  return (light + 0.05) / (dark + 0.05);
}

const PAIRS: [string, string][] = [
  ["ink", "bg"],
  ["ink-2", "bg"],
  ["ink-3", "bg"],
  ["ink", "surface"],
  ["ink-2", "surface"],
  ["ink-3", "surface"],
  ["ink", "bg-deep"],
  ["ink-2", "bg-deep"],
  ["ink-3", "bg-deep"],
  ["signal-text", "bg"],
  ["signal-text", "surface"],
  ["danger", "bg"],
  ["danger", "surface"],
  ["ok", "bg"],
  ["ok", "surface"],
  ["on-signal", "signal"],
];

describe.each([
  ["dark", ":root {"],
  ["light", ':root[data-theme="light"] {'],
  ["force-dark", ".force-dark {"],
])("%s theme contrast", (name, selector) => {
  const tokens = tokensFor(selector);

  it("defines every paired token", () => {
    for (const [foreground, background] of PAIRS) {
      expect(tokens[foreground], `${name}: --${foreground}`).toBeTruthy();
      expect(tokens[background], `${name}: --${background}`).toBeTruthy();
    }
  });

  it.each(PAIRS)("%s on %s meets WCAG AA", (foreground, background) => {
    expect(contrast(tokens[foreground], tokens[background])).toBeGreaterThanOrEqual(4.5);
  });
});
