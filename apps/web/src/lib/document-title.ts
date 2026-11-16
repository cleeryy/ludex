import { useEffect } from "react";

/* --------------------------------------------------------------------------
   Document title (#237).

   The tab announced "Ludex" on every screen, which makes a browser full of
   tabs useless and gives a screen reader no "you are here". The shell sets the
   destination title from the navigation table; a screen with real content
   (a game, a share) registers a more specific one and wins until it unmounts.

   The store is module-level and framework-free so the precedence rule is a
   pure function with a test, not a side effect buried in an effect.
   -------------------------------------------------------------------------- */

export const APP_TITLE = "Ludex";

export function formatDocumentTitle(label: string | null | undefined): string {
  const clean = label?.trim();
  return clean ? `${clean} · ${APP_TITLE}` : APP_TITLE;
}

let routeLabel: string | null = null;
let sequence = 0;
const overrides = new Map<number, string>();

/** Highest registration wins; the route label is the floor. */
export function composeDocumentTitle(): string {
  let label: string | null = routeLabel;
  for (const value of overrides.values()) label = value;
  return formatDocumentTitle(label);
}

function paint(): void {
  if (typeof document === "undefined") return;
  document.title = composeDocumentTitle();
}

/** The shell's destination title. */
export function setRouteDocumentTitle(label: string | null): void {
  if (label === routeLabel) return;
  routeLabel = label;
  paint();
}

/** Registers a screen title; returns the unregister handle. */
export function registerDocumentTitle(label: string): () => void {
  sequence += 1;
  const id = sequence;
  overrides.set(id, label);
  paint();
  return () => {
    if (!overrides.delete(id)) return;
    paint();
  };
}

export function resetDocumentTitle(): void {
  routeLabel = null;
  overrides.clear();
  sequence = 0;
}

/** Sets the title while `label` is non-null; restores the previous one after. */
export function useDocumentTitle(label: string | null | undefined): void {
  useEffect(() => {
    if (!label) return;
    return registerDocumentTitle(label);
  }, [label]);
}
