import { isNative, native } from "@/lib/native";
import type {
  LocalSaveRestoreOutcome,
  SavePruneReport,
  SaveRestorePoint,
  SaveRestorePointsView,
} from "@/lib/types";

/**
 * Local save-manager history (#295).
 *
 * Restore points are copies of this machine's save files, so they only exist in
 * the Tauri shell. The browser build lists and downloads cloud snapshots but
 * cannot roll back a local folder.
 */
export const SAVE_MANAGER_DESKTOP_ONLY =
  "Restore points keep a copy of the save files on this device, so they need the Ludex desktop app. The browser build still lists your cloud snapshots.";

/** Mirror of the desktop default (`settings::default_save_restore_retention`). */
export const DEFAULT_RETENTION = 5;
export const MIN_RETENTION = 1;
export const MAX_RETENTION = 50;

/** Clamp a retention window the same way the desktop does (1–50, default 5). */
export function clampRetention(value: number | null | undefined): number {
  if (value === null || value === undefined || !Number.isFinite(value)) return DEFAULT_RETENTION;
  return Math.min(MAX_RETENTION, Math.max(MIN_RETENTION, Math.round(value)));
}

/**
 * How many points an explicit prune would remove. The newest point is the last
 * known-good copy, so it never counts as removable.
 */
export function pointsToPrune(total: number, keep: number): number {
  if (!Number.isFinite(total) || total <= 0) return 0;
  return Math.max(0, Math.floor(total) - clampRetention(keep));
}

export const saveHistoryAvailable = (): boolean => isNative();

export const saveHistory = {
  async list(gameId: number): Promise<SaveRestorePointsView> {
    if (!isNative()) throw new Error(SAVE_MANAGER_DESKTOP_ONLY);
    return native.listSaveRestorePoints(gameId);
  },

  /** `null` when the bytes already match the newest point. */
  async capture(gameId: number, label?: string): Promise<SaveRestorePoint | null> {
    if (!isNative()) throw new Error(SAVE_MANAGER_DESKTOP_ONLY);
    return native.captureSaveRestorePoint(gameId, label);
  },

  async restore(gameId: number, pointId: number): Promise<LocalSaveRestoreOutcome> {
    if (!isNative()) throw new Error(SAVE_MANAGER_DESKTOP_ONLY);
    return native.restoreSavePoint(gameId, pointId);
  },

  /** `keep` defaults to the stored window on the desktop. */
  async prune(gameId: number, keep?: number): Promise<SavePruneReport> {
    if (!isNative()) throw new Error(SAVE_MANAGER_DESKTOP_ONLY);
    return native.pruneSaveRestorePoints(gameId, keep);
  },

  async setRetention(sessions: number): Promise<number> {
    if (!isNative()) throw new Error(SAVE_MANAGER_DESKTOP_ONLY);
    return native.setSaveRestoreRetention(clampRetention(sessions));
  },
};
