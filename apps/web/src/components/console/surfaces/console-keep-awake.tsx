import { useEffect, useRef, useState } from "react";
import { keepAwakeSentence } from "@/lib/keep-awake";
import { isNative, native } from "@/lib/native";
import { t } from "@/lib/i18n";
import type { InhibitStatus } from "@/lib/types";
import { useConsoleNotice } from "@/components/console/surfaces/console-notice";

/**
 * The console's copy of the suspend-inhibitor banner (#288): a 10-foot screen
 * has no banner strip, so the notice surface says it once when work starts.
 *
 * Self-contained on purpose: the console shell mounts outside `AppShell` and
 * does not use the desktop query client, so this subscribes to
 * `inhibit://status` directly and nothing here takes the ring.
 */
export function ConsoleKeepAwakeNotice() {
  const { notify, dismiss } = useConsoleNotice();
  const [status, setStatus] = useState<InhibitStatus | null>(null);
  const shown = useRef<string | null>(null);

  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    let off: (() => void) | undefined;
    void native
      .inhibitStatus()
      .then((next) => {
        if (!disposed && next) setStatus(next);
      })
      .catch(() => undefined);
    void native
      .onInhibitStatus((next) => {
        if (!disposed) setStatus(next);
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
  }, []);

  useEffect(() => {
    const sentence = keepAwakeSentence(status);
    if (sentence) {
      if (shown.current !== sentence) {
        shown.current = sentence;
        notify({
          title: sentence,
          description: t("Ludex blocks suspend and display blanking until the last job ends, then releases the inhibitor."),
          durationMs: 6_000,
        });
      }
      return;
    }
    if (shown.current !== null) {
      shown.current = null;
      dismiss();
    }
  }, [status, notify, dismiss]);

  return null;
}
