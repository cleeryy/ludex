import { describe, expect, it } from "vitest";
import {
  candidateCount,
  candidateNeedsConfirmation,
  confidenceLabel,
  confidenceTone,
  isGuess,
  mergeGroup,
  sourceLabel,
  type SaveCandidate,
  type SaveCandidateOverview,
} from "@/lib/save-discovery";

const candidate = (patch: Partial<SaveCandidate> = {}): SaveCandidate => ({
  path: "/home/mara/Saved Games/Hades",
  kind: "directory",
  rootId: "saved_games",
  subpath: "Hades",
  source: "engine",
  confidence: "likely",
  evidence: "Game title folders: {saved_games}/{product}",
  sizeBytes: 0,
  lastModified: null,
  firstSeen: "2026-11-11T00:00:00Z",
  lastVerified: "2026-11-11T00:00:00Z",
  confirmed: false,
  ignored: false,
  ...patch,
});

describe("save discovery helpers", () => {
  it("marks guesses without enabling them", () => {
    const guess = candidate({ confidence: "guess", evidence: "Unity default layout" });
    expect(isGuess(guess)).toBe(true);
    expect(candidateNeedsConfirmation(guess)).toBe(true);
    expect(confidenceTone(guess.confidence)).toBe("outline");
    expect(confidenceLabel(guess.confidence)).toBe("Guess");
  });

  it("keeps certain and likely candidates visible but unconfirmed", () => {
    const likely = candidate();
    expect(isGuess(likely)).toBe(false);
    expect(confidenceTone("certain")).toBe("ok");
    expect(confidenceTone("likely")).toBe("signal");
    expect(candidateNeedsConfirmation(likely)).toBe(true);
    expect(candidateNeedsConfirmation({ ...likely, confirmed: true })).toBe(false);
    expect(candidateNeedsConfirmation({ ...likely, ignored: true })).toBe(false);
  });

  it("labels every provenance tag", () => {
    expect(sourceLabel("override")).toBe("User override");
    expect(sourceLabel("store")).toBe("Store ground truth");
    expect(sourceLabel("known-folder")).toBe("Known folder");
    expect(sourceLabel("engine")).toBe("Engine default");
    expect(sourceLabel("manifest")).toBe("Save database");
    expect(sourceLabel("manual")).toBe("Added manually");
  });

  it("counts and merges per-game groups", () => {
    const overview: SaveCandidateOverview = {
      registryVersion: 1,
      maxDepth: 4,
      maxEntries: 2000,
      warnings: [],
      groups: [
        { gameId: 1, title: "Hades", candidates: [candidate()] },
        { gameId: 2, title: "Celeste", candidates: [candidate({ path: "/x" })] },
      ],
    };
    expect(candidateCount(overview)).toBe(2);
    expect(candidateCount(null)).toBe(0);

    const merged = mergeGroup(overview, {
      gameId: 1,
      title: "Hades",
      candidates: [candidate({ path: "/y" })],
    });
    expect(merged.groups).toHaveLength(2);
    expect(merged.groups.find((group) => group.gameId === 1)?.candidates[0].path).toBe("/y");

    const emptied = mergeGroup(overview, { gameId: 2, title: "Celeste", candidates: [] });
    expect(emptied.groups.map((group) => group.gameId)).toEqual([1]);
  });
});
