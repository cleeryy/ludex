import { USING_MOCKS } from "@/lib/flags";
import { isNative, native } from "@/lib/native";
import { mockInstalls } from "@/lib/mock-install";
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
 * One surface for desktop installs. In Tauri this talks to the Rust commands;
 * with mocks on, the same calls hit an in-browser job manager so the Downloads
 * UI can be reviewed without the desktop shell.
 */
export const installsAvailable = (): boolean => isNative() || USING_MOCKS;

export const installBridge = {
  list(): Promise<InstallJob[]> {
    return USING_MOCKS ? mockInstalls.list() : native.listInstallJobs();
  },

  start(args: InstallGameArgs): Promise<InstallJob> {
    return USING_MOCKS ? mockInstalls.start(args) : native.installGame(args);
  },

  cancel(jobId: string): Promise<void> {
    return USING_MOCKS ? mockInstalls.cancel(jobId) : native.cancelInstall(jobId);
  },

  pause(jobId: string): Promise<InstallJob> {
    return USING_MOCKS ? mockInstalls.pause(jobId) : native.pauseInstall(jobId);
  },

  resume(jobId: string): Promise<InstallJob | null> {
    return USING_MOCKS ? mockInstalls.resume(jobId) : native.resumeInstall(jobId);
  },

  clearCompleted(): Promise<void> {
    return USING_MOCKS ? mockInstalls.clearCompleted() : Promise.resolve();
  },

  settings(): Promise<LocalSettings | null> {
    return USING_MOCKS ? mockInstalls.getSettings() : native.getSettings();
  },

  saveSettings(settings: LocalSettings): Promise<LocalSettings | null> {
    return USING_MOCKS ? mockInstalls.saveSettings(settings) : native.saveSettings(settings);
  },

  diskUsage(path: string): Promise<DiskUsage> {
    return USING_MOCKS ? mockInstalls.diskUsage(path) : native.getDiskUsage(path);
  },

  openPath(path: string): Promise<void> {
    return native.openPath(path);
  },

  moveInstall(gameId: number, targetDir: string): Promise<InstalledGame> {
    return USING_MOCKS ? mockInstalls.moveInstall(gameId, targetDir) : native.moveInstall(gameId, targetDir);
  },

  verifyInstall(gameId: number): Promise<VerifyReport> {
    return USING_MOCKS ? mockInstalls.verifyInstall(gameId) : native.verifyInstall(gameId);
  },

  repairInstall(gameId: number): Promise<RepairReport> {
    return USING_MOCKS ? mockInstalls.repairInstall(gameId) : native.repairInstall(gameId);
  },

  subscribe(listener: () => void): () => void {
    return USING_MOCKS ? mockInstalls.subscribe(listener) : () => {};
  },
};
