/** Small event bus for UI intents that cross component boundaries. */

export const OPEN_SHORTCUTS_EVENT = "ludex:open-shortcuts";
export const OPEN_FILTERS_EVENT = "ludex:open-filters";
export const FOCUS_SEARCH_EVENT = "ludex:focus-search";

let pendingSearchFocus = false;

/** Asks the library search field to take focus, on this screen or after navigation. */
export function requestSearchFocus(): void {
  pendingSearchFocus = true;
  window.dispatchEvent(new Event(FOCUS_SEARCH_EVENT));
}

/** Consumes a focus request queued before the search field mounted. */
export function consumeSearchFocus(): boolean {
  if (!pendingSearchFocus) return false;
  pendingSearchFocus = false;
  return true;
}

export function openShortcutsHelp(): void {
  window.dispatchEvent(new Event(OPEN_SHORTCUTS_EVENT));
}

export function openFiltersPanel(): void {
  window.dispatchEvent(new Event(OPEN_FILTERS_EVENT));
}
