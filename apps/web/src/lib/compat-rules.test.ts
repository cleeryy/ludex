import { describe, expect, it } from "vitest";
import {
  appliedChangeCount,
  appliedRules,
  changeDetail,
  changeFieldLabel,
  confidenceLabel,
  hasBlockingIssue,
  issueSeverityLabel,
  prerequisiteLabel,
  ruleOriginLabel,
  ruleSourceLabel,
  skippedLabel,
} from "./compat-rules";
import type { CompatEvaluatedRule, CompatRulesReport } from "@/lib/types";

function rule(overrides: Partial<CompatEvaluatedRule> = {}): CompatEvaluatedRule {
  return {
    id: "test-rule",
    origin: "shipped",
    rulesVersion: "2026.11",
    reason: "Because.",
    confidence: "certain",
    source: "test",
    applied: true,
    changes: [{ field: "mode", value: "native", applied: true, skipped: null }],
    prerequisites: [],
    knownIssues: [],
    ...overrides,
  };
}

function report(overrides: Partial<CompatRulesReport> = {}): CompatRulesReport {
  return {
    gameId: 1,
    version: "2026.11",
    origin: "shipped",
    localRuleCount: 0,
    degraded: false,
    notes: [],
    evaluation: {
      options: {} as CompatRulesReport["evaluation"]["options"],
      extraArgs: [],
      rules: [rule()],
      prerequisites: [],
      issues: [],
    },
    ...overrides,
  };
}

describe("compatibility rule labels", () => {
  it("names the layer, not a private id", () => {
    expect(ruleSourceLabel(rule())).toBe("Rule 2026.11");
    expect(ruleSourceLabel(rule({ origin: "cached", rulesVersion: "2026.12" }))).toBe(
      "Rule 2026.12",
    );
    expect(ruleSourceLabel(rule({ origin: "local", rulesVersion: "" }))).toBe("Learned locally");
    expect(ruleOriginLabel("local", "ignored")).toBe("Learned locally");
  });

  it("translates confidence and severity", () => {
    expect(confidenceLabel("certain")).toBe("Certain");
    expect(confidenceLabel("likely")).toBe("Likely");
    expect(confidenceLabel("guess")).toBe("Guess");
    expect(issueSeverityLabel("blocking")).toBe("Blocks launch");
    expect(issueSeverityLabel("warning")).toBe("Warning");
    expect(issueSeverityLabel("info")).toBeNull();
  });

  it("keeps environment variables out of the primary vocabulary", () => {
    expect(changeFieldLabel("env.PROTON_ENABLE_NVAPI")).toBe("Environment");
    expect(changeFieldLabel("display.hdr")).toBe("Gamescope profile");
    expect(changeFieldLabel("mode")).toBe("Launch mode");
    expect(changeFieldLabel("args")).toBe("Extra arguments");
    expect(changeFieldLabel("winetricks")).toBe("Windows components");
    expect(changeFieldLabel("steamAppId")).toBe("Steam app id");
    expect(changeFieldLabel("futureField")).toBe("futureField");
    expect(changeDetail({ field: "env.DXVK_HUD", value: "0" })).toBe("DXVK_HUD = 0");
    expect(changeDetail({ field: "mode", value: "native" })).toBe("native");
  });

  it("explains why a change was kept", () => {
    expect(skippedLabel("user-choice")).toBe("Your choice kept");
    expect(skippedLabel("rule-conflict")).toBe("Another rule set this first");
    expect(skippedLabel("duplicate")).toBe("Already set");
    expect(skippedLabel(null)).toBeNull();
    expect(skippedLabel("future")).toBeNull();
  });

  it("labels known prerequisites and leaves unknown ids alone", () => {
    expect(prerequisiteLabel("vcrun")).toBe("Visual C++");
    expect(prerequisiteLabel("dotnet")).toBe(".NET");
    expect(prerequisiteLabel("directx")).toBe("DirectX");
    expect(prerequisiteLabel("physx")).toBe("PhysX");
    expect(prerequisiteLabel("metal")).toBe("metal");
  });

  it("counts applied rules and fields", () => {
    const rules = [
      rule(),
      rule({
        id: "kept",
        applied: false,
        changes: [{ field: "mode", value: "umu", applied: false, skipped: "user-choice" }],
      }),
    ];
    expect(appliedRules(rules).map((entry) => entry.id)).toEqual(["test-rule"]);
    expect(appliedChangeCount(rules)).toBe(1);
  });

  it("flags a blocking issue for the header badge", () => {
    expect(hasBlockingIssue(undefined)).toBe(false);
    expect(hasBlockingIssue(report())).toBe(false);
    const blocking = report();
    blocking.evaluation.issues = [
      { ruleId: "test-rule", code: "arm64", severity: "blocking", detail: "No." },
    ];
    expect(hasBlockingIssue(blocking)).toBe(true);
  });
});
