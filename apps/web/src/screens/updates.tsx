import { PageHeader } from "@/components/page";
import { UpdateCenter } from "@/components/updates/update-center";
import { useUpdater } from "@/components/updates/update-provider";
import { isNative } from "@/lib/native";
import { t } from "@/lib/i18n";

/**
 * `/updates` — the update center (#163).
 *
 * Reachable from Settings, the launch prompt and the "update available" badge.
 * The page owns the explanation; the Settings panel stays a one-line status.
 */
export function UpdatesScreen() {
  const { status } = useUpdater();
  const desktop = isNative();

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Update center")}
        meta={
          desktop ? (
            <>
              <span className="num">{status?.currentVersion ? `v${status.currentVersion}` : "…"}</span>
              <span>{status?.channel === "early" ? t("Early access") : t("Stable")}</span>
              {status?.available && status.notify && (
                <span className="text-signal-text">{t("Update available")}</span>
              )}
            </>
          ) : (
            <span>{t("Desktop app")}</span>
          )
        }
        description={t("See what changed, switch channel and install the next version. Ludex replaces only itself: game files, install folders and saves are never part of an app update.")}
      />

      <div className="mt-9 max-w-[1100px]">
        <UpdateCenter />
      </div>
    </div>
  );
}
