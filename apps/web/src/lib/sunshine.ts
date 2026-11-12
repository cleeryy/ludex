/**
 * Sunshine error helpers (#182).
 *
 * `sunshine_sync` rejects with a structured object once a write fails; older
 * desktop builds reject with a plain string. The settings panel must offer the
 * one-time elevation only for the structured, permission-related case, never by
 * matching the message text.
 */
export function needsSunshineElevation(error: unknown): boolean {
  return Boolean(
    error &&
      typeof error === "object" &&
      (error as { needsElevation?: unknown }).needsElevation === true,
  );
}
