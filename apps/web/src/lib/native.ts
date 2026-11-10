import type { DiskUsage, InstallGameArgs, InstallJob, InstalledGame, LocalSettings } from "@/lib/types";

/**
 * Native bridge. Everything is a no-op or a friendly rejection in a browser;
 * in Tauri the same functions call the Rust commands from docs/CONTRACT.md.
 */

export const isNative = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in (window as unknown as Record<string, unknown>);

export function nativeUnavailable(): Error {
  return new Error("That needs the Ludex desktop app. The browser build can browse and download.");
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

  async getInstallJob(jobId: string): Promise<InstallJob> {
    return invoke<InstallJob>("get_install_job", { jobId });
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

  async openPath(path: string): Promise<void> {
    if (!isNative()) throw nativeUnavailable();
    await invoke<null>("open_path", { path });
  },

  async getDiskUsage(path: string): Promise<DiskUsage> {
    return invoke<DiskUsage>("get_disk_usage", { path });
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

  async onGameExited(handler: (payload: { sessionId: string; gameId: number; minutesPlayed: number }) => void): Promise<() => void> {
    return listen<{ sessionId: string; gameId: number; minutesPlayed: number }>("game://exited", handler);
  },

  async onLocalServerStatus(handler: (payload: { running: boolean; url: string | null }) => void): Promise<() => void> {
    return listen<{ running: boolean; url: string | null }>("local-server://status", handler);
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
};
