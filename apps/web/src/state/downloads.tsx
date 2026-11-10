import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, getApiBase, tokenStore } from "@/lib/api";
import { installBridge, installsAvailable } from "@/lib/install";
import { isNative, native } from "@/lib/native";
import { USING_MOCKS } from "@/lib/flags";
import { errorMessage } from "@/lib/http";
import type { InstallJob } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { formatEta } from "@/lib/format";

const ACTIVE_PHASES: ReadonlySet<InstallJob["phase"]> = new Set(["queued", "downloading", "extracting", "finalizing"]);

export const isActiveJob = (job: InstallJob): boolean => ACTIVE_PHASES.has(job.phase);

export interface StartInstallInput {
  gameId: number;
  versionId: number;
  title: string;
  version?: string | null;
  executable?: string | null;
}

export interface ResumeInfo {
  title: string;
  version?: string | null;
  executable?: string | null;
}

interface Aggregate {
  bytesDone: number;
  bytesTotal: number | null;
  percent: number | null;
  speedBps: number;
  eta: string | null;
}

interface DownloadsValue {
  available: boolean;
  jobs: InstallJob[];
  active: InstallJob[];
  history: InstallJob[];
  activeCount: number;
  aggregate: Aggregate | null;
  startInstall: (input: StartInstallInput) => Promise<InstallJob>;
  cancel: (jobId: string) => Promise<void>;
  resume: (job: InstallJob, info: ResumeInfo) => Promise<InstallJob | null>;
  clearHistory: () => Promise<void>;
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

  const active = useMemo(() => jobs.filter(isActiveJob), [jobs]);
  const history = useMemo(
    () => jobs.filter((job) => !isActiveJob(job) && !hidden.has(job.jobId)),
    [jobs, hidden],
  );

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
          title: `${titleFor(job)} installed`,
          description: "It is ready to play from the library.",
          tone: "success",
        });
      } else if (job.phase === "error" && !announced.current.has(job.jobId)) {
        announced.current.add(job.jobId);
        push({
          title: `${titleFor(job)} failed to install`,
          description: job.message ?? "Open Downloads to resume it.",
          tone: "danger",
        });
      }
    }
    if (installedChanged) {
      void queryClient.invalidateQueries({ queryKey: ["installed"] });
      void queryClient.invalidateQueries({ queryKey: ["games"] });
      void queryClient.invalidateQueries({ queryKey: ["game"] });
    }
  }, [jobs, available, titleFor, push, queryClient]);

  const aggregate = useMemo<Aggregate | null>(() => {
    if (active.length === 0) return null;
    let bytesDone = 0;
    let bytesTotal = 0;
    let speedBps = 0;
    let unknown = false;
    for (const job of active) {
      bytesDone += job.bytesDone;
      speedBps += job.speedBps;
      if (job.bytesTotal && job.bytesTotal > 0) bytesTotal += job.bytesTotal;
      else unknown = true;
    }
    const percent = !unknown && bytesTotal > 0 ? Math.min(100, (bytesDone / bytesTotal) * 100) : null;
    return {
      bytesDone,
      bytesTotal: unknown ? null : bytesTotal,
      percent,
      speedBps,
      eta: bytesTotal > 0 ? formatEta(bytesDone, bytesTotal, speedBps) : null,
    };
  }, [active]);

  const jobForGame = useCallback((gameId: number) => active.find((job) => job.gameId === gameId), [active]);
  const resumableForGame = useCallback(
    (gameId: number) =>
      jobs.find(
        (job) => job.gameId === gameId && (job.phase === "error" || job.phase === "interrupted" || job.phase === "cancelled"),
      ),
    [jobs],
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
        installDir: settings?.installDir || undefined,
        downloadDir: settings?.downloadDir || undefined,
        stream: settings?.streamInstall ?? false,
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
        push({ title: "Could not cancel the install", description: errorMessage(cause), tone: "danger" });
      }
    },
    [refresh, push],
  );

  const resume = useCallback(
    async (job: InstallJob, info: ResumeInfo) => {
      const existing = active.find((entry) => entry.gameId === job.gameId);
      if (existing) return existing;
      try {
        if (USING_MOCKS) {
          const next = await installBridge.resumeMock(job.jobId);
          await refresh();
          return next;
        }
        return await startInstall({
          gameId: job.gameId,
          versionId: job.versionId,
          title: info.title,
          version: info.version,
          executable: info.executable,
        });
      } catch (cause) {
        push({ title: "Could not resume the install", description: errorMessage(cause), tone: "danger" });
        return null;
      }
    },
    [active, refresh, startInstall, push],
  );

  const clearHistory = useCallback(async () => {
    const finished = history.filter((job) => job.phase === "done" || job.phase === "cancelled").map((job) => job.jobId);
    setHidden((current) => new Set([...current, ...finished]));
    if (USING_MOCKS) await installBridge.clearCompleted();
    await refresh();
  }, [history, refresh]);

  const openFolder = useCallback(
    async (path: string) => {
      try {
        await installBridge.openPath(path);
      } catch {
        push({ title: "Folder paths live on the desktop", description: path });
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
      startInstall,
      cancel,
      resume,
      clearHistory,
      openFolder,
      jobForGame,
      resumableForGame,
      titleFor,
    }),
    [available, jobs, active, history, aggregate, startInstall, cancel, resume, clearHistory, openFolder, jobForGame, resumableForGame, titleFor],
  );

  return <DownloadsContext.Provider value={value}>{children}</DownloadsContext.Provider>;
}

export function useDownloads(): DownloadsValue {
  const value = useContext(DownloadsContext);
  if (!value) throw new Error("useDownloads must be used inside DownloadsProvider");
  return value;
}
