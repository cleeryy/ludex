import { describe, expect, it } from "vitest";
import type { GameDetail, MetadataCandidate } from "./types";
import {
  candidateKey,
  confidencePercent,
  fieldLabel,
  filterByYear,
  previewChanges,
  preservedFields,
} from "./metadata-match";

function game(overrides: Partial<GameDetail> = {}): GameDetail {
  return {
    id: 7,
    title: "PEAK",
    sortTitle: "PEAK",
    releaseDate: "2019-03-01T00:00:00Z",
    coverUrl: "/api/media/cover.jpg",
    backgroundUrl: null,
    rating: 12,
    genres: ["Adventure"],
    earlyAccess: false,
    versionCount: 1,
    totalSize: 1024,
    state: "unplayed",
    minutesPlayed: 0,
    bookmarked: false,
    lastPlayedAt: null,
    maxPlayers: 2,
    kind: "game",
    platform: null,
    emulatorId: null,
    description: "Wrong automatic match.",
    screenshots: [],
    trailers: [],
    videos: [],
    developers: ["Wrong Studio"],
    publishers: ["Wrong Studio"],
    tags: [],
    ageRating: null,
    provider: { slug: "fixture", id: "peak-wrong", url: null },
    altTitles: [],
    versions: [],
    launch: {
      executable: "PEAK.exe",
      parameters: null,
      installerExecutable: null,
      installerParameters: null,
      uninstallerExecutable: null,
      uninstallerParameters: null,
    },
    ...overrides,
  };
}

function candidate(overrides: Partial<MetadataCandidate> = {}): MetadataCandidate {
  return {
    provider: "igdb",
    providerName: "IGDB",
    providerId: "3527290",
    providerUrl: "https://www.igdb.com/games/peak",
    title: "PEAK",
    year: 2025,
    coverUrl: "https://images.example/peak.jpg",
    summary: "Climb with friends.",
    developers: ["Aggro Crab"],
    platforms: ["PC (Microsoft Windows)"],
    score: 1060,
    confidence: 0.91,
    selected: false,
    ...overrides,
  };
}

describe("previewChanges", () => {
  it("lists only the fields that would change", () => {
    const rows = previewChanges(game(), candidate(), []);
    expect(rows.map((row) => row.field)).toEqual([
      "releaseDate",
      "description",
      "developers",
    ]);
    const year = rows.find((row) => row.field === "releaseDate");
    expect(year).toMatchObject({ current: "2019", next: "2025", manual: false });
    expect(rows.every((row) => !row.manual)).toBe(true);
  });

  it("is empty when the record already matches the game", () => {
    const rows = previewChanges(
      game({ releaseDate: "2025-06-16T00:00:00Z", description: "Climb with friends.", developers: ["Aggro Crab"] }),
      candidate(),
      [],
    );
    expect(rows).toEqual([]);
  });

  it("marks hand-edited fields as kept", () => {
    const rows = previewChanges(game(), candidate(), ["description"]);
    const description = rows.find((row) => row.field === "description");
    expect(description?.manual).toBe(true);
    expect(preservedFields(["description", "tags"], false)).toEqual(["description"]);
    expect(preservedFields(["description"], true)).toEqual([]);
  });

  it("ignores an empty provider summary instead of previewing a wipe", () => {
    const rows = previewChanges(game(), candidate({ summary: null }), []);
    expect(rows.some((row) => row.field === "description")).toBe(false);
  });
});

describe("year filter", () => {
  it("keeps candidates without a year and drops other years", () => {
    const candidates = [
      candidate({ providerId: "a", year: 2019 }),
      candidate({ providerId: "b", year: 2025 }),
      candidate({ providerId: "c", year: null }),
    ];
    expect(filterByYear(candidates, 2025).map((entry) => entry.providerId)).toEqual(["b", "c"]);
    expect(filterByYear(candidates, null)).toHaveLength(3);
  });
});

describe("candidate metadata", () => {
  it("builds stable keys and readable values", () => {
    expect(candidateKey(candidate())).toBe("igdb:3527290");
    expect(confidencePercent(candidate({ confidence: 0.912 }))).toBe(91);
    expect(confidencePercent(candidate({ confidence: 4 }))).toBe(100);
    expect(fieldLabel("releaseDate")).toBe("Release year");
  });
});
