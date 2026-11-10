import { useEffect, useState } from "react";
import { FolderOpen } from "lucide-react";
import { installBridge } from "@/lib/install";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import type { InstalledGame, MoveProgress } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Button, Field, Input, Progress } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";

/**
 * "Move install…" dialog (#36): pick a new install root, then the game folder
 * is renamed when it stays on the same drive and copied + verified when it
 * crosses drives. Progress arrives from `install://move-progress`.
 */
export function MoveInstallDialog({
  open,
  onOpenChange,
  gameId,
  title,
  installed,
  onMoved,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  gameId: number;
  title: string;
  installed: InstalledGame;
  onMoved: (game: InstalledGame) => void;
}) {
  const { push } = useToast();
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<MoveProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setBusy(false);
      setProgress(null);
      setError(null);
    }
  }, [open]);

  useEffect(() => {
    if (!open || !isNative()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void native
      .onMoveProgress((update) => {
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
    const path = await native.pickFolder("Choose the new install folder");
    if (path) setTarget(path);
  };

  const move = async () => {
    if (!target.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const updated = await installBridge.moveInstall(gameId, target.trim());
      onMoved(updated);
      push({
        title: `${title} moved`,
        description: `It now lives in ${updated.installPath}.`,
        tone: "success",
      });
      onOpenChange(false);
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: "Could not move the game", description: message, tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  const percent = progress && progress.bytesTotal > 0 ? Math.min(100, (progress.bytesDone / progress.bytesTotal) * 100) : null;

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Move ${title}`}
      description="Choose the folder that should contain this game from now on."
      size="sm"
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} disabled={!target.trim()} onClick={() => void move()}>
            Move game
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <dl className="flex flex-col gap-2 text-[12.5px]">
          <div className="flex items-baseline justify-between gap-4">
            <dt className="text-ink-3">Current folder</dt>
            <dd className="truncate font-mono text-[11px] text-ink-2">{installed.installPath}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-4">
            <dt className="text-ink-3">Size on disk</dt>
            <dd className="num text-[12px] text-ink-2">{formatBytes(installed.sizeBytes)}</dd>
          </div>
        </dl>

        <Field label="New install folder" htmlFor="move-target" hint="The game keeps its name and version subfolder.">
          <div className="flex items-stretch gap-2">
            <Input
              id="move-target"
              value={target}
              onChange={(event) => setTarget(event.target.value)}
              placeholder={installed.installPath.replace(/[\\/][^\\/]+[\\/][^\\/]+$/, "") || "/games"}
              spellCheck={false}
              disabled={busy}
              className="h-10 font-mono text-[11.5px]"
            />
            <Button
              variant="default"
              size="sm"
              className="h-10 shrink-0"
              icon={<FolderOpen className="size-3.5" />}
              onClick={() => void pick()}
              disabled={busy}
            >
              Browse
            </Button>
          </div>
        </Field>

        {busy && (
          <div>
            <Progress value={percent} label="Move progress" />
            <p className="mt-1.5 font-mono text-[10.5px] text-ink-3">
              {progress?.message ?? "Preparing the move…"}
              {progress && progress.bytesTotal > 0 && (
                <span>
                  {" "}
                  · {formatBytes(progress.bytesDone)} of {formatBytes(progress.bytesTotal)}
                </span>
              )}
            </p>
          </div>
        )}

        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
            {error}
          </p>
        )}

        <p className="text-[11.5px] leading-relaxed text-ink-3">
          Moving to the same drive renames the folder instantly. Moving to another drive copies every file, checks the
          size, then deletes the old copy. The game cannot be running during the move.
        </p>
      </div>
    </Dialog>
  );
}
