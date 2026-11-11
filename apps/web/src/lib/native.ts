import { t } from "@/lib/i18n";
import type {
  AdapterReport,
  AdoptInstallArgs,
  AdoptPreview,
  CloudSaveRestoreOutcome,
  CloudSaveStatus,
  CloudSaveStrategy,
  CloudSyncToggleResponse,
  CompatScan,
  ConfigBackupRecord,
  ConfigDocument,
  ConfigEditInput,
  ConfigWriteReport,
  DeleteAdoptedPreview,
  DiskUsage,
  GameConfigListing,
  GameExecutables,
  GamePlaytime,
  GameUpdate,
  GameVaultAdoptEntry,
  GameVaultAdoptReport,
  GameVaultRootProposal,
  GameVaultScan,
  InstallGameArgs,
  InstallJob,
  InstalledGame,
  InstallerPlan,
  InstallerReport,
  LauncherImportEntry,
  LauncherImportReport,
  LauncherPreview,
  LaunchOptions,
  LocalSettings,
  LocationStatus,
  ManifestProgress,
  ManifestReport,
  ManifestStatus,
  ManifestUpdate,
  MoveProgress,
  PresenceStatus,
  MoonlightStatus,
  OidcLoginPoll,
  OidcLoginStart,
  PrefixInfo,
  ProtonDbRating,
  RepairProgress,
  RepairReport,
  RomCommandValidation,
  RomEmulator,
  RomLaunchResult,
  RomSettings,
  ServerList,
  ServerTest,
  SteamImportReport,
  SteamPreview,
  StorageSuggestions,
  StreamLaunch,
  StreamTarget,
  SunshinePreview,
  SunshineReport,
  SunshineStatus,
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

  /**
   * Show the native title bar (`true`) or Ludex's own (`false`). Applies to the
   * live window and is persisted immediately (#111); a no-op on macOS, which
   * always keeps the native traffic lights.
   */
  async setNativeTitleBar(enabled: boolean): Promise<LocalSettings | null> {
    if (!isNative()) return null;
    return invoke<LocalSettings>("set_native_title_bar", { enabled });
  },

  async pickFolder(title?: string): Promise<string | null> {
    if (!isNative()) return null;
    return invoke<string | null>("pick_folder", { title });
  },

  /**
   * Cached Ludusavi save-path manifest (#153): version, dates, source and
   * whether automatic downloads are on. Local only.
   */
  async ludusaviManifestStatus(): Promise<ManifestStatus> {
    return invoke<ManifestStatus>("ludusavi_manifest_status");
  },

  /** Download or refresh the save database now, ignoring the 24 h throttle (#153). */
  async updateLudusaviManifest(): Promise<ManifestUpdate> {
    return invoke<ManifestUpdate>("update_ludusavi_manifest");
  },

  /** Delete the cached save database; detection falls back to manual paths (#153). */
  async removeLudusaviManifest(): Promise<ManifestStatus> {
    return invoke<ManifestStatus>("remove_ludusavi_manifest");
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

  /* emulator registry (#52) */
  async getRomSettings(): Promise<RomSettings> {
    return invoke<RomSettings>("get_rom_settings");
  },

  async saveRomEmulator(emulator: RomEmulator): Promise<RomSettings["registry"]> {
    return invoke<RomSettings["registry"]>("save_rom_emulator", { emulator });
  },

  async deleteRomEmulator(id: string): Promise<RomSettings["registry"]> {
    return invoke<RomSettings["registry"]>("delete_rom_emulator", { id });
  },

  async setRomDefault(platform: string, emulatorId: string | null): Promise<RomSettings["registry"]> {
    return invoke<RomSettings["registry"]>("set_rom_default", { platform, emulatorId });
  },

  async validateRomCommand(command: string): Promise<RomCommandValidation> {
    return invoke<RomCommandValidation>("validate_rom_command", { command });
  },

  async launchRomGame(args: {
    gameId: number;
    platform: string;
    romPath: string;
    emulatorId?: string | null;
  }): Promise<RomLaunchResult> {
    return invoke<RomLaunchResult>("launch_rom_game", args);
  },

  async stopRomGame(sessionId: string): Promise<void> {
    await invoke<null>("stop_rom_game", { sessionId });
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

  /* --------------------- executable picker (#143) ------------------------ */

  /** Programs found in the install folder; `refresh` bypasses the 5 s cache. */
  async listGameExecutables(gameId: number, refresh = false): Promise<GameExecutables> {
    return invoke<GameExecutables>("list_game_executables", { gameId, refresh });
  },

  /** Normalise a chosen executable path; refuses anything outside the folder. */
  async checkGameExecutable(gameId: number, path: string): Promise<string> {
    return invoke<string>("check_game_executable", { gameId, path });
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

  /* ------------------- launcher discovery (#116) ------------------------- */

  async launcherScan(): Promise<LauncherPreview> {
    return invoke<LauncherPreview>("launcher_scan");
  },

  async launcherImport(entries: LauncherImportEntry[]): Promise<LauncherImportReport> {
    return invoke<LauncherImportReport>("launcher_import", { entries });
  },

  /* ---------------- adopting an existing install (#129) ------------------ */

  /** Inspect a folder: executable, size, file count, version. Writes nothing. */
  async adoptInstallation(args: {
    gameId: number;
    folder: string;
    versionId?: number | null;
    version?: string | null;
    /** Manual executable ("link anyway"), relative to the folder or absolute. */
    executable?: string | null;
    title?: string | null;
  }): Promise<AdoptPreview> {
    return invoke<AdoptPreview>("adopt_installation", args);
  },

  /** Record the adopted folder. Nothing is copied or moved. */
  async confirmAdoptInstallation(args: AdoptInstallArgs): Promise<InstalledGame> {
    return invoke<InstalledGame>("confirm_adopt_installation", { ...args });
  },

  /** Hash the adopted folder into a manifest so Verify/Repair work. */
  async buildInstallManifest(gameId: number): Promise<ManifestReport> {
    return invoke<ManifestReport>("build_install_manifest", { gameId });
  },

  /** Remove the index row and manifest; the files stay on disk. */
  async unlinkInstallation(gameId: number): Promise<void> {
    if (!isNative()) throw nativeUnavailable();
    await invoke<null>("unlink_installation", { gameId });
  },

  /**
   * Preview (`confirmPath` omitted) or delete the adopted folder. The delete
   * only runs when `confirmPath` is exactly the canonical folder.
   */
  async deleteAdoptedInstallation(gameId: number, confirmPath?: string | null): Promise<DeleteAdoptedPreview> {
    return invoke<DeleteAdoptedPreview>("delete_adopted_installation", { gameId, confirmPath: confirmPath ?? null });
  },

  /** Scan every configured GameVault root in one pass (#134). */
  async scanGameVaultFolders(roots?: string[]): Promise<GameVaultScan> {
    return invoke<GameVaultScan>("scan_gamevault_folders", { roots: roots ?? null });
  },

  /** Adopt selected GameVault candidates after the user confirmed the matches. */
  async adoptGameVaultInstallations(roots: string[], entries: GameVaultAdoptEntry[]): Promise<GameVaultAdoptReport> {
    return invoke<GameVaultAdoptReport>("adopt_gamevault_installations", { roots, entries });
  },

  /** Add one root to the ordered list, persisted in local settings (#134). */
  async addGameVaultRoot(path: string): Promise<LocalSettings> {
    return invoke<LocalSettings>("add_gamevault_root", { path });
  },

  /** Remove one root from the ordered list (#134). */
  async removeGameVaultRoot(path: string): Promise<LocalSettings> {
    return invoke<LocalSettings>("remove_gamevault_root", { path });
  },

  /** Persist a new root order (#134). */
  async reorderGameVaultRoots(roots: string[]): Promise<LocalSettings> {
    return invoke<LocalSettings>("reorder_gamevault_roots", { roots });
  },

  /** Propose GameVault roots found on this machine, read-only (#134). */
  async detectGameVaultRoots(): Promise<GameVaultRootProposal[]> {
    return invoke<GameVaultRootProposal[]>("detect_gamevault_roots");
  },

  async onManifestProgress(handler: (progress: ManifestProgress) => void): Promise<() => void> {
    return listen<ManifestProgress>("install://manifest-progress", handler);
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

  /** An automatic or manual save-database update finished (#153). */
  async onSaveManifestUpdated(handler: (update: ManifestUpdate) => void): Promise<() => void> {
    return listen<ManifestUpdate>("save-manifest://updated", handler);
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

  /* ----------------------- locations (#99) ----------------------------- */

  async listLocations(): Promise<LocationStatus[]> {
    if (!isNative()) return [];
    try {
      return await invoke<LocationStatus[]>("list_locations");
    } catch {
      return [];
    }
  },

  async checkLocation(args: { id?: string; path?: string; requiredBytes?: number }): Promise<LocationStatus> {
    return invoke<LocationStatus>("check_location", args);
  },

  /**
   * Detected install roots (#145). Null outside the desktop app or when the
   * probe fails: the quick-add section is simply hidden.
   */
  async suggestStorageLocations(): Promise<StorageSuggestions | null> {
    if (!isNative()) return null;
    try {
      return await invoke<StorageSuggestions>("suggest_storage_locations");
    } catch {
      return null;
    }
  },

  /* --------------------- window chrome (#111) --------------------------- */

  /* ------------------- game settings editor (#105) ---------------------- */

  async listGameConfigs(gameId: number): Promise<GameConfigListing> {
    return invoke<GameConfigListing>("list_game_configs", { gameId });
  },

  async readGameConfig(gameId: number, path: string): Promise<ConfigDocument> {
    return invoke<ConfigDocument>("read_game_config", { gameId, path });
  },

  async writeGameConfig(
    gameId: number,
    path: string,
    edits: ConfigEditInput[],
    origin?: "manual" | "adapter",
  ): Promise<ConfigWriteReport> {
    return invoke<ConfigWriteReport>("write_game_config", { gameId, path, edits, origin });
  },

  async restoreGameConfig(gameId: number, backupId: string): Promise<ConfigWriteReport> {
    return invoke<ConfigWriteReport>("restore_game_config", { gameId, backupId });
  },

  async listConfigBackups(gameId: number): Promise<ConfigBackupRecord[]> {
    if (!isNative()) return [];
    try {
      return await invoke<ConfigBackupRecord[]>("list_game_config_backups", { gameId });
    } catch {
      return [];
    }
  },

  async configAdapters(gameId: number): Promise<AdapterReport> {
    return invoke<AdapterReport>("game_config_adapters", { gameId });
  },

  /* ---------------------- Sunshine host (#97) ---------------------------- */

  async sunshineStatus(): Promise<SunshineStatus> {
    return invoke<SunshineStatus>("sunshine_status");
  },

  async sunshinePreview(): Promise<SunshinePreview> {
    return invoke<SunshinePreview>("sunshine_preview");
  },

  async sunshineSync(args: { dryRun?: boolean; removeMissing?: boolean; downloadCovers?: boolean } = {}): Promise<SunshineReport> {
    return invoke<SunshineReport>("sunshine_sync", {
      dryRun: args.dryRun ?? false,
      removeMissing: args.removeMissing ?? true,
      downloadCovers: args.downloadCovers ?? true,
    });
  },

  async sunshineRemoveManaged(): Promise<SunshineReport> {
    return invoke<SunshineReport>("sunshine_remove_managed");
  },

  /* --------------------- Moonlight client (#98) -------------------------- */

  async moonlightStatus(): Promise<MoonlightStatus> {
    return invoke<MoonlightStatus>("moonlight_status");
  },

  async streamTarget(gameId: number): Promise<StreamTarget> {
    return invoke<StreamTarget>("stream_target", { gameId });
  },

  async streamGame(gameId: number, host?: string | null): Promise<StreamLaunch> {
    return invoke<StreamLaunch>("stream_game", { gameId, host: host ?? null });
  },

  async moonlightStream(host: string, appName: string): Promise<StreamLaunch> {
    return invoke<StreamLaunch>("moonlight_stream", { host, appName });
  },

  /* ------------------ multiple servers and accounts (#24) ---------------- */

  async listServers(): Promise<ServerList> {
    return invoke<ServerList>("list_servers");
  },

  async saveServer(args: {
    id?: string | null;
    label: string;
    url: string;
    username?: string | null;
    token?: string | null;
  }): Promise<ServerList> {
    return invoke<ServerList>("save_server", {
      id: args.id ?? null,
      label: args.label,
      url: args.url,
      username: args.username ?? null,
      token: args.token ?? null,
    });
  },

  async removeServer(id: string): Promise<ServerList> {
    return invoke<ServerList>("remove_server", { id });
  },

  async switchServer(id: string): Promise<ServerList> {
    return invoke<ServerList>("switch_server", { id });
  },

  async testServer(url: string, token?: string | null): Promise<ServerTest> {
    return invoke<ServerTest>("test_server", { url, token: token ?? null });
  },

  /* --------------------- desktop SSO handoff (#157) ---------------------- */

  /**
   * Opens `{server}/#/login?desktop_state=…` in the system browser. No local
   * port is opened; the app then polls the claim endpoint.
   */
  async oidcLoginStart(): Promise<OidcLoginStart> {
    if (!isNative()) throw nativeUnavailable();
    return invoke<OidcLoginStart>("oidc_login_start");
  },

  /** Waits for the web page to hand the session back (up to ~5 minutes). */
  async oidcLoginPoll(sessionId: string): Promise<OidcLoginPoll> {
    if (!isNative()) throw nativeUnavailable();
    return invoke<OidcLoginPoll>("oidc_login_poll", { sessionId });
  },

  /** Stops an in-flight poll; the attempt resolves as `cancelled`. */
  async oidcLoginCancel(sessionId: string): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("oidc_login_cancel", { sessionId });
  },

  /**
   * Opens an http(s) address in the system browser. The webview cannot open a
   * second window itself, and `openPath` only accepts files.
   */
  async openExternalUrl(url: string): Promise<void> {
    if (!isNative()) throw nativeUnavailable();
    await invoke<null>("open_external_url", { url });
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
  installRootConfirmed: false,
  downloadDir: "",
  locations: [],
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
  cloudDeviceId: "",
  cloudDeviceLabel: null,
  servers: [],
  activeServerId: "",
  sunshineAppsPath: "",
  sunshineConfPath: "",
  moonlightBinary: "",
  moonlightConfigPath: "",
  moonlightHost: "",
  gamevaultRoots: [],
  gamevaultScanDepth: 6,
};
