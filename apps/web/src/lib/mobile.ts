import { t } from "@/lib/i18n";
import type { RemoteInstall, RemoteInstallStatus } from "@/lib/types";

/**
 * Mobile companion helpers (#167). Pure functions only: the screen passes the
 * current time explicitly so the "is the desktop offline?" hint is testable
 * without timers, and every status maps to one English message id that the
 * catalog translates.
 */

/** Statuses that still occupy a slot in the phone's queue. */
export const REMOTE_ACTIVE_STATUSES: readonly RemoteInstallStatus[] = [
  "queued",
  "claimed",
  "downloading",
];

/** A request that has not moved for this long is surfaced as "PC offline". */
export const REMOTE_STALE_AFTER_MS = 120_000;

/** Whether the request is still on its way to the desktop. */
export function isRemoteActive(status: RemoteInstallStatus): boolean {
  return REMOTE_ACTIVE_STATUSES.includes(status);
}

/** English message id for a status badge. */
export function remoteStatusKey(status: RemoteInstallStatus): string {
  switch (status) {
    case "queued":
      return "Waiting for a desktop";
    case "claimed":
      return "Desktop found, preparing";
    case "downloading":
      return "Downloading on the desktop";
    case "ready":
      return "Ready to play on the desktop";
    case "failed":
      return "Install failed";
    case "cancelled":
      return "Cancelled";
  }
}

/**
 * Translated status badge. The literal `t()` calls (rather than
 * `t(remoteStatusKey(...))`) let the catalog extractor see every message id.
 */
export function remoteStatusLabel(status: RemoteInstallStatus): string {
  switch (status) {
    case "queued":
      return t("Waiting for a desktop");
    case "claimed":
      return t("Desktop found, preparing");
    case "downloading":
      return t("Downloading on the desktop");
    case "ready":
      return t("Ready to play on the desktop");
    case "failed":
      return t("Install failed");
    case "cancelled":
      return t("Cancelled");
  }
}

/** Badge tone matching the status. */
export function remoteStatusTone(status: RemoteInstallStatus): "default" | "signal" | "ok" | "danger" | "outline" {
  switch (status) {
    case "queued":
      return "outline";
    case "claimed":
    case "downloading":
      return "signal";
    case "ready":
      return "ok";
    case "failed":
      return "danger";
    case "cancelled":
      return "default";
  }
}

/**
 * Percentage for the progress bar. Only a download reports real progress; a
 * queued request is indeterminate (`null`) instead of a fake 0 %.
 */
export function remotePercent(install: Pick<RemoteInstall, "status" | "progress">): number | null {
  if (install.status === "ready") return 100;
  if (install.status === "queued" || install.status === "claimed") return null;
  return Math.min(100, Math.max(0, install.progress));
}

/**
 * True when a queued or just-claimed request has waited long enough that the
 * desktop is probably closed. The phone then says "PC offline" instead of
 * silently spinning forever. A download that is making progress is left alone.
 */
export function remoteLooksOffline(
  install: Pick<RemoteInstall, "status" | "updatedAt">,
  now: number = Date.now(),
  thresholdMs: number = REMOTE_STALE_AFTER_MS,
): boolean {
  if (install.status !== "queued" && install.status !== "claimed") return false;
  const updated = Date.parse(install.updatedAt);
  return Number.isFinite(updated) && now - updated > thresholdMs;
}

/** Counts grouped for the screen header. */
export function remoteSummary(installs: readonly RemoteInstall[]): {
  active: number;
  ready: number;
  failed: number;
} {
  let active = 0;
  let ready = 0;
  let failed = 0;
  for (const install of installs) {
    if (isRemoteActive(install.status)) active += 1;
    else if (install.status === "ready") ready += 1;
    else if (install.status === "failed") failed += 1;
  }
  return { active, ready, failed };
}

/**
 * Human label for the requesting device. `device` is only a label — it never
 * grants access — so a best guess from the UA is enough.
 */
export function deviceLabel(userAgent: string): string {
  if (/iPhone/i.test(userAgent)) return "iPhone";
  if (/iPad/i.test(userAgent)) return "iPad";
  if (/Android/i.test(userAgent)) return "Android phone";
  if (/Mobile/i.test(userAgent)) return "Mobile browser";
  return "Web browser";
}

/** Phone/tablet layout threshold, matching Tailwind's `lg` breakpoint. */
export function isPhoneViewport(width: number): boolean {
  return width < 1024;
}

/** True when the PWA runs from the home screen rather than a browser tab. */
export function isStandaloneDisplay(displayMode?: string): boolean {
  return displayMode === "standalone" || displayMode === "fullscreen";
}

/**
 * Remote installs are a browser feature: inside Tauri the install is local and
 * the desktop queue already owns it.
 */
export function shouldOfferRemoteInstall(options: { native: boolean; signedIn: boolean }): boolean {
  return !options.native && options.signedIn;
}
