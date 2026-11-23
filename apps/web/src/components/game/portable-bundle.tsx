import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FolderOpen, HardDriveDownload } from "lucide-react";
import { installBridge } from "@/lib/install";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatPercent } from "@/lib/format";
import type { InstalledGame, PortableBundleInfo, PortableProgress } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Button, Field, Input, Progress } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/**
 * #332: GOG-style portable Linux bundle.
 *
 * The install folder becomes the game: `run.sh`, `install.sh` and
 * `ludex-game.json` live next to the files, so the folder starts on another
 * Linux machine without Ludex. The dialog shows the size before copying, the
 * runtime the target still needs, and copies the folder when a destination is
 * picked. Native games are self-contained; Windows games still need UMU, Proton
 * or Wine on the new machine.
 */
export function PortableBundlePanel({
  gameId,
  title,
  installed,
}: {
  gameId: number;
  title: string;
  installed?: InstalledGame;
}) {
  const { push } = useToast();
  const [open, setOpen] = useState(false);
  const desktop = isNative();

  const estimate = useQuery({
    queryKey: qk.portable.estimate(gameId),
    queryFn: () => installBridge.portableEstimate(gameId),
    enabled: desktop && Boolean(installed),
    retry: false,
    staleTime: 60_000,
  });

  // Off Linux the desktop answers `supported: false`; the action does not exist
  // rather than existing and failing.
  if (!desktop) return null;
  if (estimate.data && !estimate.data.supported) return null;

  const data = estimate.data;
  const blocked = !installed || !data || !data.executablePresent || Boolean(data.reason);
  const runtimeNote = data?.runtimeRequirement === "windows-compat";

  return (
    <div className="min-w-0">
      <SectionHeader
        level={3}
        title={t("Portable Linux bundle")}
        description={t("Start this install on another Linux machine, without Ludex")}
        actions={
          <Button
            variant="default"
            size="sm"
            icon={<HardDriveDownload className="size-3.5" />}
            disabled={blocked}
            onClick={() => setOpen(true)}
          >
            {t("Create portable Linux bundle")}
          </Button>
        }
      />
      <div className="border-t border-line pt-4">
        <p className="max-w-[70ch] text-label leading-relaxed text-ink-3">
          {t("The folder gets run.sh, install.sh and ludex-game.json, so it starts from a different path or another computer. Native Linux games need nothing else; Windows games still need UMU, Proton or Wine on the machine that runs them.")}
        </p>

        {!installed && (
          <p className="mt-2 max-w-[70ch] text-label text-ink-2">
            {t("Install the game on this machine first.")}
          </p>
        )}

        {estimate.isPending && installed && (
          <p className="mt-2 font-mono text-meta text-ink-3">{t("Estimating the bundle size…")}</p>
        )}

        {data?.reason && (
          <p role="status" className="mt-2 max-w-[70ch] border-l-2 border-danger pl-3 text-label text-danger">
            {data.reason}
          </p>
        )}

        {data && !data.reason && (
          <dl className="mt-3 flex flex-col gap-1.5 text-label">
            <div className="flex items-baseline justify-between gap-4">
              <dt className="text-ink-3">{t("Estimated size")}</dt>
              <dd className="num text-label text-ink-2">{formatBytes(data.bytes)}</dd>
            </div>
            <div className="flex items-baseline justify-between gap-4">
              <dt className="text-ink-3">{t("Compatibility runtime")}</dt>
              <dd className="text-label text-ink-2">
                {runtimeNote
                  ? t("Needed on the target machine (UMU, Proton or Wine)")
                  : t("None: native Linux build")}
              </dd>
            </div>
          </dl>
        )}

        {estimate.isError && (
          <p role="alert" className="mt-2 text-label text-danger">
            {errorMessage(estimate.error)}
          </p>
        )}
      </div>

      {installed && data && (
        <PortableBundleDialog
          open={open}
          onOpenChange={setOpen}
          gameId={gameId}
          title={title}
          installed={installed}
          estimateBytes={data.bytes}
          runtimeRequired={data.runtimeRequirement}
          onCreated={(info) => {
            push({
              title: t("Portable bundle ready"),
              description: t("Portable bundle created in {path}", { path: info.path }),
              tone: "success",
            });
          }}
        />
      )}
    </div>
  );
}

function PortableBundleDialog({
  open,
  onOpenChange,
  gameId,
  title,
  installed,
  estimateBytes,
  runtimeRequired,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  gameId: number;
  title: string;
  installed: InstalledGame;
  estimateBytes: number;
  runtimeRequired: "linux-native" | "windows-compat";
  onCreated: (info: PortableBundleInfo) => void;
}) {
  const [destination, setDestination] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<PortableProgress | null>(null);
  const [result, setResult] = useState<PortableBundleInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setBusy(false);
      setProgress(null);
      setResult(null);
      setError(null);
      setDestination("");
    }
  }, [open]);

  useEffect(() => {
    if (!open || !isNative()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void native
      .onPortableProgress((update) => {
        if (update.gameId === gameId) setProgress(update);
      })
      .then((off) => {
        if (disposed) off();
        else unlisten = off;
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [open, gameId]);

  const pick = async () => {
    const path = await native.pickFolder("Choose where the portable copy should go");
    if (path) setDestination(path);
  };

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const info = await installBridge.createPortableBundle(gameId, destination.trim() || null);
      setResult(info);
      onCreated(info);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const percent =
    progress && progress.bytesTotal > 0
      ? Math.min(100, (progress.bytesDone / progress.bytesTotal) * 100)
      : null;
  const folderName = installed.installPath.split(/[\\/]/).filter(Boolean).slice(-1)[0] ?? "game";

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("Create a portable bundle of {title}", { title })}
      description={t("The folder starts on another Linux machine without Ludex.")}
      size="sm"
      footer={
        result ? (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              {t("Close")}
            </Button>
            <Button
              variant="default"
              icon={<FolderOpen className="size-3.5" />}
              onClick={() => void native.openPath(result.path)}
            >
              {t("Open folder")}
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={busy}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" loading={busy} onClick={() => void create()}>
              {destination.trim() ? t("Copy and create bundle") : t("Create in the install folder")}
            </Button>
          </>
        )
      }
    >
      <div className="flex flex-col gap-4">
        {!result && (
          <>
            <dl className="flex flex-col gap-2 text-label">
              <div className="flex items-baseline justify-between gap-4">
                <dt className="text-ink-3">{t("Estimated size")}</dt>
                <dd className="num text-label text-ink-2">{formatBytes(estimateBytes)}</dd>
              </div>
              <div className="flex items-baseline justify-between gap-4">
                <dt className="text-ink-3">{t("Compatibility runtime")}</dt>
                <dd className="text-right text-label text-ink-2">
                  {runtimeRequired === "windows-compat"
                    ? t("Needed on the target machine (UMU, Proton or Wine)")
                    : t("None: native Linux build")}
                </dd>
              </div>
            </dl>

            <Field
              label={t("Destination folder")}
              htmlFor="portable-destination"
              hint={t("Leave empty to write the scripts in the install folder.")}
            >
              <div className="flex items-stretch gap-2">
                <Input
                  id="portable-destination"
                  value={destination}
                  onChange={(event) => setDestination(event.target.value)}
                  placeholder={installed.installPath.replace(/[\\/][^\\/]+$/, "") || "/media/usb/Games"}
                  spellCheck={false}
                  disabled={busy}
                  className="h-10 font-mono text-meta"
                />
                <Button
                  variant="default"
                  size="sm"
                  className="h-10 shrink-0"
                  icon={<FolderOpen className="size-3.5" />}
                  onClick={() => void pick()}
                  disabled={busy}
                >
                  {t("Browse")}
                </Button>
              </div>
            </Field>

            {destination.trim() && (
              <p className="text-meta leading-relaxed text-ink-3">
                {t("A folder named {name} is created inside the folder you pick; it is never overwritten.", { name: folderName })}
              </p>
            )}

            <p className="text-meta leading-relaxed text-ink-3">
              {t("Existing run.sh, install.sh and ludex-game.json are replaced. The game files are not touched, and no runtime is copied: install UMU, Proton or Wine on the machine that runs a Windows game.")}
            </p>

            {busy && (
              <div>
                <Progress value={percent} label={t("Bundle progress")} />
                <p className="mt-1.5 font-mono text-micro text-ink-3">
                  {progress?.message ?? t("Preparing the bundle…")}
                  {progress && progress.bytesTotal > 0 && (
                    <span>
                      {" "}
                      · {t("{done} of {total}", { done: formatBytes(progress.bytesDone), total: formatBytes(progress.bytesTotal) })}
                      {percent !== null && ` · ${formatPercent(percent)}`}
                    </span>
                  )}
                </p>
              </div>
            )}

            {error && (
              <p role="alert" className="border-l-2 border-danger pl-3 text-label text-danger">
                {error}
              </p>
            )}
          </>
        )}

        {result && (
          <div className="flex flex-col gap-3">
            <div className="border-l-2 border-ok pl-3">
              <p className="text-label text-ink">{t("Portable bundle ready")}</p>
              <p className="mt-0.5 break-all font-mono text-meta text-ink-2">{result.path}</p>
            </div>
            <p className="text-meta leading-relaxed text-ink-3">
              {t("Start it on the target with ./install.sh once, then ./run.sh. Extra arguments are passed through.")}
            </p>
            {result.runtimeRequired === "windows-compat" && (
              <p className="text-meta leading-relaxed text-ink-2">
                {t("This is a Windows game: the target machine still needs UMU, Proton or Wine. install.sh explains exactly what to install when none is found.")}
              </p>
            )}
            {result.replaced.length > 0 && (
              <p className="text-meta leading-relaxed text-ink-3">
                {t("Replaced files shipped with the archive: {files}", { files: result.replaced.join(", ") })}
              </p>
            )}
          </div>
        )}
      </div>
    </Dialog>
  );
}
