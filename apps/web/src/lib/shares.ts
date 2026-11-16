/**
 * Share links (#9): the two editor routes and the two public routes.
 *
 * Everything goes through `lib/api.ts` (#237). The public calls use
 * `publicRequest`, which never sends the session token and never reacts to a
 * 401: on these routes a 401 means "wrong password", not "your session
 * expired". Editor calls keep the bearer behaviour of the rest of the app.
 */

import { publicRequest, request } from "@/lib/api";

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

/** Public metadata page. Answers 404 for unknown, revoked, expired or exhausted links. */
export function getPublicShare(token: string): Promise<ShareView> {
  return publicRequest<ShareView>(`/share/${encodeURIComponent(token)}`);
}

/** Exchanges the link (and optional password) for a 15-minute download ticket. */
export function accessShare(token: string, password?: string): Promise<ShareAccess> {
  return publicRequest<ShareAccess>(`/share/${encodeURIComponent(token)}/access`, {
    method: "POST",
    body: password === undefined ? {} : { password },
  });
}

/** Creates a link and returns it with the plaintext token, shown only once. */
export function createShare(gameId: number, input: CreateShareInput): Promise<ShareLink> {
  return request<ShareLink>(`/games/${gameId}/shares`, { method: "POST", body: input });
}

/** Every share link, newest first, with the game title and no token. */
export function listShares(): Promise<ShareLink[]> {
  return request<ShareLink[]>("/shares");
}

/** Revokes a link; every public route answers 404 from then on. */
export function revokeShare(id: number): Promise<void> {
  return request<void>(`/shares/${id}`, { method: "DELETE" });
}
