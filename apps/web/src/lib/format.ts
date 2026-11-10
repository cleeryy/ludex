import { formatDateValue, formatNumber, formatRelativeTime, t } from "@/lib/i18n";

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) return "—";
  if (bytes <= 0) return "0 B";
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), BYTE_UNITS.length - 1);
  const value = bytes / 1024 ** exponent;
  const decimals = exponent === 0 ? 0 : value >= 100 ? 0 : digits;
  return `${formatNumber(value, { minimumFractionDigits: decimals, maximumFractionDigits: decimals })} ${BYTE_UNITS[exponent]}`;
}

export function formatSpeed(bps: number | null | undefined): string {
  if (!bps) return "—";
  return `${formatBytes(bps, 0)}/s`;
}

/**
 * A duration for ETA and elapsed labels: `42s`, `3m 05s`, `2h 10m`, `1d 4h`.
 * Pure and rounded once, so a gated ETA does not flicker between formats.
 */
export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0s";
  const total = Math.round(seconds);
  if (total < 60) return `${formatNumber(total)}s`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${formatNumber(minutes)}m ${formatNumber(total % 60, { minimumIntegerDigits: 2, useGrouping: false })}s`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${formatNumber(hours)}h ${formatNumber(minutes % 60, { minimumIntegerDigits: 2, useGrouping: false })}m`;
  return `${formatNumber(Math.floor(hours / 24))}d ${formatNumber(hours % 24)}h`;
}

export function formatEta(downloaded: number, total: number | null, speed: number): string | null {
  if (total === null || !Number.isFinite(total) || !Number.isFinite(speed) || speed <= 0 || total <= downloaded) {
    return null;
  }
  return formatDuration((total - downloaded) / speed);
}

export function formatMinutes(minutes: number | null | undefined): string | null {
  if (!minutes || minutes <= 0) return null;
  const hours = Math.floor(minutes / 60);
  const rest = Math.round(minutes % 60);
  if (hours === 0) return `${formatNumber(rest)}m`;
  if (rest === 0) return `${formatNumber(hours)}h`;
  return `${formatNumber(hours)}h ${formatNumber(rest)}m`;
}

/** Duration from seconds: `45s`, `12m`, `3h 05m`. */
export function formatSeconds(seconds: number | null | undefined): string | null {
  if (!seconds || seconds <= 0) return null;
  if (seconds < 60) return `${formatNumber(Math.round(seconds))}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${formatNumber(minutes)}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${formatNumber(hours)}h` : `${formatNumber(hours)}h ${formatNumber(rest, { minimumIntegerDigits: 2, useGrouping: false })}m`;
}

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  return formatDateValue(iso);
}

export function formatYear(iso: string | null | undefined): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "—";
  return formatNumber(date.getFullYear(), { useGrouping: false });
}

export function formatRelative(iso: string | null | undefined): string {
  return formatRelativeTime(iso);
}

/**
 * Provider ratings arrive as floats (IGDB sends 74.6246...). The UI only ever
 * shows a whole number out of 100, so round once here.
 */
/** Locale-aware percentage with the percent sign attached (`72,8%`, `72.8%`). */
export function formatPercent(value: number | null | undefined, digits = 0): string {
  if (value === null || value === undefined || Number.isNaN(value)) return "—";
  return `${formatNumber(value, { minimumFractionDigits: digits, maximumFractionDigits: digits })}%`;
}

export function formatRating(rating: number | null | undefined): string | null {
  if (rating === null || rating === undefined || Number.isNaN(rating)) return null;
  return formatNumber(Math.round(rating), { useGrouping: false });
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

export function sortLabel(sort: string): string {
  switch (sort) {
    case "releaseDate":
      return t("Release date");
    case "addedAt":
      return t("Date added");
    case "lastPlayed":
      return t("Last played");
    case "playtime":
      return t("Playtime");
    case "rating":
      return t("Rating");
    case "title":
    default:
      return t("Title");
  }
}

export function installPhaseLabel(phase: string): string {
  switch (phase) {
    case "queued":
      return t("Queued");
    case "downloading":
      return t("Downloading");
    case "extracting":
      return t("Extracting");
    case "finalizing":
      return t("Finalizing");
    case "paused":
      return t("Paused");
    case "done":
      return t("Installed");
    case "cancelled":
      return t("Cancelled");
    case "error":
      return t("Failed");
    case "interrupted":
      return t("Interrupted");
    default:
      return phase;
  }
}

/** "onHold" -> "On hold", "planToPlay" -> "Planned" */
export function stateLabel(state: string): string {
  switch (state) {
    case "onHold":
      return t("On hold");
    case "planToPlay":
      return t("Planned");
    case "unplayed":
      return t("Unplayed");
    case "playing":
      return t("Playing");
    case "completed":
      return t("Completed");
    case "abandoned":
      return t("Abandoned");
    default:
      return titleCase(state);
  }
}
