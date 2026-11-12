import { t } from "@/lib/i18n";
import { ALL_LAUNCHER_IDS } from "@/lib/launchers";
import type { SaveCandidateGroup, SaveCandidateOverview } from "@/lib/save-discovery";
import type {
  AdapterReport,
  AdoptInstallArgs,
  AdoptPreview,
  CloudSaveRestoreOutcome,
  CloudSaveStatus,
  CloudSaveStrategy,
  CloudSyncToggleResponse,
  CommonFieldPreview,
  CommonFieldTarget,
  CommonFieldsReport,
  CompatScan,
  ConfigBackupRecord,
  ConfigDocument,
  ConfigEditInput,
  ConfigSyncInput,
  ConfigSyncPrefs,
  ConfigWriteReport,
  ConsoleStatus,
  DeleteAdoptedPreview,
  DiagnosticsPreview,
  DiagnosticsSendOutcome,
  DiagnosticsStatus,
  DiskUsage,
  GameConfigListing,
  GameExecutables,
  GameLaunchResult,
  GamePlaytime,
  GameUpdate,
  GameVaultAdoptEntry,
  GameVaultAdoptReport,
  GameVaultRootProposal,
  GameVaultScan,
  InstallGameArgs,
  InstallHealth,
  InstallJob,
  InstalledGame,
  InstallerPlan,
  InstallerReport,
  LauncherId,
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
  OverlayGameState,
  OverlaySettingsPatch,
  OverlayStatus,
  PrefixInfo,
  ProtonDbRating,
  RemotePlayCleanup,
  RemotePlayPortPlan,
  RemotePlayReport,
  RemotePlayStatus,
  RemotePlayVerification,
  RepairProgress,
  RepairReport,
  RomCommandValidation,
  RomEmulator,
  RomLaunchResult,
  RomSettings,
  RuntimeApplyReport,
  RuntimeCatalog,
  RuntimeDiagnosis,
  RuntimeImportReport,
  RuntimeProgress,
  RuntimeRemovalReport,
  ServerList,
  ServerTest,
  SteamImportReport,
  SteamPreview,
  StorageSuggestions,
  StreamLaunch,
  StreamTarget,
  SunshineGrant,
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
  try {
    return await call<T>(command, args);
  } catch (error) {
    // #171: failed commands are recorded locally (redacted in Rust) so the
    // user can attach one to an opt-in report. Never uploaded here.
    if (command !== "diagnostics_record_error") {
      const summary = rejectionSummary(error);
      void call("diagnostics_record_error", {
        kind: commandKind(command),
        summary: `${command}: ${summary}`,
        command,
        stack: error instanceof Error ? error.stack ?? null : null,
      }).catch(() => {});
    }
    throw error;
  }
}

/** Message of a command rejection: string, Error or serialized error object. */
function rejectionSummary(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object") {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return "Command failed";
}

/** Classifies a failed Tauri command for the local report. */
function commandKind(command: string): string {
  if (/^(install_game|run_installer|move_install|update_game|check_game_updates)/.test(command)) return "install";
  if (/^(launch_game|launch_rom_game|stop_game)/.test(command)) return "launch";
  if (/^(check_for_updates|install_update|update_status)/.test(command)) return "update";
  return "command";
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
   * Persist only the launcher source selection (#155). `save_settings` merges
   * partial payloads onto the current settings, so the rest of the form is
   * untouched and no other tab's edit is dropped.
   */
  async saveLauncherSources(sources: LauncherId[]): Promise<LocalSettings | null> {
    if (!isNative()) return null;
    return invoke<LocalSettings>("save_settings", { settings: { launcherSources: sources } });
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

  /**
   * Every persisted save-location candidate (#146). Reads the local index, so
   * opening Settings never scans the disk.
   */
  async listSaveCandidates(): Promise<SaveCandidateOverview> {
    return invoke<SaveCandidateOverview>("list_save_candidates");
  },

  /** Run the OS/engine registry for every installed game (#146). */
  async scanSaveCandidates(): Promise<SaveCandidateOverview> {
    return invoke<SaveCandidateOverview>("scan_save_candidates");
  },

  /**
   * Confirm a candidate as a save path (#146). `editedPath` corrects a wrong
   * guess before saving; the original candidate is then dismissed.
   */
  async confirmSaveCandidate(
    gameId: number,
    path: string,
    editedPath?: string | null,
  ): Promise<SaveCandidateGroup> {
    return invoke<SaveCandidateGroup>("confirm_save_candidate", {
      gameId,
      path,
      editedPath: editedPath ?? null,
    });
  },

  /** Dismiss a candidate so a later scan does not resurface it (#146). */
  async ignoreSaveCandidate(gameId: number, path: string): Promise<SaveCandidateGroup> {
    return invoke<SaveCandidateGroup>("ignore_save_candidate", { gameId, path });
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

  /**
   * Claim phone-queued installs and mirror their progress (#167). Safe to call
   * on a timer; returns an empty result outside Tauri or when the server is
   * unreachable.
   */
  async remoteInstallsSync(): Promise<{ claimed: string[]; skipped: number; error: string | null }> {
    if (!isNative()) return { claimed: [], skipped: 0, error: null };
    try {
      return await invoke<{ claimed: string[]; skipped: number; error: string | null }>("remote_installs_sync");
    } catch (cause) {
      return { claimed: [], skipped: 0, error: String(cause) };
    }
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

  /** Current updater state without a network check (#163). */
  async updateStatus(): Promise<UpdateStatus | null> {
    if (!isNative()) return null;
    try {
      return await invoke<UpdateStatus>("update_status");
    } catch {
      return null;
    }
  },

  /**
   * Installed-binary self-check (#186): running exe/version/build commit,
   * other copies in the usual install roots, and a pending update that was
   * installed but is not running. Best effort: null in the browser or when the
   * command fails.
   */
  async installHealth(): Promise<InstallHealth | null> {
    if (!isNative()) return null;
    try {
      return await invoke<InstallHealth>("update_install_health");
    } catch {
      return null;
    }
  },

  /** Clear a stored update mismatch after the user acted on it (#186). */
  async dismissUpdateMismatch(): Promise<InstallHealth | null> {
    if (!isNative()) return null;
    try {
      return await invoke<InstallHealth>("dismiss_update_mismatch");
    } catch {
      return null;
    }
  },

  /** Persist the channel and re-check on it (#163). */
  async setUpdateChannel(channel: UpdateStatus["channel"]): Promise<UpdateStatus> {
    return invoke<UpdateStatus>("set_update_channel", { channel });
  },

  /** Persist (or clear with null) the skipped version (#163). */
  async setUpdateSkip(version: string | null): Promise<UpdateStatus> {
    return invoke<UpdateStatus>("set_update_skip", { version });
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

  async launchGame(args: { gameId: number; executable?: string | null; args?: string | null }): Promise<GameLaunchResult> {
    return invoke<GameLaunchResult>("launch_game", args);
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

  /**
   * Scan the enabled sources (#155). `sources` is the set the user ticked in
   * Settings → Integrations; omitting it lets the persisted setting decide.
   */
  async launcherScan(sources?: LauncherId[]): Promise<LauncherPreview> {
    return invoke<LauncherPreview>("launcher_scan", { sources: sources ?? null });
  },

  async launcherImport(
    entries: LauncherImportEntry[],
    sources?: LauncherId[],
  ): Promise<LauncherImportReport> {
    return invoke<LauncherImportReport>("launcher_import", {
      entries,
      sources: sources ?? null,
    });
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

  /* ------------------ Guided runtime installer (#178) ------------------ */

  /** The shipped, version-pinned runtime manifest. */
  async runtimeCatalog(): Promise<RuntimeCatalog> {
    return invoke<RuntimeCatalog>("runtime_catalog");
  },

  /** Diagnose why a game may not start; downloads nothing. */
  async makeItRunDiagnose(gameId: number): Promise<RuntimeDiagnosis> {
    return invoke<RuntimeDiagnosis>("make_it_run_diagnose", { gameId });
  },

  /** Apply selected steps after the consent click. URLs never leave the backend. */
  async makeItRunApply(gameId: number, stepIds: string[], consent: boolean): Promise<RuntimeApplyReport> {
    return invoke<RuntimeApplyReport>("make_it_run_apply", { gameId, stepIds, consent });
  },

  /** Ask the running apply to stop at the next chunk. */
  async makeItRunCancel(gameId: number): Promise<boolean> {
    return invoke<boolean>("make_it_run_cancel", { gameId });
  },

  /** Verify a user-supplied GPTK DMG and record it without copying it. */
  async makeItRunImport(gameId: number, providerId: string, path: string): Promise<RuntimeImportReport> {
    return invoke<RuntimeImportReport>("make_it_run_import", { gameId, providerId, path });
  },

  /** Native .dmg picker for the GPTK import. */
  async makeItRunPickImport(gameId: number): Promise<string | null> {
    return invoke<string | null>("make_it_run_pick_import", { gameId });
  },

  /** Delete a runtime Ludex installed; an imported DMG is only forgotten. */
  async makeItRunRemove(gameId: number, providerId: string): Promise<RuntimeRemovalReport> {
    return invoke<RuntimeRemovalReport>("make_it_run_remove", { gameId, providerId });
  },

  /** Persist a wrapper the user installed outside Ludex (macOS). */
  async makeItRunSelect(gameId: number, providerId: string): Promise<RuntimeDiagnosis> {
    return invoke<RuntimeDiagnosis>("make_it_run_select", { gameId, providerId });
  },

  /** Record the prerequisites a game declares. */
  async setRuntimePrerequisites(gameId: number, prerequisites: string[]): Promise<RuntimeDiagnosis> {
    return invoke<RuntimeDiagnosis>("set_runtime_prerequisites", { gameId, prerequisites });
  },

  async onRuntimeProgress(handler: (progress: RuntimeProgress) => void): Promise<() => void> {
    return listen<RuntimeProgress>("runtime://progress", handler);
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

  /** Save detection ran again (first page open or after a game exit, #158). */
  async onCloudSaveDetected(handler: (gameId: number) => void): Promise<() => void> {
    return listen<number>("cloud-save://detected", handler);
  },

  /**
   * Re-run save detection for one game (#158). Reads the Ludusavi manifest, the
   * OS registry and the manual paths; the resulting paths stay on this machine.
   */
  async detectSavePaths(gameId: number): Promise<void> {
    await invoke("redetect_save_paths", { gameId });
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

  /* ---------------- common config fields (#148) ------------------------- */

  /** Read-only detection of language/player-name keys; writes nothing. */
  async commonConfigFields(gameId: number): Promise<CommonFieldsReport> {
    return invoke<CommonFieldsReport>("game_config_common_fields", { gameId });
  },

  async previewCommonField(
    gameId: number,
    file: string,
    key: string,
    target: CommonFieldTarget,
  ): Promise<CommonFieldPreview> {
    return invoke<CommonFieldPreview>("preview_game_config_common_field", {
      gameId,
      file,
      key,
      target,
    });
  },

  async applyCommonField(
    gameId: number,
    file: string,
    key: string,
    target: CommonFieldTarget,
  ): Promise<ConfigWriteReport> {
    return invoke<ConfigWriteReport>("apply_game_config_common_field", {
      gameId,
      file,
      key,
      target,
    });
  },

  async saveConfigFieldMapping(
    gameId: number,
    field: CommonFieldTarget["field"],
    file: string,
    key: string,
  ): Promise<CommonFieldsReport> {
    return invoke<CommonFieldsReport>("save_game_config_field_mapping", {
      gameId,
      field,
      file,
      key,
    });
  },

  async deleteConfigFieldMapping(
    gameId: number,
    field: CommonFieldTarget["field"],
  ): Promise<CommonFieldsReport> {
    return invoke<CommonFieldsReport>("delete_game_config_field_mapping", { gameId, field });
  },

  /** JSON template of the pinned keys; shown so the user can save it. */
  async exportConfigMappings(gameId: number): Promise<string> {
    return invoke<string>("export_game_config_mappings", { gameId });
  },

  async configSyncPrefs(gameId: number): Promise<ConfigSyncPrefs> {
    return invoke<ConfigSyncPrefs>("game_config_sync_prefs", { gameId });
  },

  async setConfigSyncPrefs(gameId: number, prefs: ConfigSyncInput): Promise<ConfigSyncPrefs> {
    return invoke<ConfigSyncPrefs>("set_game_config_sync_prefs", { gameId, prefs });
  },

  /** Mirror the app locale/profile name for launch-time sync (#148). */
  async setConfigSyncDefaults(
    language: string,
    playerName: string,
  ): Promise<LocalSettings | null> {
    if (!isNative()) return null;
    return invoke<LocalSettings>("set_config_sync_defaults", { language, playerName });
  },

  /* ---------------------- Sunshine host (#97) ---------------------------- */

  /** Runtime console status (#172). In a browser the console is never active. */
  async consoleMode(): Promise<ConsoleStatus> {
    if (!isNative()) {
      return { active: false, fullscreen: false, appName: "Ludex", exitShortcut: "f12" };
    }
    return invoke<ConsoleStatus>("console_mode");
  },

  /** Quit a console session back to the desktop (#172). No-op in a browser. */
  async exitConsole(): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("exit_console");
  },

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

  /** One-time UAC grant for a Sunshine install under Program Files (#182). */
  async grantSunshineWriteAccess(): Promise<SunshineGrant> {
    return invoke<SunshineGrant>("grant_sunshine_write_access");
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

  /**
   * Remote play wizard (#179): detected LAN/IPv6 addresses, VPN services,
   * Sunshine state, UPnP gateway and whether direct mode can work. The public
   * IP comparison contacts a public address service; only the wizard runs it.
   */
  async remotePlayStatus(options: { probePublic?: boolean } = {}): Promise<RemotePlayStatus> {
    return invoke<RemotePlayStatus>("remote_play_status", {
      probePublic: options.probePublic ?? false,
    });
  },

  /** The exact ports, protocol and lease shown on the consent screen. */
  async remotePlayPlan(args: { microphone?: boolean; leaseSecs?: number } = {}): Promise<RemotePlayPortPlan> {
    return invoke<RemotePlayPortPlan>("remote_play_plan", {
      microphone: args.microphone ?? null,
      leaseSecs: args.leaseSecs ?? null,
    });
  },

  /**
   * Map the ports. `confirm` is the explicit consent the wizard collected; a
   * false value is refused by the backend.
   */
  async remotePlayEnable(args: {
    microphone?: boolean;
    leaseSecs?: number;
    confirm: boolean;
  }): Promise<RemotePlayReport> {
    return invoke<RemotePlayReport>("remote_play_enable", {
      microphone: args.microphone ?? null,
      leaseSecs: args.leaseSecs ?? null,
      confirm: args.confirm,
    });
  },

  async remotePlayVerify(): Promise<RemotePlayVerification> {
    return invoke<RemotePlayVerification>("remote_play_verify");
  },

  /** Re-add mappings whose lease is close to expiry. */
  async remotePlayRefresh(): Promise<RemotePlayReport> {
    return invoke<RemotePlayReport>("remote_play_refresh");
  },

  /** Remove every Ludex mapping and verify the deletion. */
  async remotePlayDisable(): Promise<RemotePlayCleanup> {
    return invoke<RemotePlayCleanup>("remote_play_disable");
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
   *
   * `serverUrl` signs in to a candidate server the first-run wizard has not
   * saved yet (#175); without it the active settings URL is used.
   */
  async oidcLoginStart(serverUrl?: string): Promise<OidcLoginStart> {
    if (!isNative()) throw nativeUnavailable();
    return invoke<OidcLoginStart>("oidc_login_start", serverUrl ? { serverUrl } : {});
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

  /* -------------------------- in-game overlay (#144) ----------------------- */

  /** Current overlay settings, running game and native listener status. */
  async overlayStatus(): Promise<OverlayStatus | null> {
    if (!isNative()) return null;
    try {
      return await invoke<OverlayStatus>("overlay_status");
    } catch {
      return null;
    }
  },

  /**
   * Persist a partial overlay change and apply it live (no restart). Only the
   * named fields are touched, so two panels cannot overwrite each other.
   */
  async setOverlaySettings(patch: OverlaySettingsPatch): Promise<OverlayStatus> {
    return invoke<OverlayStatus>("set_overlay_settings", { settings: patch });
  },

  /** Open the floating overlay (or the in-app overlay when it is unavailable). */
  async openOverlay(): Promise<OverlayStatus> {
    return invoke<OverlayStatus>("open_overlay");
  },

  /** Close the overlay and hand focus back to the game/main window. */
  async closeOverlay(): Promise<OverlayStatus> {
    return invoke<OverlayStatus>("close_overlay");
  },

  /** The game currently running, or null. */
  async overlayGameState(): Promise<OverlayGameState | null> {
    if (!isNative()) return null;
    try {
      return await invoke<OverlayGameState | null>("overlay_game_state");
    } catch {
      return null;
    }
  },

  /** Turn the overlay off (or back on) for one game. */
  async setOverlayGameDisabled(gameId: number, disabled: boolean): Promise<OverlayStatus> {
    return invoke<OverlayStatus>("set_overlay_game_disabled", { gameId, disabled });
  },

  /** Bring the main window forward on Big Picture and close the overlay. */
  async overlayShowBigPicture(): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("overlay_show_big_picture");
  },

  /** The desktop asked the main window to show the in-app overlay. */
  async onOverlayOpen(handler: () => void): Promise<() => void> {
    return listen<null>("overlay://open", () => handler());
  },

  async onOverlayClose(handler: () => void): Promise<() => void> {
    return listen<null>("overlay://close", () => handler());
  },

  /** Live settings/status update after `set_overlay_settings`. */
  async onOverlaySettings(handler: (status: OverlayStatus) => void): Promise<() => void> {
    return listen<OverlayStatus>("overlay://settings", handler);
  },

  /** "Open Big Picture" from the overlay: the path the main window must open. */
  async onOverlayNavigate(handler: (path: string) => void): Promise<() => void> {
    return listen<string>("overlay://navigate", handler);
  },

  /* ------------- opt-in diagnostics and crash reports (#171) ------------- */

  /** Local consent, install id and stored (already redacted) reports. */
  async diagnosticsStatus(): Promise<DiagnosticsStatus> {
    if (!isNative()) throw nativeUnavailable();
    return invoke<DiagnosticsStatus>("diagnostics_status");
  },

  /** Toggle the opt-in. Turning it off clears local reports and the id. */
  async setDiagnosticsEnabled(enabled: boolean): Promise<DiagnosticsStatus> {
    if (!isNative()) throw nativeUnavailable();
    return invoke<DiagnosticsStatus>("set_diagnostics_enabled", { enabled });
  },

  /** Exact payload a send would use, for the "what will be sent" view. */
  async diagnosticsPreview(reportId?: string): Promise<DiagnosticsPreview> {
    if (!isNative()) throw nativeUnavailable();
    return invoke<DiagnosticsPreview>("diagnostics_preview", { reportId: reportId ?? null });
  },

  /** Explicitly send one report. Refused while the toggle is off. */
  async diagnosticsSend(reportId: string): Promise<DiagnosticsSendOutcome> {
    if (!isNative()) throw nativeUnavailable();
    return invoke<DiagnosticsSendOutcome>("diagnostics_send", { reportId });
  },

  /** Delete every report stored on this device. */
  async diagnosticsDeleteReports(): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("diagnostics_delete_reports");
  },

  /** Record a web-view error locally; never uploads. */
  async diagnosticsRecordError(kind: string, summary: string, stack?: string | null): Promise<void> {
    if (!isNative()) return;
    await invoke<null>("diagnostics_record_error", { kind, summary, command: null, stack: stack ?? null });
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
  skippedUpdateVersion: null,
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
  deviceId: "",
  deviceName: null,
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
  overlayEnabled: true,
  overlayTrigger: "doubleHome",
  overlayOpacity: 0.92,
  overlayScale: 1,
  overlayTabs: ["game", "library", "downloads", "achievements"],
  overlayDisabledGames: [],
  diagnosticsEnabled: false,
  diagnosticsInstallId: "",
};
