/**
 * Public collection page client (#173).
 *
 * These calls deliberately never send the session token and never react to a
 * 401 by signing the user out: on this route a 401 only means the share
 * password was wrong. The public page lives outside the auth gate.
 */

import { ApiError, getApiBase } from "@/lib/api";
import { USING_MOCKS } from "@/lib/flags";
import { t } from "@/lib/i18n";
import { mockApi } from "@/lib/mock";
import type { CollectionShareView } from "@/lib/types";

function apiUrl(path: string): string {
  return `${getApiBase()}/api${path}`;
}

async function readBody(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

async function toError(response: Response): Promise<ApiError> {
  const payload = (await readBody(response)) as { error?: { code?: string; message?: string } } | null;
  return new ApiError(
    response.status,
    payload?.error?.code ?? "request_failed",
    payload?.error?.message ?? `Request failed with status ${response.status}`,
  );
}

async function publicFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("Accept", "application/json");
  if (init.body !== undefined) headers.set("Content-Type", "application/json");

  let response: Response;
  try {
    response = await fetch(apiUrl(path), { ...init, headers });
  } catch {
    throw new ApiError(0, "network_error", t("Cannot reach the server. Check that it is running and the address is correct."));
  }
  if (!response.ok) throw await toError(response);
  return (await readBody(response)) as T;
}

/** Public preview. Protected links return the metadata but no games. */
export function getCollectionShare(token: string): Promise<CollectionShareView> {
  if (USING_MOCKS) return mockApi.getCollectionShare(token);
  return publicFetch<CollectionShareView>(`/share/collection/${encodeURIComponent(token)}`);
}

/** Unlocks the game list of a password-protected link. */
export function unlockCollectionShare(token: string, password: string): Promise<CollectionShareView> {
  if (USING_MOCKS) return mockApi.getCollectionShare(token, password);
  return publicFetch<CollectionShareView>(`/share/collection/${encodeURIComponent(token)}/access`, {
    method: "POST",
    body: JSON.stringify({ password }),
  });
}
