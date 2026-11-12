/**
 * Account avatar client (#183).
 *
 * The upload is multipart, so it lives outside the JSON client like the
 * artwork uploader. The bearer token comes from the shared store and a 401
 * clears it exactly like every other request.
 */

import { ApiError, UNAUTHORIZED_EVENT, getApiBase, tokenStore } from "@/lib/api";
import type { User } from "@/lib/types";

export {
  AVATAR_ACCEPT,
  AVATAR_MAX_BYTES,
  avatarFileProblem,
  type AvatarFileProblem,
} from "@/lib/avatar-file";

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

function authHeaders(): HeadersInit | undefined {
  const token = tokenStore.get();
  return token ? { Authorization: `Bearer ${token}` } : undefined;
}

/** Uploads one image and returns the refreshed account. */
export async function uploadAvatar(file: File): Promise<User> {
  const form = new FormData();
  form.set("file", file);
  const response = await fetch(`${getApiBase()}/api/account/avatar`, {
    method: "POST",
    headers: authHeaders(),
    body: form,
  });
  return handle<User>(response);
}

/** Removes the avatar and returns the refreshed account. */
export async function deleteAvatar(): Promise<User> {
  const response = await fetch(`${getApiBase()}/api/account/avatar`, {
    method: "DELETE",
    headers: authHeaders(),
  });
  return handle<User>(response);
}
