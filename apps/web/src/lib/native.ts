import { t } from "@/lib/i18n";
import type {
  CloudSaveRestoreOutcome,
  CloudSaveStatus,
  CloudSaveStrategy,
  CloudSyncToggleResponse,
  CompatScan,
  DiskUsage,
  GamePlaytime,
  GameUpdate,
  InstallGameArgs,
  InstallJob,
  InstalledGame,
  InstallerPlan,
  InstallerReport,
  LaunchOptions,
  LocalSettings,
  MoveProgress,
  PresenceStatus,
  PrefixInfo,
  ProtonDbRating,
  RepairProgress,
  RepairReport,
  SteamImportReport,
  SteamPreview,
  UpdateInstalled,
  UpdateProgress,
  UpdateStatus,
  VerifyProgress,
  VerifyReport,
  WinetricksReport,
} from "@/lib/types";

/**
 * Native bridge. Everything is a no-op or a friendly rejection in a browser;
 * in Tauri the same functions call the Rust commands from docs/CONTRACT.md.
 */

export const isNative = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in (window as unknown as Record<string, unknown>);

export function nativeUnavailable(): Error {
  return new Error(t("That needs the Ludex desktop app. The browser build can browse and download."));
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isNative()) throw nativeUnavailable();
  const { invoke: call } = await import("@tauri-apps/api/core");
  return call<T>(command, args);
}

export const native = {
  available: isNative,

  async getSettings(): Promise<LocalSettings | null> {
    if (!isNative()) return null;
    return invoke<LocalSettings>("get_settings");
  },

  async saveSettings(settings: LocalSettings): Promise<LocalSettings | null> {
    if (!isNative()) return null;
    return invoke<LocalSettings>("save_settings", { settings });
  },

  async pickFolder(title?: string): Promise<string | null> {
    if (!isNative()) return null;
    return invoke<string | null>("pick_folder", { title });
  },

  async startLocalServer(): Promise<{ url: string; token: string }> {
    return invoke<{ url: string; token: string }>("local_server_start");
  },

  async stopLocalServer(): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("local_server_stop");
  },

  async localServerStatus(): Promise<{ running: boolean; url: string | null }> {
    if (!isNative()) return { running: false, url: null };
    return invoke<{ running: boolean; url: string | null }>("local_server_status");
  },

  async installGame(args: InstallGameArgs): Promise<InstallJob> {
    return invoke<InstallJob>("install_game", { ...args });
  },

  async listInstallJobs(): Promise<InstallJob[]> {
    if (!isNative()) return [];
    try {
      return await invoke<InstallJob[]>("list_install_jobs");
    } catch {
      return [];
    }
  },

  async cancelInstall(jobId: string): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("cancel_install", { jobId });
  },

  /** Remove one terminal job from Downloads history (#100); files are kept. */
  async deleteInstallJob(jobId: string): Promise<void> {
    if (!isNative()) throw nativeUnavailable();
    await invoke<null>("delete_install_job", { jobId });
  },

  /** Remove every terminal job from Downloads history (#100); returns rows removed. */
  async clearFinishedInstallJobs(): Promise<number> {
    if (!isNative()) return 0;
    return invoke<number>("clear_finished_install_jobs");
  },

  async pauseInstall(jobId: string): Promise<InstallJob> {
    return invoke<InstallJob>("pause_install", { jobId });
  },

  async resumeInstall(jobId: string): Promise<InstallJob> {
    return invoke<InstallJob>("resume_install", { jobId });
  },

  async getInstallJob(jobId: string): Promise<InstallJob> {
    return invoke<InstallJob>("get_install_job", { jobId });
  },

  async listRecentGames(limit?: number): Promise<InstalledGame[]> {
    if (!isNative()) return [];
    try {
      return await invoke<InstalledGame[]>("list_recent_games", { limit });
    } catch {
      return [];
    }
  },

  async getGamePlaytime(gameId: number, days?: number, sessions?: number): Promise<GamePlaytime | null> {
    if (!isNative()) return null;
    try {
      return await invoke<GamePlaytime>("get_game_playtime", { gameId, days, sessions });
    } catch {
      return null;
    }
  },

  async checkGameUpdates(): Promise<GameUpdate[]> {
    if (!isNative()) return [];
    try {
      return await invoke<GameUpdate[]>("check_game_updates");
    } catch {
      return [];
    }
  },

  async updateGame(gameId: number): Promise<InstallJob> {
    return invoke<InstallJob>("update_game", { gameId });
  },

  async checkForUpdates(manual = true): Promise<UpdateStatus> {
    return invoke<UpdateStatus>("check_for_updates", { manual });
  },

  async installUpdate(): Promise<UpdateStatus> {
    return invoke<UpdateStatus>("install_update");
  },

  async restartApp(): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("restart_app");
  },

  async listInstalled(): Promise<InstalledGame[]> {
    if (!isNative()) return [];
    try {
      return await invoke<InstalledGame[]>("list_installed");
    } catch {
      return [];
    }
  },

  async launchGame(args: { gameId: number; executable?: string | null; args?: string | null }): Promise<{ sessionId: string }> {
    return invoke<{ sessionId: string }>("launch_game", args);
  },

  async stopGame(sessionId: string): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("stop_game", { sessionId });
  },

  async uninstallGame(gameId: number): Promise<void> {
    if (!isNative()) throw nativeUnavailable();
    await invoke<null>("uninstall_game", { gameId });
  },

  async moveInstall(gameId: number, targetDir: string): Promise<InstalledGame> {
    return invoke<InstalledGame>("move_install", { gameId, targetDir });
  },

  async verifyInstall(gameId: number): Promise<VerifyReport> {
    return invoke<VerifyReport>("verify_install", { gameId });
  },

  async repairInstall(gameId: number): Promise<RepairReport> {
    return invoke<RepairReport>("repair_install", { gameId });
  },

  async openPath(path: string): Promise<void> {
    if (!isNative()) throw nativeUnavailable();
    await invoke<null>("open_path", { path });
  },

  async getDiskUsage(path: string): Promise<DiskUsage> {
    return invoke<DiskUsage>("get_disk_usage", { path });
  },

  /* ------------------------------ installers (#7) ------------------------ */

  async installerPlan(gameId?: number | null, executable?: string | null): Promise<InstallerPlan> {
    return invoke<InstallerPlan>("installer_plan", { gameId, executable });
  },

  async runInstaller(args: {
    gameId: number;
    executable?: string | null;
    interactive?: boolean;
    args?: string[] | null;
  }): Promise<InstallerReport> {
    return invoke<InstallerReport>("run_installer", { ...args });
  },

  async cancelInstaller(gameId: number): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("cancel_installer", { gameId });
  },

  async setExecutable(gameId: number, executable: string): Promise<InstalledGame> {
    return invoke<InstalledGame>("set_executable", { gameId, executable });
  },

  async chooseExecutable(gameId: number): Promise<InstalledGame | null> {
    return invoke<InstalledGame | null>("choose_executable", { gameId });
  },

  /* ---------------------------- presence (#12) --------------------------- */

  async presenceStatus(): Promise<PresenceStatus> {
    if (!isNative()) {
      return {
        enabled: false,
        configured: false,
        connected: false,
        lastError: null,
        details: null,
        retryInSeconds: 0,
      };
    }
    try {
      return await invoke<PresenceStatus>("presence_status");
    } catch {
      return {
        enabled: false,
        configured: false,
        connected: false,
        lastError: null,
        details: null,
        retryInSeconds: 0,
      };
    }
  },

  /* ------------------------------ steam (#13) ---------------------------- */

  async steamScan(): Promise<SteamPreview> {
    return invoke<SteamPreview>("steam_scan");
  },

  async steamImport(appIds: number[]): Promise<SteamImportReport> {
    return invoke<SteamImportReport>("steam_import", { appIds });
  },

  /* -------------------- Linux compatibility (#89-#93) -------------------- */

  async compatScan(): Promise<CompatScan> {
    return invoke<CompatScan>("compat_scan");
  },

  async getLaunchOptions(gameId: number): Promise<LaunchOptions> {
    return invoke<LaunchOptions>("get_launch_options", { gameId });
  },

  async setLaunchOptions(gameId: number, options: LaunchOptions): Promise<LaunchOptions> {
    return invoke<LaunchOptions>("set_launch_options", { gameId, options });
  },

  async winePrefixInfo(gameId: number): Promise<PrefixInfo> {
    return invoke<PrefixInfo>("wine_prefix_info", { gameId });
  },

  async resetWinePrefix(gameId: number): Promise<void> {
    if (!isNative()) throw nativeUnavailable();
    await invoke<null>("reset_wine_prefix", { gameId });
  },

  async runWinetricks(gameId: number, verbs?: string[]): Promise<WinetricksReport> {
    return invoke<WinetricksReport>("run_winetricks", { gameId, verbs: verbs ?? null });
  },

  async protondbRating(gameId: number, appId?: number | null, refresh = false): Promise<ProtonDbRating> {
    return invoke<ProtonDbRating>("protondb_rating", { gameId, appId: appId ?? null, refresh });
  },

  async onInstallProgress(handler: (job: InstallJob) => void): Promise<() => void> {
    return listen<InstallJob>("install://progress", handler);
  },

  async onInstallDone(handler: (job: InstallJob) => void): Promise<() => void> {
    return listen<InstallJob>("install://done", handler);
  },

  async onInstallError(handler: (payload: { jobId: string; message: string }) => void): Promise<() => void> {
    return listen<{ jobId: string; message: string }>("install://error", handler);
  },

  /** The desktop may broadcast the whole job list; harmless if the event is absent. */
  async onInstallJobs(handler: (jobs: InstallJob[]) => void): Promise<() => void> {
    return listen<InstallJob[]>("install://jobs", handler);
  },

  async onMoveProgress(handler: (progress: MoveProgress) => void): Promise<() => void> {
    return listen<MoveProgress>("install://move-progress", handler);
  },

  async onVerifyProgress(handler: (progress: VerifyProgress) => void): Promise<() => void> {
    return listen<VerifyProgress>("install://verify-progress", handler);
  },

  async onRepairProgress(handler: (progress: RepairProgress) => void): Promise<() => void> {
    return listen<RepairProgress>("install://repair-progress", handler);
  },

  async onGameExited(
    handler: (payload: { sessionId: string; gameId: number; minutesPlayed: number; secondsPlayed: number }) => void,
  ): Promise<() => void> {
    return listen<{ sessionId: string; gameId: number; minutesPlayed: number; secondsPlayed: number }>(
      "game://exited",
      handler,
    );
  },

  /** Tray navigation: the menu asked the UI to open a route. */
  async onNavigate(handler: (path: string) => void): Promise<() => void> {
    return listen<string>("ui://navigate", handler);
  },

  /** Tray "Recent games": launch this game immediately. */
  async onLaunchGame(handler: (gameId: number) => void): Promise<() => void> {
    return listen<number>("ui://launch", handler);
  },

  async onUpdateAvailable(handler: (status: UpdateStatus) => void): Promise<() => void> {
    return listen<UpdateStatus>("update://available", handler);
  },

  async onUpdateProgress(handler: (progress: UpdateProgress) => void): Promise<() => void> {
    return listen<UpdateProgress>("update://progress", handler);
  },

  async onUpdateInstalled(handler: (payload: UpdateInstalled) => void): Promise<() => void> {
    return listen<UpdateInstalled>("update://installed", handler);
  },

  async onLocalServerStatus(handler: (payload: { running: boolean; url: string | null }) => void): Promise<() => void> {
    return listen<{ running: boolean; url: string | null }>("local-server://status", handler);
  },

  async onInstallerDone(
    handler: (payload: { gameId: number; success: boolean; exitCode: number | null }) => void,
  ): Promise<() => void> {
    return listen<{ gameId: number; success: boolean; exitCode: number | null }>(
      "installer://done",
      handler,
    );
  },

  /* ------------------------------ cloud saves ------------------------------ */

  async cloudSaveStatus(gameId: number): Promise<CloudSaveStatus> {
    return invoke<CloudSaveStatus>("cloud_save_status", { gameId });
  },

  /** `gameId: null` changes the global default; a number changes one game. */
  async setCloudSaveSync(gameId: number | null, enabled: boolean): Promise<CloudSyncToggleResponse> {
    return invoke<CloudSyncToggleResponse>("set_cloud_save_sync", { gameId, enabled });
  },

  async syncCloudSaveNow(gameId: number): Promise<CloudSaveStatus> {
    return invoke<CloudSaveStatus>("sync_cloud_save_now", { gameId });
  },

  async restoreCloudSave(gameId: number, strategy: CloudSaveStrategy): Promise<CloudSaveRestoreOutcome> {
    return invoke<CloudSaveRestoreOutcome>("restore_cloud_save", { gameId, strategy });
  },

  async resolveCloudSaveConflict(
    gameId: number,
    strategy: CloudSaveStrategy,
  ): Promise<CloudSaveRestoreOutcome> {
    return invoke<CloudSaveRestoreOutcome>("resolve_cloud_save_conflict", { gameId, strategy });
  },

  async onCloudSaveSynced(handler: (gameId: number) => void): Promise<() => void> {
    return listen<number>("cloud-save://synced", handler);
  },

  async onCloudSavePending(handler: (gameId: number) => void): Promise<() => void> {
    return listen<number>("cloud-save://pending", handler);
  },
};

async function listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  if (!isNative()) return () => {};
  const { listen: on } = await import("@tauri-apps/api/event");
  const unlisten = await on<T>(event, (e) => handler(e.payload));
  return unlisten;
}

export const DEFAULT_LOCAL_SETTINGS: LocalSettings = {
  mode: "local",
  serverUrl: "",
  token: null,
  gamesDir: null,
  installDir: "",
  downloadDir: "",
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
  compatUmuPath: "",
  compatProtonPath: "",
  compatWinePath: "",
  defaultLaunchMode: "auto",
  cloudAutoSync: true,
  cloudDeviceId: "",
  cloudDeviceLabel: null,
};
