/**
 * Custom artwork uploads (#38).
 *
 * The responses keep their domain types here; the request itself goes through
 * the one adapter (`formRequest`/`request` in `lib/api.ts`), so the timeout,
 * the bearer token and the 401 policy are the same as every other call (#237).
 */

import { formRequest, request } from "@/lib/api";

export type ArtworkKind = "cover" | "background";

export interface ArtworkResponse {
  kind: ArtworkKind;
  url: string;
  format: string;
  size: number;
  sha256: string;
}

/** Uploads one image and returns its cache-busted media URL. */
export async function uploadArtwork(gameId: number, kind: ArtworkKind, file: File): Promise<ArtworkResponse> {
  const form = new FormData();
  form.set("kind", kind);
  form.set("file", file);
  return formRequest<ArtworkResponse>(`/games/${gameId}/artwork`, form);
}

/** Clears custom art so provider artwork (or the placeholder) is used again. */
export async function resetArtwork(gameId: number, kind: ArtworkKind): Promise<void> {
  await request<null>(`/games/${gameId}/artwork`, { method: "DELETE", query: { kind } });
}

/** Accepted upload formats, mirrored by the server's magic-byte check. */
export const ARTWORK_ACCEPT = "image/png,image/jpeg,image/webp,image/gif";
/** Server-side limit, shown before the upload starts. */
export const ARTWORK_MAX_BYTES = 8 * 1024 * 1024;
