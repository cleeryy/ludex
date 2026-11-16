import { Link } from "react-router-dom";
import { RefreshCw } from "lucide-react";
import { t } from "@/lib/i18n";
import { useIsAdmin } from "@/state/auth";
import { useVirusTotalQueue } from "@/state/virustotal-queue";
import { Tooltip } from "@/components/ui/tooltip";

/**
 * Shell indicator for a running VirusTotal bulk scan (#238). It is only a
 * glanceable link to Admin → Games: no modal, and navigation stays free while
 * the queue keeps running above the router.
 */
export function VirusTotalQueueIndicator() {
  const isAdmin = useIsAdmin();
  const queue = useVirusTotalQueue();
  if (!isAdmin || !queue.queue || queue.remaining <= 0) return null;

  const label = t("VirusTotal: {count} scans queued", { count: queue.remaining });

  return (
    <Tooltip label={label} side="bottom">
      <Link
        to="/admin/games"
        aria-label={label}
        className="inline-flex h-9 items-center gap-1.5 rounded-[2px] px-2 text-signal-text transition-colors hover:bg-surface"
      >
        <RefreshCw className="size-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />
        <span className="num text-[11px]">{queue.remaining}</span>
      </Link>
    </Tooltip>
  );
}
