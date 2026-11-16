import { useRef } from "react";
import { useInputScope } from "@/components/console/input/input-provider";

/**
 * Back-only pad handler for a screen that is not pad-navigable (#237, #267).
 *
 * Console can send a player to `/game/:id` or `/remote`, and those screens used
 * to have no pad input at all: a couch player with no keyboard was stuck. B
 * returns to the screen they came from. The handler is a scope on the one
 * input owner, so it never competes with the Console or a dialog above it.
 */
export function usePadBack(onBack: () => void, options: { enabled?: boolean } = {}): void {
  const backRef = useRef(onBack);
  backRef.current = onBack;
  useInputScope({
    id: "pad-back",
    enabled: options.enabled ?? true,
    onAction: (action) => {
      if (action === "back") backRef.current();
    },
  });
}
