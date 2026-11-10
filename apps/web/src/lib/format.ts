const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) return "—";
  if (bytes <= 0) return "0 B";
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), BYTE_UNITS.length - 1);
  const value = bytes / 1024 ** exponent;
  const decimals = exponent === 0 ? 0 : value >= 100 ? 0 : digits;
  return `${value.toFixed(decimals)} ${BYTE_UNITS[exponent]}`;
}

export function formatSpeed(bps: number | null | undefined): string {
  if (!bps) return "—";
  return `${formatBytes(bps, 0)}/s`;
}

export function formatEta(downloaded: number, total: number | null, speed: number): string | null {
  if (!total || !speed || total <= downloaded) return null;
  const seconds = Math.round((total - downloaded) / speed);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

export function formatMinutes(minutes: number | null | undefined): string | null {
  if (!minutes || minutes <= 0) return null;
  const hours = Math.floor(minutes / 60);
  const rest = Math.round(minutes % 60);
  if (hours === 0) return `${rest}m`;
  if (rest === 0) return `${hours}h`;
  return `${hours}h ${rest}m`;
}

/** Duration from seconds: `45s`, `12m`, `3h 05m`. */
export function formatSeconds(seconds: number | null | undefined): string | null {
  if (!seconds || seconds <= 0) return null;
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}h` : `${hours}h ${String(rest).padStart(2, "0")}m`;
}

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", year: "numeric" }).format(date);
}

export function formatYear(iso: string | null | undefined): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return String(date.getFullYear());
}

export function formatRelative(iso: string | null | undefined): string {
  if (!iso) return "Never";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "Never";
  const diff = then - Date.now();
  const minutes = Math.round(diff / 60_000);
  const formatter = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
  if (Math.abs(minutes) < 60) return formatter.format(minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (Math.abs(hours) < 24) return formatter.format(hours, "hour");
  const days = Math.round(hours / 24);
  if (Math.abs(days) < 30) return formatter.format(days, "day");
  const months = Math.round(days / 30);
  if (Math.abs(months) < 12) return formatter.format(months, "month");
  return formatter.format(Math.round(months / 12), "year");
}

/**
 * Provider ratings arrive as floats (IGDB sends 74.6246...). The UI only ever
 * shows a whole number out of 100, so round once here.
 */
export function formatRating(rating: number | null | undefined): string | null {
  if (rating === null || rating === undefined || Number.isNaN(rating)) return null;
  return String(Math.round(rating));
}

export function formatCount(n: number, singular: string, plural = `${singular}s`): string {
  return `${n} ${n === 1 ? singular : plural}`;
}

export function initials(name: string): string {
  const parts = name.trim().split(/[\s._-]+/).filter(Boolean);
  if (!parts.length) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

export function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

export function titleCase(input: string): string {
  return input.replace(/\b\w/g, (c) => c.toUpperCase());
}

export function installPhaseLabel(phase: string): string {
  switch (phase) {
    case "queued":
      return "Queued";
    case "downloading":
      return "Downloading";
    case "extracting":
      return "Extracting";
    case "finalizing":
      return "Finalizing";
    case "paused":
      return "Paused";
    case "done":
      return "Installed";
    case "cancelled":
      return "Cancelled";
    case "error":
      return "Failed";
    case "interrupted":
      return "Interrupted";
    default:
      return phase;
  }
}

/** "onHold" -> "On hold", "planToPlay" -> "Planned" */
export function stateLabel(state: string): string {
  switch (state) {
    case "onHold":
      return "On hold";
    case "planToPlay":
      return "Planned";
    case "unplayed":
      return "Unplayed";
    case "playing":
      return "Playing";
    case "completed":
      return "Completed";
    case "abandoned":
      return "Abandoned";
    default:
      return titleCase(state);
  }
}
