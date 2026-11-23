import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Download, RefreshCw, X } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";
import { planFailureNumbers } from "@/lib/launch-plan";
import { Badge, Button, Progress, Skeleton } from "@/components/ui/controls";
import { SectionHeader } from "@/components/page";
import { useToast } from "@/state/toasts";
import type { LaunchPlan, LaunchPlanProgress } from "@/lib/types";

function runtimeLabel(runtime: LaunchPlan["runtime"]): string {
  switch (runtime) {
    case "umu":
      return t("UMU");
    case "native":
      return t("Native");
    case "wine":
      return t("Wine");
    case "launcher-uri":
      return t("Launcher");
  }
}

/**
 * Per-game launch auto-configuration (#297).
 *
 * The plan is a consequence of the game and the machine, not a setting: the
 * rules engine picks the runtime, the Proton build, the environment, the
 * prerequisite recipe and the wrappers, and this panel states the decision in
 * one sentence. Manual launch options stay available below as the advanced
 * path; when they are saved the plan is informational only.
 */
export function LaunchPlanPanel({ gameId }: { gameId: number }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const nativeAvailable = isNative();
  const [progress, setProgress] = useState<LaunchPlanProgress | null>(null);
  const [busy, setBusy] = useState<"refresh" | "export" | null>(null);

  const planQuery = useQuery({
    queryKey: qk.launchPlan(gameId),
    queryFn: () => native.launchPlanGet(gameId),
    enabled: nativeAvailable,
    staleTime: 30_000,
    retry: false,
    refetchOnWindowFocus: false,
  });

  useEffect(() => {
    if (!nativeAvailable) return;
    let disposed = false;
    const listeners: Array<() => void> = [];
    void native
      .onLaunchPlanProgress((update) => {
        if (disposed || update.gameId !== gameId) return;
        setProgress(update);
        if (update.phase !== "prerequisites") {
          void queryClient.invalidateQueries({ queryKey: qk.launchPlan(gameId) });
        }
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else listeners.push(unlisten);
      });
    void native
      .onLaunchPlanFailed((payload) => {
        if (disposed || payload.gameId !== gameId) return;
        push({
          title: t("The last automatic launch failed"),
          description: t(
            "Ludex handed the session to the auto-revert path instead of retrying the same plan.",
          ),
          tone: "danger",
        });
        void queryClient.invalidateQueries({ queryKey: qk.launchPlan(gameId) });
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else listeners.push(unlisten);
      });
    return () => {
      disposed = true;
      for (const unlisten of listeners) unlisten();
    };
  }, [nativeAvailable, gameId, push, queryClient]);

  const view = planQuery.data ?? null;
  const plan = view?.plan ?? null;
  const running = view?.running || progress?.phase === "prerequisites";
  const lastFailure = view?.lastFailure ?? null;
  const failureNumbers = lastFailure ? planFailureNumbers(lastFailure) : null;
  const percent = useMemo(() => {
    if (!progress || progress.total === 0) return null;
    return Math.min(100, Math.max(0, (progress.index / progress.total) * 100));
  }, [progress]);

  const refresh = async () => {
    setBusy("refresh");
    try {
      await native.launchPlanRefresh(gameId);
      await queryClient.invalidateQueries({ queryKey: qk.launchPlan(gameId) });
    } catch (cause) {
      push({ title: t("Could not refresh the launch plan"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const exportPlan = async () => {
    setBusy("export");
    try {
      const json = await native.launchPlanExport(gameId);
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `ludex-launch-plan-${gameId}.json`;
      anchor.click();
      URL.revokeObjectURL(url);
      push({ title: t("Launch plan exported"), description: t("The JSON file can be shared with support."), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not export the launch plan"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const cancelPrerequisites = async () => {
    try {
      await native.launchPlanCancel(gameId);
    } catch (cause) {
      push({ title: t("Could not cancel the prerequisites"), description: errorMessage(cause), tone: "danger" });
    }
  };

  if (!nativeAvailable) return null;

  return (
    <div className="min-w-0" id="launch-plan">
      <SectionHeader
        level={3}
        title={t("Launch plan")}
        description={t("Chosen from the game, the machine and the rules. No compatibility setting to pick.")}
        actions={
          <>
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw className={busy === "refresh" || planQuery.isFetching ? "size-3.5 animate-spin" : "size-3.5"} />}
              onClick={() => void refresh()}
              disabled={running || busy !== null}
            >
              {t("Recalculate")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              icon={<Download className="size-3.5" />}
              onClick={() => void exportPlan()}
              disabled={!plan || busy !== null}
            >
              {t("Export")}
            </Button>
          </>
        }
      />
      <div className="border-t border-line pt-4">
        {planQuery.isLoading ? (
          <div className="space-y-2">
            <Skeleton className="h-5 w-2/3" />
            <Skeleton className="h-4 w-1/3" />
          </div>
        ) : planQuery.isError ? (
          <div className="flex items-start gap-2 text-body text-danger">
            <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <p>{t("Could not read the launch plan")}: {errorMessage(planQuery.error)}</p>
          </div>
        ) : plan && view ? (
          <div className="space-y-4">
            <p className="max-w-[80ch] text-body leading-relaxed text-ink">{plan.sentence}</p>

            <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-meta text-ink-3">
              <Badge tone="outline">{runtimeLabel(plan.runtime)}</Badge>
              {plan.source === "manual" ? (
                <Badge tone="signal">{t("Manual override")}</Badge>
              ) : (
                <Badge tone="ok">{t("Automatic")}</Badge>
              )}
              {view.stale && <span>{t("The machine changed since this plan was stored; recalculate before playing.")}</span>}
              <span className="font-mono text-micro">{plan.rules.join(" · ")}</span>
            </div>

            {plan.source === "manual" && (
              <p className="text-meta text-ink-3">
                {t("The saved launch options win; the rules are informational.")}
              </p>
            )}

            {plan.prerequisites.length > 0 && (
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-label text-ink-2">
                <span className="text-ink-3">{t("Prerequisites")}:</span>
                {plan.prerequisites.map((prerequisite) => (
                  <span key={prerequisite.id} className="border border-line-2 px-1.5 py-0.5 text-meta">
                    {prerequisite.label}
                  </span>
                ))}
              </div>
            )}

            {running && (
              <div className="space-y-2">
                <Progress value={percent} label={t("Prerequisites")} />
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <p className="text-label text-ink-3">
                    {progress?.message ?? t("Applying the prerequisite recipe…")}
                  </p>
                  <Button variant="ghost" size="sm" icon={<X className="size-3.5" />} onClick={() => void cancelPrerequisites()}>
                    {t("Cancel")}
                  </Button>
                </div>
              </div>
            )}

            {plan.notes.length > 0 && (
              <ul className="space-y-1.5">
                {plan.notes.map((note) => (
                  <li key={note} className="flex items-start gap-2 text-meta text-ink-3">
                    <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-signal-text" aria-hidden="true" />
                    <span>{note}</span>
                  </li>
                ))}
              </ul>
            )}

            {lastFailure && failureNumbers && (
              <div className="border-l-2 border-danger/60 pl-3">
                <p className="text-label font-medium text-danger">{t("The last automatic launch failed")}</p>
                <p className="mt-0.5 text-meta text-ink-3">{lastFailure.hint}</p>
                <p className="mt-1 font-mono text-micro text-ink-3">
                  {t("Budget")}: {failureNumbers.budgetSecs}s · {t("Exited after")}: {failureNumbers.seconds}s ·{" "}
                  {failureNumbers.code}
                </p>
              </div>
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}
