/**
 * Change signal for account-scoped local preferences (#201).
 *
 * Kept in its own module so the storage helpers can announce a change without
 * importing the preference document (which imports them): the provider listens
 * for the event and decides whether the new values still differ from the
 * document it knows about.
 */
export const PREFERENCES_CHANGED_EVENT = "ludex:preferences-changed";

export function notifyPreferencesChanged(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(PREFERENCES_CHANGED_EVENT));
}
