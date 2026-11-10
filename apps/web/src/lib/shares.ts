/**
 * Share links (#9): a small, self-contained client for the two editor routes
 * and the two public routes.
 *
 * The public calls deliberately do not go through `api.ts`'s shared `request`:
 * a 401 there means "wrong password", never "your session expired", so it must
 * not clear the token or fire the global unauthorized event. Editor calls keep
 * the bearer behaviour of the rest of the app.
 */

import { ApiError, UNAUTHORIZED_EVENT, getApiBase, tokenStore } from "@/lib/api";
import { t } from "@/lib/i18n";

/** Public game metadata attached to a share link. */
export interface ShareGame {
  id: number;
  title: string;
  description: string | null;
  coverUrl: string | null;
  backgroundUrl: string | null;
  screenshots: string[];
  releaseDate: string | null;
  rating: number | null;
  genres: string[];
  developers: string[];
  publishers: string[];
  ageRating: number | null;
}

/** Public version metadata attached to a share link. */
export interface ShareVersion {
  id: number;
  /** Version label parsed from the file name, when known. */
  version: string | null;
  /** Archive size in bytes. */
  size: number;
  releaseDate: string | null;
  earlyAccess: boolean;
}

/** Response of `GET /api/share/{token}`. Metadata only, no account data. */
export interface ShareView {
  game: ShareGame;
  version: ShareVersion;
  /** RFC 3339 expiry, `null` when the link never expires. */
  expiresAt: string | null;
  /** Whether the download needs a password. */
  hasPassword: boolean;
  /** Downloads left, `null` when unlimited. */
  remainingDownloads: number | null;
}

/** Editor-facing share link. `token` is only present in the creation response. */
export interface ShareLink {
  id: number;
  token?: string;
  gameId: number;
  versionId: number | null;
  gameTitle: string | null;
  label: string | null;
  expiresAt: string | null;
  hasPassword: boolean;
  maxDownloads: number | null;
  downloadCount: number;
  revokedAt: string | null;
  createdAt: string;
}

/** Response of `POST /api/share/{token}/access`: a 15-minute ticket. */
export interface ShareAccess {
  /** Origin-relative download URL carrying a scoped ticket. */
  url: string;
  expiresAt: string;
}

/** Body of `POST /api/games/{id}/shares`. Optionals are omitted when unset. */
export interface CreateShareInput {
  versionId?: number;
  expiresInHours?: number;
  password?: string;
  maxDownloads?: number;
  label?: string;
}

function apiUrl(path: string): string {
  return `${getApiBase()}/api${path}`;
}

async function readBody(response: Response): Promise<unknown> {
  if (response.status === 204) return null;
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

/**
 * Public fetch. Never sends a bearer token and never reacts to a 401: on these
 * routes 401 only means the share password was wrong.
 */
async function publicFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("Accept", "application/json");

  let response: Response;
  try {
    response = await fetch(apiUrl(path), { ...init, headers });
  } catch {
    throw new ApiError(0, "network_error", t("Cannot reach the server. Check that it is running and the address is correct."));
  }

  if (!response.ok) throw await toError(response);
  return (await readBody(response)) as T;
}

/** Editor fetch: bearer auth, and a 401 still signs the session out. */
async function editorFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  headers.set("Accept", "application/json");
  const token = tokenStore.get();
  if (token) headers.set("Authorization", `Bearer ${token}`);
  if (init.body !== undefined) headers.set("Content-Type", "application/json");

  let response: Response;
  try {
    response = await fetch(apiUrl(path), { ...init, headers });
  } catch {
    throw new ApiError(0, "network_error", t("Cannot reach the server. Check that it is running and the address is correct."));
  }

  if (response.status === 401) {
    tokenStore.clear();
    window.dispatchEvent(new CustomEvent(UNAUTHORIZED_EVENT));
  }

  if (!response.ok) throw await toError(response);
  return (await readBody(response)) as T;
}

/** Public metadata page. Answers 404 for unknown, revoked, expired or exhausted links. */
export function getPublicShare(token: string): Promise<ShareView> {
  return publicFetch<ShareView>(`/share/${encodeURIComponent(token)}`);
}

/** Exchanges the link (and optional password) for a 15-minute download ticket. */
export function accessShare(token: string, password?: string): Promise<ShareAccess> {
  return publicFetch<ShareAccess>(`/share/${encodeURIComponent(token)}/access`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(password === undefined ? {} : { password }),
  });
}

/** Creates a link and returns it with the plaintext token, shown only once. */
export function createShare(gameId: number, input: CreateShareInput): Promise<ShareLink> {
  return editorFetch<ShareLink>(`/games/${gameId}/shares`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

/** Every share link, newest first, with the game title and no token. */
export function listShares(): Promise<ShareLink[]> {
  return editorFetch<ShareLink[]>("/shares");
}

/** Revokes a link; every public route answers 404 from then on. */
export function revokeShare(id: number): Promise<void> {
  return editorFetch<void>(`/shares/${id}`, { method: "DELETE" });
}
