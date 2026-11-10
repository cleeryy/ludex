import { isNative, native } from "@/lib/native";
import type {
  CloudSaveRestoreOutcome,
  CloudSaveStatus,
  CloudSaveStrategy,
  CloudSaveToggle,
  CloudSyncToggleResponse,
} from "@/lib/types";

/**
 * Cloud save automation client (#108).
 *
 * Automatic backups archive files from this machine, so they only exist in the
 * Tauri shell. The browser build must explain that instead of pretending to
 * sync. This module is deliberately standalone: manual snapshot upload and
 * download still live in `lib/api.ts`, and nothing here duplicates them.
 */
export const cloudSavesAvailable = (): boolean => isNative();

export const CLOUD_SAVES_DESKTOP_ONLY =
  "Automatic cloud saves need the Ludex desktop app: they read and write save files on this machine. The browser build can still list, download and upload manual snapshots.";

export function cloudSaveReasonText(toggle: CloudSaveToggle): string {
  if (toggle.enabled) return "Automatic backup is on after each session.";
  if (toggle.reason === "game_off") return "Automatic backup is off for this game.";
  return "Automatic backup is off in the desktop settings.";
}

/** A device id is opaque; show a short, stable prefix only. */
export function shortDeviceId(deviceId: string | null | undefined): string {
  if (!deviceId) return "unknown device";
  return deviceId.length > 8 ? deviceId.slice(0, 8) : deviceId;
}

export const cloudSaves = {
  async status(gameId: number): Promise<CloudSaveStatus | null> {
    if (!isNative()) return null;
    return native.cloudSaveStatus(gameId);
  },

  /** `gameId: null` changes the global default. */
  async setSync(gameId: number | null, enabled: boolean): Promise<CloudSyncToggleResponse | null> {
    if (!isNative()) throw new Error(CLOUD_SAVES_DESKTOP_ONLY);
    return native.setCloudSaveSync(gameId, enabled);
  },

  async syncNow(gameId: number): Promise<CloudSaveStatus> {
    if (!isNative()) throw new Error(CLOUD_SAVES_DESKTOP_ONLY);
    return native.syncCloudSaveNow(gameId);
  },

  async restore(gameId: number, strategy: CloudSaveStrategy): Promise<CloudSaveRestoreOutcome> {
    if (!isNative()) throw new Error(CLOUD_SAVES_DESKTOP_ONLY);
    return native.restoreCloudSave(gameId, strategy);
  },

  async resolveConflict(
    gameId: number,
    strategy: CloudSaveStrategy,
  ): Promise<CloudSaveRestoreOutcome> {
    if (!isNative()) throw new Error(CLOUD_SAVES_DESKTOP_ONLY);
    return native.resolveCloudSaveConflict(gameId, strategy);
  },

  /** Live updates while a game page is open; no-op in the browser. */
  async onSynced(handler: (gameId: number) => void): Promise<() => void> {
    if (!isNative()) return () => {};
    return native.onCloudSaveSynced(handler);
  },

  async onPending(handler: (gameId: number) => void): Promise<() => void> {
    if (!isNative()) return () => {};
    return native.onCloudSavePending(handler);
  },
};
