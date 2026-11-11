/**
 * Save-location discovery types (#146).
 *
 * Candidates come from the desktop app's versioned OS/engine registry. They
 * are local only: the path never leaves the machine, and a candidate is never
 * archived (let alone uploaded) until the user confirms it.
 */

export type SaveCandidateSource =
  | "override"
  | "store"
  | "known-folder"
  | "engine"
  | "manifest"
  | "manual";

export type SaveCandidateConfidence = "certain" | "likely" | "guess";

export interface SaveCandidate {
  /** Canonical host path. Local only; never sent to the server. */
  path: string;
  kind: "directory" | "file";
  /** Known-folder id such as `saved_games`, `documents` or `proton_prefix`. */
  rootId: string;
  subpath: string;
  source: SaveCandidateSource;
  confidence: SaveCandidateConfidence;
  evidence: string;
  sizeBytes: number;
  lastModified: string | null;
  firstSeen: string | null;
  lastVerified: string | null;
  confirmed: boolean;
  ignored: boolean;
}

export interface SaveCandidateGroup {
  gameId: number;
  title: string;
  candidates: SaveCandidate[];
}

export interface SaveCandidateOverview {
  registryVersion: number;
  maxDepth: number;
  maxEntries: number;
  groups: SaveCandidateGroup[];
  warnings: string[];
}

/** A candidate still needs an explicit confirmation before it can be archived. */
export function candidateNeedsConfirmation(candidate: SaveCandidate): boolean {
  return !candidate.confirmed && !candidate.ignored;
}

/** Guesses are styled differently and are never enabled automatically. */
export function isGuess(candidate: SaveCandidate): boolean {
  return candidate.confidence === "guess";
}

/** Badge tone for a confidence: certain = ok, likely = signal, guess = outline. */
export function confidenceTone(
  confidence: SaveCandidateConfidence,
): "ok" | "signal" | "outline" {
  if (confidence === "certain") return "ok";
  if (confidence === "likely") return "signal";
  return "outline";
}

/** English message id for a provenance tag, translated by the caller. */
export function sourceLabel(source: SaveCandidateSource): string {
  switch (source) {
    case "override":
      return "User override";
    case "store":
      return "Store ground truth";
    case "known-folder":
      return "Known folder";
    case "engine":
      return "Engine default";
    case "manifest":
      return "Save database";
    case "manual":
      return "Added manually";
  }
}

/** English message id for a confidence, translated by the caller. */
export function confidenceLabel(confidence: SaveCandidateConfidence): string {
  switch (confidence) {
    case "certain":
      return "Certain";
    case "likely":
      return "Likely";
    case "guess":
      return "Guess";
  }
}

/** How many candidates the panel would show across every game. */
export function candidateCount(overview: SaveCandidateOverview | null | undefined): number {
  return overview?.groups.reduce((total, group) => total + group.candidates.length, 0) ?? 0;
}

/**
 * Merge one game's refreshed group into the overview so the panel does not
 * need a full reload after a confirm or a dismiss.
 */
export function mergeGroup(
  overview: SaveCandidateOverview,
  group: SaveCandidateGroup,
): SaveCandidateOverview {
  const groups = overview.groups.filter((entry) => entry.gameId !== group.gameId);
  if (group.candidates.length > 0) groups.push(group);
  groups.sort((left, right) => left.title.localeCompare(right.title));
  return { ...overview, groups };
}
