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

/** Detection is re-run on a game page at most once per mount, and only when the
 * last run is missing or older than this (#158). After a session the desktop
 * re-runs it itself, so a page never needs to poll. */
export const DETECTION_MAX_AGE_MS = 10 * 60_000;

/**
 * Whether opening the game page should ask the desktop to detect save folders.
 * Pure so the decision is testable: it must never become a render/poll loop.
 */
export function shouldAutoDetect(
  status: { paths: readonly unknown[]; lastDetectionAt: string | null } | null | undefined,
  now: number = Date.now(),
  maxAgeMs: number = DETECTION_MAX_AGE_MS,
): boolean {
  if (!status) return false;
  if (status.paths.length > 0) return false;
  if (!status.lastDetectionAt) return true;
  const detectedAt = Date.parse(status.lastDetectionAt);
  if (!Number.isFinite(detectedAt)) return true;
  return now - detectedAt > maxAgeMs;
}

/**
 * Backoff for the panel's manual retry. A slow or failing `cloud_save_status`
 * must not be hammered: the button is disabled for 2 s, 4 s, 8 s … capped at
 * 30 s between attempts. Returns 0 for the first attempt.
 */
export function retryDelayMs(failures: number): number {
  if (!Number.isFinite(failures) || failures <= 0) return 0;
  const exponent = Math.min(Math.floor(failures) - 1, 4);
  return Math.min(2_000 * 2 ** exponent, 30_000);
}

/**
 * One error toast per failure streak: the same error is not repeated on every
 * refetch or event invalidation. Returns true only when the key changed.
 */
export function shouldToastFailure(seenKey: string | null, nextKey: string): boolean {
  return seenKey !== nextKey;
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

  /** Save detection ran again (#158). */
  async onDetected(handler: (gameId: number) => void): Promise<() => void> {
    if (!isNative()) return () => {};
    return native.onCloudSaveDetected(handler);
  },

  /** Re-run detection for one game; resolves when the mapping is persisted. */
  async detect(gameId: number): Promise<void> {
    if (!isNative()) return;
    return native.detectSavePaths(gameId);
  },
};
