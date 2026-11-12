/**
 * One account avatar, with the initials fallback used across the shell and the
 * community (#183).
 *
 * Uploaded files are content-addressed on the server
 * (`avatar-<user-id>-<sha256>.<ext>`), so the URL itself cache-busts: a new
 * upload always produces a different `src`. Callers that keep a stale URL in a
 * cached payload get the new one on the next query invalidation.
 */

import { assetUrl } from "@/lib/api";
import { cn } from "@/lib/cn";
import { initials } from "@/lib/format";

export function UserAvatar({
  name,
  src,
  size = 32,
  className,
}: {
  /** Display name or login, used for `alt` and the initials fallback. */
  name: string;
  /** `avatarUrl` from the API, or a local object URL for a pending preview. */
  src: string | null | undefined;
  /** Rendered square size in CSS pixels. */
  size?: number;
  className?: string;
}) {
  const url = assetUrl(src);
  if (url) {
    return (
      <img
        src={url}
        alt={name}
        width={size}
        height={size}
        decoding="async"
        loading="lazy"
        style={{ width: size, height: size }}
        className={cn("shrink-0 rounded-[2px] border border-line-2 object-cover", className)}
      />
    );
  }
  return (
    <span
      aria-hidden="true"
      style={{ width: size, height: size, fontSize: Math.max(9, Math.round(size * 0.36)) }}
      className={cn(
        "grid shrink-0 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-ink",
        className,
      )}
    >
      {initials(name)}
    </span>
  );
}
