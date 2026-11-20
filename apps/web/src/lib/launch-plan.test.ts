import { describe, expect, it } from "vitest";
import { formatExitCode, planFailureNumbers, planIsApplied } from "@/lib/launch-plan";
import type { LaunchPlan, LaunchPlanFailure } from "@/lib/types";

const failure: LaunchPlanFailure = {
  kind: "no-first-frame",
  exitCode: 53,
  seconds: 2,
  budgetSecs: 5,
  hint: "the game or its Steam emulator aborted before the first frame",
};

const plan = {
  source: "auto",
} as LaunchPlan;

describe("launch plan helpers", () => {
  it("formats an exit code the way the launch log prints it", () => {
    expect(formatExitCode(53)).toBe("0x00000035");
    expect(formatExitCode(0xc0000409)).toBe("0xC0000409");
  });

  it("keeps the budget and the observed time apart", () => {
    expect(planFailureNumbers(failure)).toEqual({
      budgetSecs: 5,
      seconds: 2,
      code: "0x00000035",
    });
  });

  it("only applies an automatic plan", () => {
    expect(planIsApplied(plan)).toBe(true);
    expect(planIsApplied({ ...plan, source: "manual" })).toBe(false);
  });
});
