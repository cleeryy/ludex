#!/usr/bin/env node
/**
 * Pseudo-locale transform for the catalog check (Node side).
 *
 * This is the same transform as `apps/web/src/lib/pseudo.ts` (the runtime one
 * used by `?lang=en-XA`). Two copies exist because the check runs on plain
 * Node before Vite/TypeScript are involved; `src/lib/pseudo.test.ts` pins the
 * invariants both rely on (brackets, placeholders untouched, longer output).
 */

const MAP = {
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
export const PSEUDO_OPEN = "⟦";
export const PSEUDO_CLOSE = "⟧";

export function pseudoText(source) {
  let body = "";
  let placeholder = "";
  for (const char of source) {
    if (placeholder) {
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
  if (placeholder) body += placeholder;
  return PSEUDO_OPEN + body + PSEUDO_CLOSE;
}

export function pseudoCatalog(catalog) {
  const out = {};
  for (const [key, value] of Object.entries(catalog)) out[key] = pseudoText(value);
  return out;
}
