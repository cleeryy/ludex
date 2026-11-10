/**
 * Client-side fuzzy matching for the command palette and the mock adapter.
 *
 * The server owns library search (`GET /api/games?q=`); this module mirrors the
 * same normalisation rules so palette filtering, suggestions in demo mode and
 * highlighted substrings behave consistently. Keep the fold table in sync with
 * `crates/ludex-server/src/routes/search.rs`.
 */

const FOLD: Record<string, string> = {
  à: "a", á: "a", â: "a", ã: "a", ä: "a", å: "a", ā: "a", ă: "a", ą: "a",
  ç: "c", ć: "c", č: "c",
  è: "e", é: "e", ê: "e", ë: "e", ē: "e", ė: "e", ę: "e",
  ì: "i", í: "i", î: "i", ï: "i", ī: "i", į: "i",
  ñ: "n", ń: "n", ň: "n",
  ò: "o", ó: "o", ô: "o", õ: "o", ö: "o", ø: "o", ō: "o", ő: "o",
  ù: "u", ú: "u", û: "u", ü: "u", ū: "u", ů: "u", ű: "u",
  ý: "y", ÿ: "y",
  ß: "s", æ: "e", œ: "e", š: "s", ž: "z", ł: "l", đ: "d", þ: "t",
};

/** Lowercases, folds diacritics and collapses punctuation into spaces. */
export function normalizeSearch(input: string): string {
  let out = "";
  let pendingSpace = false;
  for (const character of input.toLowerCase()) {
    const folded = FOLD[character] ?? character;
    if (/[\p{L}\p{N}]/u.test(folded)) {
      if (pendingSpace && out) out += " ";
      pendingSpace = false;
      out += folded;
    } else {
      pendingSpace = true;
    }
  }
  return out;
}

function levenshtein(a: string, b: string): number {
  if (!a.length) return b.length;
  if (!b.length) return a.length;
  let previous = Array.from({ length: b.length + 1 }, (_, i) => i);
  let current = new Array<number>(b.length + 1).fill(0);
  for (let i = 0; i < a.length; i += 1) {
    current[0] = i + 1;
    for (let j = 0; j < b.length; j += 1) {
      const cost = a[i] === b[j] ? 0 : 1;
      current[j + 1] = Math.min(previous[j] + cost, previous[j + 1] + 1, current[j] + 1);
    }
    [previous, current] = [current, previous];
  }
  return previous[b.length];
}

function trigrams(value: string): Set<string> {
  const padded = ` ${value} `;
  const set = new Set<string>();
  if (padded.length < 3) {
    set.add(padded);
    return set;
  }
  for (let i = 0; i < padded.length - 2; i += 1) set.add(padded.slice(i, i + 3));
  return set;
}

function trigramSimilarity(a: string, b: string): number {
  const left = trigrams(a);
  const right = trigrams(b);
  let shared = 0;
  for (const gram of left) if (right.has(gram)) shared += 1;
  return (2 * shared) / (left.size + right.size);
}

function bestWindowRatio(query: string, candidate: string): number {
  const words = candidate.split(" ").filter(Boolean);
  let best = 0;
  for (let size = 1; size <= 2; size += 1) {
    for (let start = 0; start < words.length; start += 1) {
      const window = words.slice(start, start + size).join(" ");
      if (!window) continue;
      const longest = Math.max(query.length, window.length);
      if (!longest) continue;
      best = Math.max(best, 1 - levenshtein(query, window) / longest);
    }
  }
  return best;
}

/** Scores one candidate against an already-normalised query (0 when no match). */
export function scoreNormalized(query: string, candidate: string): number {
  if (!query || !candidate) return 0;
  if (query === candidate) return 1000;
  if (candidate.startsWith(query)) return 920 - Math.min(60, candidate.length - query.length);

  const position = candidate.indexOf(query);
  if (position >= 0) {
    const atBoundary = position === 0 || candidate[position - 1] === " ";
    return (atBoundary ? 870 : 820) - Math.min(40, position) * 0.5 - Math.min(60, candidate.length - query.length) * 0.2;
  }

  const queryTokens = query.split(" ");
  if (queryTokens.length > 1) {
    const words = candidate.split(" ");
    if (queryTokens.every((token) => words.some((word) => word.startsWith(token)))) {
      return 780 - Math.min(60, candidate.length - query.length) * 0.2;
    }
  }

  const dice = trigramSimilarity(query, candidate);
  if (dice >= 0.4) return 420 + dice * 380;

  const longest = Math.max(query.length, candidate.length);
  const ratio = 1 - levenshtein(query, candidate) / longest;
  if (ratio >= 0.6) return 240 + ratio * 420;

  if (query.length >= 3) {
    const best = bestWindowRatio(query, candidate);
    if (best >= 0.72) return 300 + best * 320;
  }

  return 0;
}

/** Scores free text against a title and optional aliases. */
export function scoreSearch(query: string, title: string, aliases: string[] = []): number {
  const normalized = normalizeSearch(query);
  if (!normalized) return 0;
  return Math.max(
    scoreNormalized(normalized, normalizeSearch(title)),
    ...aliases.map((alias) => scoreNormalized(normalized, normalizeSearch(alias))),
  );
}

/**
 * Returns the `[start, end)` range of the query inside `text`, ignoring case,
 * accents and punctuation, or null when the query is a typo rather than a
 * substring. Used to wrap the matched run in `<mark>`.
 */
export function highlightRange(text: string, query: string): [number, number] | null {
  const normalizedQuery = normalizeSearch(query);
  if (!normalizedQuery) return null;

  // Map each normalised character back to its index in the original string.
  const map: number[] = [];
  let normalized = "";
  let pendingSpace = false;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index].toLowerCase();
    const folded = FOLD[character] ?? character;
    if (/[\p{L}\p{N}]/u.test(folded)) {
      if (pendingSpace && normalized) {
        normalized += " ";
        map.push(index);
      }
      pendingSpace = false;
      normalized += folded;
      map.push(index);
    } else {
      pendingSpace = true;
    }
  }

  const position = normalized.indexOf(normalizedQuery);
  if (position < 0) return null;
  const start = map[position];
  const end = map[position + normalizedQuery.length - 1] + 1;
  return [start, end];
}
