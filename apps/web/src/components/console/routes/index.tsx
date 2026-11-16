import { useLocation } from "react-router-dom";
import { ConsoleShell } from "@/components/console/shell/console-shell";
import { consoleLocationFor } from "@/components/console/routes/surface";
import { CONSOLE_ROUTES } from "@/components/console/routes/table";

export { CONSOLE_ROUTES };

/**
 * The Console's route tree (#267) — one mounted shell, seven addresses.
 *
 * Every path in `CONSOLE_ROUTES` resolves to the same `ConsoleShell` instance
 * with different surface props. That is deliberate: the shell owns the
 * selection, the layout and the overlays, and a route change must not tear
 * them down. A cold deep link mounts it once; an in-shell push only changes the
 * surface.
 */
export function ConsoleRoutes() {
  const location = useLocation();
  const parsed = consoleLocationFor(location.pathname);
  return (
    <>
      {/* The resolved route, for diagnostics and the E2E checks (#267). */}
      <span hidden data-console-route={parsed?.surface ?? "home"} />
      <ConsoleShell
        surface={parsed?.surface ?? "home"}
        gameId={parsed?.gameId ?? null}
        section={parsed?.section ?? null}
      />
    </>
  );
}
