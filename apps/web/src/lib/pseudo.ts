/**
 * Pseudo-locale (en-XA) transform.
 *
 * Wrapping every translated string in brackets and accented letters makes two
 * classes of bug obvious without reading code:
 *
 *   - strings that never went through `t()` stay plain ASCII and stand out;
 *   - strings that are translated but clipped by a fixed width keep the
 *     missing bracket, because the pseudo text is slightly longer.
 *
 * `{placeholders}` are left byte-for-byte untouched so a pseudo run still
 * exercises interpolation, plurals and rich text.
 *
 * Keep the character map in sync with `apps/web/scripts/i18n-pseudo.mjs`; the
 * catalog check generates the same catalog from `en.json` and the unit test
 * (`src/lib/pseudo.test.ts`) pins the invariants both implementations rely on.
 */

const MAP: Record<string, string> = {
  a: "å",
  b: "ƀ",
  c: "ç",
  d: "ð",
  e: "é",
  f: "ƒ",
  g: "ĝ",
  h: "ĥ",
  i: "ï",
  j: "ĵ",
  k: "ķ",
  l: "ĺ",
  m: "ɱ",
  n: "ñ",
  o: "ö",
  p: "þ",
  q: "ǫ",
  r: "ŕ",
  s: "š",
  t: "ţ",
  u: "û",
  v: "ṽ",
  w: "ŵ",
  x: "ẋ",
  y: "ý",
  z: "ž",
  A: "Å",
  B: "Ɓ",
  C: "Ç",
  D: "Ð",
  E: "É",
  F: "Ƒ",
  G: "Ĝ",
  H: "Ĥ",
  I: "Ï",
  J: "Ĵ",
  K: "Ķ",
  L: "Ŀ",
  M: "Ṁ",
  N: "Ñ",
  O: "Ö",
  P: "Þ",
  Q: "Ǫ",
  R: "Ŕ",
  S: "Š",
  T: "Ţ",
  U: "Û",
  V: "Ṽ",
  W: "Ŵ",
  X: "Ẋ",
  Y: "Ý",
  Z: "Ž",
};

export const PSEUDO_LOCALE = "en-XA";
const OPEN = "⟦";
const CLOSE = "⟧";

/** Translates one English string into its pseudo-locale form. */
export function pseudoText(source: string): string {
  let body = "";
  let placeholder = "";
  for (const char of source) {
    if (placeholder) {
      // Inside a {placeholder}: copy verbatim until the closing brace.
      placeholder += char;
      if (char === "}") {
        body += placeholder;
        placeholder = "";
      }
      continue;
    }
    if (char === "{") {
      placeholder = char;
      continue;
    }
    body += MAP[char] ?? char;
  }
  // Unterminated brace: keep the text instead of dropping it.
  if (placeholder) body += placeholder;
  return OPEN + body + CLOSE;
}

/** Builds a full pseudo catalog from the English identity catalog. */
export function pseudoCatalog(catalog: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(catalog)) out[key] = pseudoText(value);
  return out;
}
