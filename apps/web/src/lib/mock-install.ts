import { DEMO_GAMES } from "@/data/demo";
import { USING_MOCKS } from "@/lib/flags";
import { ALL_LAUNCHER_IDS } from "@/lib/launchers";
import { slugify } from "@/lib/art";
import type { SaveCandidate, SaveCandidateGroup, SaveCandidateOverview } from "@/lib/save-discovery";
import type {
  DiskUsage,
  InstallGameArgs,
  InstallJob,
  InstalledGame,
  LocalSettings,
  ManifestStatus,
  ManifestUpdate,
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
/** Per-job base rate so the demo graph has variety between installs. */
const speedSeeds = new Map<string, number>();

function emit() {
  for (const listener of listeners) listener();
}

function baseSpeed(jobId: string): number {
  const seeded = speedSeeds.get(jobId);
  if (seeded !== undefined) return seeded;
  let hash = 0;
  for (const char of jobId) hash = (hash * 31 + char.charCodeAt(0)) % 1_000;
  const speed = (18 + (hash % 42)) * MB;
  speedSeeds.set(jobId, speed);
  return speed;
}

/**
 * Variable demo rate (#102): the base wobbles every tick and now and then
 * dips, so the smoothing and the two-minute graph are reviewable in a browser
 * without the desktop shell. The tick itself stays at 1 s.
 */
function nextMockSpeed(jobId: string): number {
  const wobble = 0.65 + Math.random() * 0.55;
  const dip = Math.random() < 0.07 ? 0.15 : 1;
  return Math.max(1 * MB, Math.round(baseSpeed(jobId) * wobble * dip));
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
    source: "steam",
    launchUri: "steam://rungameid/753640",
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
  {
    // Unmatched launcher import (#154): negative local id, launched through
    // Epic, never linked to a server game page.
    gameId: -2231380,
    versionId: 0,
    title: "Control Ultimate Edition",
    version: null,
    installPath: "C:/Program Files/Epic Games/ControlUltimateEdition",
    executable: null,
    sizeBytes: Math.round(34.2 * GB),
    installedAt: "2026-10-30T09:00:00Z",
    lastLaunchedAt: null,
    source: "epic",
    launchUri: "com.epicgames.launcher://apps/control",
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
  resumable = false,
): InstallJob {
  meta.set(jobId, { title, version });
  return {
    jobId,
    gameId,
    versionId,
    phase,
    bytesDone,
    bytesTotal,
    speedBps,
    message,
    installPath,
    stream,
    resumable,
    entriesDone: null,
    entriesTotal: null,
    currentEntry: null,
    resumedEntries: null,
  };
}

let jobs: InstallJob[] = [
  job(
    "job_forza",
    5,
    51,
    "downloading",
    96 * GB,
    Math.round(132.4 * GB),
    46 * MB,
    "Forza Horizon 6",
    "1.0.118",
    "Downloading archive",
    null,
    true,
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

// Demo the per-entry stream UI (#115): a resumable job shows file counts.
if (jobs[0]) {
  jobs[0].entriesDone = 1240;
  jobs[0].entriesTotal = 1830;
  jobs[0].resumedEntries = 410;
  jobs[0].currentEntry = "data/pakchunk12.pak";
}

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
        current.speedBps = nextMockSpeed(current.jobId);
        current.message = "Downloading archive";
        changed = true;
      }
    } else if (current.phase === "downloading") {
      const total = current.bytesTotal ?? 0;
      current.speedBps = nextMockSpeed(current.jobId);
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
        current.speedBps = 0;
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
  installRootConfirmed: true,
  downloadDir: "/home/mara/Downloads/Ludex",
  locations: [
    { id: "games", label: "Games", path: "/home/mara/Games/Ludex", kind: "install", default: true },
    { id: "ssd", label: "Fast SSD", path: "/mnt/ssd/Ludex", kind: "install", default: false },
    { id: "downloads", label: "Downloads", path: "/home/mara/Downloads/Ludex", kind: "download", default: true },
  ],
  serverPort: 0,
  streamInstall: true,
  keepArchives: false,
  archivePasswords: [],
  bandwidthLimitKib: 0,
  closeToTray: true,
  nativeTitleBar: false,
  updateChannel: "stable",
  checkUpdatesOnStartup: true,
  lastUpdateCheckAt: null,
  discordPresence: false,
  discordClientId: "",
  installerSilentArgs: [],
  installerTimeoutSecs: 600,
  steamRoot: "",
  compatUmuPath: "",
  compatProtonPath: "",
  compatWinePath: "",
  defaultLaunchMode: "auto",
  ludusaviManifestPath: "",
  ludusaviBinary: "",
  ludusaviManifestAutoUpdate: true,
  ludusaviManifestUrl: "",
  cloudAutoSync: true,
  cloudDeviceId: "demo-device-01",
  cloudDeviceLabel: "Living room PC",
  servers: [],
  activeServerId: "",
  sunshineAppsPath: "",
  sunshineConfPath: "",
  moonlightBinary: "",
  moonlightConfigPath: "",
  moonlightHost: "",
  consoleMode: false,
  consoleAppName: "Ludex",
  consoleFullscreen: true,
  consoleExitShortcut: "f12",
  consolePerGameApps: false,
  gamevaultRoots: [],
  gamevaultScanDepth: 6,
  launcherSources: ALL_LAUNCHER_IDS,
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

/** Cached manifest for the browser demo (#153); nothing is downloaded. */
let localManifest: ManifestStatus | null = null;

function demoManifestStatus(): ManifestStatus {
  const settings = loadSettings();
  const present = localManifest !== null || localStorage.getItem("ludex.demo.manifest") === "1";
  if (!present) {
    localManifest = null;
    return {
      present: false,
      path: null,
      version: null,
      updatedAt: null,
      checkedAt: null,
      source:
        settings.ludusaviManifestUrl ||
        "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/data/data/manifest.yaml",
      sizeBytes: 0,
      autoUpdate: settings.ludusaviManifestAutoUpdate,
      stale: true,
      error: null,
      message:
        "No save database yet. Ludex can download the official Ludusavi manifest so save detection works without installing Ludusavi.",
    };
  }
  localManifest ??= {
    present: true,
    path: "/home/mara/.local/share/ludex/manifest/ludusavi-manifest.yaml",
    version: "sha256:7c1f5d2a9b04",
    updatedAt: "2026-11-10T08:15:00Z",
    checkedAt: "2026-11-10T08:15:00Z",
    source:
      settings.ludusaviManifestUrl ||
      "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/data/data/manifest.yaml",
    sizeBytes: 14_512_640,
    autoUpdate: settings.ludusaviManifestAutoUpdate,
    stale: false,
    error: null,
    message: "The save database is ready.",
  };
  return { ...localManifest, autoUpdate: settings.ludusaviManifestAutoUpdate, source: settings.ludusaviManifestUrl || localManifest.source };
}

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
      resumable: false,
      entriesDone: null,
      entriesTotal: null,
      currentEntry: null,
      resumedEntries: null,
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
        target.speedBps = nextMockSpeed(jobId);
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
    speedSeeds.delete(jobId);
    emit();
    return Promise.resolve();
  },

  clearCompleted(): Promise<number> {
    const cleared = jobs.filter((entry) => TERMINAL.has(entry.phase));
    for (const entry of cleared) {
      meta.delete(entry.jobId);
      speedSeeds.delete(entry.jobId);
    }
    jobs = jobs.filter((entry) => !TERMINAL.has(entry.phase));
    if (cleared.length > 0) emit();
    return Promise.resolve(cleared.length);
  },

  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },

  getSettings(): Promise<LocalSettings | null> {
    return Promise.resolve(loadSettings());
  },

  /** Demo save database (#153), persisted in localStorage. */
  manifestStatus(): Promise<ManifestStatus> {
    return Promise.resolve(demoManifestStatus());
  },

  updateManifest(): Promise<ManifestUpdate> {
    localStorage.setItem("ludex.demo.manifest", "1");
    localManifest = {
      present: true,
      path: "/home/mara/.local/share/ludex/manifest/ludusavi-manifest.yaml",
      version: "sha256:7c1f5d2a9b04",
      updatedAt: new Date().toISOString(),
      checkedAt: null,
      source: "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/data/data/manifest.yaml",
      sizeBytes: 14_512_640,
      autoUpdate: true,
      stale: false,
      error: null,
      message: "Save database updated: sha256:7c1f5d2a9b04 (13.8 MiB).",
    };
    return Promise.resolve({ ...demoManifestStatus(), updated: true, notModified: false });
  },

  removeManifest(): Promise<ManifestStatus> {
    localStorage.removeItem("ludex.demo.manifest");
    localManifest = null;
    return Promise.resolve(demoManifestStatus());
  },

  /** Demo save-location candidates (#146), including one labelled guess. */
  saveCandidates(): Promise<SaveCandidateOverview> {
    return Promise.resolve(demoSaveCandidates());
  },

  scanSaveCandidates(): Promise<SaveCandidateOverview> {
    return Promise.resolve(demoSaveCandidates());
  },

  confirmSaveCandidate(
    gameId: number,
    path: string,
    editedPath?: string | null,
  ): Promise<SaveCandidateGroup> {
    const overview = demoSaveCandidates();
    const group = overview.groups.find((entry) => entry.gameId === gameId) ?? {
      gameId,
      title: "Game",
      candidates: [],
    };
    const confirmed: SaveCandidateGroup = {
      ...group,
      candidates: group.candidates.map((candidate) =>
        candidate.path === path
          ? {
              ...candidate,
              path: editedPath?.trim() || candidate.path,
              confirmed: true,
              source: editedPath ? "manual" : candidate.source,
            }
          : candidate,
      ),
    };
    return Promise.resolve(confirmed);
  },

  ignoreSaveCandidate(gameId: number, path: string): Promise<SaveCandidateGroup> {
    const group = demoSaveCandidates().groups.find((entry) => entry.gameId === gameId);
    return Promise.resolve({
      gameId,
      title: group?.title ?? "Game",
      candidates: (group?.candidates ?? []).filter((candidate) => candidate.path !== path),
    });
  },

  saveSettings(settings: LocalSettings): Promise<LocalSettings | null> {
    localStorage.setItem("ludex.demo.desktop", JSON.stringify(settings));
    return Promise.resolve(clone(settings));
  },

  /** Persist only the launcher sources (#155), like `save_settings` does. */
  saveLauncherSources(sources: LocalSettings["launcherSources"]): Promise<LocalSettings | null> {
    const next: LocalSettings = { ...loadSettings(), launcherSources: sources };
    localStorage.setItem("ludex.demo.desktop", JSON.stringify(next));
    return Promise.resolve(clone(next));
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

/** Static demo candidates (#146): one likely, one store ground truth, one guess. */
function demoSaveCandidates(): SaveCandidateOverview {
  const candidate = (patch: Partial<SaveCandidate>): SaveCandidate => ({
    path: "/home/mara/Saved Games/Hades",
    kind: "directory",
    rootId: "saved_games",
    subpath: "Hades",
    source: "engine",
    confidence: "likely",
    evidence: "Game title folders: {saved_games}/{product}",
    sizeBytes: 0,
    lastModified: null,
    firstSeen: "2026-11-11T00:00:00Z",
    lastVerified: "2026-11-11T00:00:00Z",
    confirmed: false,
    ignored: false,
    ...patch,
  });
  return {
    registryVersion: 1,
    maxDepth: 4,
    maxEntries: 2000,
    warnings: [],
    groups: [
      {
        gameId: 1,
        title: "Hades",
        candidates: [
          candidate({}),
          candidate({
            path: "/home/mara/.config/unity3d/Supergiant/Hades",
            rootId: "xdg_config",
            subpath: "unity3d/Supergiant/Hades",
            source: "engine",
            confidence: "guess",
            evidence: "Unity default layout: {xdg_config}/unity3d/{company}/{product}",
          }),
        ],
      },
      {
        gameId: 2,
        title: "Celeste",
        candidates: [
          candidate({
            path: "/home/mara/.local/share/Celeste",
            rootId: "xdg_data",
            subpath: "Celeste",
            source: "store",
            confidence: "certain",
            evidence: "Steam Cloud remotecache.vdf lists files under SteamCloudDocuments",
          }),
        ],
      },
    ],
  };
}
