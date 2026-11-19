import { describe, expect, it } from "vitest";
import { keepAwakeLabels, keepAwakeSentence } from "@/lib/keep-awake";
import type { InhibitStatus } from "@/lib/types";

function status(patch: Partial<InhibitStatus> = {}): InhibitStatus {
  return {
    supported: true,
    active: true,
    holding: true,
    jobs: 0,
    count: 0,
    reasons: [],
    graceMinutes: 5,
    ...patch,
  };
}

describe("keepAwakeSentence", () => {
  it("names the download count the way the issue asks", () => {
    const jobs = (count: number) =>
      status({
        jobs: count,
        count,
        reasons: [{ kind: "install", label: "Hades II" }],
      });
    expect(keepAwakeSentence(jobs(1))).toBe(
      "Keeping the machine awake while downloading (1 job)",
    );
    expect(keepAwakeSentence(jobs(3))).toBe(
      "Keeping the machine awake while downloading (3 jobs)",
    );
  });

  it("names a game, a stream or a verification when no install runs", () => {
    expect(
      keepAwakeSentence(status({ count: 1, reasons: [{ kind: "game", label: "Celeste" }] })),
    ).toBe("Keeping the machine awake while a game is running");
    expect(
      keepAwakeSentence(status({ count: 1, reasons: [{ kind: "stream", label: "Hades II" }] })),
    ).toBe("Keeping the machine awake during a stream");
    expect(
      keepAwakeSentence(status({ count: 1, reasons: [{ kind: "verify", label: "Hades II" }] })),
    ).toBe("Keeping the machine awake while checking the install");
  });

  it("stays hidden while nothing runs and during the grace window", () => {
    expect(keepAwakeSentence(null)).toBeNull();
    expect(keepAwakeSentence(status({ active: false, holding: true }))).toBeNull();
  });

  it("lists the reasons for the diagnostics row", () => {
    expect(
      keepAwakeLabels(
        status({
          count: 2,
          reasons: [
            { kind: "install", label: "Hades II" },
            { kind: "game", label: "Celeste" },
          ],
        }),
      ),
    ).toBe("Hades II · Celeste");
    expect(keepAwakeLabels(null)).toBe("");
  });
});
