import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { FileTerminal, Square, TerminalSquare } from "lucide-react";
import { native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import type { InstallerPlan, InstallerReport } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button } from "@/components/ui/controls";
import { SectionHeader } from "@/components/page";
import { cn } from "@/lib/cn";
import { formatDuration } from "@/lib/format";
import { t } from "@/lib/i18n";

/**
 * Silent installer panel (#7).
 *
 * Some DRM-free releases ship a `setup.exe` instead of the game. Ludex can run
 * it with the usual silent switches and record the game executable afterwards.
 * Nothing runs without a click: the plan is only a preview.
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
  const [running, setRunning] = useState<"silent" | "interactive" | null>(null);
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

  return (
    <section className="mt-12" aria-labelledby="installer-heading">
      <SectionHeader
        title={t("Installer")}
        description={t("Run the shipped setup program, then record the game executable")}
        actions={
          <Button variant="ghost" size="sm" className="text-ink-2" onClick={() => void pickExecutable()}>
            {t("Choose executable")}
          </Button>
        }
      />
      <div id="installer-heading" className="border-t border-line pt-4">
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
          <p className="mt-3 flex items-start gap-2 text-[12.5px] text-ink-3">
            <FileTerminal className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            {t("No installer was found in this install folder. Use “Choose executable” to point at the game binary.")}
          </p>
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
    </section>
  );
}
