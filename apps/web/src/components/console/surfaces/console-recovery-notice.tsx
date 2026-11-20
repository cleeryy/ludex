import { useEffect } from "react";
import { isNative, native } from "@/lib/native";
import { t } from "@/lib/i18n";

import { useConsoleNotice } from "@/components/console/surfaces/console-notice";

/**
 * The console's copy of the auto-revert notice (#298).
 *
 * The console shell mounts outside `AppShell`, so it does not receive the
 * desktop toasts. A revert happens while the player is looking at the library:
 * the notice surface says it once, in one sentence, and the next Play already
 * uses the restored plan. Nothing here takes the ring.
 */
export function ConsoleRecoveryNotice() {
  const { notify } = useConsoleNotice();

  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    let off: (() => void) | undefined;
    void native
      .onGameRecovery(({ title }) => {
        if (disposed) return;
        notify({
          title: t("Previous launch setup restored"),
          description: t(
            "{title} kept failing to start, so Ludex put back the last launch setup that worked. The next launch uses it.",
            { title },
          ),
          tone: "success",
          durationMs: 10_000,
        });
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else off = unlisten;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      off?.();
    };
  }, [notify]);

  return null;
}
