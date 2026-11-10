import { t } from "@/lib/i18n";
import type { GameSummary, SmartField, SmartOperator, SmartRule, SmartRules } from "@/lib/types";

/** Fields a smart collection rule may target. */
export function smartFields(): { value: SmartField; label: string }[] {
  return [
    { value: "genre", label: t("Genre") },
    { value: "year", label: t("Release year") },
    { value: "players", label: t("Players") },
    { value: "playtime", label: t("Playtime (minutes)") },
    { value: "rating", label: t("Rating") },
    { value: "installed", label: t("Available on server") },
    { value: "provider", label: t("Metadata provider") },
    { value: "title", label: t("Title") },
  ];
}

const OPERATORS: Record<SmartField, { value: SmartOperator; label: string }[]> = {
  genre: [
    { value: "is", label: "is" },
    { value: "contains", label: "contains" },
  ],
  title: [
    { value: "contains", label: "contains" },
    { value: "is", label: "is" },
  ],
  year: [
    { value: "is", label: "is" },
    { value: "gte", label: "is at least" },
    { value: "lte", label: "is at most" },
    { value: "between", label: "is between" },
  ],
  players: [
    { value: "gte", label: "is at least" },
    { value: "is", label: "is" },
    { value: "lte", label: "is at most" },
  ],
  playtime: [
    { value: "gte", label: "is at least" },
    { value: "lte", label: "is at most" },
    { value: "between", label: "is between" },
  ],
  rating: [
    { value: "gte", label: "is at least" },
    { value: "lte", label: "is at most" },
    { value: "between", label: "is between" },
  ],
  installed: [{ value: "is", label: "is" }],
  provider: [{ value: "is", label: "is" }],
};

function operatorLabel(label: string): string {
  switch (label) {
    case "is":
      return t("is");
    case "contains":
      return t("contains");
    case "is at least":
      return t("is at least");
    case "is at most":
      return t("is at most");
    case "is between":
      return t("is between");
    default:
      return label;
  }
}

export function operatorsFor(field: SmartField) {
  return OPERATORS[field].map((operator) => ({ ...operator, label: operatorLabel(operator.label) }));
}

export function defaultOperator(field: SmartField): SmartOperator {
  return OPERATORS[field][0].value;
}

export function defaultRule(field: SmartField): SmartRule {
  switch (field) {
    case "installed":
      return { field, op: "is", value: true };
    case "year":
    case "players":
    case "playtime":
    case "rating":
      return { field, op: defaultOperator(field), value: field === "rating" ? 80 : field === "players" ? 2 : 2020 };
    default:
      return { field, op: defaultOperator(field), value: "" };
  }
}

/** Human summary of one rule, used on collection pages. */
export function ruleLabel(rule: SmartRule): string {
  const field = smartFields().find((entry) => entry.value === rule.field)?.label ?? rule.field;
  const op = operatorsFor(rule.field).find((entry) => entry.value === (rule.op ?? defaultOperator(rule.field)))?.label ?? t("is");
  if (rule.field === "installed") {
    return rule.value ? t("{field} is available", { field }) : t("{field} is not available", { field });
  }
  if (rule.op === "between") {
    return t("{field} {operator} {from} and {to}", { field, operator: op, from: String(rule.value), to: rule.value2 ?? "?" });
  }
  return t("{field} {operator} {value}", { field, operator: op, value: String(rule.value) });
}

export function rulesSummary(rules: SmartRules): string {
  if (!rules.rules.length) return t("No rules yet");
  const joiner = rules.match === "any" ? t(" or ") : t(" and ");
  return rules.rules.map(ruleLabel).join(joiner);
}

type RuleTarget = Pick<
  GameSummary,
  "title" | "genres" | "releaseDate" | "rating" | "maxPlayers" | "minutesPlayed" | "versionCount"
> & { provider?: { slug: string } | null };

function normalize(value: string): string {
  return value
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}

function numberValue(value: unknown): number | null {
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (typeof value === "string" && value.trim() !== "") {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

function compare(actual: number, op: string | undefined, value: number, value2: number | null): boolean {
  switch (op) {
    case "gte":
      return actual >= value;
    case "lte":
      return actual <= value;
    case "between":
      return value2 !== null && actual >= Math.min(value, value2) && actual <= Math.max(value, value2);
    default:
      return actual === value;
  }
}

/** Mirrors the server evaluator for demo mode. */
export function evaluateRules(rules: SmartRules, game: RuleTarget): boolean {
  if (!rules.rules.length) return false;
  const results = rules.rules.map((rule) => {
    switch (rule.field) {
      case "genre": {
        const wanted = String(rule.value ?? "");
        if (!wanted) return false;
        return game.genres.some((genre) =>
          rule.op === "contains" ? normalize(genre).includes(normalize(wanted)) : genre.toLowerCase() === wanted.toLowerCase(),
        );
      }
      case "title": {
        const wanted = String(rule.value ?? "");
        if (!wanted) return false;
        return rule.op === "is"
          ? normalize(game.title) === normalize(wanted)
          : normalize(game.title).includes(normalize(wanted));
      }
      case "provider":
        return (game.provider?.slug ?? "").toLowerCase() === String(rule.value ?? "").toLowerCase();
      case "installed":
        return (game.versionCount > 0) === Boolean(rule.value);
      case "year": {
        const year = game.releaseDate ? Number(game.releaseDate.slice(0, 4)) : null;
        const value = numberValue(rule.value);
        return year !== null && value !== null && compare(year, rule.op, value, numberValue(rule.value2));
      }
      case "players": {
        const value = numberValue(rule.value);
        return game.maxPlayers !== null && value !== null && compare(game.maxPlayers, rule.op, value, numberValue(rule.value2));
      }
      case "playtime": {
        const value = numberValue(rule.value);
        return value !== null && compare(game.minutesPlayed, rule.op, value, numberValue(rule.value2));
      }
      case "rating": {
        const value = numberValue(rule.value);
        return game.rating !== null && value !== null && compare(game.rating, rule.op, value, numberValue(rule.value2));
      }
      default:
        return false;
    }
  });
  return rules.match === "any" ? results.some(Boolean) : results.every(Boolean);
}
