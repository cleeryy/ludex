import type { ManifestStatus } from "@/lib/types";

/**
 * Pure state helpers for the save-database panel (#153). Kept out of the
 * component so the states are testable without a Tauri shell or a network.
 */

/** Coarse state the panel renders. */
export type ManifestState = "missing" | "ready" | "stale" | "error";

/**
 * A cached copy with a check error still works: the error only explains why
 * the last refresh did nothing. It is reported separately, and the state falls
 * back to `stale` when the copy is also old.
 */
export function manifestState(status: ManifestStatus | null | undefined): ManifestState {
  if (!status || !status.present) return "missing";
  if (status.error) return "error";
  return status.stale ? "stale" : "ready";
}

/** Badge tone matching a state: ready = ok, stale = signal, others outline. */
export function manifestTone(state: ManifestState): "ok" | "signal" | "outline" {
  if (state === "ready") return "ok";
  if (state === "stale") return "signal";
  return "outline";
}

export interface ManifestActions {
  /** A download can always be attempted, even when a copy exists. */
  canUpdate: boolean;
  /** Only a cached file can be removed. */
  canRemove: boolean;
}

export function manifestActions(status: ManifestStatus | null | undefined): ManifestActions {
  return {
    canUpdate: true,
    canRemove: Boolean(status?.present),
  };
}

/**
 * Short version label. `sha256:<hex>` keeps its prefix and a long digest is
 * shortened for the panel; an ETag-style value is shown as-is. A missing
 * version is an empty string, so the panel can render its own fallback.
 */
export function formatManifestVersion(version: string | null | undefined): string {
  const value = version?.trim() ?? "";
  if (!value) return "";
  const prefix = value.startsWith("sha256:") ? "sha256:" : "";
  const digest = value.slice(prefix.length);
  if (digest.length <= 20) return `${prefix}${digest}`;
  return `${prefix}${digest.slice(0, 16)}…`;
}
