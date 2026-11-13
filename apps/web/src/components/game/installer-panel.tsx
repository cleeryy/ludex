import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Square, TerminalSquare } from "lucide-react";
import { native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import type { InstallerPlan, InstallerReport, RepackRunReport } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button } from "@/components/ui/controls";
import { EmptyState, SectionHeader } from "@/components/page";
import { cn } from "@/lib/cn";
import { formatDuration } from "@/lib/format";
import { t } from "@/lib/i18n";

function CandidateList({
  candidates,
  saving,
  onPick,
}: {
  candidates: string[];
  saving: string | null;
  onPick: (candidate: string) => void | Promise<void>;
}) {
  return (
    <ul className="mt-2 border-t border-line">
      {candidates.map((candidate) => (
        <li key={candidate} className="flex items-center gap-3 border-b border-line py-1.5">
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-ink-2" title={candidate}>
            {candidate}
          </span>
          <Button
            variant="ghost"
            size="sm"
            loading={saving === candidate}
            disabled={saving !== null}
            onClick={() => void onPick(candidate)}
          >
            {t("Use this executable")}
          </Button>
        </li>
      ))}
    </ul>
  );
}

/**
 * Silent installer panel (#7) and the guided repack flow (#214).
 *
 * Some DRM-free releases ship a `setup.exe` instead of the game. When the
 * archive finishes with no executable, the panel leads with the detected setup:
 * one interactive run, then the game binary it produced is recorded. Nothing
 * runs without a click, nothing runs elevated, and the manual silent plan stays
 * available below.
 */
export function InstallerPanel({
  gameId,
  title,
  executable,
}: {
  gameId: number;
  title: string;
  executable?: string | null;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [plan, setPlan] = useState<InstallerPlan | null>(null);
  const [choice, setChoice] = useState<string | null>(null);
  const [report, setReport] = useState<InstallerReport | null>(null);
  const [repackReport, setRepackReport] = useState<RepackRunReport | null>(null);
  const [setupChoice, setSetupChoice] = useState<string | null>(null);
  const [running, setRunning] = useState<"silent" | "interactive" | "guided" | null>(null);
  const [saving, setSaving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadPlan = async () => {
    try {
      const next = await native.installerPlan(gameId);
      setPlan(next);
      setChoice((current) => current ?? next.executable ?? next.detected[0] ?? null);
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  useEffect(() => {
    void loadPlan();
  }, [gameId]);

  const run = async (interactive: boolean) => {
    if (!choice) return;
    setRunning(interactive ? "interactive" : "silent");
    setError(null);
    setReport(null);
    try {
      const result = await native.runInstaller({
        gameId,
        executable: choice,
        interactive,
      });
      setReport(result);
      if (result.recordedExecutable) {
        void queryClient.invalidateQueries({ queryKey: ["installed"] });
      }
      // An installer may have written new programs into the folder (#143).
      void queryClient.invalidateQueries({ queryKey: ["game-executables"] });
      push({
        title: result.success ? `${title} installed` : `${title} installer needs attention`,
        description: result.message,
        tone: result.success ? "success" : result.cancelled ? "default" : "danger",
      });
      await loadPlan();
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not run the installer"), description: message, tone: "danger" });
    } finally {
      setRunning(null);
    }
  };

  /** #214: run the detected setup interactively, then rescan for the game. */
  const runGuided = async () => {
    const repackNow = plan?.repack;
    const setup =
      setupChoice && repackNow?.candidates.includes(setupChoice)
        ? setupChoice
        : repackNow?.setup;
    if (!setup) return;
    setRunning("guided");
    setError(null);
    setRepackReport(null);
    try {
      const result = await native.runRepackInstaller({ gameId, setup });
      setRepackReport(result);
      if (result.recordedExecutable) {
        void queryClient.invalidateQueries({ queryKey: ["installed"] });
      }
      void queryClient.invalidateQueries({ queryKey: ["game-executables"] });
      push({
        title: result.recordedExecutable
          ? `${title} is ready to launch`
          : result.success
            ? t("Choose the game executable")
            : `${title} setup needs attention`,
        description: result.message,
        tone: result.cancelled ? "default" : result.success ? "success" : "danger",
      });
      await loadPlan();
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not run the installer"), description: message, tone: "danger" });
    } finally {
      setRunning(null);
    }
  };

  const useCandidate = async (candidate: string) => {
    setSaving(candidate);
    setError(null);
    try {
      await native.setExecutable(gameId, candidate);
      void queryClient.invalidateQueries({ queryKey: ["installed"] });
      void queryClient.invalidateQueries({ queryKey: ["game-executables"] });
      setRepackReport(null);
      push({ title: t("Executable updated"), description: candidate, tone: "success" });
      await loadPlan();
    } catch (cause) {
      push({ title: t("Could not set the executable"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setSaving(null);
    }
  };

  const stop = async () => {
    try {
      await native.cancelInstaller(gameId);
    } catch (cause) {
      push({ title: t("Could not stop the installer"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const pickExecutable = async () => {
    try {
      const updated = await native.chooseExecutable(gameId);
      if (!updated) return;
      void queryClient.invalidateQueries({ queryKey: ["installed"] });
      push({ title: t("Executable updated"), description: updated.executable ?? undefined, tone: "success" });
    } catch (cause) {
      push({ title: t("Could not set the executable"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const windows = plan?.platform === "windows";
  const linux = plan?.platform === "linux";
  const linuxTool = plan?.compatTool ?? null;
  const canRun = windows || (linux && Boolean(linuxTool));
  const installers = plan?.detected ?? [];
  const repack = plan?.repack ?? null;
  const guided = repack?.needed === true;
  const guidedSetup =
    setupChoice && repack?.candidates.includes(setupChoice) ? setupChoice : repack?.setup ?? null;
  // A pre-existing install with no executable and no setup: keep the rescan
  // list visible so the user can point at the game binary (#214).
  const rescan = !guided && repack !== null && !repack.ready ? repack.executableCandidates : [];

  return (
    <div className="min-w-0">
      <SectionHeader
        level={3}
        title={t("Setup program")}
        description={t("Run the shipped setup program, then record the game executable")}
        actions={
          <Button variant="ghost" size="sm" className="text-ink-2" onClick={() => void pickExecutable()}>
            {t("Choose executable")}
          </Button>
        }
      />
      <div className="border-t border-line pt-4">
        {guided && repack && (
          <div className="mb-5 border-l-2 border-signal pl-3">
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone="signal">{t("Setup program detected")}</Badge>
              <span className="truncate font-mono text-[11px] text-ink-2">{guidedSetup}</span>
            </div>
            <p className="mt-2 max-w-[70ch] text-[12.5px] leading-relaxed text-ink-3">
              {t("This archive contains a setup program. Run it once to install the game files; Ludex then finds the game executable it produced.")}
            </p>
            {repack.candidates.length > 1 && (
              <ul className="mt-2 border-t border-line">
                {repack.candidates.map((candidate) => (
                  <li key={candidate} className="border-b border-line py-1.5">
                    <button
                      type="button"
                      onClick={() => setSetupChoice(candidate)}
                      aria-pressed={guidedSetup === candidate}
                      className={cn(
                        "flex w-full min-w-0 items-center gap-2 text-left text-[12.5px]",
                        guidedSetup === candidate ? "text-ink" : "text-ink-3 hover:text-ink",
                      )}
                    >
                      <span
                        className={cn(
                          "size-2.5 shrink-0 rounded-full border",
                          guidedSetup === candidate ? "border-signal bg-signal" : "border-line-2",
                        )}
                        aria-hidden="true"
                      />
                      <span className="truncate font-mono text-[11px]">{candidate}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
            {repack.markers.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-1.5 font-mono text-[10.5px] text-ink-3">
                {repack.markers.map((marker) => (
                  <span key={marker} className="border border-line-2 px-1.5 py-0.5">
                    {marker}
                  </span>
                ))}
              </div>
            )}
            {linux && !linuxTool && (
              <p className="mt-2 max-w-[70ch] text-[12px] leading-relaxed text-ink-2">
                {t("No compatibility tool is installed, so this setup program cannot run yet. Install UMU, GE-Proton/Proton or Wine, then rescan in Settings → Compatibility.")}
              </p>
            )}
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <Button
                variant="primary"
                size="sm"
                icon={<TerminalSquare className="size-3.5" />}
                loading={running === "guided"}
                disabled={running !== null || !canRun}
                onClick={() => void runGuided()}
              >
                {repackReport && !repackReport.success && !repackReport.cancelled
                  ? t("Retry the installer")
                  : t("Run the installer")}
              </Button>
              {running === "guided" && (
                <Button variant="ghost" size="sm" icon={<Square className="size-3.5" />} onClick={() => void stop()}>
                  {t("Cancel")}
                </Button>
              )}
              <Button variant="ghost" size="sm" onClick={() => void pickExecutable()}>
                {t("Choose executable")}
              </Button>
            </div>
            <p className="mt-2 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
              {t("The setup window stays open until you finish it. Nothing runs silently and Ludex never elevates the installer.")}
            </p>

            {repackReport && (
              <div className="mt-3 flex flex-col gap-2 border-l-2 border-line-2 pl-3">
                <div className="flex flex-wrap items-baseline gap-2">
                  <Badge
                    tone={
                      repackReport.success ? "ok" : repackReport.cancelled ? "outline" : "danger"
                    }
                  >
                    {repackReport.success
                      ? t("Installed")
                      : repackReport.cancelled
                        ? t("Cancelled")
                        : repackReport.timedOut
                          ? t("Timed out")
                          : t("Failed")}
                  </Badge>
                  {repackReport.exitCode !== null && (
                    <span className="num text-[11px] text-ink-3">
                      {t("exit")} {repackReport.exitCode}
                    </span>
                  )}
                  {repackReport.recordedExecutable && (
                    <span className="truncate font-mono text-[10.5px] text-ink-2">
                      {repackReport.recordedExecutable}
                    </span>
                  )}
                </div>
                <p className="text-[12px] leading-relaxed text-ink-2">{repackReport.message}</p>
                {repackReport.candidates.length > 0 && (
                  <>
                    <p className="text-[11.5px] text-ink-3">{t("Choose the game executable")}</p>
                    <CandidateList
                      candidates={repackReport.candidates}
                      saving={saving}
                      onPick={useCandidate}
                    />
                  </>
                )}
                {repackReport.logTail && (
                  <pre className="max-h-40 overflow-auto whitespace-pre-wrap border border-line bg-surface p-2 font-mono text-[10.5px] leading-relaxed text-ink-3">
                    {repackReport.logTail}
                  </pre>
                )}
              </div>
            )}
          </div>
        )}

        {rescan.length > 0 && (
          <div className="mb-5 border-l-2 border-line-2 pl-3">
            <p className="max-w-[70ch] text-[12.5px] leading-relaxed text-ink-2">
              {t("Programs found in the install folder. Pick the one Ludex should launch.")}
            </p>
            <CandidateList candidates={rescan} saving={saving} onPick={useCandidate} />
          </div>
        )}

        <p className="max-w-[70ch] text-[12.5px] leading-relaxed text-ink-3">
          {t("Some releases are an archive of a setup program. Silent switches are tried in order and the exit code is checked; if every attempt fails you can run the installer interactively.")}{" "}
          {linux
            ? t("On Linux it runs through the game's compatibility tool inside its Wine prefix and the log is captured.")
            : t("On Windows the setup program runs directly.")}{" "}
          {t("Current executable:")} <span className="font-mono text-[11px] text-ink-2">{executable ?? t("none recorded")}</span>.
        </p>

        {linux && !linuxTool && (
          <p className="mt-3 border-l-2 border-signal/60 pl-3 text-[12.5px] leading-relaxed text-ink-2">
            {t("No compatibility tool is installed, so this setup program cannot run yet. Install UMU, GE-Proton/Proton or Wine, then rescan in Settings → Compatibility.")}
          </p>
        )}

        {installers.length === 0 ? (
          <div className="mt-3">
            <EmptyState
              compact
              level={3}
              title={t("No setup program found")}
              description={t("This install folder has no setup, install or gog program. Use “Choose executable” to point at the game binary.")}
            />
          </div>
        ) : (
          <div className="mt-4 flex flex-col gap-3">
            <ul className="border-t border-line">
              {installers.map((installer) => (
                <li key={installer} className="flex items-center gap-3 border-b border-line py-2">
                  <button
                    type="button"
                    onClick={() => setChoice(installer)}
                    aria-pressed={choice === installer}
                    className={cn(
                      "flex min-w-0 flex-1 items-center gap-2 text-left text-[12.5px]",
                      choice === installer ? "text-ink" : "text-ink-3 hover:text-ink",
                    )}
                  >
                    <span
                      className={cn(
                        "size-2.5 shrink-0 rounded-full border",
                        choice === installer ? "border-signal bg-signal" : "border-line-2",
                      )}
                      aria-hidden="true"
                    />
                    <span className="truncate font-mono text-[11px]">{installer}</span>
                  </button>
                </li>
              ))}
            </ul>

            {plan && (
              <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
                <span>
                  {windows
                    ? t("silent plan")
                    : linux && linuxTool
                      ? `via ${linuxTool}`
                      : linux
                        ? t("no compatibility tool")
                        : t("silent runs are Windows-only")}
                </span>
                {plan.attempts.map((attempt) => (
                  <span key={attempt.join(" ")} className="border border-line-2 px-1.5 py-0.5">
                    {attempt.join(" ")}
                  </span>
                ))}
                <span>{plan.timeoutSecs}{t("s per attempt")}</span>
                {plan.prefix && <span className="truncate">{t("prefix")} {plan.prefix}</span>}
              </div>
            )}

            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="primary"
                size="sm"
                icon={<TerminalSquare className="size-3.5" />}
                loading={running === "silent"}
                disabled={!choice || running !== null || !canRun}
                onClick={() => void run(false)}
              >
                {t("Run silently")}
              </Button>
              <Button
                variant="default"
                size="sm"
                loading={running === "interactive"}
                disabled={!choice || running !== null || !canRun}
                onClick={() => void run(true)}
              >
                {t("Run interactively")}
              </Button>
              {running && (
                <Button variant="ghost" size="sm" icon={<Square className="size-3.5" />} onClick={() => void stop()}>
                  {t("Cancel")}
                </Button>
              )}
            </div>

            {report && (
              <div className="flex flex-col gap-2 border-l-2 border-line-2 pl-3">
                <div className="flex flex-wrap items-baseline gap-2">
                  <Badge tone={report.success ? "ok" : report.cancelled ? "outline" : "danger"}>
                    {report.success ? t("Installed") : report.cancelled ? t("Cancelled") : report.timedOut ? t("Timed out") : t("Failed")}
                  </Badge>
                  {report.exitCode !== null && <span className="num text-[11px] text-ink-3">{t("exit")} {report.exitCode}</span>}
                  <span className="text-[12px] text-ink-2">{report.message}</span>
                </div>
                {report.attempts.length > 0 && (
                  <ul className="flex flex-col gap-0.5 font-mono text-[10.5px] text-ink-3">
                    {report.attempts.map((attempt, index) => (
                      <li key={`${attempt.args.join(" ")}-${index}`} className="truncate">
                        {attempt.args.length > 0 ? attempt.args.join(" ") : t("(no arguments)")} ·{" "}
                        {attempt.error
                          ? attempt.error
                          : attempt.timedOut
                            ? t("timed out")
                            : `exit ${attempt.exitCode ?? "?"}`}{" "}
                        · {formatDuration(attempt.durationMs / 1000)}
                      </li>
                    ))}
                  </ul>
                )}
                {report.logTail && (
                  <pre className="max-h-40 overflow-auto whitespace-pre-wrap border border-line bg-surface p-2 font-mono text-[10.5px] leading-relaxed text-ink-3">
                    {report.logTail}
                  </pre>
                )}
                {!report.success && !report.cancelled && (
                  <p className="text-[11.5px] leading-relaxed text-ink-3">
                    {t("Run it interactively to see what the installer is asking for. The arguments can be changed in Settings → Desktop.")}
                  </p>
                )}
              </div>
            )}
          </div>
        )}

        {error && (
          <p role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12.5px] text-danger">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
