import type { CommonFieldCandidate, CommonFieldMatch, CommonFieldsReport } from "./types";

/**
 * Pure helpers behind the "Common fields" panel (#148).
 *
 * Detection, normalisation and the write itself live in Rust; the UI only
 * decides what to render and which action is safe. An ambiguous match has no
 * file/key and must never be turned into a write without an explicit pin, so
 * that rule is kept here (and tested) instead of inside a component.
 */

/** A match that points at one concrete file/key and can be previewed. */
export type ResolvedCommonField = CommonFieldMatch & { file: string; key: string };

export function isResolvedField(match: CommonFieldMatch): match is ResolvedCommonField {
  return match.confidence !== "ambiguous" && Boolean(match.file && match.key);
}

/** Fields the panel can offer an apply action for. */
export function resolvableFields(report: CommonFieldsReport | undefined): ResolvedCommonField[] {
  return (report?.fields ?? []).filter(isResolvedField);
}

/**
 * Whether a game may opt into launch-time sync: at least one field resolves to
 * a file/key. An ambiguous-only report cannot be synced without a pin.
 */
export function canKeepInSync(report: CommonFieldsReport | undefined): boolean {
  return resolvableFields(report).length > 0;
}

/** What "apply Ludex language/name" would write, or null when unavailable. */
export function targetForField(
  field: CommonFieldMatch["field"],
  locale: string,
  displayName: string | null | undefined,
): { field: CommonFieldMatch["field"]; value: string } | null {
  if (field === "language") {
    const value = locale.trim();
    return value ? { field, value } : null;
  }
  const value = displayName?.trim() ?? "";
  return value ? { field, value } : null;
}

/** `file · key` label, used in buttons and dialogs. */
export function candidateLabel(candidate: Pick<CommonFieldCandidate, "file" | "key">): string {
  return `${candidate.file} · ${candidate.key}`;
}

/** Badge tone for a confidence value. */
export function confidenceTone(confidence: CommonFieldMatch["confidence"]): "ok" | "signal" | "danger" {
  if (confidence === "certain") return "ok";
  if (confidence === "likely") return "signal";
  return "danger";
}

/** Pinned mapping for a field, when this game has one. */
export function mappingFor(
  report: CommonFieldsReport | undefined,
  field: CommonFieldMatch["field"],
) {
  return report?.mappings.find((mapping) => mapping.field === field) ?? null;
}
