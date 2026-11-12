import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/** The hook `styles/index.css` uses to unlock the viewport for public routes. */
export const PUBLIC_PAGE_CLASS = "public-page";

/** Base classes every account-free page shares: full height, app background. */
export const PUBLIC_PAGE_BASE_CLASSES = `${PUBLIC_PAGE_CLASS} min-h-dvh bg-bg`;

/**
 * Root of an account-free route (#176).
 *
 * The authenticated app is a fixed shell: `body { overflow: hidden }` and
 * AppShell owns the only scroll container. Public pages are normal documents,
 * so this wrapper opts the route into `body:has(.public-page) { overflow-y:
 * auto }` and lets the browser scroll the page itself — wheel, touch, keyboard
 * paging and the iOS safe areas all behave natively.
 *
 * Every route outside the `Gate` must render inside this wrapper; a plain
 * `min-h-dvh` div would be clipped at the fold.
 */
export function PublicPage({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn(PUBLIC_PAGE_BASE_CLASSES, className)}>{children}</div>;
}
