import { USING_MOCKS } from "@/lib/flags";
import { isNative, native } from "@/lib/native";
import { mockInstalls } from "@/lib/mock-install";
import type { SaveCandidateGroup, SaveCandidateOverview } from "@/lib/save-discovery";
import type {
  DiskUsage,
  InstallGameArgs,
  InstallJob,
  InstalledGame,
  LauncherId,
  InstallResumeMode,
  LocalSettings,
  ManifestStatus,
  ManifestUpdate,
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

  resume(jobId: string, mode?: InstallResumeMode | null): Promise<InstallJob | null> {
    return USING_MOCKS ? mockInstalls.resume(jobId, mode) : native.resumeInstall(jobId, mode);
  },

  /** Delete one finished job from history, never its files (#100). */
  remove(jobId: string): Promise<void> {
    return USING_MOCKS ? mockInstalls.remove(jobId) : native.deleteInstallJob(jobId);
  },

  /** Delete every finished job from history; resolves with the rows removed. */
  clearCompleted(): Promise<number> {
    return USING_MOCKS ? mockInstalls.clearCompleted() : native.clearFinishedInstallJobs();
  },

  settings(): Promise<LocalSettings | null> {
    return USING_MOCKS ? mockInstalls.getSettings() : native.getSettings();
  },

  saveSettings(settings: LocalSettings): Promise<LocalSettings | null> {
    return USING_MOCKS ? mockInstalls.saveSettings(settings) : native.saveSettings(settings);
  },

  /**
   * Persist the launcher source selection on its own (#155). The source
   * checkboxes are the whole interaction in Settings → Integrations, so they
   * must not wait for the shared "Save desktop settings" button. The backend
   * merges partial payloads, so nothing else is rewritten.
   */
  saveLauncherSources(sources: LauncherId[]): Promise<LocalSettings | null> {
    return USING_MOCKS
      ? mockInstalls.saveLauncherSources(sources)
      : native.saveLauncherSources(sources);
  },

  diskUsage(path: string): Promise<DiskUsage> {
    return USING_MOCKS ? mockInstalls.diskUsage(path) : native.getDiskUsage(path);
  },

  /** Cached Ludusavi save-path manifest (#153). Local to the desktop app. */
  manifestStatus(): Promise<ManifestStatus> {
    return USING_MOCKS ? mockInstalls.manifestStatus() : native.ludusaviManifestStatus();
  },

  /** Download or refresh the save database now (#153). */
  updateManifest(): Promise<ManifestUpdate> {
    return USING_MOCKS ? mockInstalls.updateManifest() : native.updateLudusaviManifest();
  },

  /** Delete the cached save database (#153). */
  removeManifest(): Promise<ManifestStatus> {
    return USING_MOCKS ? mockInstalls.removeManifest() : native.removeLudusaviManifest();
  },

  /** Persisted save-location candidates (#146). Local to the desktop app. */
  saveCandidates(): Promise<SaveCandidateOverview> {
    return USING_MOCKS ? mockInstalls.saveCandidates() : native.listSaveCandidates();
  },

  /** Run the OS/engine save-location registry for every installed game (#146). */
  scanSaveCandidates(): Promise<SaveCandidateOverview> {
    return USING_MOCKS ? mockInstalls.scanSaveCandidates() : native.scanSaveCandidates();
  },

  /** Confirm a candidate; `editedPath` corrects a bad guess first (#146). */
  confirmSaveCandidate(
    gameId: number,
    path: string,
    editedPath?: string | null,
  ): Promise<SaveCandidateGroup> {
    return USING_MOCKS
      ? mockInstalls.confirmSaveCandidate(gameId, path, editedPath)
      : native.confirmSaveCandidate(gameId, path, editedPath);
  },

  /** Dismiss a candidate (#146). */
  ignoreSaveCandidate(gameId: number, path: string): Promise<SaveCandidateGroup> {
    return USING_MOCKS
      ? mockInstalls.ignoreSaveCandidate(gameId, path)
      : native.ignoreSaveCandidate(gameId, path);
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
