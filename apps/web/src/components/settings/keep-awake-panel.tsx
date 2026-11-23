import { Plug } from "lucide-react";
import { keepAwakeLabels, keepAwakeSentence, useInhibitStatus } from "@/lib/keep-awake";
import { isNative } from "@/lib/native";
import { t } from "@/lib/i18n";

/**
 * The status/diagnostics entry for the suspend inhibitor (#288).
 *
 * It answers the two questions a stuck inhibitor raises: is Ludex holding the
 * machine awake right now, and why. The last line names the command to check
 * from a terminal, so "it must be my power settings" is never the first guess.
 */
export function KeepAwakeStatus() {
  const query = useInhibitStatus();
  const status = query.data;

  if (!isNative()) {
    return (
      <p className="text-label leading-relaxed text-ink-3">
        {t("The suspend inhibitor lives on the desktop app. A browser build leaves the machine's power settings alone.")}
      </p>
    );
  }

  if (query.isLoading || !status) {
    return <p className="text-label leading-relaxed text-ink-3">{t("Checking the inhibitor…")}</p>;
  }

  if (!status.supported) {
    return (
      <p className="flex items-start gap-2 text-label leading-relaxed text-ink-3">
        <Plug className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
        <span>
          {t("Suspend inhibition needs Linux with systemd-inhibit. This machine keeps its own power settings.")}
        </span>
      </p>
    );
  }

  const sentence = keepAwakeSentence(status);
  const labels = keepAwakeLabels(status);
  return (
    <div className="text-label leading-relaxed">
      <p className="flex items-start gap-2 text-ink">
        <Plug
          className={status.holding ? "mt-0.5 size-3.5 shrink-0 text-signal-text" : "mt-0.5 size-3.5 shrink-0 text-ink-3"}
          aria-hidden="true"
        />
        <span>{sentence ?? t("Nothing is holding the machine awake.")}</span>
      </p>
      {labels && <p className="mt-1 truncate text-meta text-ink-3">{labels}</p>}
      <p className="mt-2 font-mono text-micro text-ink-3">
        {t("Check with {command}. Ludex has no entry once the last job ends and the window above expires.", {
          command: "systemd-inhibit --list",
        })}
      </p>
    </div>
  );
}
