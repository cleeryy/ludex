/** Tiny class-name joiner. No dependency, no memoisation needed. */
export function cn(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}
