import { formatBytes, formatDuration } from "@/lib/format";
import { t } from "@/lib/i18n";
import type { InstallJob } from "@/lib/types";

/**
 * Download/verify/space presentation rules (#221/#222/#223).
 *
 * Kept pure so the Downloads row, the install dialog and the queue strip agree,
 * and so the "never a frozen 100 % download" rule is testable without React:
 * a `verifying` job has no speed, no ETA and an indeterminate bar even though
 * its bytes are complete.
 */

/** Safety margin the desktop adds to a peak estimate: 5 % + 2 GiB (#222). */
export const SPACE_MARGIN_PERCENT = 5;
export const SPACE_MARGIN_FLOOR_BYTES = 2 * 1024 ** 3;
/** Extracted-tree estimate when only the archive size is known (#222). */
export const UNKNOWN_EXTRACTION_PERCENT = 150;

/** True while the archive is being re-hashed before extraction (#223). */
export const isVerifyingJob = (job: InstallJob): boolean => job.phase === "verifying";

/** True while the job waits for its disk reservation (#222). */
export const isWaitingForSpaceJob = (job: InstallJob): boolean => job.phase === "waitingForSpace";

/** True while this archive downloads ahead of its turn (#221). */
export const isPrefetchingJob = (job: InstallJob): boolean => job.prefetching === true;

/**
 * Progress bar value. `null` means indeterminate, which is what a verifying or
 * finalizing job shows: the bytes are already complete and the remaining work
 * is a local read or a commit, so a 100 % bar would look frozen (#223/#225).
 */
export function jobProgressPercent(job: InstallJob): number | null {
  if (isVerifyingJob(job) || job.phase === "finalizing") return null;
  if (!job.bytesTotal || job.bytesTotal <= 0) return null;
  return Math.min(100, (job.bytesDone / job.bytesTotal) * 100);
}

/**
 * Rate shown on the row (#223). A verifying or extracting job reports 0: the
 * measured download rate is meaningless during a local read and a leftover
 * value would look like a stalled transfer.
 */
export function jobDisplaySpeed(job: InstallJob, smoothed?: number): number {
  if (job.phase !== "downloading") return 0;
  return smoothed ?? job.speedBps;
}

/** "Verifying archive (42.5 GiB of 96.3 GiB)" while the rehash runs. */
export function verifyingLabel(job: InstallJob): string | null {
  if (!isVerifyingJob(job)) return null;
  if (job.bytesTotal && job.bytesTotal > 0) {
    return t("Verifying archive ({done} of {total})", {
      done: formatBytes(job.bytesDone),
      total: formatBytes(job.bytesTotal),
    });
  }
  return t("Verifying archive");
}

/** "Verifying for 2 min" — the timing line the row shows instead of an ETA. */
export function verifyingElapsedLabel(elapsedMs: number): string {
  return t("Verifying for {duration}", {
    duration: formatDuration(Math.max(0, Math.round(elapsedMs / 1000))),
  });
}

/** The digest proof, once verification succeeded (#223). */
export function verifiedDigestLabel(job: InstallJob): string | null {
  if (!job.verifiedSha256) return null;
  return t("SHA-256 {digest}", { digest: job.verifiedSha256.slice(0, 16) });
}

/** The readable reason a job waits for room (#222). */
export function spaceWaitLabel(job: InstallJob): string | null {
  if (!isWaitingForSpaceJob(job)) return null;
  const space = job.space;
  if (!space) return t("Waiting for disk space");
  return t(
    "Waiting for disk space on {volume}: {needed} needed, {available} available after the other installs ({reserved} reserved, {free} free).",
    {
      volume: space.volume,
      needed: formatBytes(space.neededBytes),
      available: formatBytes(space.availableBytes),
      reserved: formatBytes(space.reservedBytes),
      free: formatBytes(space.freeBytes),
    },
  );
}

/** "Downloading ahead" while the previous install keeps the extractor (#221). */
export function prefetchLabel(job: InstallJob): string | null {
  if (!isPrefetchingJob(job) || job.phase !== "downloading") return null;
  return t("Downloading ahead of the install that is still unpacking");
}

export interface PeakEstimate {
  /** Archive bytes on the download volume. */
  archiveBytes: number;
  /** Extracted bytes, estimated at 1.5× when unknown. */
  extractedBytes: number;
  /** Safety margin. */
  marginBytes: number;
  /** Peak on a single volume: archive + extracted + margin. */
  peakBytes: number;
  /** True when the extracted size is an estimate, not a measured value. */
  estimated: boolean;
}

/**
 * Peak disk estimate shown before an install starts (#222).
 *
 * `archiveBytes` is the version size the server reports. The extracted tree is
 * unknown, so it is estimated at 1.5× — the same factor the desktop uses — and
 * flagged so the dialog can say "about".
 */
export function peakEstimate(archiveBytes: number, sameVolume = true): PeakEstimate {
  const archive = Math.max(0, archiveBytes);
  const extracted = Math.round((archive * UNKNOWN_EXTRACTION_PERCENT) / 100);
  // Mirrors `space::with_margin`: 5 % of the peak on that volume + 2 GiB.
  const raw = sameVolume ? archive + extracted : extracted;
  const margin = Math.round(raw / 100) * SPACE_MARGIN_PERCENT + SPACE_MARGIN_FLOOR_BYTES;
  return {
    archiveBytes: archive,
    extractedBytes: extracted,
    marginBytes: margin,
    peakBytes: raw + margin,
    estimated: true,
  };
}

/** "Peak disk use: 12.4 GiB (archive 4.1 GiB + extracted about 6.2 GiB + 2.0 GiB margin)". */
export function peakEstimateLabel(estimate: PeakEstimate, sameVolume = true): string {
  return sameVolume
    ? t(
        "Peak disk use on this volume: {peak} (archive {archive} + extracted about {extracted} + {margin} safety margin).",
        {
          peak: formatBytes(estimate.peakBytes),
          archive: formatBytes(estimate.archiveBytes),
          extracted: formatBytes(estimate.extractedBytes),
          margin: formatBytes(estimate.marginBytes),
        },
      )
    : t(
        "Peak disk use: {peak} on the install volume (extracted about {extracted} + {margin} margin); the {archive} archive goes to the download volume.",
        {
          peak: formatBytes(estimate.peakBytes),
          archive: formatBytes(estimate.archiveBytes),
          extracted: formatBytes(estimate.extractedBytes),
          margin: formatBytes(estimate.marginBytes),
        },
      );
}
