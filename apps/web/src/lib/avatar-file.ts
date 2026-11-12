/**
 * Pure client-side checks for avatar uploads (#183). Kept apart from the
 * uploader so tests can exercise them without importing the API client (which
 * boots the demo mocks and touches `localStorage`).
 */

/** Accepted upload formats, mirrored by the server's magic-byte check. */
export const AVATAR_ACCEPT = "image/png,image/jpeg,image/webp";
/** Server-side limit, checked client-side before the upload starts. */
export const AVATAR_MAX_BYTES = 2 * 1024 * 1024;

export type AvatarFileProblem = "type" | "size";

/** Rejects a file before any bytes leave the browser. */
export function avatarFileProblem(file: File): AvatarFileProblem | null {
  if (!AVATAR_ACCEPT.split(",").includes(file.type)) return "type";
  if (file.size <= 0) return "type";
  if (file.size > AVATAR_MAX_BYTES) return "size";
  return null;
}
