import { Link } from "react-router-dom";
import { Upload } from "lucide-react";
import { formatPercent } from "@/lib/format";
import { t } from "@/lib/i18n";
import { uploadIndicator } from "@/lib/upload";
import { useIsEditor } from "@/state/auth";
import { useUpload } from "@/state/upload";
import { Tooltip } from "@/components/ui/tooltip";

const SIZE = 20;
const STROKE = 2.5;
const RADIUS = (SIZE - STROKE) / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

/**
 * Shell progress for a running archive upload (#185). Clicking it opens the
 * Upload screen, where the same state is shown in full. Big Picture has its own
 * route without the shell, and fullscreen drops the top bar that hosts this.
 */
export function UploadIndicator() {
  const upload = useUpload();
  const isEditor = useIsEditor();
  const status = uploadIndicator(upload);
  if (!isEditor || !status) return null;

  const label =
    status.phase === "verifying"
      ? `${t("Verifying")} · ${status.fileName}`
      : t("Uploading {file} — {percent}%", {
          file: status.fileName,
          percent: Math.round(status.percent),
        });
  const dash = (CIRCUMFERENCE * status.percent) / 100;

  return (
    <Tooltip label={label} side="bottom">
      <Link
        to="/admin/upload"
        aria-label={label}
        className="relative grid size-9 place-items-center rounded-[2px] transition-colors hover:bg-surface"
      >
        <svg
          viewBox={`0 0 ${SIZE} ${SIZE}`}
          className="absolute size-5 -rotate-90"
          aria-hidden="true"
        >
          <circle
            cx={SIZE / 2}
            cy={SIZE / 2}
            r={RADIUS}
            fill="none"
            strokeWidth={STROKE}
            className="stroke-line-2"
          />
          <circle
            cx={SIZE / 2}
            cy={SIZE / 2}
            r={RADIUS}
            fill="none"
            strokeWidth={STROKE}
            strokeLinecap="round"
            strokeDasharray={`${dash} ${CIRCUMFERENCE}`}
            className="stroke-signal-text transition-[stroke-dasharray] duration-300 motion-reduce:transition-none"
          />
        </svg>
        <Upload className="size-3 text-ink-2" aria-hidden="true" />
        <span className="sr-only">{formatPercent(status.percent)}</span>
      </Link>
    </Tooltip>
  );
}
