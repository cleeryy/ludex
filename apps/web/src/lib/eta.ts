import type { InstallPhase } from "@/lib/types";

/**
 * Client-side transfer sampling for the Downloads screen (#102).
 *
 * The backend reports `speedBps` as an instantaneous rate over short windows,
 * so the raw value flickers. The provider samples every job once per second,
 * keeps a two-minute ring buffer and drives the UI from a rolling median over
 * the last few seconds instead. Everything in this module is pure so the
 * smoothing and ETA gates are unit-tested with an explicit clock.
 */

/** Raw observation of one install job. */
export interface TransferSample {
  at: number;
  bytesDone: number;
  speedBps: number;
  phase: InstallPhase;
}

/** One point of the aggregate speed graph. */
export interface SpeedPoint {
  at: number;
  speedBps: number;
}

/** The provider samples jobs this often. */
export const SAMPLE_INTERVAL_MS = 1_000;
/** 120 samples at 1 Hz span the two minutes the graph shows. */
export const MAX_SAMPLES = 120;
/** Older samples do not contribute to the smoothed speed. */
export const SMOOTHING_WINDOW_MS = 12_000;
/** The transfer graph shows the last two minutes. */
export const HISTORY_WINDOW_MS = 120_000;
/** A displayed ETA never changes more than once per second. */
export const ETA_MIN_INTERVAL_MS = 1_000;
/** Ignore sub-2 s wobble in the displayed ETA. */
export const ETA_MIN_SECONDS = 2;
/** Ignore sub-10% wobble in the displayed ETA. */
export const ETA_MIN_RELATIVE = 0.1;
/** A download needs a few samples before its rate means anything. */
export const MIN_SAMPLES_FOR_ETA = 3;

/** Keep only the newest `max` entries of a ring buffer. */
export function capSamples<T>(samples: T[], max = MAX_SAMPLES): T[] {
  return samples.length > max ? samples.slice(samples.length - max) : samples;
}

/**
 * Append a sample to a ring buffer. Repeated ticks in the same millisecond
 * replace the last sample instead of growing the buffer; out-of-order ticks
 * are ignored.
 */
export function pushSample<T extends { at: number }>(samples: T[], sample: T, max = MAX_SAMPLES): T[] {
  const last = samples[samples.length - 1];
  if (last && sample.at < last.at) return samples;
  if (last && sample.at === last.at) return capSamples([...samples.slice(0, -1), sample], max);
  return capSamples([...samples, sample], max);
}

/** Samples inside the window ending at `at`, oldest first. */
export function samplesInWindow<T extends { at: number }>(
  samples: T[],
  at: number,
  windowMs = SMOOTHING_WINDOW_MS,
): T[] {
  const cutoff = at - windowMs;
  return samples.filter((sample) => sample.at > cutoff && sample.at <= at);
}

/** Middle value; the mean of the two middle values for even-length input. */
export function median(values: number[]): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

/**
 * Smoothed rate for display. The median over the last 10–15 s ignores the
 * single-sample spikes the backend's short windows produce.
 */
export function smoothedSpeed(samples: TransferSample[], at: number, windowMs = SMOOTHING_WINDOW_MS): number {
  return median(samplesInWindow(samples, at, windowMs).map((sample) => sample.speedBps));
}

/** Seconds left at a steady rate; null when there is nothing to estimate. */
export function etaSeconds(remainingBytes: number, speedBps: number): number | null {
  if (!Number.isFinite(remainingBytes) || !Number.isFinite(speedBps)) return null;
  if (remainingBytes <= 0 || speedBps <= 0) return null;
  return remainingBytes / speedBps;
}

/** The last ETA handed to the UI, with the time it was published. */
export interface EtaSnapshot {
  seconds: number;
  at: number;
}

/** A new estimate is material when it moves by >2 s or >10%. */
export function isMaterialEtaChange(previous: number, next: number): boolean {
  if (!Number.isFinite(previous) || !Number.isFinite(next)) return true;
  return Math.abs(next - previous) > ETA_MIN_SECONDS || Math.abs(next - previous) > Math.abs(previous) * ETA_MIN_RELATIVE;
}

/**
 * Gate a freshly computed ETA: at most one update per second, and only when
 * the change is visible. A null candidate clears the estimate immediately
 * (phase change, stall, unknown total).
 */
export function nextEta(previous: EtaSnapshot | null, candidate: number | null, at: number): EtaSnapshot | null {
  if (candidate === null || !Number.isFinite(candidate) || candidate <= 0) return null;
  if (!previous) return { seconds: candidate, at };
  if (at - previous.at < ETA_MIN_INTERVAL_MS) return previous;
  if (!isMaterialEtaChange(previous.seconds, candidate)) return previous;
  return { seconds: candidate, at };
}

/** The fields of an install job the sampler needs. */
export interface JobView {
  jobId: string;
  phase: InstallPhase;
  bytesDone: number;
  bytesTotal: number | null;
  speedBps: number;
}

/** Display state derived from one job's samples. */
export interface JobMeter {
  jobId: string;
  phase: InstallPhase;
  phaseStartedAt: number;
  samples: TransferSample[];
  /** Rolling-median rate; 0 outside the downloading phase. */
  speedBps: number;
  /** Gated display ETA in seconds, null while estimating or unknown. */
  etaSeconds: number | null;
  /** When `etaSeconds` was published, for the materiality gate. */
  etaAt: number | null;
  /** First samples (or a stall) have no reliable rate yet. */
  estimating: boolean;
  /** Time spent in the current phase. */
  elapsedMs: number;
}

/**
 * Fold one job observation into its meter. New jobs, resumes, pauses and
 * phase changes restart the sample window so a new phase never inherits the
 * previous phase's rate or ETA.
 */
export function sampleJob(previous: JobMeter | null, job: JobView, at: number): JobMeter {
  const phaseChanged = previous === null || previous.phase !== job.phase;
  const phaseStartedAt = phaseChanged ? at : previous.phaseStartedAt;
  const samples = pushSample(phaseChanged ? [] : previous.samples, {
    at,
    bytesDone: job.bytesDone,
    speedBps: job.speedBps,
    phase: job.phase,
  });
  const window = samplesInWindow(samples, at);
  const downloading = job.phase === "downloading";
  const speedBps = downloading ? median(window.map((sample) => sample.speedBps)) : 0;
  const remaining = job.bytesTotal !== null ? job.bytesTotal - job.bytesDone : null;
  const enoughSamples = window.length >= MIN_SAMPLES_FOR_ETA;
  const candidate = downloading && enoughSamples && remaining !== null ? etaSeconds(remaining, speedBps) : null;
  const previousEta =
    !phaseChanged && previous.etaSeconds !== null && previous.etaAt !== null
      ? { seconds: previous.etaSeconds, at: previous.etaAt }
      : null;
  const eta = nextEta(previousEta, candidate, at);
  return {
    jobId: job.jobId,
    phase: job.phase,
    phaseStartedAt,
    samples,
    speedBps,
    etaSeconds: eta?.seconds ?? null,
    etaAt: eta?.at ?? null,
    estimating: downloading && (!enoughSamples || speedBps <= 0),
    elapsedMs: at - phaseStartedAt,
  };
}

/** The fields of an install job the aggregate needs. */
export interface AggregateJobInput {
  phase: InstallPhase;
  bytesDone: number;
  bytesTotal: number | null;
  speedBps: number;
  estimating: boolean;
}

/** Totals across every listed job. */
export interface AggregateTransfer {
  bytesDone: number;
  bytesTotal: number | null;
  percent: number | null;
  /** Sum of the smoothed rates of jobs that are downloading right now. */
  speedBps: number;
  /** Every active job reports a total size, so the overall percent is honest. */
  totalKnown: boolean;
  /** Every downloading job reports a total, so an aggregate ETA is honest. */
  etaKnown: boolean;
  /** A running download has no usable rate yet. */
  estimating: boolean;
  /** Jobs currently in the downloading phase; paused jobs do not count. */
  downloading: number;
  /** `remaining bytes / summed speed`. Never a sum of per-job ETAs. */
  etaCandidate: number | null;
}

/**
 * Aggregate every active job. Paused jobs keep their progress but contribute
 * neither speed nor ETA; an aggregate ETA is only produced when every
 * downloading job knows its total.
 */
export function aggregateTransfer(jobs: AggregateJobInput[]): AggregateTransfer {
  const downloading = jobs.filter((job) => job.phase === "downloading");
  const bytesDone = jobs.reduce((sum, job) => sum + job.bytesDone, 0);
  const totalKnown = jobs.every((job) => job.bytesTotal !== null && job.bytesTotal > 0);
  const bytesTotal = totalKnown ? jobs.reduce((sum, job) => sum + (job.bytesTotal ?? 0), 0) : null;
  const percent = bytesTotal !== null && bytesTotal > 0 ? Math.min(100, (bytesDone / bytesTotal) * 100) : null;
  const speedBps = downloading.reduce((sum, job) => sum + job.speedBps, 0);
  const etaKnown = downloading.every((job) => job.bytesTotal !== null && job.bytesTotal > 0);
  const remaining = downloading.reduce((sum, job) => sum + Math.max(0, (job.bytesTotal ?? 0) - job.bytesDone), 0);
  const etaCandidate = etaKnown ? etaSeconds(remaining, speedBps) : null;
  const estimating = downloading.length > 0 && (downloading.some((job) => job.estimating) || speedBps <= 0);
  return {
    bytesDone,
    bytesTotal,
    percent,
    speedBps,
    totalKnown,
    etaKnown,
    estimating,
    downloading: downloading.length,
    etaCandidate,
  };
}
