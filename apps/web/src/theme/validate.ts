import { applyAccent } from "./derive";
import { contrastRatio, isHexColor, normalizeHex, roundRatio } from "./contrast";
import { CONTRAST_PAIRS, builtinTheme, isBuiltinThemeId } from "./registry";
import {
  COLOR_TOKEN_KEYS,
  THEME_TOKEN_KEYS,
  type BuiltinTheme,
  type ThemeFile,
  type ThemeIssue,
  type ThemeTokenKey,
  type ThemeTokens,
} from "./types";

export type ThemeFileResult = { ok: true; theme: ThemeFile } | { ok: false; issues: ThemeIssue[] };

const KNOWN_TOKENS = new Set<string>(THEME_TOKEN_KEYS);

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

export function validateTokens(input: unknown): { tokens: ThemeTokens | null; issues: ThemeIssue[] } {
  const issues: ThemeIssue[] = [];
  const record = asRecord(input);
  if (!record) {
    return { tokens: null, issues: [{ code: "bad-field", field: "tokens" }] };
  }

  for (const key of Object.keys(record)) {
    if (!KNOWN_TOKENS.has(key)) issues.push({ code: "unknown-token", token: key });
  }

  const tokens = {} as ThemeTokens;
  for (const key of THEME_TOKEN_KEYS) {
    const value = record[key];
    if (value === undefined || value === null || value === "") {
      issues.push({ code: "missing-token", token: key });
      continue;
    }
    if (key === "grain-opacity") {
      const numeric = typeof value === "number" ? value : typeof value === "string" ? Number(value) : Number.NaN;
      if (!Number.isFinite(numeric) || numeric < 0 || numeric > 0.2) {
        issues.push({ code: "bad-grain", token: key });
        continue;
      }
      tokens[key] = String(numeric);
      continue;
    }
    if (typeof value !== "string") {
      issues.push({ code: "bad-color", token: key });
      continue;
    }
    if (!COLOR_TOKEN_KEYS.includes(key as ThemeTokenKey) || !isHexColor(value)) {
      issues.push({ code: "bad-color", token: key });
      continue;
    }
    tokens[key] = normalizeHex(value)!;
  }

  return { tokens: issues.length ? null : tokens, issues };
}

/** Contrast failures with the measured and required ratio, ready to localise. */
export function contrastIssues(tokens: ThemeTokens, limit = 8): ThemeIssue[] {
  const issues: ThemeIssue[] = [];
  const seen = new Set<string>();
  for (const pair of CONTRAST_PAIRS) {
    const ratio = roundRatio(contrastRatio(tokens[pair.foreground], tokens[pair.background]));
    if (ratio >= pair.min) continue;
    const key = `${pair.foreground}/${pair.background}`;
    if (seen.has(key)) continue;
    seen.add(key);
    issues.push({
      code: "low-contrast",
      foreground: pair.foreground,
      background: pair.background,
      ratio,
      min: pair.min,
    });
    if (issues.length >= limit) break;
  }
  return issues;
}

export function validateThemeFile(input: unknown): ThemeFileResult {
  const record = asRecord(input);
  if (!record) return { ok: false, issues: [{ code: "bad-format" }] };

  const issues: ThemeIssue[] = [];

  if (record.format !== undefined && record.format !== "ludex-theme") {
    issues.push({ code: "bad-format", field: "format" });
  }
  if (record.version !== undefined && record.version !== 1) {
    issues.push({ code: "bad-format", field: "version" });
  }

  const name = typeof record.name === "string" ? record.name.trim() : "";
  if (!name || name.length > 48) issues.push({ code: "bad-name", field: "name" });

  const base = typeof record.base === "string" ? record.base : "";
  const baseTheme = isBuiltinThemeId(base) ? builtinTheme(base) : null;
  if (!baseTheme) issues.push({ code: "bad-base", base });

  if (record.mode !== undefined && baseTheme && record.mode !== baseTheme.mode) {
    issues.push({ code: "bad-mode", field: "mode" });
  }

  const { tokens, issues: tokenIssues } = validateTokens(record.tokens);
  issues.push(...tokenIssues);

  if (tokens && baseTheme) {
    issues.push(...contrastIssues(tokens));

    if (record.accent !== undefined && record.accent !== null) {
      if (typeof record.accent !== "string" || !isHexColor(record.accent)) {
        issues.push({ code: "bad-color", foreground: "accent" });
      } else {
        const accented = applyAccent(tokens, record.accent);
        if (accented.issue) issues.push(accented.issue);
      }
    }
  }

  if (issues.length > 0 || !tokens || !baseTheme) {
    return { ok: false, issues: issues.length ? issues : [{ code: "bad-format" }] };
  }

  return {
    ok: true,
    theme: {
      format: "ludex-theme",
      version: 1,
      name,
      base: baseTheme.id,
      mode: baseTheme.mode,
      tokens,
      accent: typeof record.accent === "string" ? normalizeHex(record.accent) : null,
    },
  };
}

export function parseThemeFile(source: string): ThemeFileResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { ok: false, issues: [{ code: "bad-format", field: "json" }] };
  }
  return validateThemeFile(parsed);
}

export function serializeThemeFile(theme: ThemeFile): string {
  return `${JSON.stringify(theme, null, 2)}\n`;
}

export function themeFileFor(
  name: string,
  base: BuiltinTheme,
  tokens: ThemeTokens,
  accent: string | null,
): ThemeFile {
  return {
    format: "ludex-theme",
    version: 1,
    name: name.trim().slice(0, 48) || base.label,
    base: base.id,
    mode: base.mode,
    tokens,
    accent,
  };
}
