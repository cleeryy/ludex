import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Check,
  Circle,
  Download,
  ExternalLink,
  Info,
  Loader2,
  Play,
  RefreshCw,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatDate, formatPercent } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import { cn } from "@/lib/cn";
import type {
  ManagedRuntime,
  RuntimeAuditEvent,
  RuntimeIssue,
  RuntimeProgress,
  RuntimeStep,
  RuntimeStepStatus,
  RuntimeApplyReport,
} from "@/lib/types";
import {
  applyableSteps,
  importStep,
  manualSteps,
  planProgress,
  primaryAction,
  progressPercent,
  stepStatusTone,
  totalDownloadBytes,
} from "@/lib/make-it-run";
import { useToast } from "@/state/toasts";
import { Badge, Button, Checkbox, Progress, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { t } from "@/lib/i18n";

/**
 * "Make it run" (#178).
 *
 * The backend detects what the machine lacks, returns a numbered plan with the
 * exact download, size, checksum and licence, and only runs after the consent
 * click. This panel never invents a URL and never offers a silent install.
 */

function stepTitle(step: RuntimeStep): string {
  switch (step.id) {
    case "linux-umu":
      return t("Install UMU Launcher");
    case "linux-proton":
      return t("Install the pinned Proton build");
    case "linux-prefix":
      return t("Create the per-game Wine prefix");
    case "linux-prefix-reset":
      return t("Reset the Wine prefix path");
    case "linux-configure":
      return t("Use the managed runtime for this game");
    case "linux-arch":
      return t("Use an x86_64 Linux machine");
    case "windows-vcrun-2022-x64":
    case "windows-vcrun-2022-x86":
      return t("Install the Visual C++ runtime");
    case "windows-dotnet":
      return t("Install .NET Desktop Runtime 10");
    case "windows-directx":
      return t("Install the DirectX End-User Runtime");
    case "windows-physx":
      return t("Install PhysX from NVIDIA");
    case "macos-rosetta":
      return t("Install Rosetta 2");
    case "macos-whisky":
      return t("Install Whisky, a signed Wine 11 wrapper");
    case "macos-heroic":
      return t("Install Heroic, which manages its own Wine builds");
    case "macos-crossover":
      return t("Install CrossOver");
    case "macos-sikarugir":
      return t("Install Sikarugir");
    case "macos-gptk":
      return t("Import a Game Porting Toolkit DMG");
    default:
      return step.title;
  }
}

function stepDetail(step: RuntimeStep): string {
  switch (step.id) {
    case "linux-umu":
      return t("Downloads the pinned UMU zipapp and verifies its SHA-256 before use.");
    case "linux-proton":
      return t("Downloads GE-Proton, verifies its SHA-256, and keeps it in the Ludex runtime folder.");
    case "linux-prefix":
      return t("Ludex creates the folder and records which runtime created it.");
    case "linux-prefix-reset":
      return t("The current prefix path would touch the install folder or escape it. Reset it to the per-game default.");
    case "linux-configure":
      return t("Sets the launch mode to UMU, clears a stale tool id and keeps the per-game prefix.");
    case "linux-arch":
      return t("Ludex cannot install an x86_64 Proton runtime on this architecture.");
    case "windows-vcrun-2022-x64":
    case "windows-vcrun-2022-x86":
      return t("Runs Microsoft's signed redistributable with /install /quiet /norestart after verification.");
    case "windows-dotnet":
      return t("Runs Microsoft's signed installer with /install /quiet /norestart after verification.");
    case "windows-directx":
      return t("Extracts Microsoft's redistributable, then runs DXSETUP.exe /silent. It is idempotent.");
    case "windows-physx":
      return t("Only NVIDIA distributes PhysX; Ludex never bundles it.");
    case "macos-rosetta":
      return t("Run the displayed command in Terminal; macOS asks for your password.");
    case "macos-whisky":
    case "macos-heroic":
    case "macos-sikarugir":
      return t("Ludex opens the official page; the download and licence are yours to accept.");
    case "macos-crossover":
      return t("CrossOver 26 is the last release that supports Intel Macs.");
    case "macos-gptk":
      return t("Verifies the file's size and SHA-256, shows the evaluation licence, and records its path. Nothing is copied.");
    default:
      return step.detail;
  }
}

function issueTitle(issue: RuntimeIssue): string {
  switch (issue.code) {
    case "native-game":
      return t("Native game");
    case "missing-umu":
      return t("UMU Launcher is missing");
    case "missing-proton":
      return t("No Proton build is installed");
    case "prefix-unsafe":
      return t("The Wine prefix path is unsafe");
    case "prefix-missing":
      return t("The per-game prefix does not exist yet");
    case "no-vulkan":
      return t("No Vulkan ICD found");
    case "launch-mode":
      return t("This game does not use UMU yet");
    case "unsupported-arch":
      return t("Unsupported CPU architecture");
    case "disk-space":
      return t("Not enough free disk space");
    case "missing-vcrun":
      return t("The Visual C++ runtime is missing");
    case "missing-dotnet":
      return t(".NET Desktop Runtime is missing");
    case "missing-directx":
      return t("The DirectX runtime is missing");
    case "missing-physx":
      return t("PhysX is missing");
    case "admin-required":
      return t("Administrator rights are needed");
    case "windows-arm":
      return t("Windows on Arm");
    case "no-wrapper":
      return t("No macOS compatibility wrapper found");
    case "rosetta-missing":
      return t("Rosetta 2 is not installed");
    case "macos-too-old":
      return t("macOS 15 or newer is required by Whisky");
    case "intel-end-of-life":
      return t("Intel Mac support is ending");
    case "gptk-optional":
      return t("Game Porting Toolkit is optional");
    default:
      return issue.title;
  }
}

function issueDetail(issue: RuntimeIssue): string {
  switch (issue.code) {
    case "native-game":
      return t("This game is not a Windows executable, so no compatibility runtime is needed.");
    case "missing-umu":
      return t("UMU runs the game through the Steam Linux Runtime and the pinned Proton build.");
    case "missing-proton":
      return t("Ludex pins GE-Proton so every launch uses the same tested build.");
    case "prefix-unsafe":
      return t("A prefix must be a dedicated folder so resetting it never deletes the game.");
    case "prefix-missing":
      return t("Ludex creates it and records which runtime created it.");
    case "no-vulkan":
      return t("Install your GPU driver or a Vulkan ICD; Ludex does not install drivers.");
    case "launch-mode":
      return t("Ludex can point this game at UMU without changing other games.");
    case "unsupported-arch":
      return t("Proton builds are x86_64 only. Use an x86_64 Linux host or the game's native build.");
    case "disk-space":
      return t("Free some space on the runtime volume, then re-scan.");
    case "missing-vcrun":
      return t("Most Windows games need the Microsoft Visual C++ 2015–2022 redistributable.");
    case "missing-dotnet":
      return t("The game ships a .NET build; the desktop runtime is required before it can start.");
    case "missing-directx":
      return t("The game folder contains a June 2010 DirectX redistributable.");
    case "missing-physx":
      return t("The game declares PhysX. Ludex links to NVIDIA instead of redistributing it.");
    case "admin-required":
      return t("Machine-wide redistributables need elevation. Ludex downloads and verifies the signed installer, then shows the exact command to run.");
    case "windows-arm":
      return t("x64 redistributables run under Windows' emulation layer; a game that needs a native Arm build is out of scope.");
    case "no-wrapper":
      return t("Install Whisky, Heroic or CrossOver, or select a wrapper you already have. Ludex never bundles them.");
    case "rosetta-missing":
      return t("Intel-only Windows code needs Rosetta 2 on Apple Silicon. macOS can also prompt for it on first use.");
    case "macos-too-old":
      return t("On an older macOS, use Heroic or CrossOver instead.");
    case "intel-end-of-life":
      return t("macOS 26/27/28 drop Intel hardware. Keep that risk in mind before buying a licence.");
    case "gptk-optional":
      return t("Download the DMG from Apple with your own Apple ID, then import it. Ludex verifies and records it, and never copies or bundles it.");
    default:
      return issue.detail;
  }
}

function noticeText(notice: string): string {
  switch (notice) {
    case "Kernel-level anti-cheat (EAC/BattlEye without developer Proton support) does not run under Wine or Proton and can put accounts at risk. Ludex never bypasses anti-cheat.":
      return t("Kernel-level anti-cheat (EAC/BattlEye without developer Proton support) does not run under Wine or Proton and can put accounts at risk. Ludex never bypasses anti-cheat.");
    case "Ludex never bundles or redistributes GPTK, CrossOver, Steam, games or GPU drivers.":
      return t("Ludex never bundles or redistributes GPTK, CrossOver, Steam, games or GPU drivers.");
    case "No runtime is installed without an explicit click, and every checksum mismatch aborts before anything runs.":
      return t("No runtime is installed without an explicit click, and every checksum mismatch aborts before anything runs.");
    default:
      return notice;
  }
}

function auditLabel(event: RuntimeAuditEvent): string {
  switch (event.action) {
    case "install":
      return t("Installed");
    case "configure":
      return t("Configured");
    case "import":
      return t("Imported");
    case "remove":
      return t("Removed");
    case "select":
      return t("Selected");
    case "cancel":
      return t("Cancelled");
    case "needs-elevation":
      return t("Needs administrator");
    case "prepare-prefix":
      return t("Prepared the prefix");
    default:
      return event.action;
  }
}

function StatusIcon({ status }: { status: RuntimeStepStatus }) {
  const tone = stepStatusTone(status);
  if (status === "done") return <Check className="size-4 text-ok" aria-hidden="true" />;
  if (status === "failed") return <X className="size-4 text-danger" aria-hidden="true" />;
  if (status === "running") return <Loader2 className="size-4 animate-spin text-signal" aria-hidden="true" />;
  if (tone === "warning") return <AlertTriangle className="size-4 text-warn" aria-hidden="true" />;
  return <Circle className="size-3 text-ink-3" aria-hidden="true" />;
}

function statusLabel(status: RuntimeStepStatus): string | null {
  switch (status) {
    case "done":
      return t("Done");
    case "failed":
      return t("Failed");
    case "blocked":
      return t("Blocked");
    case "running":
      return t("Running");
    case "needsElevation":
      return t("Needs administrator");
    default:
      return null;
  }
}

export function MakeItRunPanel({
  gameId,
}: {
  gameId: number;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const nativeAvailable = isNative();
  const [consentOpen, setConsentOpen] = useState(false);
  const [accepted, setAccepted] = useState(false);
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);
  const [report, setReport] = useState<RuntimeApplyReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [removeTarget, setRemoveTarget] = useState<ManagedRuntime | null>(null);

  const diagnosis = useQuery({
    queryKey: ["make-it-run", gameId],
    queryFn: () => native.makeItRunDiagnose(gameId),
    enabled: nativeAvailable,
    staleTime: 15_000,
    retry: false,
    refetchOnWindowFocus: false,
  });

  useEffect(() => {
    if (!nativeAvailable) return;
    let disposed = false;
    let off: (() => void) | undefined;
    void native.onRuntimeProgress((update) => {
      if (disposed || update.gameId !== gameId) return;
      setProgress(update);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else off = unlisten;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, [nativeAvailable, gameId]);

  const data = diagnosis.data ?? report?.diagnosis ?? null;
  const removeForgets = removeTarget !== null && (removeTarget.imported || removeTarget.path === "");
  const refresh = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["make-it-run", gameId] }),
      queryClient.invalidateQueries({ queryKey: ["compat-scan"] }),
      queryClient.invalidateQueries({ queryKey: ["launch-options", gameId] }),
      queryClient.invalidateQueries({ queryKey: ["prefix-info", gameId] }),
    ]);
  };

  const applyable = useMemo(() => applyableSteps(data), [data]);
  const manual = useMemo(() => manualSteps(data), [data]);
  const target = useMemo(() => importStep(data), [data]);
  const action = primaryAction(data);
  const progressPct = progressPercent(progress);
  const steps = data?.steps ?? [];
  const plan = planProgress(steps);
  const reportStep = (stepId: string) => report?.steps.find((entry) => entry.stepId === stepId);
  const liveStatus = (step: RuntimeStep): RuntimeStepStatus => {
    if (running && progress?.stepId === step.id) return "running";
    return reportStep(step.id)?.status ?? step.status;
  };

  const apply = async () => {
    if (!data || applyable.length === 0) return;
    setRunning(true);
    setProgress(null);
    setReport(null);
    try {
      const result = await native.makeItRunApply(
        gameId,
        applyable.map((step) => step.id),
        true,
      );
      setReport(result);
      await refresh();
      if (result.needsElevation) {
        push({
          title: t("Administrator rights are required"),
          description: t("The installer was downloaded and verified. The exact command is shown in the plan."),
          tone: "default",
        });
      } else if (result.cancelled) {
        push({ title: t("Make it run cancelled"), description: t("Nothing else was installed."), tone: "default" });
      } else if (result.failed) {
        push({
          title: t("The plan did not finish"),
          description: result.steps.find((step) => step.status === "failed")?.message ?? "",
          tone: "danger",
        });
      } else {
        push({
          title: result.diagnosis.ready ? t("Runtime ready") : t("Step finished"),
          description: result.diagnosis.ready
            ? t("Test launch is the source of truth: start the game once to confirm.")
            : t("Re-scan to see what remains."),
          tone: result.diagnosis.ready ? "success" : "default",
        });
      }
    } catch (cause) {
      push({ title: t("Could not apply the plan"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setRunning(false);
      setProgress(null);
    }
  };

  const cancel = async () => {
    try {
      await native.makeItRunCancel(gameId);
    } catch (cause) {
      push({ title: t("Could not cancel"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const launch = async () => {
    try {
      const result = await native.launchGame({ gameId });
      push({
        title: t("Test launch started"),
        description: result.tool
          ? t("Ludex used {tool}.", { tool: result.tool })
          : t("Ludex started the game without a compatibility layer."),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Test launch failed"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const importToolkit = async (step: RuntimeStep) => {
    if (!step.providerId) return;
    setBusy(step.id);
    try {
      const path = await native.makeItRunPickImport(gameId);
      if (!path) return;
      const result = await native.makeItRunImport(gameId, step.providerId, path);
      setReport(null);
      await refresh();
      push({
        title: t("Game Porting Toolkit imported"),
        description: t("{size} verified and recorded. Nothing was copied.", {
          size: formatBytes(result.sizeBytes),
        }),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not import the toolkit"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    if (!removeTarget) return;
    setBusy(removeTarget.providerId);
    try {
      const result = await native.makeItRunRemove(gameId, removeTarget.providerId);
      setRemoveTarget(null);
      setReport(null);
      await refresh();
      push({
        title: result.forgotImport
          ? t("Record forgotten")
          : t("{size} freed", { size: formatBytes(result.freedBytes) }),
        description: result.forgotImport
          ? t("Ludex removed its record. No user file was deleted.")
          : t("Only files Ludex created were deleted. The game and its saves are untouched."),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not remove the runtime"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const copyCommand = async (command: string) => {
    try {
      await navigator.clipboard.writeText(command);
      push({ title: t("Command copied"), tone: "success" });
    } catch {
      push({ title: t("Could not copy the command"), tone: "danger" });
    }
  };

  const selectRuntime = async (providerId: string, name: string) => {
    setBusy(providerId);
    try {
      await native.makeItRunSelect(gameId, providerId);
      await refresh();
      push({
        title: t("Runtime selected"),
        description: t("This game now records {name} as its runtime.", { name }),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not select the runtime"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const togglePrerequisite = async (id: string) => {
    if (!data) return;
    const next = data.prerequisites.includes(id)
      ? data.prerequisites.filter((value) => value !== id)
      : [...data.prerequisites, id];
    setBusy(`prereq-${id}`);
    try {
      await native.setRuntimePrerequisites(gameId, next);
      await refresh();
    } catch (cause) {
      push({ title: t("Could not save the prerequisites"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  if (!nativeAvailable) return null;

  const description = !data
    ? t("Detect and install the compatibility runtime this game needs.")
    : data.ready
      ? t("Everything this game needs is installed. Start it once to confirm.")
      : t("{done} of {total} steps done. Nothing is installed without your click.", {
          done: String(plan.done),
          total: String(plan.total),
        });

  return (
    <section className="mt-12 scroll-mt-6" id="make-it-run" aria-labelledby="make-it-run-heading">
      <SectionHeader
        title={t("Make it run")}
        description={description}
        id="make-it-run-heading"
        actions={
          <>
            {data?.ready && action === "launch" && (
              <Button variant="primary" size="sm" icon={<Play className="size-3.5" />} onClick={() => void launch()}>
                {t("Test launch")}
              </Button>
            )}
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw className={cn("size-3.5", diagnosis.isFetching && "animate-spin")} />}
              onClick={() => void refresh()}
              disabled={running}
            >
              {t("Re-scan")}
            </Button>
          </>
        }
      />
      <div className="border-t border-line pt-4">
        {diagnosis.isLoading ? (
          <div className="space-y-2">
            <Skeleton className="h-4 w-1/3" />
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        ) : diagnosis.isError ? (
          <div className="flex items-start gap-2 text-[13px] text-danger">
            <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <p>{errorMessage(diagnosis.error)}</p>
          </div>
        ) : data ? (
          <div className="space-y-5">
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 text-[11.5px] text-ink-3">
              <Badge tone="outline">
                {data.platform === "linux" ? "Linux" : data.platform === "windows" ? "Windows" : "macOS"}
              </Badge>
              <span className="font-mono">{data.arch}</span>
              {data.osVersion && <span>macOS {data.osVersion}</span>}
              {data.freeBytes !== null && <span>{t("Free disk: {size}", { size: formatBytes(data.freeBytes) })}</span>}
              <span>{data.elevated ? t("Administrator") : t("User session")}</span>
              {data.running && <span className="text-signal-text">{t("A task is running")}</span>}
            </div>

            {data.running && !running && (
              <div className="flex flex-wrap items-center gap-3">
                <p className="text-[12px] text-ink-3">{t("A Make it run task is already in progress.")}</p>
                <Button variant="ghost" size="sm" icon={<X className="size-3.5" />} onClick={() => void cancel()}>
                  {t("Cancel running task")}
                </Button>
              </div>
            )}

            {running && (
              <div className="space-y-2">
                <Progress value={progressPct} label={t("Runtime progress")} />
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <p className="text-[12px] text-ink-3">
                    {progress?.message ?? t("Starting…")}
                    {progressPct !== null && <span className="ms-2 font-mono">{formatPercent(progressPct)}</span>}
                  </p>
                  <Button variant="ghost" size="sm" icon={<X className="size-3.5" />} onClick={() => void cancel()}>
                    {t("Cancel")}
                  </Button>
                </div>
              </div>
            )}

            {data.issues.length > 0 && (
              <ul className="space-y-2">
                {data.issues.map((issue) => (
                  <li key={issue.code} className="flex items-start gap-2.5">
                    <span
                      className={cn(
                        "mt-1.5 size-1.5 shrink-0 rounded-full",
                        issue.severity === "blocking"
                          ? "bg-danger"
                          : issue.severity === "warning"
                            ? "bg-warn"
                            : "bg-ink-3",
                      )}
                      aria-hidden="true"
                    />
                    <div>
                      <p className="text-[13px] text-ink">{issueTitle(issue)}</p>
                      <p className="text-[12px] leading-relaxed text-ink-3">{issueDetail(issue)}</p>
                    </div>
                  </li>
                ))}
              </ul>
            )}

            {steps.length > 0 && (
              <ol className="divide-y divide-line border-y border-line">
                {steps.map((step, index) => {
                  const status = liveStatus(step);
                  const applied = reportStep(step.id);
                  return (
                    <li key={step.id} className="flex gap-3 py-3">
                      <span className="mt-0.5 grid size-6 shrink-0 place-items-center rounded-full border border-line text-[10.5px] font-medium text-ink-3">
                        {index + 1}
                      </span>
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                          <p className={cn("text-[13.5px]", status === "done" ? "text-ink-2" : "text-ink")}>
                            {stepTitle(step)}
                          </p>
                          {step.optional && <Badge>{t("Optional")}</Badge>}
                          {step.requiresAdmin && <Badge tone="outline">{t("Administrator")}</Badge>}
                          {step.download && (
                            <span className="font-mono text-[10.5px] text-ink-3">
                              {formatBytes(step.download.sizeBytes)}
                            </span>
                          )}
                          <span className="ms-auto flex items-center gap-1.5 text-[10.5px] uppercase tracking-[0.08em] text-ink-3">
                            <StatusIcon status={status} />
                            {statusLabel(status)}
                          </span>
                        </div>
                        <p className="mt-1 text-[12px] leading-relaxed text-ink-3">{stepDetail(step)}</p>

                        {step.download && (
                          <details className="mt-1.5 text-[11px] text-ink-3">
                            <summary className="cursor-pointer select-none">
                              {t("Source, licence and checksum")}
                            </summary>
                            <dl className="mt-1.5 grid gap-1 font-mono text-[10.5px]">
                              <div className="flex gap-2">
                                <dt className="w-16 shrink-0 text-ink-3">{t("Source")}</dt>
                                <dd className="break-all">{step.download.url}</dd>
                              </div>
                              <div className="flex gap-2">
                                <dt className="w-16 shrink-0 text-ink-3">{t("SHA-256")}</dt>
                                <dd className="break-all">{step.download.sha256}</dd>
                              </div>
                              <div className="flex gap-2">
                                <dt className="w-16 shrink-0 text-ink-3">{t("Licence")}</dt>
                                <dd>{step.download.licence}</dd>
                              </div>
                            </dl>
                          </details>
                        )}

                        {step.manual?.command && (
                          <div className="mt-2 flex flex-wrap items-center gap-2">
                            <code className="min-w-0 flex-1 overflow-x-auto rounded-[2px] border border-line bg-surface px-2 py-1.5 font-mono text-[11px] text-ink-2">
                              {step.manual.command}
                            </code>
                            <Button size="sm" variant="ghost" onClick={() => void copyCommand(step.manual!.command!)}>
                              {t("Copy command")}
                            </Button>
                          </div>
                        )}
                        {step.manual?.url && (
                          <Button
                            size="sm"
                            variant="ghost"
                            className="mt-1.5"
                            icon={<ExternalLink className="size-3.5" />}
                            onClick={() => void native.openExternalUrl(step.manual!.url!)}
                          >
                            {t("Open page")}
                          </Button>
                        )}
                        {step.action === "import" && (
                          <Button
                            size="sm"
                            variant="default"
                            className="mt-2"
                            loading={busy === step.id}
                            icon={<Download className="size-3.5" />}
                            onClick={() => void importToolkit(step)}
                          >
                            {t("Import GPTK DMG")}
                          </Button>
                        )}
                        {applied && applied.status === "needsElevation" && (
                          <p className="mt-2 rounded-[2px] border border-warn/40 bg-warn/10 px-2.5 py-2 font-mono text-[11px] leading-relaxed text-ink-2">
                            {applied.message}
                          </p>
                        )}
                        {applied && applied.status === "failed" && (
                          <p className="mt-2 text-[12px] text-danger">{applied.message}</p>
                        )}
                      </div>
                    </li>
                  );
                })}
              </ol>
            )}

            {action === "apply" && (
              <div className="flex flex-wrap items-center gap-3">
                <Button
                  variant="primary"
                  size="md"
                  icon={<ShieldCheck className="size-4" />}
                  disabled={running}
                  onClick={() => {
                    setAccepted(false);
                    setConsentOpen(true);
                  }}
                >
                  {t("Fix it")}
                </Button>
                <p className="text-[12px] text-ink-3">
                  {t("{count} step(s) will run after you accept the licences.", { count: String(applyable.length) })}
                </p>
              </div>
            )}

            {action === "launch" && (
              <div className="flex flex-wrap items-center gap-3">
                <Button
                  variant="primary"
                  size="md"
                  icon={<Play className="size-4" />}
                  onClick={() => void launch()}
                >
                  {t("Test launch")}
                </Button>
                <p className="text-[12px] text-ink-3">
                  {t("A test launch is the source of truth. Ludex records which runtime it used.")}
                </p>
              </div>
            )}

            {action === "rescan" && manual.length > 0 && (
              <p className="flex items-start gap-2 text-[12px] text-ink-3">
                <Info className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
                {t("Finish the manual steps above, then re-scan. Ludex detects what you installed.")}
              </p>
            )}

            {target && action !== "apply" && (
              <p className="text-[11.5px] text-ink-3">
                {t("Game Porting Toolkit is optional and never downloaded by Ludex.")}
              </p>
            )}

            {report && (
              <div className="rounded-[2px] border border-line bg-surface p-3">
                <p className="text-[12px] font-medium text-ink">{t("Last run")}</p>
                <ul className="mt-1.5 space-y-1">
                  {report.steps.map((step) => (
                    <li key={step.stepId} className="flex items-baseline justify-between gap-4 text-[11.5px]">
                      <span className="font-mono text-ink-3">{step.stepId}</span>
                      <span className={step.status === "failed" ? "text-danger" : "text-ink-2"}>
                        {statusLabel(step.status) ?? step.status}
                      </span>
                    </li>
                  ))}
                </ul>
                <p className="mt-2 break-all font-mono text-[10.5px] text-ink-3">{report.logPath}</p>
              </div>
            )}

            {data.platform === "windows" && (
              <details className="text-[12px]">
                <summary className="cursor-pointer select-none text-ink-2">
                  {t("Prerequisites this game declares")}
                </summary>
                <p className="mt-1.5 text-[11.5px] text-ink-3">
                  {t("Redistributables found in the game folder are always included.")}
                </p>
                <div className="mt-2 flex flex-wrap gap-x-5 gap-y-2">
                  {[
                    ["vcrun", t("Visual C++")],
                    ["dotnet", t(".NET")],
                    ["directx", t("DirectX")],
                    ["physx", t("PhysX")],
                  ].map(([id, label]) => (
                    <div key={id} className="flex items-center gap-2">
                      <Checkbox
                        checked={data.prerequisites.includes(id)}
                        onCheckedChange={() => void togglePrerequisite(id)}
                        label={label}
                      />
                      <button
                        type="button"
                        className="text-[12px] text-ink-2"
                        disabled={busy === `prereq-${id}`}
                        onClick={() => void togglePrerequisite(id)}
                      >
                        {label}
                      </button>
                    </div>
                  ))}
                </div>
              </details>
            )}

            {data.detections.some((detection) => detection.installed && detection.applicable) && (
              <details className="text-[12px]">
                <summary className="cursor-pointer select-none text-ink-2">
                  {t("Detected on this machine")}
                </summary>
                <ul className="mt-2 space-y-1.5">
                  {data.detections
                    .filter((detection) => detection.installed && detection.applicable)
                    .map((detection) => (
                      <li key={detection.providerId} className="flex flex-wrap items-center gap-x-2 gap-y-1">
                        <span className="text-ink-2">{detection.name}</span>
                        {detection.version && (
                          <span className="font-mono text-[10.5px] text-ink-3">{detection.version}</span>
                        )}
                        {data.platform === "macos" && detection.kind === "wrapper" && (
                            <Button
                              size="sm"
                              variant="quiet"
                              className="ms-auto"
                              loading={busy === detection.providerId}
                              onClick={() => void selectRuntime(detection.providerId, detection.name)}
                            >
                              {t("Use")}
                            </Button>
                          )}
                      </li>
                    ))}
                </ul>
              </details>
            )}

            {data.managed.length > 0 && (
              <details className="text-[12px]">
                <summary className="cursor-pointer select-none text-ink-2">
                  {t("Runtimes managed by Ludex")}
                </summary>
                <ul className="mt-2 space-y-2">
                  {data.managed.map((runtime) => (
                    <li key={runtime.providerId} className="flex flex-wrap items-center gap-x-3 gap-y-1">
                      <span className="text-ink-2">
                        {runtime.name} {runtime.version}
                      </span>
                      <span className="font-mono text-[10.5px] text-ink-3">
                        {runtime.imported
                          ? t("imported")
                          : runtime.path
                            ? formatBytes(runtime.sizeBytes)
                            : t("system install")}
                      </span>
                      {!runtime.present && <Badge tone="danger">{t("Missing files")}</Badge>}
                      <Button
                        size="sm"
                        variant="quiet"
                        className="ms-auto text-danger"
                        icon={<Trash2 className="size-3.5" />}
                        loading={busy === runtime.providerId}
                        onClick={() => setRemoveTarget(runtime)}
                      >
                        {runtime.imported || !runtime.path ? t("Forget") : t("Remove")}
                      </Button>
                    </li>
                  ))}
                </ul>
              </details>
            )}

            {data.audit.length > 0 && (
              <details className="text-[12px]">
                <summary className="cursor-pointer select-none text-ink-2">{t("Recent activity")}</summary>
                <ul className="mt-2 space-y-1 text-[11.5px] text-ink-3">
                  {data.audit.slice(0, 8).map((event) => (
                    <li key={event.id} className="flex flex-wrap items-baseline gap-x-2">
                      <span>{auditLabel(event)}</span>
                      <span className="font-mono text-[10.5px]">{event.providerId || "—"}</span>
                      <span className={event.outcome === "failed" ? "text-danger" : "text-ink-3"}>
                        {event.outcome}
                      </span>
                      <span className="ms-auto">{formatDate(event.createdAt)}</span>
                    </li>
                  ))}
                </ul>
              </details>
            )}

            {data.notices.length > 0 && (
              <details className="text-[11.5px] text-ink-3">
                <summary className="cursor-pointer select-none">{t("Compatibility notes")}</summary>
                <ul className="mt-1.5 list-disc space-y-1 ps-4">
                  {data.notices.map((notice) => (
                    <li key={notice}>{noticeText(notice)}</li>
                  ))}
                </ul>
              </details>
            )}
          </div>
        ) : null}
      </div>

      <Dialog
        open={consentOpen}
        onOpenChange={setConsentOpen}
        title={t("Install these runtimes?")}
        description={t("Ludex downloads from the pinned sources below, verifies every size and SHA-256, then installs. Nothing runs before you accept.")}
        size="md"
        footer={
          <>
            <Button variant="ghost" onClick={() => setConsentOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={!accepted}
              icon={<Download className="size-4" />}
              onClick={() => {
                setConsentOpen(false);
                void apply();
              }}
            >
              {t("Download and install")}
            </Button>
          </>
        }
      >
        <ul className="space-y-3">
          {applyable.map((step) => (
            <li key={step.id} className="border-b border-line pb-2 last:border-0 last:pb-0">
              <div className="flex flex-wrap items-baseline justify-between gap-2">
                <p className="text-[13px] text-ink">{stepTitle(step)}</p>
                {step.download && (
                  <span className="font-mono text-[10.5px] text-ink-3">
                    {formatBytes(step.download.sizeBytes)}
                  </span>
                )}
              </div>
              {step.download && (
                <>
                  <p className="mt-0.5 break-all font-mono text-[10.5px] text-ink-3">{step.download.url}</p>
                  <p className="mt-0.5 text-[10.5px] text-ink-3">
                    {step.download.licence} · SHA-256 <span className="font-mono">{step.download.sha256.slice(0, 16)}…</span>
                  </p>
                </>
              )}
              {step.requiresAdmin && <Badge tone="outline" className="mt-1">{t("Administrator")}</Badge>}
            </li>
          ))}
        </ul>
        <p className="mt-3 text-[11.5px] text-ink-3">
          {t("Downloads are cached in {path}.", { path: data?.cacheDir ?? "" })}
        </p>
        <p className="mt-1 text-[11.5px] text-ink-2">
          {t("Total download: {size}", { size: formatBytes(totalDownloadBytes(applyable)) })}
        </p>
        <div className="mt-3 flex items-start gap-2.5 text-[12px] leading-relaxed text-ink-2">
          <Checkbox checked={accepted} onCheckedChange={setAccepted} label={t("Accept the licences")} />
          <span className="cursor-pointer select-none" onClick={() => setAccepted(!accepted)}>
            {t("I accept the third-party licences and want to download and install these runtimes.")}
          </span>
        </div>
      </Dialog>

      <ConfirmDialog
        open={removeTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRemoveTarget(null);
        }}
        title={
          removeTarget
            ? t("Remove {name}?", { name: `${removeTarget.name} ${removeTarget.version}` })
            : t("Remove runtime")
        }
        description={
          removeForgets
            ? t("Ludex removes its record only. No user file is deleted.")
            : t("Only files Ludex created in its runtime folder are deleted. The game, its saves and its prefix are untouched.")
        }
        confirmLabel={removeForgets ? t("Forget") : t("Remove")}
        onConfirm={() => void remove()}
        loading={busy !== null}
      />
    </section>
  );
}
