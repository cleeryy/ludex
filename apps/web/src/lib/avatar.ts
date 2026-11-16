/**
 * Account avatar client (#183).
 *
 * The multipart request goes through the one adapter (`formRequest` in
 * `lib/api.ts`), so the timeout, the bearer token and the 401 policy match
 * every other call (#237).
 */

import { formRequest, request } from "@/lib/api";
import type { User } from "@/lib/types";

export {
  AVATAR_ACCEPT,
  AVATAR_MAX_BYTES,
  avatarFileProblem,
  type AvatarFileProblem,
} from "@/lib/avatar-file";

/** Uploads one image and returns the refreshed account. */
export async function uploadAvatar(file: File): Promise<User> {
  const form = new FormData();
  form.set("file", file);
  return formRequest<User>("/account/avatar", form);
}

/** Removes the avatar and returns the refreshed account. */
export async function deleteAvatar(): Promise<User> {
  return request<User>("/account/avatar", { method: "DELETE" });
}
