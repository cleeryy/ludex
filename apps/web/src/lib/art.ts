import paletteData from "@/theme/art-palettes.json";

/**
 * Generated cover art.
 *
 * No third-party artwork ships with Ludex: every demo cover is a "plate" in a
 * single series, drawn from a palette and a motif. Real games without a cached
 * cover fall back to the same system, seeded from the title, so the library
 * never shows a broken image.
 */

export type PaletteKey =
  | "oxblood"
  | "petrol"
  | "ochre"
  | "indigo"
  | "moss"
  | "clay"
  | "ice"
  | "bone"
  | "graphite"
  | "dusk"
  | "rust"
  | "jade";

export type MotifKey =
  | "grid"
  | "halftone"
  | "chevron"
  | "arcs"
  | "horizon"
  | "cross"
  | "split"
  | "type"
  | "topo"
  | "waves"
  | "blocks"
  | "orbit";

export interface Palette {
  base: string;
  field: string;
  accent: string;
  paper: string;
}

export const PALETTES = paletteData as Record<PaletteKey, Palette>;

export interface ArtSpec {
  palette: PaletteKey;
  motif: MotifKey;
  /** first letter used by the "type" motif */
  letter?: string;
}

/** Curated assignments for the demo library, keyed by slug. */
export const ART_BY_SLUG: Record<string, ArtSpec> = {
  "blue-prince": { palette: "indigo", motif: "grid", letter: "B" },
  bodycam: { palette: "graphite", motif: "halftone", letter: "B" },
  "call-of-duty-black-ops-iii": { palette: "rust", motif: "chevron", letter: "C" },
  "cat-quest-ii": { palette: "petrol", motif: "arcs", letter: "C" },
  "forza-horizon-6": { palette: "clay", motif: "horizon", letter: "F" },
  "hitman-3": { palette: "ice", motif: "cross", letter: "H" },
  "it-takes-two": { palette: "clay", motif: "split", letter: "I" },
  "little-nightmares": { palette: "bone", motif: "type", letter: "L" },
  "little-nightmares-ii": { palette: "moss", motif: "topo", letter: "L" },
  "little-nightmares-iii": { palette: "jade", motif: "waves", letter: "L" },
  "marvels-spider-man-2": { palette: "oxblood", motif: "blocks", letter: "S" },
  "minecraft-dungeons": { palette: "moss", motif: "grid", letter: "M" },
  "minecraft-dungeons-ii": { palette: "dusk", motif: "blocks", letter: "M" },
  "outer-wilds": { palette: "indigo", motif: "orbit", letter: "O" },
  "rv-there-yet": { palette: "ochre", motif: "horizon", letter: "R" },
  "sackboy-a-big-adventure": { palette: "ochre", motif: "arcs", letter: "S" },
  "uncharted-legacy-of-thieves-collection": { palette: "rust", motif: "topo", letter: "U" },
  "over-the-hill": { palette: "dusk", motif: "split", letter: "O" },
};

export const PALETTE_KEYS = Object.keys(PALETTES) as PaletteKey[];
export const MOTIF_KEYS: MotifKey[] = [
  "grid",
  "halftone",
  "chevron",
  "arcs",
  "horizon",
  "cross",
  "split",
  "type",
  "topo",
  "waves",
  "blocks",
  "orbit",
];

export function slugify(input: string): string {
  return input
    .toLowerCase()
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/['’]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function hashString(input: string): number {
  let h = 2166136261;
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

export function mulberry32(seed: number): () => number {
  let a = seed;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Deterministic art for any title, used for real games without a cached cover. */
export function artForTitle(title: string): ArtSpec {
  const slug = slugify(title);
  const known = ART_BY_SLUG[slug];
  if (known) return known;
  const h = hashString(slug);
  return {
    palette: PALETTE_KEYS[h % PALETTE_KEYS.length],
    motif: MOTIF_KEYS[(h >>> 5) % MOTIF_KEYS.length],
    letter: title.trim().charAt(0).toUpperCase() || "L",
  };
}

export function artFromUrl(url: string | null, title: string): ArtSpec {
  if (url && url.startsWith("demo://")) {
    const slug = url.replace("demo://", "").split("/")[1] ?? "";
    const spec = ART_BY_SLUG[slug];
    if (spec) return spec;
    return artForTitle(slug.replace(/-/g, " ") || title);
  }
  return artForTitle(title);
}

export function demoCover(slug: string): string {
  return `demo://cover/${slug}`;
}

export function demoBackdrop(slug: string): string {
  return `demo://backdrop/${slug}`;
}

export function demoStill(slug: string, index: number): string {
  return `demo://still/${slug}/${index}`;
}
