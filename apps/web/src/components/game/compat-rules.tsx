import { useQuery } from "@tanstack/react-query";
import { isNative, native } from "@/lib/native";
import { qk } from "@/lib/query-keys";
import { t } from "@/lib/i18n";
import { cn } from "@/lib/cn";
import type { CompatEvaluatedRule } from "@/lib/types";
import { SectionHeader } from "@/components/page";
import { Badge, Skeleton } from "@/components/ui/controls";
import {
  changeDetail,
  changeFieldLabel,
  confidenceLabel,
  hasBlockingIssue,
  issueSeverityLabel,
  prerequisiteLabel,
  ruleSourceLabel,
  skippedLabel,
} from "@/lib/compat-rules";

/**
 * "Why these launch settings" (#301).
 *
 * Read-only on purpose: the compatibility rules database is data shipped with
 * the app (or downloaded, signed), and a local override file is the only way to
 * edit it. This panel answers "why is the game configured like this?" with the
 * rule's own reason, its source badge and every field it did — or did not —
 * set. The user's launch options always win, and the panel says so.
 */
export function CompatRulesPanel({ gameId }: { gameId: number }) {
  const available = isNative();
  const report = useQuery({
    queryKey: qk.compatRules(gameId),
    queryFn: () => native.compatRulesReport(gameId),
    enabled: available,
    staleTime: 60_000,
    retry: false,
    refetchOnWindowFocus: false,
  });

  if (!available) return null;

  const data = report.data;
  const rules = data?.evaluation.rules ?? [];
  const blocking = hasBlockingIssue(data);
  const hasLocalRule = rules.some((rule) => rule.origin === "local");

  return (
    <div className="min-w-0" id="compat-rules">
      <SectionHeader
        level={3}
        title={t("Why these launch settings")}
        description={t(
          "The rules Ludex matched for this game. They fill in defaults only: anything you set under Launch options wins.",
        )}
        actions={
          data ? (
            <div className="flex flex-wrap items-center gap-1.5">
              {blocking && <Badge tone="danger">{t("Blocks launch")}</Badge>}
              <Badge tone="outline">{t("Rule {version}", { version: data.version })}</Badge>
              {hasLocalRule && <Badge tone="signal">{t("Learned locally")}</Badge>}
            </div>
          ) : undefined
        }
      />
      <div className="border-t border-line pt-4">
        {report.isLoading ? (
          <div className="space-y-2">
            <Skeleton className="h-3 w-2/3" />
            <Skeleton className="h-3 w-1/2" />
          </div>
        ) : report.isError ? (
          <p className="text-label text-ink-3">{t("The rules could not be read for this game.")}</p>
        ) : data ? (
          <div className="space-y-4">
            {data.degraded && (
              <p className="max-w-[80ch] text-meta leading-relaxed text-ink-3">
                {t("The rules file could not be read. Ludex uses the shipped defaults.")}
              </p>
            )}

            {rules.length === 0 ? (
              <p className="max-w-[80ch] text-label leading-relaxed text-ink-3">
                {t("No compatibility rule targets this game yet. Ludex uses your launch options as they are.")}
              </p>
            ) : (
              <ul className="space-y-4">
                {rules.map((rule) => (
                  <RuleRow key={rule.id} rule={rule} />
                ))}
              </ul>
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}

function RuleRow({ rule }: { rule: CompatEvaluatedRule }) {
  return (
    <li>
      <div className="flex flex-wrap items-center gap-1.5">
        <Badge tone={rule.origin === "local" ? "signal" : "outline"}>
          {ruleSourceLabel(rule)}
        </Badge>
        <Badge tone="default">{confidenceLabel(rule.confidence)}</Badge>
        {rule.applied && <Badge tone="ok">{t("Applied")}</Badge>}
      </div>
      <p className="mt-2 max-w-[80ch] text-label leading-relaxed text-ink-2">{rule.reason}</p>
      {rule.source && (
        <p className="mt-1 max-w-[80ch] break-words text-meta text-ink-3">
          {t("Source: {source}", { source: rule.source })}
        </p>
      )}

      {rule.changes.length > 0 && (
        <ul className="mt-2 space-y-1">
          {rule.changes.map((change) => {
            const skipped = skippedLabel(change.skipped);
            return (
              <li
                key={`${rule.id}-${change.field}-${change.value}`}
                className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-meta"
              >
                <span className="text-ink-3">{changeFieldLabel(change.field)}</span>
                <code className="break-all font-mono text-meta text-ink-2">
                  {changeDetail(change)}
                </code>
                {!change.applied && skipped && (
                  <span className="text-ink-3">· {skipped}</span>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {rule.knownIssues.length > 0 && (
        <div className="mt-2.5">
          <p className="text-meta font-medium text-ink-3">{t("Known issues")}</p>
          <ul className="mt-1 space-y-1.5">
            {rule.knownIssues.map((issue) => {
              const label = issueSeverityLabel(issue.severity);
              return (
                <li key={issue.code} className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                  {label && (
                    <Badge tone={issue.severity === "blocking" ? "danger" : "signal"}>
                      {label}
                    </Badge>
                  )}
                  <span
                    className={cn(
                      "max-w-[80ch] text-meta leading-relaxed",
                      issue.severity === "blocking" ? "text-ink-2" : "text-ink-3",
                    )}
                  >
                    {issue.detail}
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {rule.prerequisites.length > 0 && (
        <p className="mt-2 text-meta text-ink-3">
          {t("Prerequisites: {list}", {
            list: rule.prerequisites.map(prerequisiteLabel).join(", "),
          })}
        </p>
      )}
    </li>
  );
}
