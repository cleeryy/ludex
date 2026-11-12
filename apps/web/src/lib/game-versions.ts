import type { GameVersion, GameVersionInfo } from "@/lib/types";

/**
 * Version state and rollback decisions (#168).
 *
 * The server is the authority (it refuses unsafe deletions), but the UI must
 * explain the same rules before an editor clicks: which archive is the
 * preferred one, whether installing it is a rollback, what may be deleted, and
 * which fingerprint to show. These helpers stay pure so they can be unit
 * tested without React or a server.
 */

type VersionLike = Pick<GameVersion, "id" | "fileName" | "version" | "size" | "releaseDate" | "updatedAt"> & {
  indexedAt?: string | null;
  missing?: boolean;
  rootUnavailable?: boolean;
};

type VersionStateLike = Pick<GameVersionInfo, "id" | "missing" | "rootUnavailable">;

/** How one row should be presented in the manager. */
export type VersionState = "default" | "available" | "missing" | "unavailable";

/** Human label for a version: `v1.2.3`, else the archive name. */
export function versionLabel(version: Pick<VersionLike, "version" | "fileName">): string {
  return version.version?.trim() ? `v${version.version.trim()}` : version.fileName;
}

/** Timestamp used by the server ordering: release date, else indexed date. */
function versionStamp(version: VersionLike): number {
  const raw = version.releaseDate ?? version.indexedAt ?? version.updatedAt ?? null;
  const parsed = raw ? Date.parse(raw) : Number.NaN;
  return Number.isNaN(parsed) ? 0 : parsed;
}

/**
 * Newest first, mirroring `ORDER BY COALESCE(release_date, indexed_at) DESC,
 * id DESC`. Stable for equal timestamps, so a table never jumps between
 * renders.
 */
export function sortVersionsNewestFirst<T extends VersionLike>(versions: readonly T[]): T[] {
  return [...versions].sort((a, b) => versionStamp(b) - versionStamp(a) || b.id - a.id);
}

/** The pinned version, or the newest available row when no pin is set. */
export function preferredVersion<T extends VersionLike>(
  versions: readonly T[],
  defaultVersionId: number | null | undefined,
): T | null {
  const ordered = sortVersionsNewestFirst(versions);
  const available = ordered.filter((version) => !version.missing && !version.rootUnavailable);
  if (defaultVersionId != null) {
    const pinned = available.find((version) => version.id === defaultVersionId);
    if (pinned) return pinned;
  }
  return available[0] ?? ordered[0] ?? null;
}

/** Presentation state for one row. */
export function versionState(
  version: VersionStateLike,
  defaultVersionId: number | null | undefined,
): VersionState {
  if (version.missing) return "missing";
  if (version.rootUnavailable) return "unavailable";
  if (defaultVersionId != null && version.id === defaultVersionId) return "default";
  return "available";
}

/**
 * Whether `target` is older than `reference` in the server ordering. A target
 * or reference that is not in the list (a deleted row, an imported install) is
 * not declared a rollback.
 */
export function isRollbackVersion<T extends VersionLike>(
  target: Pick<T, "id"> | null | undefined,
  reference: Pick<T, "id"> | null | undefined,
  versions: readonly T[],
): boolean {
  if (!target || !reference || target.id === reference.id) return false;
  const ordered = sortVersionsNewestFirst(versions);
  const targetIndex = ordered.findIndex((version) => version.id === target.id);
  const referenceIndex = ordered.findIndex((version) => version.id === reference.id);
  return targetIndex >= 0 && referenceIndex >= 0 && targetIndex > referenceIndex;
}

/**
 * Whether installing `targetId` replaces an installed newer version — the
 * explicit rollback warning in the install dialog.
 */
export function isRollbackInstall<T extends VersionLike>(
  targetId: number | null | undefined,
  installedVersionId: number | null | undefined,
  versions: readonly T[],
): boolean {
  if (targetId == null || installedVersionId == null) return false;
  return isRollbackVersion({ id: targetId }, { id: installedVersionId }, versions);
}

/** Why the UI must not offer a delete for one row. */
export type DeleteBlockReason = "missing" | "unavailable" | "last";

export interface DeleteEligibility {
  allowed: boolean;
  reason: DeleteBlockReason | null;
}

/**
 * Mirrors the server guard for the rows the client knows about. Active
 * downloads are detected server-side (`409 version_in_use`): the browser
 * cannot see another device's transfer.
 */
export function canDeleteVersion<T extends VersionLike & VersionStateLike>(
  versions: readonly T[],
  target: VersionStateLike & Pick<GameVersion, "id">,
): DeleteEligibility {
  if ("missing" in target && target.missing) return { allowed: false, reason: "missing" };
  if ("rootUnavailable" in target && target.rootUnavailable) {
    return { allowed: false, reason: "unavailable" };
  }
  const available = versions.filter((version) => !version.missing && !version.rootUnavailable);
  if (available.length <= 1) return { allowed: false, reason: "last" };
  return { allowed: true, reason: null };
}

/** Bytes freed when one archive is deleted. */
export function freedVersionBytes(version: Pick<VersionLike, "size">): number {
  return Math.max(0, version.size);
}

/**
 * Short fingerprint for a table row: `sha256-sample:abcd1234…` becomes the
 * first 12 significant characters, with the scheme kept separate.
 */
export function versionHashShort(hash: string | null | undefined): string | null {
  const value = hash?.trim();
  if (!value) return null;
  const [, digest] = value.includes(":") ? value.split(":", 2) : [value, value];
  const trimmed = digest?.trim();
  if (!trimmed) return null;
  return trimmed.slice(0, 12);
}

/**
 * Whether the preferred version differs from what is installed. The server's
 * `/versions/latest` payload follows the pin, so a different id means an
 * update (or a deliberate rollback target) is available.
 */
export function preferredDiffersFromInstalled(
  defaultVersionId: number | null | undefined,
  installedVersionId: number | null | undefined,
): boolean {
  return defaultVersionId != null && installedVersionId != null && defaultVersionId !== installedVersionId;
}
