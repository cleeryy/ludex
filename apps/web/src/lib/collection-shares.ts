/**
 * Public collection page (#173).
 *
 * `publicRequest` never sends the session token and never signs the user out on
 * a 401: on this route a 401 only means the share password was wrong. The
 * public page lives outside the auth gate (#237).
 */

import { publicRequest } from "@/lib/api";
import { USING_MOCKS } from "@/lib/flags";
import { mockApi } from "@/lib/mock";
import type { CollectionShareView } from "@/lib/types";

/** Public preview. Protected links return the metadata but no games. */
export function getCollectionShare(token: string): Promise<CollectionShareView> {
  if (USING_MOCKS) return mockApi.getCollectionShare(token);
  return publicRequest<CollectionShareView>(`/share/collection/${encodeURIComponent(token)}`);
}

/** Unlocks the game list of a password-protected link. */
export function unlockCollectionShare(token: string, password: string): Promise<CollectionShareView> {
  if (USING_MOCKS) return mockApi.getCollectionShare(token, password);
  return publicRequest<CollectionShareView>(`/share/collection/${encodeURIComponent(token)}/access`, {
    method: "POST",
    body: { password },
  });
}
