import type { AudioVolumeState } from "@/lib/types";

/**
 * The console's volume OSD (#289).
 *
 * The desktop OSD cannot be used from a pad or from the fullscreen console, so
 * Ludex draws its own: the state comes from `pactl`, the OSD shows the same
 * percentage the sound server reports, and it hides itself after a short idle.
 * The maths is pure so the expiry and the step clamp are testable without a
 * timer or an audio device.
 */

/** How long the OSD stays on screen after the last change. */
export const VOLUME_OSD_MS = 1_800;

export interface VolumeOsd {
  sink: string;
  label: string;
  percent: number;
  muted: boolean;
  /** Monotonic stamp of the state this OSD shows. */
  at: number;
}

/** Clamp a reported percentage to the only range an OSD can draw. */
export function clampVolumePercent(percent: number): number {
  if (!Number.isFinite(percent)) return 0;
  return Math.min(100, Math.max(0, Math.round(percent)));
}

/** Build the OSD from one backend state, stamped for the auto-hide timer. */
export function volumeOsdFor(state: AudioVolumeState, at: number): VolumeOsd {
  return {
    sink: state.sink,
    label: state.label,
    percent: clampVolumePercent(state.percent),
    muted: state.muted,
    at,
  };
}

/** True while the OSD should stay visible. */
export function volumeOsdVisible(osd: VolumeOsd | null, now: number): boolean {
  return osd !== null && now - osd.at < VOLUME_OSD_MS;
}

/** The label announced by the OSD, e.g. `55%`. */
export function volumeOsdLabel(osd: VolumeOsd): string {
  return `${osd.percent}%`;
}

/**
 * Apply one step locally before the server answers, so a held pad direction
 * feels immediate. The backend state replaces it on the next frame.
 */
export function steppedVolumePercent(percent: number, delta: -1 | 1, step = 5): number {
  return clampVolumePercent(percent + delta * step);
}

export type VolumeKeyAction = "up" | "down" | "toggleMute";

/**
 * Map a keyboard/remote media key to a volume action.
 *
 * Browsers disagree on the spelling (`AudioVolumeUp`, `VolumeUp`) and X11
 * key names can arrive through a remote bridge (`XF86AudioRaiseVolume`), so the
 * match is case-insensitive and covers all three families. `null` means the
 * Console leaves the key to the DOM.
 */
export function volumeKeyAction(key: string): VolumeKeyAction | null {
  switch (key.trim().toLowerCase()) {
    case "audiovolumeup":
    case "volumeup":
    case "xf86audioraisevolume":
    case "audioraisevolume":
      return "up";
    case "audiovolumedown":
    case "volumedown":
    case "xf86audiolowervolume":
    case "audiolowervolume":
      return "down";
    case "audiovolumemute":
    case "volumemute":
    case "xf86audiomute":
    case "audiomute":
      return "toggleMute";
    default:
      return null;
  }
}
