// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import {
  clearGameNote,
  listNotedGameIds,
  loadGameNote,
  MAX_GAME_NOTE_LENGTH,
  saveGameNote,
} from "./game-notes";

beforeEach(() => {
  window.localStorage.clear();
});

describe("game notes storage", () => {
  it("saves, loads, lists, and clears per-game notes", () => {
    expect(loadGameNote(42)).toBeNull();

    const saved = saveGameNote(42, "Use shield build for phase 2", new Date("2026-11-24T12:00:00Z"));
    expect(saved?.body).toBe("Use shield build for phase 2");
    expect(saved?.updatedAt).toBe("2026-11-24T12:00:00.000Z");
    expect(loadGameNote(42)?.body).toBe("Use shield build for phase 2");
    expect(listNotedGameIds()).toEqual([42]);

    clearGameNote(42);
    expect(loadGameNote(42)).toBeNull();
    expect(listNotedGameIds()).toEqual([]);
  });

  it("removes whitespace-only saves and clamps overly long notes", () => {
    saveGameNote(7, "Initial note");
    expect(saveGameNote(7, "   \n  ")).toBeNull();
    expect(loadGameNote(7)).toBeNull();

    const oversized = "x".repeat(MAX_GAME_NOTE_LENGTH + 250);
    const saved = saveGameNote(9, oversized);
    expect(saved?.body).toHaveLength(MAX_GAME_NOTE_LENGTH);
  });
});
