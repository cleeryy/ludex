/**
 * The single boundary where a string becomes a URL the UI may render (#266).
 *
 * The server hands the web app two kinds of addresses: origin-relative paths
 * (`/api/media/...`) and absolute URLs (provider artwork, `data:` and `blob:`
 * previews, `demo:` fixtures). Both end up in `src`, `href` and
 * `window.location.assign`, so they are validated once here instead of at each
 * sink:
 *
 * * absolute: `http:`, `https:`, `data:`, `blob:` and `demo:` only;
 * * relative: a path that starts with a single `/` (never `//` or `/\`, which
 *   browsers resolve as protocol-relative);
 * * everything else — `javascript:`, `vbscript:`, `file:`, `mailto:`, control
 *   characters, whitespace-only values — is refused with `null`.
 *
 * The `SafeUrl` brand makes the proof visible to TypeScript: a component that
 * needs a URL accepts `SafeUrl`, and only `toSafeUrl`/`assetUrl`/`safeRedirectUrl`
 * produce one.
 */

/** Schemes a media element, a link or a redirect may use. */
const ALLOWED_ABSOLUTE = /^(?:https?:|data:|blob:|demo:)/i;

/** RFC 3986 scheme prefix, used to reject everything not in the allow list. */
const SCHEME_PREFIX = /^[a-z][a-z0-9+.-]*:/i;

/** C0 controls (including NUL, CR and LF) never belong in a URL. */
// eslint-disable-next-line no-control-regex -- the control range is the point
const UNSAFE_CHARACTERS = /[\u0000-\u001f\u007f]/;

/** A string that passed {@link toSafeUrl}. */
export type SafeUrl = string & { readonly __ludexSafeUrl: unique symbol };

/**
 * Validates one candidate URL.
 *
 * Returns the same string branded as {@link SafeUrl}, or `null` when the value
 * is not a scheme the app accepts and not an origin-relative path.
 */
export function toSafeUrl(raw: string | null | undefined): SafeUrl | null {
  if (typeof raw !== "string") return null;
  const value = raw.trim();
  if (value.length === 0) return null;
  if (UNSAFE_CHARACTERS.test(value)) return null;

  if (ALLOWED_ABSOLUTE.test(value)) {
    // `https:` with nothing after the colon is not a navigable URL.
    const colon = value.indexOf(":");
    return value.length > colon + 1 ? (value as SafeUrl) : null;
  }

  // Browsers treat `\` like `/`, so `\\host` and `/\host` are protocol-relative
  // even though they do not literally start with `//`.
  if (/^[\\/]{2}/.test(value)) return null;
  if (SCHEME_PREFIX.test(value)) return null;
  if (!value.startsWith("/")) return null;
  return value as SafeUrl;
}

/** Narrows an arbitrary value to {@link SafeUrl}. */
export function isSafeUrl(value: unknown): value is SafeUrl {
  return typeof value === "string" && toSafeUrl(value) === value;
}

/** Origin of a URL, resolved against `baseUrl` when it is relative. */
export function originOf(
  url: string | null | undefined,
  baseUrl = "http://localhost",
): string | null {
  if (!url) return null;
  try {
    return new URL(url, baseUrl).origin;
  } catch {
    return null;
  }
}

/**
 * Validates a navigation target against an origin allow list.
 *
 * Used for the SSO hand-off addresses (`webLoginUrl`/`handoffUrl`) that come
 * back from a server: a malicious or misconfigured server must not be able to
 * turn the login screen into an open redirect. Only `http:`/`https:` targets
 * whose origin is explicitly allowed survive; everything else is `null` and the
 * caller falls back to its own login page.
 */
export function safeRedirectUrl(
  target: string | null | undefined,
  allowedOrigins: readonly (string | null | undefined)[],
  baseUrl: string,
): SafeUrl | null {
  if (!target) return null;
  let parsed: URL;
  try {
    parsed = new URL(target, baseUrl);
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
  // `https://user:pass@host/` is a credential-smuggling shape, not a login URL.
  if (parsed.username !== "" || parsed.password !== "") return null;
  const allowed = allowedOrigins.filter((origin): origin is string => Boolean(origin));
  if (!allowed.includes(parsed.origin)) return null;
  return toSafeUrl(parsed.href);
}
