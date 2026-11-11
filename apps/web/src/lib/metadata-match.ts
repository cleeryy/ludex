import type { GameDetail, MetadataCandidate } from "@/lib/types";

/**
 * Pure helpers behind the manual metadata match dialog (#135): what applying a
 * candidate would change, which fields are kept because they were edited by
 * hand, and the client-side year filter.
 *
 * Labels are message ids, translated by the component.
 */

/** Fields `apply` can overwrite, in preview order. */
export const MATCH_FIELDS = [
  "title",
  "releaseDate",
  "description",
  "genres",
  "developers",
  "publishers",
  "rating",
  "ageRating",
  "altTitles",
  "players",
] as const;

export type MatchField = (typeof MATCH_FIELDS)[number];

const FIELD_LABELS: Record<MatchField, string> = {
  title: "Title",
  releaseDate: "Release year",
  description: "Description",
  genres: "Genres",
  developers: "Developers",
  publishers: "Publishers",
  rating: "Rating",
  ageRating: "Age rating",
  altTitles: "Alternative titles",
  players: "Players",
};

/** Message id of a field label. */
export function fieldLabel(field: MatchField): string {
  return FIELD_LABELS[field];
}

export interface MetadataPreviewRow {
  field: MatchField;
  /** Message id of the field label. */
  label: string;
  current: string;
  next: string;
  /** The field was edited by hand and is kept unless `replaceManual`. */
  manual: boolean;
}

/** Stable key for one candidate. */
export function candidateKey(candidate: MetadataCandidate): string {
  return `${candidate.provider}:${candidate.providerId}`;
}

function year(value: string | null): number | null {
  const match = /^(\d{4})/.exec(value ?? "");
  return match ? Number(match[1]) : null;
}

function join(values: string[] | null | undefined): string {
  return (values ?? []).filter(Boolean).join(", ");
}

/**
 * Rows the dialog shows for `candidate`. Only differences are listed; the
 * candidate response carries title, year, summary and developers, so the
 * remaining fields are summarised once in the dialog instead of guessed here.
 */
export function previewChanges(
  game: GameDetail,
  candidate: MetadataCandidate,
  manualFields: string[],
): MetadataPreviewRow[] {
  const manual = new Set(manualFields);
  const rows: MetadataPreviewRow[] = [];
  const push = (field: MatchField, current: string, next: string) => {
    if (!next.trim() || current.trim() === next.trim()) return;
    rows.push({
      field,
      label: FIELD_LABELS[field],
      current: current.trim(),
      next: next.trim(),
      manual: manual.has(field),
    });
  };

  push("title", game.title, candidate.title);
  const currentYear = year(game.releaseDate);
  const nextYear = candidate.year === null ? "" : String(candidate.year);
  if (nextYear && nextYear !== String(currentYear ?? "")) {
    push("releaseDate", currentYear === null ? "" : String(currentYear), nextYear);
  }
  push("description", game.description ?? "", candidate.summary ?? "");
  push("developers", join(game.developers), join(candidate.developers));
  return rows;
}

/** Manual fields that `apply` will keep (empty when `replaceManual`). */
export function preservedFields(manualFields: string[], replaceManual: boolean): MatchField[] {
  if (replaceManual) return [];
  return MATCH_FIELDS.filter((field) => manualFields.includes(field));
}

/**
 * Applies the dialog's year filter. Candidates without a year stay visible: a
 * missing year must never hide the record the editor is looking for.
 */
export function filterByYear(
  candidates: MetadataCandidate[],
  selectedYear: number | null,
): MetadataCandidate[] {
  if (selectedYear === null) return candidates;
  return candidates.filter((candidate) => candidate.year === null || candidate.year === selectedYear);
}

/** `score` shown as a whole percentage. */
export function confidencePercent(candidate: MetadataCandidate): number {
  return Math.round(Math.max(0, Math.min(1, candidate.confidence)) * 100);
}
