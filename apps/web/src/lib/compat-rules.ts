/**
 * Compatibility rules helpers (#301).
 *
 * The Rust command returns the matched rules with structured fields; this
 * module turns them into the sentences the "why" panel shows. Everything that
 * is copy lives behind `t()`, so a rule reason (data, authored in English) is
 * the only untranslated text on screen.
 */

import { t } from "@/lib/i18n";
import type {
  CompatConfidence,
  CompatEvaluatedRule,
  CompatIssueSeverity,
  CompatRuleOrigin,
  CompatRulesReport,
} from "@/lib/types";

/** Badge text for the layer a rule came from ("learned locally" wins). */
export function ruleSourceLabel(
  rule: Pick<CompatEvaluatedRule, "origin" | "rulesVersion">,
): string {
  return ruleOriginLabel(rule.origin, rule.rulesVersion);
}

export function ruleOriginLabel(origin: CompatRuleOrigin, version: string): string {
  if (origin === "local") return t("Learned locally");
  return t("Rule {version}", { version });
}

export function confidenceLabel(confidence: CompatConfidence): string {
  switch (confidence) {
    case "certain":
      return t("Certain");
    case "likely":
      return t("Likely");
    case "guess":
      return t("Guess");
  }
}

/** `null` for an informational issue: it needs no severity badge. */
export function issueSeverityLabel(severity: CompatIssueSeverity): string | null {
  switch (severity) {
    case "blocking":
      return t("Blocks launch");
    case "warning":
      return t("Warning");
    case "info":
      return null;
  }
}

/**
 * Human name for a plan field. Environment variables are named by their
 * purpose, never by `PROTON_*`, in line with the settings vocabulary (#283).
 */
export function changeFieldLabel(field: string): string {
  if (field.startsWith("env.")) return t("Environment");
  if (field.startsWith("display.")) return t("Gamescope profile");
  switch (field) {
    case "mode":
      return t("Launch mode");
    case "args":
      return t("Extra arguments");
    case "winetricks":
      return t("Windows components");
    case "steamAppId":
      return t("Steam app id");
    default:
      return field;
  }
}

/** The technical half of a change, shown in a monospace span. */
export function changeDetail(change: { field: string; value: string }): string {
  if (change.field.startsWith("env.")) {
    return `${change.field.slice("env.".length)} = ${change.value}`;
  }
  return change.value;
}

/**
 * Why a matched rule did not set a field. `null` when the change was applied.
 */
export function skippedLabel(reason: string | null): string | null {
  switch (reason) {
    case "user-choice":
      return t("Your choice kept");
    case "rule-conflict":
      return t("Another rule set this first");
    case "duplicate":
      return t("Already set");
    default:
      return null;
  }
}

export function prerequisiteLabel(id: string): string {
  switch (id) {
    case "vcrun":
      return t("Visual C++");
    case "dotnet":
      return t(".NET");
    case "directx":
      return t("DirectX");
    case "physx":
      return t("PhysX");
    default:
      return id;
  }
}

/** Rules that changed the plan or declared a prerequisite. */
export function appliedRules(rules: readonly CompatEvaluatedRule[]): CompatEvaluatedRule[] {
  return rules.filter((rule) => rule.applied);
}

/** True when the panel must say that this game cannot start as configured. */
export function hasBlockingIssue(report: CompatRulesReport | undefined): boolean {
  return Boolean(report?.evaluation.issues.some((issue) => issue.severity === "blocking"));
}

/** Total number of fields the rules actually set. */
export function appliedChangeCount(rules: readonly CompatEvaluatedRule[]): number {
  return rules.reduce(
    (total, rule) => total + rule.changes.filter((change) => change.applied).length,
    0,
  );
}
