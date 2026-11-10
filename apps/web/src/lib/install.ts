import { USING_MOCKS } from "@/lib/flags";
import { isNative, native } from "@/lib/native";
import { mockInstalls } from "@/lib/mock-install";
import type { DiskUsage, InstallGameArgs, InstallJob, LocalSettings } from "@/lib/types";

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

  /** Mock-only: keeps the bytes already received. Native resumes through install_game. */
  resumeMock(jobId: string): Promise<InstallJob | null> {
    return USING_MOCKS ? mockInstalls.resume(jobId) : Promise.resolve(null);
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

  subscribe(listener: () => void): () => void {
    return USING_MOCKS ? mockInstalls.subscribe(listener) : () => {};
  },
};
