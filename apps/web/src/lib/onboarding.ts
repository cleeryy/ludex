/**
 * Onboarding wizard state (#119).
 *
 * The canonical step list lives on the server as an index so a half-finished
 * tour resumes on another device. `admin` is part of the canonical order even
 * for normal accounts, so promoting someone later keeps their position
 * meaningful; the UI simply skips it for non-administrators.
 */

export const ONBOARDING_STEP_IDS = [
  "welcome",
  "library",
  "downloads",
  "saves",
  "appearance",
  "admin",
  "finish",
] as const;

export type OnboardingStepId = (typeof ONBOARDING_STEP_IDS)[number];

export const ONBOARDING_LAST_STEP = ONBOARDING_STEP_IDS.length - 1;

const SKIP_KEY = "ludex.onboarding.skipped";
const CARD_KEY = "ludex.onboarding.gettingStartedDismissed";

export const OPEN_ONBOARDING_EVENT = "ludex:onboarding:open";

export function clampOnboardingStep(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(ONBOARDING_LAST_STEP, Math.max(0, Math.trunc(value)));
}

/** Canonical indices visible to this account, in order. */
export function visibleOnboardingSteps(isAdmin: boolean): number[] {
  return ONBOARDING_STEP_IDS.map((_, index) => index).filter((index) => index !== 5 || isAdmin);
}

/** Closest visible step at or before `value` (never past it). */
export function nearestVisibleOnboardingStep(value: number, isAdmin: boolean): number {
  const visible = visibleOnboardingSteps(isAdmin);
  let best = visible[0];
  for (const index of visible) {
    if (index <= clampOnboardingStep(value)) best = index;
  }
  return best;
}

/* ------------------------------ session skip ----------------------------- */

/** "Skip for now" is per account and browser session; the server flag is untouched. */
export function hasSkippedOnboarding(userId: number): boolean {
  try {
    return sessionStorage.getItem(SKIP_KEY) === String(userId);
  } catch {
    return false;
  }
}

export function markOnboardingSkipped(userId: number): void {
  try {
    sessionStorage.setItem(SKIP_KEY, String(userId));
  } catch {
    /* session-only preference is unavailable; close for this mount anyway */
  }
}

export function clearOnboardingSkip(): void {
  try {
    sessionStorage.removeItem(SKIP_KEY);
  } catch {
    /* nothing to clear */
  }
}

/* --------------------------- getting-started card ------------------------ */

export function gettingStartedDismissed(): boolean {
  try {
    return localStorage.getItem(CARD_KEY) === "1";
  } catch {
    return false;
  }
}

export function dismissGettingStarted(): void {
  try {
    localStorage.setItem(CARD_KEY, "1");
  } catch {
    /* local preference only */
  }
}

export function restoreGettingStarted(): void {
  try {
    localStorage.removeItem(CARD_KEY);
  } catch {
    /* nothing to restore */
  }
}

/* --------------------------- wizard open bus ----------------------------- */

let wizardActive = false;
const wizardListeners = new Set<() => void>();

export function setWizardActive(open: boolean): void {
  if (wizardActive === open) return;
  wizardActive = open;
  for (const listener of wizardListeners) listener();
}

export function subscribeWizard(listener: () => void): () => void {
  wizardListeners.add(listener);
  return () => wizardListeners.delete(listener);
}

export function isWizardActive(): boolean {
  return wizardActive;
}

/** Opens the wizard from Settings or the getting-started card. */
export function openOnboarding(options: { restart?: boolean } = {}): void {
  window.dispatchEvent(new CustomEvent(OPEN_ONBOARDING_EVENT, { detail: options }));
}
