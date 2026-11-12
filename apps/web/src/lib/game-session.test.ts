import { describe, expect, it } from "vitest";
import {
  heroAction,
  nextForceClosePhase,
  sessionForGame,
  type ForceCloseEvent,
  type ForceClosePhase,
} from "@/lib/game-session";
import type { GameSession } from "@/lib/types";

const session = (gameId: number): GameSession => ({
  sessionId: "session",
  gameId,
  pid: 4242,
  startedAt: "2026-11-12T10:00:00Z",
  elapsedSeconds: 90,
  executable: "bin/game.exe",
  title: "Uncharted",
});

describe("heroAction", () => {
  it("offers Resume while the game is running", () => {
    expect(heroAction({ running: true, desktop: true, installed: true })).toBe("resume");
    expect(heroAction({ running: true, desktop: true, installed: false })).toBe("resume");
  });

  it("plays an installed game that is not running", () => {
    expect(heroAction({ running: false, desktop: true, installed: true })).toBe("play");
  });

  it("installs an uninstalled game on the desktop", () => {
    expect(heroAction({ running: false, desktop: true, installed: false })).toBe("install");
  });

  it("downloads in the browser build, where nothing runs locally", () => {
    expect(heroAction({ running: false, desktop: false, installed: false })).toBe("download");
    expect(heroAction({ running: true, desktop: false, installed: false })).toBe("download");
  });

  it("keeps the install job actions when nothing is running", () => {
    const base = { running: false, desktop: true, installed: false };
    expect(heroAction({ ...base, hasActiveJob: true, activeJobPhase: "paused" })).toBe("resumeInstall");
    expect(heroAction({ ...base, hasActiveJob: true, activeJobPhase: "downloading" })).toBe("installing");
    expect(heroAction({ ...base, hasResumableJob: true })).toBe("resumeInstall");
  });
});

describe("sessionForGame", () => {
  it("only keeps the session of the page's game", () => {
    expect(sessionForGame(session(7), 7)?.pid).toBe(4242);
    expect(sessionForGame(session(8), 7)).toBeNull();
    expect(sessionForGame(null, 7)).toBeNull();
  });
});

describe("nextForceClosePhase", () => {
  const step = (phase: ForceClosePhase, events: ForceCloseEvent[]) =>
    events.reduce(nextForceClosePhase, phase);

  it("asks before closing", () => {
    expect(step("idle", ["confirm"])).toBe("idle");
    expect(step("idle", ["ask"])).toBe("confirming");
    expect(step("confirming", ["confirm"])).toBe("closing");
  });

  it("cannot confirm twice or cancel while closing", () => {
    expect(step("closing", ["confirm"])).toBe("closing");
    expect(step("closing", ["ask"])).toBe("closing");
    expect(step("closing", ["cancel"])).toBe("closing");
  });

  it("cancels back to idle and settles after the command", () => {
    expect(step("confirming", ["cancel"])).toBe("idle");
    expect(step("closing", ["done"])).toBe("idle");
  });
});
