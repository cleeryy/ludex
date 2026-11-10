import { useEffect, useState } from "react";
import { RefreshCw, RotateCcw } from "lucide-react";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import type { UpdateInstalled, UpdateProgress, UpdateStatus } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Progress } from "@/components/ui/controls";

/**
 * Application updater panel (#16). Checks on demand, shows the channel and
 * current version, downloads with progress and offers a restart. When the build
 * has no signing key the check reports it instead of failing silently.
 */
export function DesktopUpdatePanel() {
  const { push } = useToast();
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [installed, setInstalled] = useState<UpdateInstalled | null>(null);

  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    let off: (() => void) | undefined;
    void native.onUpdateProgress((next) => {
      if (!disposed) setProgress(next);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else off = unlisten;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, []);

  const check = async () => {
    setChecking(true);
    setInstalled(null);
    try {
      const next = await native.checkForUpdates(true);
      setStatus(next);
      if (next.available) {
        push({ title: `Ludex ${next.available.version} is available`, tone: "success" });
      } else if (next.configured) {
        push({ title: "You are up to date" });
      }
    } catch (cause) {
      push({ title: "Could not check for updates", description: errorMessage(cause), tone: "danger" });
    } finally {
      setChecking(false);
    }
  };

  const install = async () => {
    setInstalling(true);
    setProgress(null);
    try {
      const next = await native.installUpdate();
      setStatus(next);
      setInstalled(
        next.available
          ? { version: next.available.version, restarted: false }
          : null,
      );
      push({ title: "Update installed", description: next.message ?? undefined, tone: "success" });
    } catch (cause) {
      push({ title: "Could not install the update", description: errorMessage(cause), tone: "danger" });
    } finally {
      setInstalling(false);
    }
  };

  const configured = status?.configured ?? true;
  const percent = progress?.percent ?? null;

  return (
    <div className="border-b border-line py-4">
      <div className="flex items-start justify-between gap-6">
        <div>
          <p className="text-[13px] font-medium text-ink">App updates</p>
          <p className="mt-1 max-w-[46ch] text-[11.5px] leading-relaxed text-ink-3">
            Follows GitHub releases. <span className="font-mono">stable</span> tracks the latest release;{" "}
            <span className="font-mono">early</span> tracks the <span className="font-mono">edge</span> prerelease.
            Releases are signed; a build without the public key cannot install updates.
          </p>
        </div>
        <Button
          variant="default"
          size="sm"
          icon={<RefreshCw className="size-3.5" />}
          loading={checking}
          onClick={() => void check()}
        >
          Check for updates
        </Button>
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-[10.5px] text-ink-3">
        {status && <span>Current {status.currentVersion}</span>}
        {status && <span>Channel {status.channel}</span>}
        {status?.lastCheckedAt && <span>Last check {new Date(status.lastCheckedAt).toLocaleString()}</span>}
        {!configured && <Badge tone="danger">Signing key missing</Badge>}
      </div>

      {status?.message && (
        <p className={configured ? "mt-2 text-[12px] text-ink-2" : "mt-2 text-[12px] text-danger"}>{status.message}</p>
      )}

      {status?.available && !installed && (
        <div className="mt-3 border border-line p-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <p className="text-[13px] text-ink">Ludex {status.available.version}</p>
              {status.available.publishedAt && (
                <p className="mt-0.5 font-mono text-[10.5px] text-ink-3">Published {status.available.publishedAt}</p>
              )}
              {status.available.notes && (
                <p className="mt-1 max-w-[60ch] whitespace-pre-line text-[11.5px] leading-relaxed text-ink-3">
                  {status.available.notes}
                </p>
              )}
            </div>
            <Button variant="primary" size="sm" loading={installing} onClick={() => void install()}>
              Download and install
            </Button>
          </div>
          {installing && (
            <div className="mt-3">
              <Progress value={percent} label="Update download" />
              <p className="mt-1 font-mono text-[10.5px] text-ink-3">
                {formatBytes(progress?.downloaded ?? 0)}
                {progress?.total ? ` of ${formatBytes(progress.total)}` : ""}
              </p>
            </div>
          )}
        </div>
      )}

      {installed && (
        <div className="mt-3 flex flex-wrap items-center gap-3 border border-line p-3">
          <p className="text-[12.5px] text-ink-2">
            Ludex {installed.version} is installed. {installed.restarted ? "The installer restarts the app." : "Restart to use it."}
          </p>
          {!installed.restarted && (
            <Button
              variant="primary"
              size="sm"
              icon={<RotateCcw className="size-3.5" />}
              onClick={() => void native.restartApp()}
            >
              Restart Ludex
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
