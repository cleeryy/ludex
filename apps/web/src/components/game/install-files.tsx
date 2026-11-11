import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { FileSignature, RefreshCw, ShieldCheck, Wrench } from "lucide-react";
import { installBridge } from "@/lib/install";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import type { ManifestProgress, RepairProgress, RepairReport, VerifyProgress, VerifyReport } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { SectionHeader } from "@/components/page";
import { cn } from "@/lib/cn";
import { t, tn } from "@/lib/i18n";

/**
 * Verify / repair panel (#37) for an installed game. Verification compares the
 * files on disk with the manifest recorded at install time; repair restores the
 * damaged files from the version archive. An adopted folder (#129) has no
 * manifest until "Build manifest" hashes it; until then Verify reports
 * `noManifest` instead of a clean pass.
 */
export function InstallFiles({ gameId, title }: { gameId: number; title: string }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<"verify" | "repair" | "manifest" | null>(null);
  const [report, setReport] = useState<VerifyReport | null>(null);
  const [repair, setRepair] = useState<RepairReport | null>(null);
  const [progress, setProgress] = useState<VerifyProgress | null>(null);
  const [manifestProgress, setManifestProgress] = useState<ManifestProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    const offs: Array<() => void> = [];
    const add = (pending: Promise<() => void>) => {
      void pending.then((off) => {
        if (disposed) off();
        else offs.push(off);
      });
    };
    add(
      native.onVerifyProgress((update) => {
        if (update.gameId === gameId) setProgress(update);
      }),
    );
    add(
      native.onRepairProgress((update: RepairProgress) => {
        if (update.gameId === gameId) setProgress(update);
      }),
    );
    add(
      native.onManifestProgress((update) => {
        if (update.gameId === gameId) setManifestProgress(update);
      }),
    );
    return () => {
      disposed = true;
      for (const off of offs) off();
    };
  }, [gameId]);

  const verify = async () => {
    setBusy("verify");
    setError(null);
    setRepair(null);
    setProgress(null);
    try {
      const next = await installBridge.verifyInstall(gameId);
      setReport(next);
      push({
        title: next.status === "ok" ? `${title} files look intact` : `${title} has file problems`,
        description:
          next.status === "ok"
            ? `${next.filesChecked} files checked.`
            : next.status === "noManifest"
              ? (next.message ?? "Reinstall the game to record a manifest.")
              : `${next.missing.length} missing, ${next.corrupt.length} corrupt, ${next.extra.length} extra.`,
        tone: next.status === "ok" ? "success" : next.status === "issues" ? "danger" : "default",
      });
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not verify the files"), description: message, tone: "danger" });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  };

  const repairFiles = async () => {
    setBusy("repair");
    setError(null);
    setProgress(null);
    try {
      const next = await installBridge.repairInstall(gameId);
      setRepair(next);
      setReport(next.verify);
      void queryClient.invalidateQueries({ queryKey: ["installed"] });
      void queryClient.invalidateQueries({ queryKey: ["game-executables"] });
      push({
        title: next.fallback ? "Repair needs a reinstall" : `${title} repaired`,
        description: next.message,
        tone: next.fallback ? "danger" : next.failed.length > 0 ? "default" : "success",
      });
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not repair the files"), description: message, tone: "danger" });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  };

  const percent = progress && progress.bytesTotal > 0 ? Math.min(100, (progress.bytesDone / progress.bytesTotal) * 100) : null;
  const manifestPercent =
    manifestProgress && manifestProgress.bytesTotal > 0
      ? Math.min(100, (manifestProgress.bytesDone / manifestProgress.bytesTotal) * 100)
      : null;
  const damaged = report ? report.missing.length + report.corrupt.length : 0;

  /** Hash the folder into a manifest so Verify/Repair can run (#129). */
  const buildManifest = async () => {
    setBusy("manifest");
    setError(null);
    setManifestProgress(null);
    try {
      const built = await native.buildInstallManifest(gameId);
      push({
        title: t("Manifest built for {title}", { title }),
        description: t("{files} hashed ({size}). Verify can now compare the folder with it.", {
          files: tn(built.files, "1 file", "{count} files"),
          size: formatBytes(built.bytes),
        }),
        tone: "success",
      });
      setReport(null);
      setRepair(null);
      setProgress(null);
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not build the manifest"), description: message, tone: "danger" });
    } finally {
      setBusy(null);
      setManifestProgress(null);
    }
  };

  return (
    <section className="mt-12" aria-labelledby="installed-files-heading">
      <SectionHeader
        title={t("Installed files")}
        description={t("Checks the files on this disk against the manifest recorded at install time")}
        actions={
          <div className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              className="text-ink-2"
              icon={<FileSignature className="size-3.5" />}
              loading={busy === "manifest"}
              disabled={busy !== null}
              onClick={() => void buildManifest()}
            >
              {t("Build manifest")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="text-ink-2"
              icon={<ShieldCheck className="size-3.5" />}
              loading={busy === "verify"}
              disabled={busy !== null}
              onClick={() => void verify()}
            >
              {t("Verify")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className={cn("text-ink-2", damaged > 0 && "text-signal-text")}
              icon={<Wrench className="size-3.5" />}
              loading={busy === "repair"}
              disabled={busy !== null}
              onClick={() => void repairFiles()}
            >
              {t("Repair")}
            </Button>
          </div>
        }
      />
      <div id="installed-files-heading" className="border-t border-line pt-4">
        {busy === "manifest" && (
          <div className="max-w-[520px]">
            <Progress value={manifestPercent} label={t("Manifest progress")} />
            <p className="mt-1.5 flex flex-wrap items-baseline gap-x-2 font-mono text-[10.5px] text-ink-3">
              <RefreshCw className="size-3 animate-spin" aria-hidden="true" />
              <span>{manifestProgress?.message ?? t("Hashing the installed files…")}</span>
              {manifestProgress && manifestProgress.filesTotal > 0 && (
                <span>
                  {t("{done} / {total} files", { done: manifestProgress.filesDone, total: manifestProgress.filesTotal })}
                </span>
              )}
            </p>
          </div>
        )}
        {busy && (
          <div className="max-w-[520px]">
            <Progress value={percent} label={t("File check progress")} />
            <p className="mt-1.5 flex flex-wrap items-baseline gap-x-2 font-mono text-[10.5px] text-ink-3">
              <RefreshCw className="size-3 animate-spin" aria-hidden="true" />
              <span>{progress?.message ?? (busy === "verify" ? t("Hashing installed files…") : t("Preparing the repair…"))}</span>
              {progress && progress.filesTotal > 0 && (
                <span>
                  {t("{done} / {total} files", { done: progress.filesDone, total: progress.filesTotal })}
                </span>
              )}
            </p>
          </div>
        )}

        {!busy && !report && (
          <p className="text-[12.5px] leading-relaxed text-ink-3">
            {t("Verify reads every installed file and reports missing, damaged or extra ones. Repair restores only the damaged files from the server archive.")}
          </p>
        )}

        {!busy && report && (
          <div className="flex flex-col gap-3">
            <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
              <Badge tone={report.status === "ok" ? "ok" : report.status === "issues" ? "danger" : "outline"}>
                {report.status === "ok" ? t("Verified") : report.status === "issues" ? t("Needs repair") : t("No manifest")}
              </Badge>
              <span className="num text-[11px] text-ink-3">
                {t("{files} · {size} hashed", { files: tn(report.filesChecked, "1 file", "{count} files"), size: formatBytes(report.bytesChecked) })}
              </span>
              {report.extra.length > 0 && (
                <span className="num text-[11px] text-ink-3">{tn(report.extra.length, "1 extra file", "{count} extra files")}</span>
              )}
            </div>

            {report.status === "noManifest" && (
              <div className="flex flex-col gap-2">
                <p className="max-w-[70ch] text-[12.5px] leading-relaxed text-ink-3">{report.message}</p>
                <div>
                  <Button
                    variant="default"
                    size="sm"
                    icon={<FileSignature className="size-3.5" />}
                    loading={busy === "manifest"}
                    disabled={busy !== null}
                    onClick={() => void buildManifest()}
                  >
                    {t("Build manifest")}
                  </Button>
                </div>
              </div>
            )}

            {report.status === "ok" && (
              <p className="text-[12.5px] text-ok">{t("Every file matches the manifest.")}</p>
            )}

            {report.status === "issues" && (
              <div className="grid gap-3 sm:grid-cols-2">
                <FileList title={t("Missing")} files={report.missing.map((issue) => issue.path)} tone="danger" />
                <FileList title={t("Corrupt")} files={report.corrupt.map((issue) => issue.path)} tone="danger" />
              </div>
            )}

            {repair && (
              <div className="flex flex-col gap-1.5 border-l-2 border-line-2 pl-3">
                <p className={cn("text-[12.5px]", repair.fallback ? "text-danger" : "text-ink-2")}>{repair.message}</p>
                {repair.repaired.length > 0 && (
                  <p className="font-mono text-[10.5px] text-ink-3">{t("Restored:")} {repair.repaired.join(", ")}</p>
                )}
                {repair.failed.length > 0 && (
                  <p className="font-mono text-[10.5px] text-danger">{t("Could not restore:")} {repair.failed.join(", ")}</p>
                )}
                {repair.fallback && (
                  <p className="text-[11.5px] text-ink-3">
                    {t("Use the install action above to download the game again; your playtime and progress stay on the server.")}
                  </p>
                )}
              </div>
            )}

            {error && (
              <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
                {error}
              </p>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

function FileList({ title, files, tone }: { title: string; files: string[]; tone: "danger" }) {
  if (files.length === 0) return null;
  const shown = files.slice(0, 6);
  return (
    <div>
      <p className={cn("text-[11px] uppercase tracking-[0.08em]", tone === "danger" ? "text-danger" : "text-ink-3")}>
        {title} · {files.length}
      </p>
      <ul className="mt-1 flex flex-col gap-0.5 font-mono text-[10.5px] text-ink-3">
        {shown.map((file) => (
          <li key={file} className="truncate" title={file}>
            {file}
          </li>
        ))}
        {files.length > shown.length && <li>{t("…and {count} more", { count: files.length - shown.length })}</li>}
      </ul>
    </div>
  );
}
