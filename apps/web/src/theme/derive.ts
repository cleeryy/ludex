import {
  AA_FOCUS,
  AA_TEXT,
  TV_FOCUS,
  TV_TEXT,
  bestForeground,
  contrastRatio,
  ensureContrast,
  mixHex,
  normalizeHex,
  roundRatio,
} from "./contrast";
import { contrastResults } from "./registry";
import type { BuiltinTheme, ThemeIssue, ThemeTokens } from "./types";

/**
 * Derived variants. A user accent, a background tint and the TV-safe variant
 * are all computed from a complete base theme, so the result is always a full
 * token map and components never need to know which layer produced a value.
 */

export interface DerivedResult {
  tokens: ThemeTokens;
  issue: ThemeIssue | null;
  ratio: number;
}

function minContrast(foreground: string, backgrounds: readonly string[]): number {
  return roundRatio(Math.min(...backgrounds.map((background) => contrastRatio(foreground, background))));
}

const ACCENT_BACKGROUNDS = ["bg", "surface", "surface-2", "elevated"] as const;

export function applyAccent(base: ThemeTokens, accentInput: string | null | undefined): DerivedResult {
  if (!accentInput) return { tokens: base, issue: null, ratio: 0 };
  const accent = normalizeHex(accentInput);
  if (!accent) {
    return {
      tokens: base,
      issue: { code: "bad-color", token: "signal", foreground: "accent" },
      ratio: 0,
    };
  }

  const backgrounds = ACCENT_BACKGROUNDS.map((key) => base[key]);
  const ratio = minContrast(accent, backgrounds);
  if (ratio < AA_TEXT) {
    return {
      tokens: base,
      issue: {
        code: "low-contrast",
        foreground: "accent",
        background: "background",
        ratio,
        min: AA_TEXT,
      },
      ratio,
    };
  }

  const inkCandidates = [base["bg-deep"], base.ink, "#ffffff", "#000000"];
  const onSignal = bestForeground(accent, inkCandidates);
  const signalRatio = roundRatio(contrastRatio(onSignal, accent));
  if (signalRatio < AA_TEXT) {
    return {
      tokens: base,
      issue: {
        code: "low-contrast",
        foreground: "accent",
        background: "signal",
        ratio: signalRatio,
        min: AA_TEXT,
      },
      ratio,
    };
  }

  return {
    tokens: {
      ...base,
      signal: accent,
      "on-signal": onSignal,
      "signal-text": accent,
      "signal-line": mixHex(accent, base["line-2"], 0.55),
      focus: ensureContrast(accent, [base.bg, base.surface], AA_FOCUS),
    },
    issue: null,
    ratio,
  };
}

const TINT_STRENGTH = {
  bg: 0.1,
  "bg-deep": 0.12,
  surface: 0.07,
  "surface-2": 0.07,
  elevated: 0.06,
  line: 0.12,
  "line-2": 0.16,
} as const;

export function applyTint(base: ThemeTokens, tintInput: string | null | undefined): DerivedResult {
  if (!tintInput) return { tokens: base, issue: null, ratio: 0 };
  const tint = normalizeHex(tintInput);
  if (!tint) {
    return {
      tokens: base,
      issue: { code: "bad-color", token: "bg", foreground: "tint" },
      ratio: 0,
    };
  }

  const tokens: ThemeTokens = { ...base };
  for (const [key, amount] of Object.entries(TINT_STRENGTH) as [keyof typeof TINT_STRENGTH, number][]) {
    tokens[key] = mixHex(base[key], tint, amount);
  }

  const failure = contrastResults(tokens).find((check) => !check.ok);
  if (failure) {
    return {
      tokens: base,
      issue: {
        code: "low-contrast",
        foreground: "tint",
        background: failure.pair.background,
        ratio: failure.ratio,
        min: failure.pair.min,
      },
      ratio: failure.ratio,
    };
  }

  return {
    tokens,
    issue: null,
    ratio: minContrast(tokens["ink-3"], [tokens.bg, tokens.surface, tokens["surface-2"], tokens.elevated]),
  };
}

const TV_TEXT_PAIRS: readonly [keyof ThemeTokens, readonly (keyof ThemeTokens)[]][] = [
  ["ink", ["bg", "surface", "surface-2", "elevated"]],
  ["ink-2", ["bg", "surface", "elevated"]],
  ["ink-3", ["bg", "surface", "surface-2", "elevated"]],
  ["signal-text", ["bg", "surface"]],
  ["danger", ["bg", "surface"]],
  ["ok", ["bg", "surface"]],
  ["warn", ["bg", "surface"]],
  ["stage-ink", ["stage"]],
  ["stage-ink-2", ["stage"]],
  ["stage-ok", ["stage"]],
];

/**
 * TV-safe variant: 7:1 for every piece of text (AAA), a stronger focus ring and
 * no decorative grain. The stage text tokens are lifted too: Big Picture draws
 * its hero over artwork, so those pairs must stay readable at ten feet.
 */
export function tvTokens(base: ThemeTokens): ThemeTokens {
  const tokens: ThemeTokens = { ...base };
  for (const [foreground, backgrounds] of TV_TEXT_PAIRS) {
    tokens[foreground] = ensureContrast(
      base[foreground],
      backgrounds.map((background) => base[background]),
      TV_TEXT,
    );
  }
  tokens.focus = ensureContrast(base.focus, [base.bg, base.surface], TV_FOCUS);
  tokens.line = mixHex(base.line, tokens.ink, 0.18);
  tokens["line-2"] = mixHex(base["line-2"], tokens.ink, 0.28);
  tokens["grain-opacity"] = "0";
  return tokens;
}

export interface ResolvedTheme {
  tokens: ThemeTokens;
  /** Base theme the tokens were derived from. */
  theme: BuiltinTheme;
  accentIssue: ThemeIssue | null;
  tintIssue: ThemeIssue | null;
}

/** Base theme + optional accent + optional tint, with safe fallbacks. */
export function deriveTheme(
  theme: BuiltinTheme,
  accent: string | null,
  tint: string | null,
): ResolvedTheme {
  const accented = applyAccent(theme.tokens, accent);
  const tinted = applyTint(accented.tokens, tint);
  return {
    tokens: tinted.tokens,
    theme,
    accentIssue: accented.issue,
    tintIssue: tinted.issue,
  };
}
