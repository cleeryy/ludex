/**
 * Custom artwork uploads (#38).
 *
 * A separate client because the upload is multipart: `api.ts` only speaks JSON.
 * The bearer token is read from the shared store, and a 401 clears it exactly
 * like the JSON client does.
 */

import { ApiError, UNAUTHORIZED_EVENT, getApiBase, tokenStore } from "@/lib/api";

export type ArtworkKind = "cover" | "background";

export interface ArtworkResponse {
  kind: ArtworkKind;
  url: string;
  format: string;
  size: number;
  sha256: string;
}

async function handle<T>(response: Response): Promise<T> {
  if (response.status === 401) {
    tokenStore.clear();
    window.dispatchEvent(new CustomEvent(UNAUTHORIZED_EVENT));
  }
  const text = await response.text();
  let payload: unknown = null;
  if (text) {
    try {
      payload = JSON.parse(text);
    } catch {
      payload = null;
    }
  }
  if (!response.ok) {
    const envelope = payload as { error?: { code?: string; message?: string } } | null;
    throw new ApiError(
      response.status,
      envelope?.error?.code ?? "request_failed",
      envelope?.error?.message ?? `Request failed with status ${response.status}`,
    );
  }
  return payload as T;
}

/** Uploads one image and returns its cache-busted media URL. */
export async function uploadArtwork(gameId: number, kind: ArtworkKind, file: File): Promise<ArtworkResponse> {
  const form = new FormData();
  form.set("kind", kind);
  form.set("file", file);
  const token = tokenStore.get();
  const response = await fetch(`${getApiBase()}/api/games/${gameId}/artwork`, {
    method: "POST",
    headers: token ? { Authorization: `Bearer ${token}` } : undefined,
    body: form,
  });
  return handle<ArtworkResponse>(response);
}

/** Clears custom art so provider artwork (or the placeholder) is used again. */
export async function resetArtwork(gameId: number, kind: ArtworkKind): Promise<void> {
  const token = tokenStore.get();
  const response = await fetch(`${getApiBase()}/api/games/${gameId}/artwork?kind=${kind}`, {
    method: "DELETE",
    headers: token ? { Authorization: `Bearer ${token}` } : undefined,
  });
  await handle<null>(response);
}

/** Accepted upload formats, mirrored by the server's magic-byte check. */
export const ARTWORK_ACCEPT = "image/png,image/jpeg,image/webp,image/gif";
/** Server-side limit, shown before the upload starts. */
export const ARTWORK_MAX_BYTES = 8 * 1024 * 1024;
