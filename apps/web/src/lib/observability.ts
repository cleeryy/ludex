/**
 * Pure helpers for the admin observability dashboard (#170).
 *
 * The screen renders SVG sparklines without a chart dependency; keeping the
 * geometry here means it can be tested without a DOM and the component stays
 * about layout only.
 */

import type { ObservabilityPoint, ObservabilityWindow } from "@/lib/types";

/** Windows offered by the dashboard selector. */
export const OBSERVABILITY_WINDOWS: readonly ObservabilityWindow[] = ["1h", "24h"];

/** Type guard for a query value coming from the selector or the URL. */
export function isObservabilityWindow(value: string): value is ObservabilityWindow {
  return (OBSERVABILITY_WINDOWS as readonly string[]).includes(value);
}

/** Human label for a window: `1 h` / `24 h`. */
export function windowLabel(window: ObservabilityWindow): string {
  return window === "1h" ? "1 h" : "24 h";
}

export interface SparklineGeometry {
  /** Polyline `points` attribute, empty when there is no data. */
  line: string;
  /** Closed path filling the area under the line, empty when there is no data. */
  area: string;
  /** Largest value in the series (0 when empty). */
  max: number;
  /** Value of the last point, or 0. */
  last: number;
}

/**
 * Projects a number series onto a fixed viewBox.
 *
 * Handles the empty series (no path), a single point (flat line) and a flat
 * series (line at the middle) without dividing by zero.
 */
export function sparkline(
  values: readonly number[],
  width = 240,
  height = 56,
  padding = 4,
): SparklineGeometry {
  if (values.length === 0) return { line: "", area: "", max: 0, last: 0 };
  const max = Math.max(...values);
  const scale = max > 0 ? max : 1;
  const usableHeight = Math.max(1, height - padding * 2);
  const yFor = (value: number) => height - padding - (Math.max(0, value) / scale) * usableHeight;
  const xFor = (index: number) =>
    values.length === 1 ? 0 : (index / (values.length - 1)) * width;
  const coords =
    values.length === 1
      ? [
          [0, yFor(values[0])] as const,
          [width, yFor(values[0])] as const,
        ]
      : values.map((value, index) => [xFor(index), yFor(value)] as const);
  const line = coords.map(([x, y]) => `${round(x)},${round(y)}`).join(" ");
  const last = values[values.length - 1] ?? 0;
  const area = `M0,${height} ${coords.map(([x, y]) => `L${round(x)},${round(y)}`).join(" ")} L${width},${height} Z`;
  return { line, area, max, last };
}

/** Values of the series, optionally only the error-carrying points. */
export function seriesValues(
  points: readonly ObservabilityPoint[],
  metric: "requests" | "errors" | "latency" = "requests",
): number[] {
  return points.map((point) => {
    if (metric === "errors") return point.clientErrors + point.serverErrors;
    if (metric === "latency") return point.averageMs;
    return point.requests;
  });
}

/** `0.0234` -> `2.3%`; keeps small shares visible instead of rounding to 0 %. */
export function formatShare(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0%";
  const percent = value * 100;
  return `${percent < 10 ? percent.toFixed(1) : Math.round(percent)}%`;
}

/** Milliseconds, with one decimal below 10 ms and none above. */
export function formatMs(value: number): string {
  if (!Number.isFinite(value) || value < 0) return "0 ms";
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ms`;
}

/** Requests per second, with enough precision to show idle vs busy. */
export function formatRate(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0/s";
  return `${value < 1 ? value.toFixed(2) : value.toFixed(1)}/s`;
}

function round(value: number): number {
  return Math.round(value * 10) / 10;
}
