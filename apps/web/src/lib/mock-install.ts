import { DEMO_GAMES } from "@/data/demo";
import { USING_MOCKS } from "@/lib/flags";
import { slugify } from "@/lib/art";
import type {
  DiskUsage,
  InstallGameArgs,
  InstallJob,
  InstalledGame,
  LocalSettings,
  RepairReport,
  VerifyReport,
} from "@/lib/types";

/**
 * Browser-side stand-in for the desktop install manager. Only used when the
 * mock adapter is on, so the Downloads screen, the global strip and the
 * Installing states can be reviewed without Tauri. Jobs advance on a timer.
 */

const MB = 1024 ** 2;
const GB = 1024 ** 3;
const ACTIVE: ReadonlySet<InstallJob["phase"]> = new Set(["queued", "downloading", "extracting", "finalizing"]);
/** Phases that may leave Downloads history (#100); mirrors the SQLite check. */
const TERMINAL: ReadonlySet<InstallJob["phase"]> = new Set(["done", "cancelled", "error", "interrupted"]);

const now = () => Date.now();
const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

interface JobMeta {
  title: string;
  version: string | null;
}

const meta = new Map<string, JobMeta>();
const phaseEnds = new Map<string, number>();
const listeners = new Set<() => void>();

function emit() {
  for (const listener of listeners) listener();
}

function sizeOf(gameId: number, versionId: number): number {
  const game = DEMO_GAMES.find((entry) => entry.id === gameId);
  const version = game?.versions.find((entry) => entry.id === versionId);
  return version?.size ?? 8 * GB;
}

function pathFor(title: string, version: string | null): string {
  return `/home/mara/Games/${slugify(title)}/${version ?? "1.0"}`;
}

let installed: InstalledGame[] = [
  {
    gameId: 14,
    versionId: 141,
    title: "Outer Wilds",
    version: "1.1.15",
    installPath: "/home/mara/Games/outer-wilds/1.1.15",
    executable: "OuterWilds.exe",
    sizeBytes: Math.round(8.7 * GB),
    installedAt: "2026-11-02T19:40:00Z",
    lastLaunchedAt: "2026-05-02T23:00:00Z",
  },
  {
    gameId: 7,
    versionId: 71,
    title: "It Takes Two",
    version: "1.0.0.2",
    installPath: "/home/mara/Games/it-takes-two/1.0.0.2",
    executable: "ItTakesTwo.exe",
    sizeBytes: Math.round(43.7 * GB),
    installedAt: "2026-11-05T18:12:00Z",
    lastLaunchedAt: "2026-11-06T20:00:00Z",
  },
];

function job(
  jobId: string,
  gameId: number,
  versionId: number,
  phase: InstallJob["phase"],
  bytesDone: number,
  bytesTotal: number,
  speedBps: number,
  title: string,
  version: string,
  message: string | null,
  installPath: string | null = null,
  stream = false,
): InstallJob {
  meta.set(jobId, { title, version });
  return { jobId, gameId, versionId, phase, bytesDone, bytesTotal, speedBps, message, installPath, stream };
}

let jobs: InstallJob[] = [
  job(
    "job_forza",
    5,
    51,
    "downloading",
    96 * GB,
    Math.round(132.4 * GB),
    240 * MB,
    "Forza Horizon 6",
    "1.0.118",
    "Downloading archive",
    null,
    true,
  ),
  job("job_ln3", 10, 101, "queued", 0, Math.round(18.5 * GB), 0, "Little Nightmares III", "1.0.3", "Waiting for a slot"),
  job(
    "job_spidey",
    11,
    111,
    "error",
    0,
    Math.round(86.3 * GB),
    0,
    "Marvel's Spider-Man 2",
    "1.2.1",
    "Not enough free space on the install volume",
  ),
  job(
    "job_bodycam",
    16,
    161,
    "error",
    Math.round(40.7 * 1024 ** 3),
    Math.round(55.9 * 1024 ** 3),
    0,
    "Bodycam",
    "1.0",
    "The stream was interrupted while writing pakchunk47-Windows.pak. Stream installs are not resumable.",
    null,
    true,
  ),
  job(
    "job_hitman",
    6,
    61,
    "interrupted",
    21.4 * GB,
    Math.round(64.2 * GB),
    0,
    "HITMAN 3",
    "3.210",
    "The app closed while extracting. The partial archive is kept.",
  ),
  job(
    "job_outer",
    14,
    141,
    "done",
    Math.round(8.7 * GB),
    Math.round(8.7 * GB),
    0,
    "Outer Wilds",
    "1.1.15",
    null,
    "/home/mara/Games/outer-wilds/1.1.15",
  ),
];

phaseEnds.set("job_ln3", now() + 5000);

function addInstalled(job: InstallJob) {
  if (installed.some((entry) => entry.gameId === job.gameId)) return;
  const info = meta.get(job.jobId);
  const title = info?.title ?? `Game ${job.gameId}`;
  const version = info?.version ?? null;
  installed = [
    ...installed,
    {
      gameId: job.gameId,
      versionId: job.versionId,
      title,
      version,
      installPath: job.installPath ?? pathFor(title, version),
      executable: null,
      sizeBytes: job.bytesTotal ?? 0,
      installedAt: new Date().toISOString(),
      lastLaunchedAt: null,
    },
  ];
}

if (USING_MOCKS) setInterval(() => {
  let changed = false;
  const time = now();

  for (const current of jobs) {
    if (current.phase === "queued") {
      const end = phaseEnds.get(current.jobId) ?? time;
      if (time >= end) {
        current.phase = "downloading";
        current.speedBps = 52 * MB + Math.round(Math.random() * 18 * MB);
        current.message = "Downloading archive";
        changed = true;
      }
    } else if (current.phase === "downloading") {
      const total = current.bytesTotal ?? 0;
      current.speedBps = 48 * MB + Math.round(Math.random() * 26 * MB);
      current.bytesDone = Math.min(total, current.bytesDone + current.speedBps);
      if (total > 0 && current.bytesDone >= total) {
        current.phase = "extracting";
        current.speedBps = 0;
        current.message = "Extracting files";
        phaseEnds.set(current.jobId, time + 4000);
      }
      changed = true;
    } else if (current.phase === "extracting") {
      if (time >= (phaseEnds.get(current.jobId) ?? time)) {
        current.phase = "finalizing";
        current.message = "Writing the desktop index";
        phaseEnds.set(current.jobId, time + 2500);
      }
      changed = true;
    } else if (current.phase === "finalizing") {
      if (time >= (phaseEnds.get(current.jobId) ?? time)) {
        current.phase = "done";
        current.message = null;
        current.installPath = pathFor(meta.get(current.jobId)?.title ?? "game", meta.get(current.jobId)?.version ?? null);
        addInstalled(current);
      }
      changed = true;
    }
  }

  if (changed) emit();
}, 1000);

const DEFAULT_SETTINGS: LocalSettings = {
  mode: "remote",
  serverUrl: "https://ludex.mara.lan",
  token: null,
  gamesDir: "/home/mara/Games",
  installDir: "/home/mara/Games/Ludex",
  downloadDir: "/home/mara/Downloads/Ludex",
  serverPort: 0,
  streamInstall: true,
  keepArchives: false,
  archivePasswords: [],
  bandwidthLimitKib: 0,
  closeToTray: true,
  updateChannel: "stable",
  checkUpdatesOnStartup: true,
  lastUpdateCheckAt: null,
  discordPresence: false,
  discordClientId: "",
  installerSilentArgs: [],
  installerTimeoutSecs: 600,
  steamRoot: "",
};

function loadSettings(): LocalSettings {
  try {
    const raw = localStorage.getItem("ludex.demo.desktop");
    if (raw) return { ...DEFAULT_SETTINGS, ...(JSON.parse(raw) as Partial<LocalSettings>) };
  } catch {
    /* ignore corrupt demo state */
  }
  return { ...DEFAULT_SETTINGS };
}

const libraryEmpty = () => localStorage.getItem("ludex.demo.empty") === "1";

export const mockInstalls = {
  available: true,

  list(): Promise<InstallJob[]> {
    return Promise.resolve(libraryEmpty() ? [] : clone(jobs));
  },

  installed(): Promise<InstalledGame[]> {
    return Promise.resolve(libraryEmpty() ? [] : clone(installed));
  },

  start(args: InstallGameArgs): Promise<InstallJob> {
    const existing = jobs.find((entry) => entry.gameId === args.gameId && ACTIVE.has(entry.phase));
    if (existing) return Promise.resolve(clone(existing));

    const id = `job_${Math.random().toString(16).slice(2, 8)}`;
    meta.set(id, { title: args.title, version: args.version ?? null });
    const next: InstallJob = {
      jobId: id,
      gameId: args.gameId,
      versionId: args.versionId,
      phase: "queued",
      bytesDone: 0,
      bytesTotal: sizeOf(args.gameId, args.versionId),
      speedBps: 0,
      message: "Queued",
      installPath: null,
      stream: args.stream ?? false,
    };
    phaseEnds.set(id, now() + 1500);
    jobs = [next, ...jobs];
    emit();
    return Promise.resolve(clone(next));
  },

  cancel(jobId: string): Promise<void> {
    const target = jobs.find((entry) => entry.jobId === jobId);
    if (target && ACTIVE.has(target.phase)) {
      target.phase = "cancelled";
      target.speedBps = 0;
      target.message = "Cancelled";
      emit();
    }
    return Promise.resolve();
  },

  pause(jobId: string): Promise<InstallJob> {
    const target = jobs.find((entry) => entry.jobId === jobId);
    if (!target) return Promise.reject(new Error("Unknown install job"));
    if (target.phase === "queued" || target.phase === "downloading") {
      target.phase = "paused";
      target.speedBps = 0;
      target.message = "Paused";
      emit();
    }
    return Promise.resolve(clone(target));
  },

  resume(jobId: string): Promise<InstallJob | null> {
    const target = jobs.find((entry) => entry.jobId === jobId);
    if (!target) return Promise.resolve(null);
    if (!ACTIVE.has(target.phase)) {
      const total = target.bytesTotal ?? 0;
      if (total > 0 && target.bytesDone >= total) {
        target.phase = "extracting";
        target.message = "Extracting files";
        phaseEnds.set(jobId, now() + 3000);
      } else {
        target.phase = "downloading";
        target.speedBps = 52 * MB;
        target.message = "Resuming";
        phaseEnds.delete(jobId);
      }
      emit();
    }
    return Promise.resolve(clone(target));
  },

  remove(jobId: string): Promise<void> {
    const target = jobs.find((entry) => entry.jobId === jobId);
    if (!target) return Promise.reject(new Error("Unknown install job"));
    if (!TERMINAL.has(target.phase)) {
      return Promise.reject(
        new Error(
          target.phase === "paused"
            ? "This install is paused. Cancel it before removing it from history."
            : "This install is still running. Cancel it before removing it from history.",
        ),
      );
    }
    jobs = jobs.filter((entry) => entry.jobId !== jobId);
    meta.delete(jobId);
    phaseEnds.delete(jobId);
    emit();
    return Promise.resolve();
  },

  clearCompleted(): Promise<number> {
    const before = jobs.length;
    jobs = jobs.filter((entry) => !TERMINAL.has(entry.phase));
    const cleared = before - jobs.length;
    if (cleared > 0) emit();
    return Promise.resolve(cleared);
  },

  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },

  getSettings(): Promise<LocalSettings | null> {
    return Promise.resolve(loadSettings());
  },

  saveSettings(settings: LocalSettings): Promise<LocalSettings | null> {
    localStorage.setItem("ludex.demo.desktop", JSON.stringify(settings));
    return Promise.resolve(clone(settings));
  },

  moveInstall(gameId: number, targetDir: string): Promise<InstalledGame> {
    const entry = installed.find((game) => game.gameId === gameId);
    if (!entry) return Promise.reject(new Error("Game is not installed"));
    const slug = entry.installPath.split("/").slice(-2).join("/");
    entry.installPath = `${targetDir.replace(/\/+$/, "")}/${slug}`;
    emit();
    return Promise.resolve(clone(entry));
  },

  verifyInstall(gameId: number): Promise<VerifyReport> {
    const entry = installed.find((game) => game.gameId === gameId);
    if (!entry) return Promise.reject(new Error("Game is not installed"));
    return Promise.resolve({
      gameId,
      status: "ok",
      filesChecked: 1842,
      bytesChecked: entry.sizeBytes,
      missing: [],
      corrupt: [],
      extra: [],
      checkedAt: new Date().toISOString(),
      message: null,
    });
  },

  repairInstall(gameId: number): Promise<RepairReport> {
    return this.verifyInstall(gameId).then((verify) => ({
      gameId,
      repaired: [],
      failed: [],
      fallback: false,
      message: "All files match the manifest. Nothing to repair.",
      verify,
    }));
  },

  diskUsage(path: string): Promise<DiskUsage> {
    if (path.toLowerCase().includes("download")) {
      return Promise.resolve({ totalBytes: 1024 * GB, freeBytes: Math.round(318 * GB) });
    }
    return Promise.resolve({ totalBytes: 4 * 1024 * GB, freeBytes: Math.round(1.24 * 1024 * GB) });
  },
};

