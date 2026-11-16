import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { PackageOpen, RefreshCw } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import { useToast } from "@/state/toasts";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { t, tn } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/** States that still move; anything else ends the poll. */
function isActive(status: string | undefined): boolean {
  return status === "queued" || status === "extracting";
}

export interface PrepareTransportProps {
  versionId: number;
  /** Container the server detected (`zip`, `7z`, `rar`, …); null is unknown. */
  archiveFormat?: string | null;
  /** `enabled` from the extracted-cache overview. */
  cacheEnabled: boolean;
  /** False while the overview is still loading; the action waits for an answer. */
  cacheKnown?: boolean;
  /** Why the overview could not be read; shown as the disabled reason. */
  cacheError?: string | null;
  /** Why this version cannot be prepared (missing archive, offline root). */
  unavailableReason?: string | null;
  /** Poll cadence; tests shorten it. */
  pollMs?: number;
  /** Called once when the entry becomes ready, so the cache screen can refresh. */
  onReady?: () => void;
}

/**
 * "Prepare file transport" for one version (#231).
 *
 * The server extracts the archive once and keeps it in a bounded cache; the
 * first `POST` starts the job and every later call joins it and reports the
 * same progress, so this is both the trigger and the poll. The control stays
 * quiet: one ghost button in the version list until the administrator asks.
 */
export function PrepareTransport({
  versionId,
  archiveFormat,
  cacheEnabled,
  cacheKnown = true,
  cacheError = null,
  unavailableReason = null,
  pollMs = 1000,
  onReady,
}: PrepareTransportProps) {
  const { push } = useToast();
  const [started, setStarted] = useState(false);
  const notified = useRef(false);

  const poll = useQuery({
    queryKey: qk.admin.extract(versionId),
    queryFn: () => api.extractVersion(versionId),
    enabled: started,
    staleTime: 0,
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      return isActive(status) ? pollMs : false;
    },
  });

  const status = poll.data?.status;

  useEffect(() => {
    if (!poll.data || poll.data.status !== "ready" || notified.current) return;
    notified.current = true;
    onReady?.();
    push({
      title: t("File transport ready"),
      description: `${tn(poll.data.files, "1 file", "{count} files")} · ${formatBytes(poll.data.bytes)}`,
      tone: "success",
    });
  }, [poll.data, onReady, push]);

  const disabledReason = !cacheKnown
    ? t("Checking the extracted cache")
    : cacheError
      ? cacheError
      : !cacheEnabled
        ? t("The extracted cache is off. Turn it on to prepare archives.")
        : unavailableReason
          ? unavailableReason
          : !archiveFormat
            ? t("The archive format is unknown. Run a scan first.")
            : null;

  const failure =
    status === "failed" || status === "unavailable"
      ? poll.data?.error ?? t("Extraction failed")
      : poll.error
        ? errorMessage(poll.error)
        : null;

  if (disabledReason) {
    return (
      <Button
        variant="ghost"
        size="sm"
        icon={<PackageOpen className="size-3.5" />}
        disabled
        title={disabledReason}
      >
        {t("Prepare file transport")}
      </Button>
    );
  }

  if (status === "ready") {
    return (
      <span className="flex flex-wrap items-center gap-2 text-[11px] text-ink-3">
        <Badge tone="ok">{t("Ready")}</Badge>
        <span className="num" role="status">
          {tn(poll.data?.files ?? 0, "1 file", "{count} files")} · {formatBytes(poll.data?.bytes)}
        </span>
      </span>
    );
  }

  if (failure) {
    return (
      <span className="flex max-w-[320px] flex-col items-end gap-1">
        <span role="alert" className="text-end text-[11px] leading-snug text-danger">
          {failure}
        </span>
        {status !== "unavailable" && (
          <Button
            variant="ghost"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            onClick={() => void poll.refetch()}
          >
            {t("Try again")}
          </Button>
        )}
      </span>
    );
  }

  if (started) {
    return (
      <span className="flex min-w-[180px] max-w-[260px] flex-col items-end gap-1.5">
        <span className="flex flex-wrap items-baseline justify-end gap-x-2 text-[10.5px] text-ink-3">
          <span className="font-mono uppercase tracking-[0.08em]">
            {status === "extracting" ? t("Extracting") : status === "queued" ? t("Queued") : t("Preparing")}
          </span>
          {status === "extracting" && (
            <span className="num">
              {tn(poll.data?.files ?? 0, "1 file", "{count} files")} · {formatBytes(poll.data?.bytes)}
            </span>
          )}
        </span>
        <Progress
          value={isActive(status) || poll.isPending ? null : 100}
          tone="signal"
          label={t("Extraction progress")}
          className="w-full"
        />
      </span>
    );
  }

  return (
    <Button
      variant="ghost"
      size="sm"
      icon={<PackageOpen className="size-3.5" />}
      onClick={() => setStarted(true)}
    >
      {t("Prepare file transport")}
    </Button>
  );
}
