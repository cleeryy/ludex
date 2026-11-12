import type { OverlayStatus, OverlayTabId } from "@/lib/types";

/**
 * Pure overlay logic (#144), shared by the floating overlay window, the in-app
 * overlay and the settings panel. Kept out of the components so the close,
 * focus-trap and tab rules can be unit-tested without a DOM.
 */

/** Tabs in canonical display order. */
export const OVERLAY_TABS: readonly OverlayTabId[] = [
  "game",
  "library",
  "downloads",
  "achievements",
];

/** Everything enabled, used when an older desktop does not report tabs. */
export const OVERLAY_DEFAULT_TABS: readonly OverlayTabId[] = [...OVERLAY_TABS];

/**
 * Tabs actually drawn. The game tab is always present (it owns Resume/Exit),
 * unknown ids are dropped, and the canonical order wins so a settings file
 * cannot reorder the overlay.
 */
export function visibleOverlayTabs(tabs: readonly string[] | null | undefined): OverlayTabId[] {
  const wanted = new Set<OverlayTabId>(["game"]);
  for (const tab of tabs ?? []) {
    if (OVERLAY_TABS.includes(tab as OverlayTabId)) wanted.add(tab as OverlayTabId);
  }
  return OVERLAY_TABS.filter((tab) => wanted.has(tab));
}

/** First tab to show: the running game, otherwise the first enabled content tab. */
export function defaultOverlayTab(status: Pick<OverlayStatus, "game" | "tabs"> | null | undefined): OverlayTabId {
  const tabs = visibleOverlayTabs(status?.tabs);
  if (status?.game) return "game";
  return tabs.find((tab) => tab !== "game") ?? "game";
}

/** Move the active tab by one with a wrap-around (LB/RB or Ctrl+Tab). */
export function switchOverlayTab(
  tabs: readonly OverlayTabId[],
  current: OverlayTabId,
  offset: number,
): OverlayTabId {
  if (tabs.length === 0) return "game";
  const index = Math.max(0, tabs.indexOf(current));
  const next = (index + offset + tabs.length * Math.abs(offset || 1)) % tabs.length;
  return tabs[next] ?? tabs[0];
}

/** Opacity is bounded so the game always stays visible behind the panel. */
export function clampOverlayOpacity(value: number | null | undefined): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return 0.92;
  return Math.round(Math.min(1, Math.max(0.35, value)) * 100) / 100;
}

/** Panel zoom, 0.6-1.3. */
export function clampOverlayScale(value: number | null | undefined): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return 1;
  return Math.round(Math.min(1.3, Math.max(0.6, value)) * 100) / 100;
}

/** Seconds elapsed since an RFC 3339 launch time; 0 for missing/invalid dates. */
export function overlayElapsedSeconds(startedAt: string | null | undefined, now = Date.now()): number {
  if (!startedAt) return 0;
  const started = Date.parse(startedAt);
  if (!Number.isFinite(started)) return 0;
  return Math.max(0, Math.floor((now - started) / 1000));
}

/** Session clock (`1:23:45`), never a localized string. */
export function formatOverlayClock(seconds: number): string {
  const safe = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  const hours = Math.floor(safe / 3600);
  const minutes = Math.floor((safe % 3600) / 60);
  const rest = safe % 60;
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${String(rest).padStart(2, "0")}`;
  }
  return `${minutes}:${String(rest).padStart(2, "0")}`;
}

/** Split a duration into translatable parts instead of baking English in. */
export function overlayDurationParts(seconds: number): { hours: number; minutes: number } {
  const safe = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  return { hours: Math.floor(safe / 3600), minutes: Math.floor((safe % 3600) / 60) };
}

/** Shift+Tab (no other modifier) is the keyboard trigger while Ludex is focused. */
export function isOverlayShortcut(event: {
  key: string;
  shiftKey: boolean;
  altKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
}): boolean {
  return (
    event.key === "Tab" &&
    event.shiftKey &&
    !event.altKey &&
    !event.ctrlKey &&
    !event.metaKey
  );
}

/** Escape and B close the overlay; both must work with a controller. */
export function isOverlayCloseKey(key: string): boolean {
  return key === "Escape" || key === "b" || key === "B";
}

/**
 * Tab focus trap. Returns the next index inside `[0, count)`, wrapping at both
 * ends so focus never escapes to the game behind the overlay.
 */
export function trapFocusIndex(index: number, count: number, shift: boolean): number {
  if (count <= 0) return 0;
  const current = Math.min(Math.max(0, index), count - 1);
  if (shift) return current <= 0 ? count - 1 : current - 1;
  return current >= count - 1 ? 0 : current + 1;
}

/** D-pad/stick wrap-around for a single row of targets. */
export function moveFocusIndex(index: number, count: number, offset: number): number {
  if (count <= 0) return 0;
  const current = Math.min(Math.max(0, index), count - 1);
  const step = offset === 0 ? 1 : offset;
  return ((current + step) % count + count) % count;
}

/**
 * Defensive merge for a desktop older than the web bundle (or a browser, where
 * the bridge returns nothing): every field gets a usable default so the overlay
 * still renders and Settings still shows the controls.
 */
export function normalizeOverlayStatus(raw: Partial<OverlayStatus> | null | undefined): OverlayStatus {
  const listener = raw?.listener;
  const diagnostics = raw?.diagnostics;
  const capabilities = raw?.capabilities;
  const triggerStatus = raw?.triggerStatus;
  return {
    enabled: raw?.enabled ?? true,
    open: raw?.open ?? false,
    trigger: raw?.trigger ?? "doubleHome",
    opacity: clampOverlayOpacity(raw?.opacity),
    scale: clampOverlayScale(raw?.scale),
    tabs: visibleOverlayTabs(raw?.tabs ?? OVERLAY_DEFAULT_TABS),
    disabledGames: Array.isArray(raw?.disabledGames) ? raw.disabledGames.filter((id) => id > 0) : [],
    game: raw?.game ?? null,
    gameDisabled: raw?.gameDisabled ?? false,
    presentation: raw?.presentation ?? "closed",
    listener: {
      backend: listener?.backend ?? "none",
      available: listener?.available ?? false,
      running: listener?.running ?? false,
      platform: listener?.platform ?? "unknown",
      reason: listener?.reason ?? null,
      controllers: Array.isArray(listener?.controllers) ? listener.controllers : [],
    },
    capabilities: {
      platform: capabilities?.platform ?? "unknown",
      floatingWindow: capabilities?.floatingWindow ?? false,
      alwaysOnTop: capabilities?.alwaysOnTop ?? false,
      globalListener: capabilities?.globalListener ?? false,
      guideButton: capabilities?.guideButton ?? false,
      chord: capabilities?.chord ?? false,
    },
    triggerStatus: {
      trigger: triggerStatus?.trigger ?? raw?.trigger ?? "doubleHome",
      guideEnabled: triggerStatus?.guideEnabled ?? true,
      chordEnabled: triggerStatus?.chordEnabled ?? true,
      effective: triggerStatus?.effective ?? "none",
      deliverable: triggerStatus?.deliverable ?? false,
      noteCode: triggerStatus?.noteCode ?? null,
      note: triggerStatus?.note ?? null,
    },
    diagnostics: {
      canOpen: diagnostics?.canOpen ?? ((raw?.enabled ?? true) && !(raw?.gameDisabled ?? false)),
      triggerDeliverable: diagnostics?.triggerDeliverable ?? false,
      overGame: diagnostics?.overGame ?? false,
      summary: diagnostics?.summary ?? "unknown",
      checks: Array.isArray(diagnostics?.checks) ? diagnostics.checks : [],
    },
    foreground: raw?.foreground ?? null,
    lastTrigger: raw?.lastTrigger ?? null,
    platformNote: raw?.platformNote ?? "",
    keyboardShortcut: raw?.keyboardShortcut ?? "Shift+Tab",
  };
}
