import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, getApiBase, tokenStore } from "@/lib/api";
import { installBridge, installsAvailable } from "@/lib/install";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { planRetry } from "@/lib/install-retry";
import { moveInQueue, orderActiveQueue, type QueueMove } from "@/lib/install-queue";
import { visibleHistory } from "@/lib/install-history";
import type { InstallJob, InstallPhase, InstallResumeMode } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { formatDuration } from "@/lib/format";
import {
  SAMPLE_INTERVAL_MS,
  aggregateTransfer,
  nextEta,
  pushSample,
  sampleJob,
  type AggregateJobInput,
  type EtaSnapshot,
  type JobMeter,
  type SpeedPoint,
} from "@/lib/eta";
import { t } from "@/lib/i18n";

const ACTIVE_PHASES: ReadonlySet<InstallJob["phase"]> = new Set([
  "queued",
  "waitingForSpace",
  "downloading",
  "verifying",
  "extracting",
  "finalizing",
]);

/** Paused jobs stay in the queue and are listed with the active installs. */
export const isPausedJob = (job: InstallJob): boolean => job.phase === "paused";
export const isActiveJob = (job: InstallJob): boolean => ACTIVE_PHASES.has(job.phase) || isPausedJob(job);

const FAILED_PHASES: ReadonlySet<InstallJob["phase"]> = new Set(["error", "cancelled", "interrupted"]);

/**
 * A failed stream install with no per-entry checkpoint cannot resume from
 * bytes: the staging folder was discarded. Per-entry stream installs (#115)
 * persist completed entries, so they are resumable and keep the Resume button.
 * Use the persisted flags, never the English error message (#114).
 */
export const isStreamFailedJob = (job: InstallJob): boolean =>
  job.stream === true && job.resumable !== true && FAILED_PHASES.has(job.phase);

export interface StartInstallInput {
  gameId: number;
  versionId: number;
  title: string;
  version?: string | null;
  executable?: string | null;
  /** Named install location (#99); omitted uses the default. */
  locationId?: string | null;
  /**
   * Direct install root override (#145), used when the install dialog
   * preselects a detected drive that is not saved as a location yet.
   */
  installDir?: string | null;
  /** Named download location (#99); omitted uses the default or the install volume. */
  downloadLocationId?: string | null;
  /** Archive/extracted size estimate for the free-space guard (#99). */
  expectedBytes?: number | null;
  /** Overrides the stream-install setting for this job (e.g. retry as a normal download). */
  stream?: boolean;
}

export interface ResumeInfo {
  title: string;
  version?: string | null;
  executable?: string | null;
}

export interface Aggregate {
  bytesDone: number;
  bytesTotal: number | null;
  percent: number | null;
  /** Sum of the smoothed rates of jobs downloading right now. */
  speedBps: number;
  /** Display-ready ETA for the status strip; null while estimating/unknown. */
  eta: string | null;
  etaSeconds: number | null;
  totalKnown: boolean;
  etaKnown: boolean;
  estimating: boolean;
  downloading: number;
  /** The phase to narrate when no download is moving (extracting, …). */
  phase: InstallPhase | null;
  phaseElapsedMs: number | null;
}

/** Why the aggregate shows a phase instead of an ETA. */
const PHASE_ORDER: readonly InstallPhase[] = [
  "downloading",
  "verifying",
  "extracting",
  "finalizing",
  "queued",
  "waitingForSpace",
  "paused",
];

/** Sampling state rendered by the Downloads screen (#102). */
interface TransferState {
  meters: Record<string, JobMeter>;
  etaSeconds: number | null;
  eta: EtaSnapshot | null;
  history: SpeedPoint[];
  updatedAt: number;
}

const EMPTY_TRANSFER: TransferState = {
  meters: {},
  etaSeconds: null,
  eta: null,
  history: [],
  updatedAt: 0,
};

interface DownloadsValue {
  available: boolean;
  jobs: InstallJob[];
  active: InstallJob[];
  history: InstallJob[];
  activeCount: number;
  aggregate: Aggregate | null;
  /** Per-job smoothed speed/ETA, keyed by job id (#102). */
  meters: Record<string, JobMeter>;
  /** Aggregate speed samples for the transfer graph, oldest first (#102). */
  speedHistory: SpeedPoint[];
  /** Timestamp of the newest sample; the graph's right edge. */
  speedHistoryAt: number;
  startInstall: (input: StartInstallInput) => Promise<InstallJob>;
  cancel: (jobId: string) => Promise<void>;
  pause: (jobId: string) => Promise<void>;
  resume: (job: InstallJob, info: ResumeInfo) => Promise<InstallJob | null>;
  retryAsDownload: (job: InstallJob, info: ResumeInfo) => Promise<InstallJob | null>;
  retryStream: (job: InstallJob, info: ResumeInfo) => Promise<InstallJob | null>;
  /** Skip the current retry backoff of a waiting job (#212). */
  retryNow: (jobId: string) => Promise<void>;
  /** Re-check the disk reservation of a job parked on space (#222). */
  checkSpace: (jobId: string) => Promise<void>;
  /**
   * Move a waiting install in the queue (#202): `top`, `up`, `down` or an
   * explicit `position`. Optimistic, rolled back when the desktop refuses.
   */
  reorder: (jobId: string, move: QueueMove, position?: number) => Promise<void>;
  /** Delete one finished job from history; files and `.part` downloads stay. */
  remove: (jobId: string) => Promise<void>;
  /** Delete every finished job from history; resolves with the entries removed. */
  clearHistory: () => Promise<number>;
  openFolder: (path: string) => Promise<void>;
  jobForGame: (gameId: number) => InstallJob | undefined;
  resumableForGame: (gameId: number) => InstallJob | undefined;
  titleFor: (job: InstallJob) => string;
}

const DownloadsContext = createContext<DownloadsValue | null>(null);

export function DownloadsProvider({ children }: { children: ReactNode }) {
  const { user } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const available = installsAvailable();

  const [jobs, setJobs] = useState<InstallJob[]>([]);
  const [hidden, setHidden] = useState<Set<string>>(() => new Set());
  const [titles, setTitles] = useState<Record<string, string>>({});
  const announced = useRef(new Set<string>());
  const primed = useRef(false);

  // Sampling layer (#102): one meter per active job plus the aggregate speed
  // history the Downloads graph reads. Samples are capped at two minutes.
  const [transfer, setTransfer] = useState<TransferState>(EMPTY_TRANSFER);
  const transferRef = useRef<TransferState>(EMPTY_TRANSFER);
  const jobsRef = useRef<InstallJob[]>(jobs);
  jobsRef.current = jobs;

  const gamesQuery = useQuery({
    queryKey: ["games", "all"],
    queryFn: () => api.listGames({ limit: 200 }),
    enabled: available && Boolean(user),
    staleTime: 60_000,
  });

  const gamesById = useMemo(() => new Map((gamesQuery.data?.items ?? []).map((game) => [game.id, game])), [gamesQuery.data]);

  const refresh = useCallback(async () => {
    if (!available || !user) return;
    try {
      const list = await installBridge.list();
      setJobs(list);
    } catch {
      /* a transient IPC failure should not clear the list */
    }
  }, [available, user]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Mock jobs emit on every tick; native jobs arrive as events. Either way the
  // provider owns one list so the strip, rail badge and screens agree.
  useEffect(() => {
    if (!available || !user) return;
    const unsubscribe = installBridge.subscribe(() => void refresh());
    return unsubscribe;
  }, [available, user, refresh]);

  useEffect(() => {
    if (!available || !user || !isNative()) return;
    let disposed = false;
    const offs: Array<() => void> = [];
    const add = (pending: Promise<() => void>) => {
      void pending.then((off) => {
        if (disposed) off();
        else offs.push(off);
      });
    };
    add(native.onInstallProgress(() => void refresh()));
    add(native.onInstallDone(() => void refresh()));
    add(native.onInstallError(() => void refresh()));
    add(native.onInstallJobs((list) => setJobs(list)));
    return () => {
      disposed = true;
      for (const off of offs) off();
    };
  }, [available, user, refresh]);

  // The running install stays pinned at the top, then the waiting queue in
  // start order (#202); the desktop confirms the same order.
  const active = useMemo(() => orderActiveQueue(jobs.filter(isActiveJob)), [jobs]);
  // One row per game (#225): a game with an active job has no history row, a
  // superseded attempt is hidden, and older terminal rows for the same game
  // collapse into the one worth showing (done, else newest).
  const history = useMemo(
    () => visibleHistory(active, jobs.filter((job) => !isActiveJob(job) && !hidden.has(job.jobId))),
    [active, jobs, hidden],
  );

  /**
   * Fold one observation per active job into the sampling state. The rolling
   * median and ETA gates live in `lib/eta.ts`; this only wires them to the
   * current job list.
   */
  const sampleTransfers = useCallback(() => {
    const at = Date.now();
    const activeJobs = jobsRef.current.filter(isActiveJob);
    if (activeJobs.length === 0) {
      if (transferRef.current !== EMPTY_TRANSFER) {
        transferRef.current = EMPTY_TRANSFER;
        setTransfer(EMPTY_TRANSFER);
      }
      return;
    }
    const previous = transferRef.current;
    const meters: Record<string, JobMeter> = {};
    const inputs: AggregateJobInput[] = [];
    for (const job of activeJobs) {
      const meter = sampleJob(previous.meters[job.jobId] ?? null, job, at);
      meters[job.jobId] = meter;
      inputs.push({
        phase: job.phase,
        bytesDone: job.bytesDone,
        bytesTotal: job.bytesTotal,
        speedBps: meter.speedBps,
        estimating: meter.estimating,
      });
    }
    const aggregateSample = aggregateTransfer(inputs);
    const eta = nextEta(previous.eta, aggregateSample.etaCandidate, at);
    const next: TransferState = {
      meters,
      etaSeconds: eta?.seconds ?? null,
      eta,
      history: pushSample(previous.history, { at, speedBps: aggregateSample.speedBps }),
      updatedAt: at,
    };
    transferRef.current = next;
    setTransfer(next);
  }, []);

  useEffect(() => {
    if (!available || !user) {
      transferRef.current = EMPTY_TRANSFER;
      setTransfer(EMPTY_TRANSFER);
      return;
    }
    sampleTransfers();
    if (active.length === 0) return;
    const timer = setInterval(sampleTransfers, SAMPLE_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [available, user, active.length, sampleTransfers]);

  useEffect(() => {
    if (!available || !user || active.length === 0) return;
    const timer = setInterval(() => void refresh(), 1000);
    return () => clearInterval(timer);
  }, [available, user, active.length, refresh]);

  useEffect(() => {
    if (!available) return;
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [available, refresh]);

  // #212: the desktop owns the retry timers, so it needs the OS reachability.
  // Report once on mount, then on every transition; while offline a retry
  // parks instead of counting a failed attempt.
  useEffect(() => {
    if (!isNative()) return;
    const report = () => {
      void native.setDownloadNetworkState(navigator.onLine);
    };
    report();
    window.addEventListener("online", report);
    window.addEventListener("offline", report);
    return () => {
      window.removeEventListener("online", report);
      window.removeEventListener("offline", report);
    };
  }, []);

  const titleFor = useCallback(
    (job: InstallJob) => titles[job.jobId] ?? gamesById.get(job.gameId)?.title ?? `Game ${job.gameId}`,
    [titles, gamesById],
  );

  // Announce only transitions that happen while the app is open: seed the
  // first load silently so persisted history does not toast on launch.
  useEffect(() => {
    if (primed.current || jobs.length === 0) return;
    for (const job of jobs) if (!isActiveJob(job)) announced.current.add(job.jobId);
    primed.current = true;
  }, [jobs]);

  useEffect(() => {
    if (!available) return;
    let installedChanged = false;
    for (const job of jobs) {
      if (job.phase === "done" && !announced.current.has(job.jobId)) {
        announced.current.add(job.jobId);
        installedChanged = true;
        push({
          title: t("{title} installed", { title: titleFor(job) }),
          description: t("It is ready to play from the library."),
          tone: "success",
        });
      } else if (job.phase === "error" && !announced.current.has(job.jobId)) {
        announced.current.add(job.jobId);
        push({
          title: t("{title} failed to install", { title: titleFor(job) }),
          description: job.message ?? t("Open Downloads to resume it."),
          tone: "danger",
        });
      }
    }
    if (installedChanged) {
      void queryClient.invalidateQueries({ queryKey: ["installed"] });
      void queryClient.invalidateQueries({ queryKey: ["recent-games"] });
      void queryClient.invalidateQueries({ queryKey: ["game-updates"] });
      void queryClient.invalidateQueries({ queryKey: ["games"] });
      void queryClient.invalidateQueries({ queryKey: ["game"] });
      // A finished install has a new folder to scan for the executable picker (#143).
      void queryClient.invalidateQueries({ queryKey: ["game-executables"] });
    }
  }, [jobs, available, titleFor, push, queryClient]);

  const aggregate = useMemo<Aggregate | null>(() => {
    if (active.length === 0) return null;
    const inputs: AggregateJobInput[] = active.map((job) => {
      const meter = transfer.meters[job.jobId];
      return {
        phase: job.phase,
        bytesDone: job.bytesDone,
        bytesTotal: job.bytesTotal,
        speedBps: meter?.speedBps ?? (job.phase === "downloading" ? job.speedBps : 0),
        estimating: meter?.estimating ?? job.phase === "downloading",
      };
    });
    const sample = aggregateTransfer(inputs);
    // Paused jobs keep their progress but never drive the phase narration.
    const running = active.filter((job) => job.phase !== "paused");
    const phase = PHASE_ORDER.find((candidate) => running.some((job) => job.phase === candidate)) ?? null;
    let phaseElapsedMs: number | null = null;
    if (phase !== null) {
      phaseElapsedMs = 0;
      for (const job of running) {
        if (job.phase !== phase) continue;
        phaseElapsedMs = Math.max(phaseElapsedMs, transfer.meters[job.jobId]?.elapsedMs ?? 0);
      }
    }
    return {
      bytesDone: sample.bytesDone,
      bytesTotal: sample.bytesTotal,
      percent: sample.percent,
      speedBps: sample.speedBps,
      eta: transfer.etaSeconds !== null ? formatDuration(transfer.etaSeconds) : null,
      etaSeconds: transfer.etaSeconds,
      totalKnown: sample.totalKnown,
      etaKnown: sample.etaKnown,
      estimating: sample.estimating,
      downloading: sample.downloading,
      phase,
      phaseElapsedMs,
    };
  }, [active, transfer]);

  // Active job first, then the one history row the list shows for the game
  // (#225). Because `history` is deduped, a finished game never offers a
  // Resume button for an older failed attempt, and a game with a running
  // install has no resumable row either.
  const jobForGame = useCallback((gameId: number) => active.find((job) => job.gameId === gameId), [active]);
  const resumableForGame = useCallback(
    (gameId: number) => history.find((job) => job.gameId === gameId && FAILED_PHASES.has(job.phase)),
    [history],
  );

  const startInstall = useCallback(
    async (input: StartInstallInput): Promise<InstallJob> => {
      const existing = active.find((job) => job.gameId === input.gameId);
      if (existing) return existing;

      const settings = await installBridge.settings();
      const job = await installBridge.start({
        ...input,
        serverUrl: getApiBase(),
        token: tokenStore.get() ?? "",
        installDir: input.installDir || settings?.installDir || undefined,
        downloadDir: settings?.downloadDir || undefined,
        locationId: input.locationId ?? undefined,
        downloadLocationId: input.downloadLocationId ?? undefined,
        expectedBytes: input.expectedBytes ?? undefined,
        stream: input.stream ?? settings?.streamInstall ?? false,
      });
      setTitles((current) => ({ ...current, [job.jobId]: input.title }));
      setJobs((current) => [job, ...current.filter((entry) => entry.jobId !== job.jobId)]);
      announced.current.delete(job.jobId);
      return job;
    },
    [active],
  );

  const cancel = useCallback(
    async (jobId: string) => {
      try {
        await installBridge.cancel(jobId);
        await refresh();
      } catch (cause) {
        push({ title: t("Could not cancel the install"), description: errorMessage(cause), tone: "danger" });
      }
    },
    [refresh, push],
  );

  const pause = useCallback(
    async (jobId: string) => {
      try {
        await installBridge.pause(jobId);
        await refresh();
      } catch (cause) {
        push({ title: t("Could not pause the install"), description: errorMessage(cause), tone: "danger" });
      }
    },
    [refresh, push],
  );

  /**
   * Every retry resumes the existing job (#205/#208): the desktop rebuilds the
   * worker from the persisted parameters and continues at the checkpoint.
   * `mode` only switches the transport; it never creates a new row.
   */
  const runRetry = useCallback(
    async (job: InstallJob, mode: InstallResumeMode | null, failureTitle: string): Promise<InstallJob | null> => {
      const plan = planRetry(job, active, mode);
      if (plan.action === "focus") {
        return active.find((entry) => entry.jobId === plan.jobId) ?? null;
      }
      try {
        const next = await installBridge.resume(plan.jobId, plan.mode);
        await refresh();
        return next;
      } catch (cause) {
        push({ title: failureTitle, description: errorMessage(cause), tone: "danger" });
        return null;
      }
    },
    [active, refresh, push],
  );

  const resume = useCallback(
    (job: InstallJob, _info: ResumeInfo) => runRetry(job, null, t("Could not resume the install")),
    [runRetry],
  );

  const retryAsDownload = useCallback(
    (job: InstallJob, _info: ResumeInfo): Promise<InstallJob | null> =>
      runRetry(job, "download", t("Could not retry the install as a download")),
    [runRetry],
  );

  const retryStream = useCallback(
    (job: InstallJob, _info: ResumeInfo): Promise<InstallJob | null> =>
      runRetry(job, "stream", t("Could not retry the stream install")),
    [runRetry],
  );

  /** Re-evaluate the reservation of a `waitingForSpace` job (#222). */
  const checkSpace = useCallback(
    async (jobId: string) => {
      try {
        const next = await installBridge.checkSpace(jobId);
        setJobs((current) => current.map((job) => (job.jobId === next.jobId ? next : job)));
        await refresh();
      } catch (cause) {
        push({ title: t("Could not check the disk space"), description: errorMessage(cause), tone: "danger" });
      }
    },
    [refresh, push],
  );

  /** Skip the wait, not the job: same id, same offset, same attempt budget. */
  const retryNow = useCallback(
    async (jobId: string) => {
      try {
        const next = await installBridge.retryNow(jobId);
        setJobs((current) => current.map((job) => (job.jobId === next.jobId ? next : job)));
        await refresh();
      } catch (cause) {
        push({ title: t("Could not retry now"), description: errorMessage(cause), tone: "danger" });
      }
    },
    [refresh, push],
  );

  /**
   * Reorder the queue (#202). The local list moves first so the buttons react
   * immediately; the desktop's confirmed order replaces it, and any refusal
   * (a running job, a stale queue) restores the previous list.
   */
  const reorder = useCallback(
    async (jobId: string, move: QueueMove, position?: number) => {
      const previous = jobsRef.current;
      let optimistic: InstallJob[];
      try {
        optimistic = moveInQueue(previous, jobId, move, position);
      } catch (cause) {
        push({ title: t("Could not reorder the queue"), description: errorMessage(cause), tone: "danger" });
        return;
      }
      setJobs(optimistic);
      try {
        const confirmed = await installBridge.reorder(jobId, move, position ?? null);
        setJobs(confirmed);
      } catch (cause) {
        setJobs(previous);
        push({ title: t("Could not reorder the queue"), description: errorMessage(cause), tone: "danger" });
      }
    },
    [push],
  );

  const remove = useCallback(
    async (jobId: string) => {
      try {
        // Persisted first: the desktop deletes the SQLite row and never the
        // install folder or `.part` file.
        await installBridge.remove(jobId);
      } catch (cause) {
        push({ title: t("Could not remove the download from history"), description: errorMessage(cause), tone: "danger" });
        return;
      }
      // Drop it locally right away so the row disappears without waiting for
      // the next poll; the deletion already happened in the database.
      setJobs((current) => current.filter((job) => job.jobId !== jobId));
      setHidden((current) => {
        if (!current.has(jobId)) return current;
        const next = new Set(current);
        next.delete(jobId);
        return next;
      });
      setTitles((current) => {
        if (!(jobId in current)) return current;
        const next = { ...current };
        delete next[jobId];
        return next;
      });
      announced.current.delete(jobId);
    },
    [push],
  );

  const clearHistory = useCallback(async () => {
    try {
      // Persisted: every terminal row goes from the desktop database, so the
      // history stays empty after a restart instead of reappearing.
      const cleared = await installBridge.clearCompleted();
      if (cleared > 0) setHidden(new Set());
      await refresh();
      return cleared;
    } catch (cause) {
      // Keep the history as-is; the next list poll restores nothing that was
      // not already there.
      push({ title: t("Could not clear the download history"), description: errorMessage(cause), tone: "danger" });
      return 0;
    }
  }, [refresh, push]);

  const openFolder = useCallback(
    async (path: string) => {
      try {
        await installBridge.openPath(path);
      } catch {
        push({ title: t("Folder paths live on the desktop"), description: path });
      }
    },
    [push],
  );

  const value = useMemo<DownloadsValue>(
    () => ({
      available,
      jobs,
      active,
      history,
      activeCount: active.length,
      aggregate,
      meters: transfer.meters,
      speedHistory: transfer.history,
      speedHistoryAt: transfer.updatedAt,
      startInstall,
      cancel,
      pause,
      resume,
      retryAsDownload,
      retryStream,
      retryNow,
      checkSpace,
      reorder,
      remove,
      clearHistory,
      openFolder,
      jobForGame,
      resumableForGame,
      titleFor,
    }),
    [available, jobs, active, history, aggregate, transfer, startInstall, cancel, pause, resume, retryAsDownload, retryStream, retryNow, checkSpace, reorder, remove, clearHistory, openFolder, jobForGame, resumableForGame, titleFor],
  );

  return <DownloadsContext.Provider value={value}>{children}</DownloadsContext.Provider>;
}

export function useDownloads(): DownloadsValue {
  const value = useContext(DownloadsContext);
  if (!value) throw new Error("useDownloads must be used inside DownloadsProvider");
  return value;
}
