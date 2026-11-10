import { useQuery } from "@tanstack/react-query";
import { native } from "@/lib/native";
import type { PresenceStatus as PresenceStatusPayload } from "@/lib/types";
import { t } from "@/lib/i18n";

/**
 * Live Discord presence state from the desktop worker (#12). Reads the saved
 * settings, so the label can lag the form until "Save desktop settings".
 */
export function PresenceStatus({ enabled }: { enabled: boolean }) {
  const { data } = useQuery<PresenceStatusPayload>({
    queryKey: ["presence-status"],
    queryFn: () => native.presenceStatus(),
    refetchInterval: 5000,
    staleTime: 2000,
  });

  if (!enabled) {
    return <p className="mt-2 font-mono text-[10.5px] text-ink-3">{t("Presence is off.")}</p>;
  }
  if (!data) {
    return <p className="mt-2 font-mono text-[10.5px] text-ink-3">{t("Checking Discord…")}</p>;
  }
  if (!data.configured) {
    return (
      <p className="mt-2 font-mono text-[10.5px] text-signal-text">
        {t("Add a Discord application id, then save.")}
      </p>
    );
  }
  if (data.connected) {
    return (
      <p className="mt-2 font-mono text-[10.5px] text-ok">
        {t("Connected")}{data.details ? ` · showing ${data.details}` : ""}
      </p>
    );
  }
  return (
    <p className="mt-2 font-mono text-[10.5px] text-ink-3">
      {data.lastError ?? t("Waiting for the Discord app")}
      {data.retryInSeconds > 0 ? ` · retrying in ${data.retryInSeconds}s` : ""}
    </p>
  );
}
